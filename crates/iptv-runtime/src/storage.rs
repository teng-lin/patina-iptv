//! Native private-directory protection shared by startup and recovery tests.
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

/// Serializes catalog file replacement, startup restore activation, and store
/// opens across independent owners sharing the same private storage directory.
pub(crate) fn with_directory_replacement<T>(directory: &Path, run: impl FnOnce() -> T) -> T {
    static REPLACEMENT: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let gate = {
        let mut gates = REPLACEMENT
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(directory).and_then(Weak::upgrade) {
            gate
        } else {
            let gate = Arc::new(Mutex::new(()));
            gates.insert(directory.to_owned(), Arc::downgrade(&gate));
            gate
        }
    };
    let _guard = gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    run()
}

pub fn sibling(root: &Path, suffix: &str) -> PathBuf {
    root.with_file_name(format!(
        "{}.{}",
        root.file_name().unwrap_or_default().to_string_lossy(),
        suffix
    ))
}

/// Correct old installations too. Never follow a link out of private storage.
pub fn protect_directory(path: &Path, exclude: fn(&Path) -> io::Result<()>) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    protect_tree(path)?;
    exclude(path)
}

fn protect_tree(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Reject junctions and all other reparse points, not just symlinks.
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "App storage contains a reparse point",
            ));
        }
    }
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "App storage contains a symbolic link",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if metadata.is_dir() { 0o700 } else { 0o600 }),
        )?;
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            protect_tree(&entry?.path())?;
        }
    }
    Ok(())
}
