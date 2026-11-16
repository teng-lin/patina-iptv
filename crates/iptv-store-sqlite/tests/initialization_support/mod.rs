//! Fresh-process checks must never leave a stuck child behind.
use std::fs::{self, File};
use std::{
    process::{Command, Output},
    time::{Duration, Instant},
};

pub fn child(test: &str, marker: &str, timeout: Duration) -> Output {
    // A verbose failure must not fill a pipe while the parent waits for exit.
    let captured = tempfile::tempdir().unwrap();
    let stdout = captured.path().join("stdout");
    let stderr = captured.path().join("stderr");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--include-ignored", "--nocapture"])
        .env(marker, "1")
        .stdout(File::create(&stdout).unwrap())
        .stderr(File::create(&stderr).unwrap())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + timeout;
    let output = |status| Output {
        status,
        stdout: fs::read(&stdout).unwrap(),
        stderr: fs::read(&stderr).unwrap(),
    };
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = output(status);
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("running 1 test"),
                "child selector {test:?} must execute exactly one test:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return output;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = output(child.wait().unwrap());
            panic!(
                "test child {test:?} timed out and was reaped:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
