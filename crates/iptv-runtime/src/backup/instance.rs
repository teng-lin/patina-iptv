//! Hold each application storage lock for the process lifetime. A lock is a
//! sibling of its data directory, so activating a restore never moves it.
use super::error::{Context, Error, Result};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const ALREADY_RUNNING: &str =
    "Patina is already running. Close the other instance before opening it again";
/// The storage roots this process holds, each with its lock file. An
/// operating-system lock belongs to the process and its directory, not to
/// an application owner: every owner of a root shares its entry.
static HELD: Mutex<Vec<(PathBuf, File)>> = Mutex::new(Vec::new());

/// Acquire before settings, restore activation or any storage worker starts.
/// Activity/root recreation in this process reuses the already-held lock.
pub(super) fn acquire(root: &Path) -> Result<()> {
    let mut held = HELD.lock().context("Could not lock application storage")?;
    if held.iter().any(|(locked, _)| locked == root) {
        return Ok(());
    }
    let name = root
        .file_name()
        .ok_or("Invalid application storage directory")?;
    let parent = root.parent().ok_or("Application storage has no parent")?;
    std::fs::create_dir_all(parent).context("Could not open application storage")?;
    let mut lock_name = name.to_os_string();
    lock_name.push(".instance-lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(parent.join(lock_name))
        .context("Could not open the application storage lock")?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Err(ALREADY_RUNNING.into()),
        Err(std::fs::TryLockError::Error(error)) => {
            return Err(Error::new("Could not lock application storage", error));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .context("Could not protect the application storage lock")?;
    }
    held.push((root.to_owned(), file));
    // Do not unlink or explicitly unlock: every app-owned detached worker may
    // still have database handles until this process exits.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Command, Stdio};
    const ROOT: &str = "IPTV_RS_INSTANCE_LOCK_TEST_ROOT";
    const MODE: &str = "IPTV_RS_INSTANCE_LOCK_TEST_MODE";
    const SECOND_ROOT: &str = "IPTV_RS_INSTANCE_LOCK_TEST_SECOND_ROOT";
    const CHILD: &str = "backup::instance::tests::lock_child_process";

    fn child(root: &Path, mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", CHILD, "--nocapture"])
            .env(ROOT, root)
            .env(MODE, mode);
        command
    }
    #[test]
    fn lock_child_process() {
        let Some(root) = std::env::var_os(ROOT) else {
            return;
        };
        let root = Path::new(&root);
        match std::env::var(MODE).unwrap().as_str() {
            "holder" => {
                acquire(root).unwrap();
                acquire(root).unwrap(); // Same-process root recreation is safe.
                if let Some(second) = std::env::var_os(SECOND_ROOT) {
                    acquire(Path::new(&second)).unwrap();
                    acquire(Path::new(&second)).unwrap();
                }
                println!("instance-lock-ready");
                std::io::stdout().flush().unwrap();
                let mut release = [0];
                std::io::stdin().read_exact(&mut release).unwrap();
            }
            "contender" => assert_eq!(
                acquire(root).map_err(|error| error.message()),
                Err(ALREADY_RUNNING)
            ),
            "successor" => acquire(root).unwrap(),
            mode => panic!("unknown synthetic child mode: {mode}"),
        }
    }
    #[test]
    fn another_process_is_rejected_during_restore_directory_swap_until_owner_exits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("synthetic-app");
        std::fs::create_dir(&root).unwrap();
        let mut holder = child(&root, "holder")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(holder.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert!(
                output.read_line(&mut line).unwrap() > 0,
                "holder ended before acquiring the lock"
            );
            if line.contains("instance-lock-ready") {
                break;
            }
        }
        // The data directory can be moved and replaced while the held lock
        // remains outside it. A second process still cannot open that storage.
        std::fs::rename(&root, dir.path().join("synthetic-app.restore-previous")).unwrap();
        std::fs::create_dir(&root).unwrap();
        assert!(child(&root, "contender").output().unwrap().status.success());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.path().join("synthetic-app.instance-lock"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        holder.stdin.take().unwrap().write_all(b"x").unwrap();
        assert!(holder.wait().unwrap().success());
        assert!(child(&root, "successor").output().unwrap().status.success());
    }

    #[test]
    fn one_process_locks_each_storage_root_it_opens() {
        let directory = tempfile::tempdir().unwrap();
        let alpha = directory.path().join("alpha");
        let beta = directory.path().join("beta");
        let mut holder = child(&alpha, "holder")
            .env(SECOND_ROOT, &beta)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(holder.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert!(
                output.read_line(&mut line).unwrap() > 0,
                "holder exited before locking both roots"
            );
            if line.contains("instance-lock-ready") {
                break;
            }
        }
        for root in [&alpha, &beta] {
            let result = child(root, "contender").output().unwrap();
            assert!(
                result.status.success(),
                "{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        holder.stdin.take().unwrap().write_all(b"x").unwrap();
        assert!(holder.wait().unwrap().success());
        for root in [&alpha, &beta] {
            assert!(child(root, "successor").output().unwrap().status.success());
        }
    }
}
