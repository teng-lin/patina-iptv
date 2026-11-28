//! Verify the timestamped Authenticode publisher and launch the per-user setup.
use super::ReleaseAsset;
use super::verifier::{CancelOnDrop, Verification, verified};
use std::{
    io::Write,
    os::windows::{ffi::OsStringExt, process::CommandExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
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
    armed: bool,
}

impl PendingSetup {
    /// The helper starts Setup only after this process has exited. Closing stdin cancels.
    pub fn install_after_exit(&mut self) -> bool {
        let armed = self
            .child
            .stdin
            .take()
            .is_some_and(|mut pipe| pipe.write_all(b"install\n").is_ok());
        self.armed |= armed;
        armed
    }
}

impl Drop for PendingSetup {
    fn drop(&mut self) {
        if !self.armed {
            // This also handles a verified result whose caller was cancelled
            // before receiving it. Reap the helper before deleting locked bytes.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
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
        self.prepare_windows_setup_with_script(stage, asset, publisher, failure_message, VERIFIER)
            .await
    }

    async fn prepare_windows_setup_with_script(
        &self,
        stage: TempPath,
        asset: ReleaseAsset,
        publisher: &'static str,
        failure_message: &'static str,
        script: &'static str,
    ) -> Option<PendingSetup> {
        if publisher.trim().is_empty() {
            return None;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let _cancel_on_drop = CancelOnDrop(Arc::clone(&cancelled));
        self.services()
            .spawn_blocking(move || {
                prepare(
                    stage,
                    &asset.sha256,
                    publisher,
                    std::process::id(),
                    failure_message,
                    script,
                    &cancelled,
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
    cancelled: &AtomicBool,
) -> Result<PendingSetup, Verification> {
    if cancelled.load(Ordering::Acquire) {
        return Err(Verification::Unavailable);
    }
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
    if let Err(reason) = verified(&mut child, Duration::from_secs(90), cancelled) {
        // Release any verifier lock before the stage guard removes its file.
        let _ = child.kill();
        let _ = child.wait();
        return Err(reason);
    }
    Ok(PendingSetup {
        child,
        _stage: stage,
        armed: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn asset(bytes: &[u8]) -> ReleaseAsset {
        let index = serde_json::json!({
            "schema": 1,
            "packages": [{
                "version": "9.8.7", "source": "github-release",
                "platform": "windows", "format": "exe", "architectures": ["x86_64"],
                "minimum": {"windows": "10.0"}, "signing": "signed",
                "notes": "https://github.com/teng-lin/patina-iptv/releases/tag/v9.8.7",
                "url": "https://github.com/teng-lin/patina-iptv/releases/download/v9.8.7/Patina-9.8.7-windows-x86_64-setup.exe",
                "length": bytes.len(), "sha256": format!("{:x}", Sha256::digest(bytes))
            }]
        });
        let installed = super::super::InstalledPackage {
            platform: "windows",
            architecture: "x86_64",
            format: Some("exe".into()),
            version: "9.8.6",
            android_code: None,
            minimum: serde_json::json!({"windows": "10.0"}),
        };
        let Some(super::super::UpdateResult::Available { asset, .. }) =
            super::super::selection::select(&serde_json::to_vec(&index).unwrap(), installed)
        else {
            panic!("expected generated installer record")
        };
        asset
    }

    #[tokio::test]
    async fn dropping_the_runtime_waiter_kills_the_held_verifier_before_cleaning_locked_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let mut stage = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        let bytes = b"synthetic installer";
        stage.write_all(bytes).unwrap();
        let stage = stage.into_temp_path();
        let location = stage.to_path_buf();
        let marker = PathBuf::from(format!("{}.waiting", location.display()));
        let owner = crate::Runtime::default();
        let mut waiter = Box::pin(owner.prepare_windows_setup_with_script(
            stage,
            asset(bytes),
            "CN=Synthetic Patina",
            "",
            concat!(
                "$held = [IO.File]::Open($env:PATINA_STAGED_SETUP, 'Open', 'Read', 'Read'); ",
                "[IO.File]::WriteAllText($env:PATINA_STAGED_SETUP + '.waiting', 'locked'); ",
                "[Threading.Thread]::Sleep(60000)"
            ),
        ));
        tokio::select! {
            _ = &mut waiter => panic!("held verifier unexpectedly completed"),
            _ = async {
                tokio::time::timeout(Duration::from_secs(30), async {
                    while !marker.exists() {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                }).await.expect("verifier did not acquire the stage lock");
            } => {}
        }
        assert!(location.exists());
        drop(waiter);
        tokio::time::timeout(Duration::from_secs(10), async {
            while location.exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("cancellation must kill the verifier and clean its locked stage promptly");
    }

    #[test]
    fn dropping_an_unarmed_successful_result_reaps_the_helper_and_removes_its_locked_stage() {
        let directory = tempfile::tempdir().unwrap();
        let stage = tempfile::NamedTempFile::new_in(directory.path())
            .unwrap()
            .into_temp_path();
        let location = stage.to_path_buf();
        let setup = prepare(
            stage,
            "",
            "",
            std::process::id(),
            "",
            concat!(
                "$held = [IO.File]::Open($env:PATINA_STAGED_SETUP, 'Open', 'Read', 'Read'); ",
                "[Console]::Out.WriteLine('verified'); [Console]::Out.Flush(); ",
                "[Threading.Thread]::Sleep(60000)"
            ),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(location.exists());
        drop(setup);
        assert!(
            !location.exists(),
            "the helper's read lock is released before deleting its stage"
        );
    }

    #[test]
    fn an_armed_handoff_survives_owner_drop_and_finishes_after_the_viewer_exits() {
        let directory = tempfile::tempdir().unwrap();
        let system = system_directory().unwrap();
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
        let bytes = std::fs::read(system.join("whoami.exe")).unwrap();
        let mut stage = tempfile::Builder::new()
            .suffix(".exe")
            .tempfile_in(directory.path())
            .unwrap();
        stage.write_all(&bytes).unwrap();
        let stage = stage.into_temp_path();
        let location = stage.to_path_buf();
        let mut setup = prepare(
            stage,
            &format!("{:x}", Sha256::digest(&bytes)),
            "",
            parent.id(),
            "",
            concat!(
                "function Test-PatinaPublisher { return $true }\n",
                include_str!("windows.ps1")
            ),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(setup.install_after_exit());
        drop(setup);
        assert!(
            location.exists(),
            "the armed helper keeps its staged file locked"
        );
        drop(parent.stdin.take());
        parent.wait().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while location.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the armed handoff did not finish"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

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
                    VERIFIER,
                    &AtomicBool::new(false)
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
            let mut setup = prepare(
                stage,
                &digest,
                "",
                parent.id(),
                "",
                script,
                &AtomicBool::new(false),
            )
            .unwrap();
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
