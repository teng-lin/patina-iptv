//! Verify the timestamped Authenticode publisher and launch the per-user setup.
use super::ReleaseAsset;
use std::{
    io::{BufRead, BufReader, Write},
    os::windows::{ffi::OsStringExt, process::CommandExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};
use tempfile::TempPath;

const VERIFIER: &str = concat!(
    include_str!("windows-publisher.ps1"),
    "\n",
    include_str!("windows.ps1")
);

pub struct PendingSetup {
    child: Child,
    _stage: TempPath,
}

impl PendingSetup {
    /// The helper starts Setup only after this process has exited. Closing stdin cancels.
    pub fn install_after_exit(&mut self) -> bool {
        self.child
            .stdin
            .take()
            .is_some_and(|mut pipe| pipe.write_all(b"install\n").is_ok())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Verification {
    Digest,
    Signature,
    Unavailable,
}

impl crate::Runtime {
    /// The verifier holds a read lock on the approved bytes while the viewer decides.
    pub async fn prepare_windows_setup(
        &self,
        stage: TempPath,
        asset: ReleaseAsset,
        publisher: &'static str,
        failure_message: &'static str,
    ) -> Option<PendingSetup> {
        if publisher.trim().is_empty() {
            return None;
        }
        self.services()
            .spawn_blocking(move || {
                prepare(
                    stage,
                    &asset.sha256,
                    publisher,
                    std::process::id(),
                    failure_message,
                    VERIFIER,
                )
                .ok()
            })
            .await
            .ok()
            .flatten()
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
}

fn system_directory() -> Option<PathBuf> {
    let mut buffer = vec![0u16; 32768];
    // SAFETY: Windows writes at most the supplied number of UTF-16 code units.
    let count = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    (count > 0 && count < buffer.len())
        .then(|| PathBuf::from(std::ffi::OsString::from_wide(&buffer[..count])))
}

fn prepare(
    stage: TempPath,
    digest: &str,
    publisher: &str,
    parent: u32,
    failure_message: &str,
    script: &str,
) -> Result<PendingSetup, Verification> {
    let system = system_directory().ok_or(Verification::Unavailable)?;
    let interpreter = system.join(r"WindowsPowerShell\v1.0\powershell.exe");
    let mut child = Command::new(interpreter)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW for the verifier, installer has its own UI.
        .env("PATINA_STAGED_SETUP", &stage)
        .env("PATINA_UPDATE_PARENT", parent.to_string())
        .env("PATINA_SETUP_ERROR", failure_message)
        .env("PATINA_SETUP_DIGEST", digest)
        .env(
            "PSModulePath",
            system.join(r"WindowsPowerShell\v1.0\Modules"),
        )
        .env("PATINA_SETUP_SUBJECT", publisher)
        .current_dir(system)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Verification::Unavailable)?;
    if let Err(reason) = verified(&mut child, Duration::from_secs(90)) {
        // Release any verifier lock before the stage guard removes its file.
        let _ = child.kill();
        let _ = child.wait();
        return Err(reason);
    }
    Ok(PendingSetup {
        child,
        _stage: stage,
    })
}

fn verified(child: &mut Child, timeout: Duration) -> Result<(), Verification> {
    let output = child.stdout.take().ok_or(Verification::Unavailable)?;
    let (send, recv) = mpsc::channel();
    std::thread::Builder::new()
        .spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(output).read_line(&mut line).map(|_| line);
            let _ = send.send(result);
        })
        .map_err(|_| Verification::Unavailable)?;
    let line = recv
        .recv_timeout(timeout)
        .ok()
        .and_then(Result::ok)
        .ok_or(Verification::Unavailable)?;
    match line.trim_end() {
        "verified" => Ok(()),
        "digest" => Err(Verification::Digest),
        "signature" => Err(Verification::Signature),
        _ => Err(Verification::Unavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    #[test]
    fn refuses_and_cleans_up_unsigned_or_changed_installer_bytes() {
        let dir = tempfile::tempdir().unwrap();
        for (expected, reason) in [
            (b"synthetic setup".as_slice(), Verification::Signature),
            (b"different", Verification::Digest),
        ] {
            let mut file = tempfile::Builder::new()
                .suffix(".exe")
                .tempfile_in(dir.path())
                .unwrap();
            file.write_all(b"synthetic setup").unwrap();
            let path = file.into_temp_path();
            let location = path.to_path_buf();
            let digest = format!("{:x}", Sha256::digest(expected));
            assert_eq!(
                prepare(
                    path,
                    &digest,
                    "CN=Synthetic Patina",
                    std::process::id(),
                    "",
                    VERIFIER
                )
                .err(),
                Some(reason)
            );
            assert!(!location.exists());
        }
    }

    #[test]
    fn confirmation_waits_for_exit_and_cancellation_cleans_the_verified_file() {
        // Exercise the production protocol, locking, process wait, launch and cleanup with
        // a harmless system executable. Only the separate publisher predicate is synthetic.
        let script = concat!(
            "function Test-PatinaPublisher { return $true }\n",
            include_str!("windows.ps1")
        );
        let dir = tempfile::tempdir().unwrap();
        let system = system_directory().unwrap();
        let bytes = std::fs::read(system.join("whoami.exe")).unwrap();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        for confirm in [true, false] {
            let mut parent = Command::new(system.join(r"WindowsPowerShell\v1.0\powershell.exe"))
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "[Console]::In.ReadLine()",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            let mut stage = tempfile::Builder::new()
                .suffix(".exe")
                .tempfile_in(dir.path())
                .unwrap();
            stage.write_all(&bytes).unwrap();
            let stage = stage.into_temp_path();
            let location = stage.to_path_buf();
            let mut setup = prepare(stage, &digest, "", parent.id(), "", script).unwrap();
            if confirm {
                assert!(setup.install_after_exit());
                assert!(setup.child.try_wait().unwrap().is_none());
                assert!(location.exists());
            } else {
                drop(setup.child.stdin.take());
            }
            drop(parent.stdin.take());
            parent.wait().unwrap();
            let end = std::time::Instant::now() + Duration::from_secs(15);
            let status = loop {
                if let Some(status) = setup.child.try_wait().unwrap() {
                    break status;
                }
                if std::time::Instant::now() >= end {
                    let _ = setup.child.kill();
                    let _ = setup.child.wait();
                    panic!("installer handoff did not finish");
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            assert_eq!(status.success(), confirm);
            assert!(!location.exists());
        }
    }
}
