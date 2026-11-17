//! Local playlist/guide files (`file://` URLs and absolute paths).

use std::path::PathBuf;

use iptv_import::{FetchError, FetchErrorKind};

/// The local path a request URL names, if it names one.
pub(crate) fn local_path(url: &str) -> Option<Result<PathBuf, FetchError>> {
    let url = url.trim();
    if url
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
    {
        return Some(
            url::Url::parse(url)
                .ok()
                .and_then(|u| u.to_file_path().ok())
                .ok_or_else(|| {
                    FetchError::new(FetchErrorKind::InvalidRequest, "not a local file URL")
                }),
        );
    }
    (url.starts_with('/') || std::path::Path::new(url).is_absolute())
        .then(|| Ok(PathBuf::from(url)))
}

pub(crate) fn io_error(e: &std::io::Error) -> FetchError {
    // The kind only: the message may repeat the path.
    FetchError::new(FetchErrorKind::Io, e.kind().to_string())
}

pub(crate) async fn open(path: &PathBuf) -> Result<(tokio::fs::File, Option<u64>), FetchError> {
    let meta = tokio::fs::metadata(path).await.map_err(|e| io_error(&e))?;
    if !meta.is_file() {
        return Err(FetchError::new(FetchErrorKind::Io, "not a regular file"));
    }
    open_regular(path).await
}

/// Opens `path` and checks what was opened, not what the name pointed to a
/// moment earlier.
///
/// The name can be replaced between the check in [`open`] and this call.
/// Opening a FIFO for reading waits for a writer, forever if none comes,
/// on a blocking thread no timeout can release. On Unix the file is
/// therefore opened without blocking; a regular file reads the same either
/// way, and anything else is refused by the check that follows.
async fn open_regular(path: &PathBuf) -> Result<(tokio::fs::File, Option<u64>), FetchError> {
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let file = options.open(path).await.map_err(|e| io_error(&e))?;
    let meta = file.metadata().await.map_err(|e| io_error(&e))?;
    if !meta.is_file() {
        return Err(FetchError::new(FetchErrorKind::Io, "not a regular file"));
    }
    Ok((file, Some(meta.len())))
}

/// Access to app-owned recording files. Picked playlists retain their separate
/// symlink policy; these private media paths must never follow a replacement.
pub(crate) enum PrivateAccess {
    Read,
    Resume,
    Create,
}

/// Acquire first, then validate the actual descriptor before any read/truncate.
/// Nonblocking Unix acquisition makes a substituted FIFO fail without waiting
/// for its peer on a blocking worker. Windows opens the reparse point itself.
pub(crate) fn open_private_regular(
    path: &std::path::Path,
    access: PrivateAccess,
) -> std::io::Result<(std::fs::File, std::fs::Metadata)> {
    let mut options = std::fs::OpenOptions::new();
    match access {
        PrivateAccess::Read => {
            options.read(true);
        }
        PrivateAccess::Resume => {
            options.write(true);
        }
        PrivateAccess::Create => {
            options.write(true).create_new(true);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || private_reparse_point(&metadata) {
        return Err(std::io::ErrorKind::PermissionDenied.into());
    }
    Ok((file, metadata))
}

#[cfg(windows)]
fn private_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}
#[cfg(not(windows))]
fn private_reparse_point(_: &std::fs::Metadata) -> bool {
    false
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn a_fifo_swapped_in_after_the_check_is_refused_without_waiting_for_a_writer() {
        let dir = std::env::temp_dir().join(format!("iptv-net-fifo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("list.m3u");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        // What `open` does once its own check has passed.
        let opened = tokio::time::timeout(Duration::from_secs(5), open_regular(&fifo)).await;
        let regular = dir.join("regular.m3u");
        std::fs::write(&regular, b"#EXTM3U\n").unwrap();
        let read = open_regular(&regular).await;
        std::fs::remove_dir_all(&dir).unwrap();

        let error = opened
            .expect("the open must not wait for a writer")
            .unwrap_err();
        assert_eq!(error.kind, FetchErrorKind::Io);
        assert_eq!(error.detail, "not a regular file");
        assert_eq!(read.unwrap().1, Some(8));
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn native_absolute_paths_and_file_urls_open_the_same_local_file() {
        let expected = PathBuf::from("C:/Synthetic Data/list.m3u");
        for input in [
            "C:/Synthetic Data/list.m3u",
            "file:///C:/Synthetic%20Data/list.m3u",
        ] {
            assert_eq!(local_path(input).unwrap().unwrap(), expected);
        }
        assert!(local_path("C:relative.m3u").is_none());
        assert!(local_path("https://example.test/list.m3u").is_none());
    }
}

#[cfg(test)]
mod private_tests {
    use super::*;

    fn directory() -> PathBuf {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).unwrap();
        let path = std::env::temp_dir().join(format!("patina-private-recording-{random:x?}"));
        std::fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn private_reads_keep_the_opened_handle_and_its_metadata() {
        use std::io::Read;
        let dir = directory();
        let path = dir.join("recording.ts");
        std::fs::write(&path, b"synthetic old recording").unwrap();
        let (mut file, metadata) = open_private_regular(&path, PrivateAccess::Read).unwrap();
        // Replacing the name must not change which bytes or length are served.
        std::fs::rename(&path, dir.join("old.ts")).unwrap();
        std::fs::write(&path, b"new").unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"synthetic old recording");
        assert_eq!(metadata.len(), bytes.len() as u64);
        drop(file);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn private_reads_and_resume_refuse_a_substituted_symlink() {
        let dir = directory();
        let path = dir.join("recording.ts");
        let target = dir.join("target.ts");
        std::fs::write(&path, b"synthetic old recording").unwrap();
        std::fs::write(&target, b"synthetic target must remain intact").unwrap();
        assert!(std::fs::symlink_metadata(&path).unwrap().is_file());
        std::fs::remove_file(&path).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &path).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, &path)
            .expect("Windows reparse regression requires symlink creation permission");
        for access in [PrivateAccess::Read, PrivateAccess::Resume] {
            assert!(open_private_regular(&path, access).is_err());
        }
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"synthetic target must remain intact"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_substituted_fifo_never_waits_for_a_reader_or_writer() {
        const CHILD: &str = "PATINA_NET_PRIVATE_FIFO_CHILD";
        const TEST: &str =
            "file::private_tests::a_substituted_fifo_never_waits_for_a_reader_or_writer";
        if std::env::var(CHILD).as_deref() != Ok(TEST) {
            // A regression must fail within this same executable, not hang the
            // test runtime's uninterruptible blocking worker during shutdown.
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env(CHILD, TEST)
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success(), "FIFO child failed: {status}");
                    return;
                }
                if std::time::Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("private recording open waited for a FIFO peer");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        let dir = directory();
        let path = dir.join("recording.ts");
        std::fs::write(&path, b"synthetic old recording").unwrap();
        assert!(std::fs::symlink_metadata(&path).unwrap().is_file());
        std::fs::remove_file(&path).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        for access in [PrivateAccess::Read, PrivateAccess::Resume] {
            assert!(open_private_regular(&path, access).is_err());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
