//! What a backup operation failed with: the message the user is shown, and
//! the cause underneath for the log.
//!
//! A cause is a class, never the failed call's own text: operating-system
//! and SQLite errors name the files they failed on, and a backup's files
//! sit beside the user's playlists and logins. So an I/O failure keeps its
//! [`io::ErrorKind`] and a library failure the kind of store error, as the
//! rest of the app logs them.
use std::{fmt, io};

use iptv_store_sqlite::StoreError;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    message: &'static str,
    cause: Cause,
}

/// Why, as far as it may be logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// A rule of the backup itself refused it: the message says which.
    Rule,
    Io(io::ErrorKind),
    /// The library (SQLite, its key or its schema).
    Store(&'static str),
    /// Any other failed step, by a fixed name.
    Step(&'static str),
}

impl Error {
    pub fn new(message: &'static str, cause: impl Into<Cause>) -> Self {
        Self {
            message,
            cause: cause.into(),
        }
    }

    /// The text for the user.
    pub fn message(&self) -> &'static str {
        self.message
    }

    #[cfg(test)]
    pub fn cause(&self) -> Cause {
        self.cause
    }
}

/// For the log: the message, then the cause when there is one underneath.
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.cause {
            Cause::Rule => f.write_str(self.message),
            Cause::Io(kind) => write!(f, "{} (I/O: {kind:?})", self.message),
            Cause::Store(class) => write!(f, "{} (library: {class})", self.message),
            Cause::Step(step) => write!(f, "{} ({step})", self.message),
        }
    }
}

impl std::error::Error for Error {}

/// A refusal by rule: `return Err("…".into())`, `.ok_or("…")?`.
impl From<&'static str> for Error {
    fn from(message: &'static str) -> Self {
        Self::new(message, Cause::Rule)
    }
}

/// Gives a failed step the message the user is shown, keeping its cause.
pub trait Context<T> {
    fn context(self, message: &'static str) -> Result<T>;
}

impl<T, E: Into<Cause>> Context<T> for std::result::Result<T, E> {
    fn context(self, message: &'static str) -> Result<T> {
        self.map_err(|cause| Error::new(message, cause))
    }
}

impl From<io::Error> for Cause {
    fn from(error: io::Error) -> Self {
        Self::Io(error.kind())
    }
}

impl From<StoreError> for Cause {
    fn from(error: StoreError) -> Self {
        Self::Store(match &error {
            _ if error.is_not_database() => "not a database, or another key",
            StoreError::Unavailable(reason) => reason,
            StoreError::Sqlite(_) => "SQLite failed",
            StoreError::Migration { .. } => "a schema migration failed",
            StoreError::SchemaTooNew { .. } => "the schema is newer than this build",
            StoreError::SchemaCorrupt(_) => "the schema bookkeeping is inconsistent",
            StoreError::SourceNotFound(_) => "a playlist does not exist",
            StoreError::CustomGroupNotFound(_) => "a custom group does not exist",
            StoreError::InvalidData(_) => "a stored row is invalid",
            _ => "another store error",
        })
    }
}

impl From<serde_json::Error> for Cause {
    fn from(error: serde_json::Error) -> Self {
        use serde_json::error::Category;
        Self::Step(match error.classify() {
            Category::Io => "JSON: I/O",
            Category::Syntax => "JSON: syntax",
            Category::Data => "JSON: unexpected data",
            Category::Eof => "JSON: unexpected end",
        })
    }
}

impl From<std::path::StripPrefixError> for Cause {
    fn from(_: std::path::StripPrefixError) -> Self {
        Self::Step("the path is outside the expected directory")
    }
}

impl From<iptv_import::backup_manifest::EntryLimit> for Cause {
    fn from(_: iptv_import::backup_manifest::EntryLimit) -> Self {
        Self::Step("over the entry's size limit")
    }
}

impl From<iptv_providers::content_url::ContentUrlError> for Cause {
    fn from(_: iptv_providers::content_url::ContentUrlError) -> Self {
        Self::Step("not a usable content address")
    }
}

impl From<iptv_settings::LoadError> for Cause {
    fn from(error: iptv_settings::LoadError) -> Self {
        Self::Step(match error {
            iptv_settings::LoadError::NotADocument(_) => "settings: not a settings document",
            _ => "settings: unsupported document",
        })
    }
}

impl From<futures_channel::oneshot::Canceled> for Cause {
    fn from(_: futures_channel::oneshot::Canceled) -> Self {
        Self::Step("the task ended without an answer")
    }
}

impl<T> From<std::sync::PoisonError<T>> for Cause {
    fn from(_: std::sync::PoisonError<T>) -> Self {
        Self::Step("a thread panicked holding the lock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_line_names_the_class_of_the_cause_and_no_path() {
        let missing = std::fs::File::open("/synthetic/private/patina.patinabackup")
            .context("Could not read the chosen backup")
            .unwrap_err();
        assert_eq!(missing.message(), "Could not read the chosen backup");
        assert_eq!(missing.cause(), Cause::Io(io::ErrorKind::NotFound));
        assert_eq!(
            missing.to_string(),
            "Could not read the chosen backup (I/O: NotFound)"
        );

        let store: Result<()> =
            Err(StoreError::Unavailable("store worker stopped")).context("Could not snapshot");
        assert_eq!(
            store.unwrap_err().to_string(),
            "Could not snapshot (library: store worker stopped)"
        );

        // A refusal by the backup's own rules has nothing underneath.
        let refused = Error::from("Backup is too large");
        assert_eq!(refused.cause(), Cause::Rule);
        assert_eq!(refused.to_string(), "Backup is too large");
    }
}
