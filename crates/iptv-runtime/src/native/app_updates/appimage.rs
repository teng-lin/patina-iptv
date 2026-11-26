//! Full-file AppImage replacement on the same filesystem as the running image.
use rustix::fs::{AtFlags, CWD, RenameFlags, linkat, renameat_with};
use std::{
    fs::{self, File},
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, PermissionsExt},
    },
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
    /// The AppImage launcher records the installed file's identity at startup.
    fn current() -> Option<Self> {
        let path = PathBuf::from(std::env::var_os("APPIMAGE")?);
        let id = std::env::var("PATINA_APPIMAGE_ID").ok()?;
        let (device, inode) = id.split_once(':')?;
        let image = Self {
            path,
            device: device.parse().ok()?,
            inode: inode.parse().ok()?,
        };
        image.inspect().ok()?;
        Some(image)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn matches(&self, meta: &fs::Metadata) -> bool {
        meta.is_file() && meta.dev() == self.device && meta.ino() == self.inode
    }

    fn inspect(&self) -> io::Result<()> {
        let meta = fs::symlink_metadata(&self.path)?;
        if !self.path.is_absolute() || meta.permissions().readonly() || !self.matches(&meta) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(())
    }

    fn replace(&self, stage: TempPath) -> io::Result<PathBuf> {
        let installed = File::open(&self.path)?;
        installed.try_lock().map_err(io::Error::other)?;
        self.inspect()?;
        let installed_meta = installed.metadata()?;
        if !self.matches(&installed_meta) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut name = self.path.as_os_str().to_owned();
        name.push(concat!(".", env!("CARGO_PKG_VERSION"), ".previous"));
        let previous = PathBuf::from(name);
        let parent = self.path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        if !trusted_directory(parent) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        executable(&stage, installed_meta.permissions().mode() & 0o755)?;
        let directory = File::open(parent)?;
        let source = format!("/proc/self/fd/{}", installed.as_raw_fd());
        match linkat(
            CWD,
            source.as_str(),
            CWD,
            &previous,
            AtFlags::SYMLINK_FOLLOW,
        )
        .map_err(io::Error::from)
        {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let backup = fs::symlink_metadata(&previous)?;
                if !self.matches(&backup) {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
        directory.sync_all()?;
        self.exchange(stage)?;
        directory.sync_all()?;
        Ok(previous)
    }

    fn exchange(&self, stage: TempPath) -> io::Result<()> {
        let candidate = fs::symlink_metadata(&stage)?;
        let exchange = || renameat_with(CWD, &*stage, CWD, &self.path, RenameFlags::EXCHANGE);
        exchange()?;
        // The displaced inode tells us exactly what the atomic exchange replaced.
        // An external rename immediately before the exchange must not lose a file.
        if fs::symlink_metadata(&stage).is_ok_and(|old| self.matches(&old)) {
            return Ok(());
        }
        let still_candidate = fs::symlink_metadata(&self.path)
            .is_ok_and(|now| now.dev() == candidate.dev() && now.ino() == candidate.ino());
        if !still_candidate || exchange().is_err() {
            // The directory changed again: leave the displaced file available for recovery.
            let _ = stage.keep();
        }
        Err(io::ErrorKind::AlreadyExists.into())
    }
}

fn trusted_directory(path: &Path) -> bool {
    // SAFETY: geteuid reads the calling process's effective user ID without arguments.
    let uid = unsafe { libc::geteuid() };
    fs::metadata(path).is_ok_and(|meta| {
        meta.is_dir()
            && (meta.uid() == uid || meta.uid() == 0)
            && (meta.mode() & 0o022 == 0 || meta.mode() & 0o1000 != 0)
    })
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
    pub async fn can_stage_appimage(
        &self,
        directory: PathBuf,
        destination: Option<PathBuf>,
    ) -> bool {
        self.services()
            .spawn_blocking(move || {
                trusted_directory(&directory)
                    && destination.is_none_or(|path| {
                        fs::symlink_metadata(path)
                            .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
                    })
            })
            .await
            .unwrap_or(false)
    }

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
                if !trusted_directory(parent) {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
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
    fn exchange_restores_a_file_changed_by_an_external_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Patina.AppImage");
        let old = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        let meta = old.as_file().metadata().unwrap();
        let image = RunningImage {
            path: path.clone(),
            device: meta.dev(),
            inode: meta.ino(),
        };
        fs::write(&path, b"external replacement").unwrap();
        assert!(image.exchange(staged(dir.path())).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"external replacement");
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
