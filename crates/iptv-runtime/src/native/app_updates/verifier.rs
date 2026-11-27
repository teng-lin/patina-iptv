//! Cancellable wait for the Windows installer's isolated verifier process.
use std::{
    io::{BufRead, BufReader},
    process::Child,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub(super) struct CancelOnDrop(pub Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verification {
    Digest,
    Signature,
    Unavailable,
}

pub(super) fn verified(
    child: &mut Child,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<(), Verification> {
    let output = child.stdout.take().ok_or(Verification::Unavailable)?;
    let (send, recv) = mpsc::channel();
    let reader = std::thread::Builder::new()
        .spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(output).read_line(&mut line).map(|_| line);
            let _ = send.send(result);
        })
        .map_err(|_| Verification::Unavailable)?;
    let deadline = Instant::now() + timeout;
    let result = loop {
        if cancelled.load(Ordering::Acquire) {
            break Err(Verification::Unavailable);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break Err(Verification::Unavailable);
        }
        match recv.recv_timeout(remaining.min(Duration::from_millis(20))) {
            Ok(Ok(line)) => {
                break match line.trim_end() {
                    "verified" => Ok(()),
                    "digest" => Err(Verification::Digest),
                    "signature" => Err(Verification::Signature),
                    _ => Err(Verification::Unavailable),
                };
            }
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err(Verification::Unavailable);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    };
    if result.is_err() {
        // Closing the process first releases stdout and its staged-file lock.
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = reader.join();
    result
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    #[test]
    fn caller_drop_interrupts_a_held_verifier_and_reaps_it_before_stage_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let mut stage = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        stage.write_all(b"synthetic installer").unwrap();
        let path = stage.path().to_owned();
        let marker = directory.path().join("started");
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exec 3<\"$PATINA_STAGED_SETUP\"; printf started > \"$PATINA_TEST_MARKER\"; exec sleep 60"])
            .env("PATINA_STAGED_SETUP", &path)
            .env("PATINA_TEST_MARKER", &marker)
            .stdout(Stdio::piped())
            .spawn().unwrap();
        let cancellation = Arc::new(AtomicBool::new(false));
        let caller = CancelOnDrop(Arc::clone(&cancellation));
        let worker = std::thread::spawn(move || {
            let result = verified(&mut child, Duration::from_secs(90), &cancellation);
            assert!(
                child.try_wait().unwrap().is_some(),
                "the verifier is reaped"
            );
            drop(stage);
            result
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            assert!(Instant::now() < deadline, "verifier did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        let cancelled_at = Instant::now();
        drop(caller);
        assert_eq!(worker.join().unwrap(), Err(Verification::Unavailable));
        assert!(cancelled_at.elapsed() < Duration::from_secs(2));
        assert!(!path.exists(), "the private installer stage is cleaned");
    }
}
