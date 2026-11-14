//! Native private-directory protection shared by startup and recovery tests.
use std::{fs, io, path::Path};

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
