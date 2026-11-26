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

pub struct PendingSetup {
    child: Child,
}

impl PendingSetup {
    /// The helper starts Setup only after this process has exited. Closing stdin cancels.
    pub fn install_after_exit(mut self) -> bool {
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
    ) -> Option<PendingSetup> {
        if publisher.trim().is_empty() {
            return None;
        }
        self.services()
            .spawn_blocking(move || {
                prepare(stage, &asset.sha256, publisher, include_str!("windows.ps1")).ok()
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
    script: &str,
) -> Result<PendingSetup, Verification> {
    let system = system_directory().ok_or(Verification::Unavailable)?;
    let interpreter = system.join(r"WindowsPowerShell\v1.0\powershell.exe");
    let path = stage.keep().map_err(|_| Verification::Unavailable)?;
    let child = Command::new(interpreter)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW for the verifier, installer has its own UI.
        .env("PATINA_STAGED_SETUP", &path)
        .env("PATINA_UPDATE_PARENT", std::process::id().to_string())
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
        .spawn();
    let setup = child
        .map_err(|_| Verification::Unavailable)
        .and_then(|mut child| {
            let result = verified(&mut child, Duration::from_secs(90));
            if let Err(reason) = result {
                // A hung trust provider may still hold the staged file open.
                let _ = child.kill();
                let _ = child.wait();
                return Err(reason);
            }
            Ok(PendingSetup { child })
        });
    if setup.is_err() {
        let _ = std::fs::remove_file(path);
    }
    setup
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
                    include_str!("windows.ps1")
                )
                .err(),
                Some(reason)
            );
            assert!(!location.exists());
        }
    }

    #[test]
    fn confirmation_writes_once_and_cancellation_closes_the_pipe() {
        // A synthetic verifier records the parent's decision without running an installer.
        let script = r#"
            [Console]::Out.WriteLine('verified')
            [Console]::Out.Flush()
            $decision = [Console]::In.ReadLine()
            if ($decision -ceq 'install') { $text = 'confirmed' } else { $text = 'cancelled' }
            [IO.File]::Delete($env:PATINA_STAGED_SETUP)
            [IO.File]::WriteAllText(($env:PATINA_STAGED_SETUP + '.result'), $text)
        "#;
        let dir = tempfile::tempdir().unwrap();
        for confirm in [true, false] {
            let stage = tempfile::NamedTempFile::new_in(dir.path())
                .unwrap()
                .into_temp_path();
            let location = stage.to_path_buf();
            let mut marker = location.as_os_str().to_owned();
            marker.push(".result");
            let setup = prepare(stage, "", "", script).unwrap();
            if confirm {
                assert!(setup.install_after_exit());
            } else {
                drop(setup);
            }
            let end = std::time::Instant::now() + Duration::from_secs(15);
            let expected = if confirm { "confirmed" } else { "cancelled" };
            while std::fs::read_to_string(&marker).ok().as_deref() != Some(expected) {
                assert!(std::time::Instant::now() < end);
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(!location.exists());
        }
    }
}
