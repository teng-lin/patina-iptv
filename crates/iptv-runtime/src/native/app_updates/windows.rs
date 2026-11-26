//! Verify the timestamped Authenticode publisher and launch the per-user setup.
use super::ReleaseAsset;
use std::{
    io::{BufRead, BufReader, Write},
    os::windows::{ffi::OsStringExt, process::CommandExt},
    path::PathBuf,
    process::{Child, ChildStdout, Command, Stdio},
};
use tempfile::TempPath;

pub struct PendingSetup {
    child: Child,
    output: BufReader<ChildStdout>,
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
            .spawn_blocking(move || prepare(stage, &asset.sha256, publisher))
            .await
            .ok()
            .flatten()
    }

    pub async fn launch_windows_setup(&self, mut setup: PendingSetup) -> bool {
        if self.recording_background_needed() {
            return false;
        }
        // Closing stdin without this fixed message cancels a pending handoff.
        if setup
            .child
            .stdin
            .take()
            .is_none_or(|mut pipe| pipe.write_all(b"install\n").is_err())
        {
            return false;
        }
        self.services()
            .spawn_blocking(move || message(&mut setup.output, "started"))
            .await
            .unwrap_or(false)
    }
}

fn message(output: &mut BufReader<ChildStdout>, expected: &str) -> bool {
    let mut line = String::new();
    output.read_line(&mut line).is_ok() && line.trim_end() == expected
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

fn prepare(stage: TempPath, digest: &str, publisher: &str) -> Option<PendingSetup> {
    let system = system_directory()?;
    let interpreter = system.join(r"WindowsPowerShell\v1.0\powershell.exe");
    let path = stage.keep().ok()?;
    let child = Command::new(interpreter)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            include_str!("windows.ps1"),
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW for the verifier, installer has its own UI.
        .env("PATINA_STAGED_SETUP", &path)
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
    let setup = child.ok().and_then(|mut child| {
        let mut output = BufReader::new(child.stdout.take()?);
        message(&mut output, "verified").then_some(PendingSetup { child, output })
    });
    if setup.is_none() {
        let _ = std::fs::remove_file(path);
    }
    setup
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    #[test]
    fn refuses_and_cleans_up_unsigned_or_changed_installer_bytes() {
        let dir = tempfile::tempdir().unwrap();
        for expected in [b"synthetic setup".as_slice(), b"different"] {
            let mut file = tempfile::Builder::new()
                .suffix(".exe")
                .tempfile_in(dir.path())
                .unwrap();
            file.write_all(b"synthetic setup").unwrap();
            let path = file.into_temp_path();
            let location = path.to_path_buf();
            let digest = format!("{:x}", Sha256::digest(expected));
            assert!(prepare(path, &digest, "CN=Synthetic Patina").is_none());
            assert!(!location.exists());
        }
    }
}
