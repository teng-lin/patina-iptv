//! Fresh-process checks must never leave a stuck child behind.
use std::fs::{self, File};
use std::{
    io,
    process::{Command, Output},
    time::{Duration, Instant},
};

pub fn child(test: &str, marker: &str, timeout: Duration) -> io::Result<Output> {
    // A verbose failure must not fill a pipe while the parent waits for exit.
    let captured = tempfile::tempdir()?;
    let stdout = captured.path().join("stdout");
    let stderr = captured.path().join("stderr");
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--exact", test, "--include-ignored", "--nocapture"])
        .env(marker, "1")
        .stdout(File::create(&stdout)?)
        .stderr(File::create(&stderr)?)
        .spawn()?;
    let deadline = Instant::now() + timeout;
    let output = |status| -> io::Result<Output> {
        Ok(Output {
            status,
            stdout: fs::read(&stdout)?,
            stderr: fs::read(&stderr)?,
        })
    };
    loop {
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        if let Some(status) = status {
            let output = output(status)?;
            if !String::from_utf8_lossy(&output.stdout).contains("running 1 test") {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "child selector {test:?} must execute exactly one test:\n{}\n{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ),
                ));
            }
            return Ok(output);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = output(child.wait()?)?;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "test child {test:?} timed out and was reaped:\n{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
