//! Verify the timestamped Authenticode publisher and launch the per-user setup.
use super::ReleaseAsset;
use std::{
    io::{BufRead, BufReader, Write},
    os::windows::process::CommandExt,
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
            .spawn_blocking(move || prepare(stage, &asset, publisher))
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

fn prepare(stage: TempPath, asset: &ReleaseAsset, publisher: &str) -> Option<PendingSetup> {
    let root = std::env::var_os("SystemRoot")?;
    let system = PathBuf::from(root).join("System32");
    if !system.is_absolute() {
        return None;
    }
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
        .env("PATINA_SETUP_DIGEST", &asset.sha256)
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
