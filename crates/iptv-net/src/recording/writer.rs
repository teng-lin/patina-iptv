//! Byte-bounded file worker. Accepted writes drain before the finish acknowledgement.
use super::{Failure, Sink};
use std::path::Path;
use tokio::sync::{Semaphore, mpsc, oneshot};

const CHUNK_BYTES: usize = 256 * 1024;
// Includes workers whose async owner was dropped during an uninterruptible OS write.
static WORKERS: Semaphore = Semaphore::const_new(8);

type Chunk = (Vec<u8>, oneshot::Sender<Result<(), Failure>>);

/// Owns a recording spool until all accepted writes have stopped.
///
/// Keep this value outside a cancelable capture future and call [`Self::finish`]
/// before renaming or deleting the spool. Dropping it requests shutdown but cannot
/// interrupt an OS write; worker capacity remains reserved until actual termination.
pub struct CaptureFile {
    sender: mpsc::Sender<Chunk>,
    finished: oneshot::Receiver<Result<u64, Failure>>,
}
impl CaptureFile {
    /// Opens a private regular spool on its file worker, optionally retaining whole packets.
    pub async fn open(path: &Path, max_bytes: u64, resume: bool) -> Result<Self, Failure> {
        Self::open_with(path, max_bytes, resume, || Ok(())).await
    }

    async fn open_with(
        path: &Path,
        max_bytes: u64,
        resume: bool,
        before_write: impl FnOnce() -> std::io::Result<()> + Send + 'static,
    ) -> Result<Self, Failure> {
        let permit = WORKERS.acquire().await.map_err(|_| Failure::Limit)?;
        let path = path.to_owned();
        let (sender, mut input) = mpsc::channel::<Chunk>(1);
        let (ready, opened) = oneshot::channel();
        let (done, finished) = oneshot::channel();
        std::thread::Builder::new()
            .name("iptv-capture-file".into())
            .spawn(move || {
                let _permit = permit;
                let mut writer = match super::writer(&path, max_bytes, resume) {
                    Ok(writer) => writer,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if ready.send(Ok(())).is_err() {
                    return;
                }
                let mut result = Ok(());
                let mut before_write = Some(before_write);
                while let Some((bytes, reply)) = input.blocking_recv() {
                    if let Some(before_write) = before_write.take() {
                        result = before_write().map_err(|_| Failure::Storage);
                    }
                    if result.is_ok() {
                        result = writer.write_blocking(&bytes);
                    }
                    let _ = reply.send(result);
                }
                // Even cancellation acknowledges descriptor closure only after sync.
                let closed = writer
                    .file
                    .set_len(writer.bytes / 188 * 188)
                    .and_then(|()| writer.file.sync_all())
                    .map_err(|_| Failure::Storage);
                let bytes = writer.bytes / 188 * 188;
                drop(writer);
                let _ = done.send(result.and(closed).map(|()| bytes));
            })
            .map_err(|_| Failure::Storage)?;
        opened.await.map_err(|_| Failure::Storage)??;
        Ok(Self { sender, finished })
    }

    /// Closes admission, drains accepted chunks, syncs and closes the file.
    pub async fn finish(self) -> Result<u64, Failure> {
        let Self { sender, finished } = self;
        drop(sender);
        finished.await.map_err(|_| Failure::Storage)?
    }
}
impl Sink for CaptureFile {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        for bytes in bytes.chunks(CHUNK_BYTES) {
            let (reply, answer) = oneshot::channel();
            self.sender
                .send((bytes.to_vec(), reply))
                .await
                .map_err(|_| Failure::Storage)?;
            answer.await.map_err(|_| Failure::Storage)??;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    fn path() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "synthetic-recording-writer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[tokio::test]
    async fn drain_acknowledgement_keeps_executor_live_and_precedes_rename() {
        let path = path();
        let (entered, started) = oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let mut writer = CaptureFile::open_with(&path, 188 * 10, false, move || {
            let _ = entered.send(());
            wait.recv_timeout(Duration::from_secs(10))
                .expect("release synthetic disk");
            Ok(())
        })
        .await
        .unwrap();
        {
            let bytes = vec![0x47; 188];
            let write = writer.write(&bytes);
            tokio::pin!(write);
            tokio::select! {
                result = &mut write => panic!("premature write: {result:?}"),
                _ = started => {},
            }
        }
        let finish = writer.finish();
        tokio::pin!(finish);
        tokio::select! {
            result = &mut finish => panic!("finish must await disk acknowledgement: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(10)) => {},
        }
        release.send(()).unwrap();
        assert_eq!(finish.await.unwrap(), 188);
        let destination = path.with_extension("ts");
        std::fs::rename(&path, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), vec![0x47; 188]);
        std::fs::remove_file(destination).unwrap();
    }

    #[tokio::test]
    async fn disk_full_is_reported_by_write_and_drain_without_successful_media() {
        let path = path();
        let mut writer = CaptureFile::open_with(&path, 188 * 10, false, || {
            Err(std::io::ErrorKind::StorageFull.into())
        })
        .await
        .unwrap();
        assert_eq!(writer.write(&[0x47; 188]).await, Err(Failure::Storage));
        assert_eq!(writer.finish().await, Err(Failure::Storage));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn byte_limit_failure_drains_and_reports_failure_without_discarding_prefix() {
        let path = path();
        let mut writer = CaptureFile::open(&path, 188 * 2, false).await.unwrap();
        writer.write(&[0x47; 188]).await.unwrap();
        assert_eq!(writer.write(&[0x47; 188 * 2]).await, Err(Failure::Limit));
        assert_eq!(writer.finish().await, Err(Failure::Limit));
        assert_eq!(std::fs::read(&path).unwrap().len(), 188);
        std::fs::remove_file(path).unwrap();
    }
}
