//! Full-file AppImage replacement on the same filesystem as the running image.
use std::{
    fs::{self, File},
    io::{self, Read},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use tempfile::TempPath;

#[derive(Clone, Debug)]
pub struct RunningImage {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl RunningImage {
    /// AppRun keeps its actual image open across exec. A checkout, extracted
    /// AppDir, renamed installation or already replaced image falls back to Save.
    fn current() -> Option<Self> {
        let path = PathBuf::from(std::env::var_os("APPIMAGE")?);
        let fd = std::env::var("PATINA_APPIMAGE_FD")
            .ok()?
            .parse::<u32>()
            .ok()?;
        let running = fs::metadata(format!("/proc/self/fd/{fd}")).ok()?;
        let image = Self {
            path,
            device: running.dev(),
            inode: running.ino(),
        };
        image.inspect().ok()?;
        Some(image)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn inspect(&self) -> io::Result<()> {
        let meta = fs::symlink_metadata(&self.path)?;
        if !self.path.is_absolute()
            || !meta.is_file()
            || meta.permissions().readonly()
            || meta.dev() != self.device
            || meta.ino() != self.inode
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(())
    }

    fn replace(&self, stage: TempPath) -> io::Result<PathBuf> {
        let installed = File::open(&self.path)?;
        installed.try_lock().map_err(io::Error::other)?;
        self.inspect()?;
        let mut name = self.path.as_os_str().to_owned();
        name.push(concat!(".", env!("CARGO_PKG_VERSION"), ".previous"));
        let previous = PathBuf::from(name);
        let parent = self.path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        executable(&stage, installed.metadata()?.permissions().mode() & 0o755)?;
        let directory = File::open(parent)?;
        match fs::hard_link(&self.path, &previous) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let backup = fs::symlink_metadata(&previous)?;
                if !backup.is_file() || backup.dev() != self.device || backup.ino() != self.inode {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
        directory.sync_all()?;
        stage.persist(&self.path)?;
        directory.sync_all()?;
        Ok(previous)
    }
}

fn executable(path: &Path, permissions: u32) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut header = [0; 20];
    file.read_exact(&mut header)?;
    if &header[..6] != b"\x7fELF\x02\x01"
        || &header[8..11] != b"AI\x02"
        || header[18..20] != [62, 0]
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    file.set_permissions(fs::Permissions::from_mode(permissions | 0o700))?;
    file.sync_all()
}

impl crate::Runtime {
    pub async fn running_appimage(&self) -> Option<RunningImage> {
        self.services()
            .spawn_blocking(RunningImage::current)
            .await
            .ok()
            .flatten()
    }

    /// Keep a recovery copy until the viewer has launched and checked the new build.
    pub async fn replace_appimage(&self, image: RunningImage, stage: TempPath) -> Option<PathBuf> {
        self.services()
            .spawn_blocking(move || image.replace(stage).ok())
            .await
            .ok()
            .flatten()
    }

    /// Save to a new name chosen by the viewer, including on another filesystem
    /// when the verified download was staged in that destination directory.
    pub async fn save_appimage(&self, destination: PathBuf, stage: TempPath) -> bool {
        self.services()
            .spawn_blocking(move || {
                let parent = destination.parent().ok_or(io::ErrorKind::InvalidInput)?;
                executable(&stage, 0o700)?;
                stage.persist_noclobber(&destination)?;
                File::open(parent)?.sync_all()
            })
            .await
            .is_ok_and(|result| result.is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn staged(dir: &Path) -> TempPath {
        let mut candidate = tempfile::NamedTempFile::new_in(dir).unwrap();
        candidate
            .write_all(&[
                0x7f, b'E', b'L', b'F', 2, 1, 1, 0, b'A', b'I', 2, 0, 0, 0, 0, 0, 0, 0, 62, 0,
            ])
            .unwrap();
        candidate.write_all(b"synthetic new image").unwrap();
        candidate.into_temp_path()
    }

    #[test]
    fn replaces_only_the_running_inode_and_preserves_recovery_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Patina.AppImage");
        fs::write(&path, b"synthetic old image").unwrap();
        let meta = path.metadata().unwrap();
        let image = RunningImage {
            path: path.clone(),
            device: meta.dev(),
            inode: meta.ino(),
        };
        let previous = image.replace(staged(dir.path())).unwrap();
        assert_eq!(fs::read(&previous).unwrap(), b"synthetic old image");
        assert!(fs::read(&path).unwrap().ends_with(b"synthetic new image"));
        assert_ne!(path.metadata().unwrap().permissions().mode() & 0o100, 0);
        assert!(image.replace(staged(dir.path())).is_err());
        assert_eq!(fs::read(&previous).unwrap(), b"synthetic old image");
    }
    #[test]
    fn invalid_download_readonly_image_and_existing_backup_preserve_the_installation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Patina.AppImage");
        fs::write(&path, b"original").unwrap();
        let meta = path.metadata().unwrap();
        let image = RunningImage {
            path: path.clone(),
            device: meta.dev(),
            inode: meta.ino(),
        };
        let bad = tempfile::NamedTempFile::new_in(dir.path())
            .unwrap()
            .into_temp_path();
        assert!(image.replace(bad).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).unwrap();
        assert!(image.replace(staged(dir.path())).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let backup = dir.path().join(concat!(
            "Patina.AppImage.",
            env!("CARGO_PKG_VERSION"),
            ".previous"
        ));
        fs::write(&backup, b"unrelated").unwrap();
        assert!(image.replace(staged(dir.path())).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read(&backup).unwrap(), b"unrelated");
    }
}
