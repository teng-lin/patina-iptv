//! Commit the verifier's last accepted root and rollback state together.
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

use tempfile::{NamedTempFile, TempDir};

const FILES: &[&str] = &[
    "root.json",
    "targets.json",
    "snapshot.json",
    "timestamp.json",
    "latest_known_time.json",
];

pub(super) struct Session {
    _lock: File,
    directory: PathBuf,
    pub(super) work: TempDir,
    pub(super) root: Vec<u8>,
}

impl Session {
    pub(super) fn open(directory: &Path, embedded: &[u8]) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("lock"))?;
        lock.try_lock().map_err(io::Error::other)?;
        let work = tempfile::tempdir_in(directory)?;
        let mut root = embedded.to_vec();
        match fs::read(directory.join("trusted.json")) {
            Ok(bytes) => {
                let files: BTreeMap<String, String> = serde_json::from_slice(&bytes)?;
                for &name in FILES {
                    let contents = files
                        .get(name)
                        .ok_or_else(|| io::Error::other("incomplete update state"))?;
                    if name == "root.json" && version(contents.as_bytes())? > version(embedded)? {
                        root = contents.as_bytes().to_vec();
                    }
                    fs::write(work.path().join(name), contents)?;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok(Self {
            _lock: lock,
            directory: directory.to_owned(),
            work,
            root,
        })
    }

    pub(super) fn commit(&self) -> io::Result<()> {
        let mut files = BTreeMap::new();
        for &name in FILES {
            files.insert(name, fs::read_to_string(self.work.path().join(name))?);
        }
        let mut state = NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer(&mut state, &files)?;
        state.as_file().sync_all()?;
        state.persist(self.directory.join("trusted.json"))?;
        #[cfg(unix)]
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}

fn version(root: &[u8]) -> io::Result<u64> {
    let root: serde_json::Value = serde_json::from_slice(root)?;
    root["signed"]["version"]
        .as_u64()
        .ok_or_else(|| io::Error::other("invalid trusted root version"))
}
