//! Errors returned by the store.
//!
//! Messages name tables, columns and ids, never credential values: SQLite
//! errors carry the statement text but not bound parameters, and every
//! secret is a bound parameter.

use std::fmt;

use crate::SourceId;

#[derive(Debug)]
#[non_exhaustive]
pub enum StoreError {
    /// The service or platform key broker is unavailable; contains no secrets.
    Unavailable(&'static str),
    /// The host must initialize storage again before the library can be opened.
    RestartRequired(&'static str),
    /// SQLite failed (I/O, constraint, busy, corrupt file).
    Sqlite(rusqlite::Error),
    /// Applying a numbered migration failed; the database stays at the
    /// version before it (all pending migrations run in one transaction).
    Migration {
        version: u32,
        source: rusqlite::Error,
    },
    /// The file was written by a newer build. Migrations are forward-only,
    /// so this build refuses to touch it.
    SchemaTooNew {
        found: u32,
        supported: u32,
    },
    /// `schema_migrations` has holes or unknown rows.
    SchemaCorrupt(String),
    SourceNotFound(SourceId),
    CustomGroupNotFound(i64),
    /// A stored row could not be decoded. Names the column, never the value.
    InvalidData(String),
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

impl StoreError {
    /// Transient contention or device write failure may succeed on a bounded retry.
    pub fn retryable_write(&self) -> bool {
        matches!(self, Self::Sqlite(rusqlite::Error::SqliteFailure(error, _))
            if matches!(error.code, rusqlite::ErrorCode::DatabaseBusy
                | rusqlite::ErrorCode::DatabaseLocked | rusqlite::ErrorCode::DiskFull
                | rusqlite::ErrorCode::SystemIoFailure))
    }

    /// An unavailable key or device may recover after the user acts.
    pub fn retryable_open(&self) -> bool {
        match self {
            Self::Unavailable(reason) => !matches!(
                *reason,
                "SQLCipher is not available"
                    | "database path is not UTF-8"
                    | "could not initialize database encryption"
                    | "refusing plaintext database in place of encrypted installation"
                    | "store worker stopped"
                    | "request belongs to a closed application"
            ),
            Self::Sqlite(error) | Self::Migration { source: error, .. } => matches!(
                error,
                rusqlite::Error::SqliteFailure(error, _)
                    if matches!(
                        error.code,
                        rusqlite::ErrorCode::CannotOpen
                            | rusqlite::ErrorCode::PermissionDenied
                            | rusqlite::ErrorCode::ReadOnly
                            | rusqlite::ErrorCode::DatabaseBusy
                            | rusqlite::ErrorCode::DatabaseLocked
                            | rusqlite::ErrorCode::DiskFull
                            | rusqlite::ErrorCode::SystemIoFailure
                    )
            ),
            _ => false,
        }
    }

    /// The bytes could not be interpreted as a database with the supplied key.
    /// I/O, locking and schema errors deliberately do not match this class.
    pub fn is_not_database(&self) -> bool {
        matches!(self, Self::Sqlite(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::NotADatabase)
    }

    /// Positive content-error classification for [`crate::Store::validate_snapshot`]
    /// results only. Its fixed queries decode untrusted schema versions and text,
    /// so type, integer-range and UTF-8 failures establish invalid snapshot data.
    /// Device, migration, arbitrary conversion and unknown errors do not establish
    /// corruption and deliberately remain unclassified.
    pub fn is_snapshot_content_error(&self) -> bool {
        match self {
            Self::SchemaCorrupt(_) | Self::InvalidData(_) => true,
            Self::Sqlite(rusqlite::Error::SqliteFailure(error, _)) => matches!(
                error.code,
                rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt
            ),
            Self::Sqlite(
                rusqlite::Error::InvalidColumnType(..)
                | rusqlite::Error::IntegralValueOutOfRange(..)
                | rusqlite::Error::Utf8Error(..),
            ) => true,
            _ => false,
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) | Self::RestartRequired(reason) => {
                write!(f, "store unavailable: {reason}")
            }
            Self::Sqlite(err) => write!(f, "database error: {err}"),
            Self::Migration { version, source } => {
                write!(f, "schema migration {version} failed: {source}")
            }
            Self::SchemaTooNew { found, supported } => write!(
                f,
                "database schema version {found} is newer than this build supports ({supported})"
            ),
            Self::SchemaCorrupt(why) => write!(f, "schema bookkeeping is inconsistent: {why}"),
            Self::SourceNotFound(id) => write!(f, "source {} does not exist", id.0),
            Self::CustomGroupNotFound(id) => write!(f, "custom group {id} does not exist"),
            Self::InvalidData(what) => write!(f, "stored data is invalid: {what}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(err) | Self::Migration { source: err, .. } => Some(err),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sqlite(err)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(err: serde_json::Error) -> Self {
        // serde_json's Display can quote the offending value (a stream URL
        // may embed credentials), so keep only the category and position.
        Self::InvalidData(format!(
            "JSON column: {:?} error at line {} column {}",
            err.classify(),
            err.line(),
            err.column()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqlite(code: i32) -> StoreError {
        StoreError::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(code),
            None,
        ))
    }

    #[test]
    fn snapshot_content_errors_do_not_include_device_or_unknown_sqlite_failures() {
        use rusqlite::ffi::*;
        for (code, content) in [
            (SQLITE_NOTADB, true),
            (SQLITE_CORRUPT, true),
            (SQLITE_CORRUPT | (1 << 8), true),
            (SQLITE_NOMEM, false),
            (SQLITE_BUSY, false),
            (SQLITE_LOCKED, false),
            (SQLITE_IOERR, false),
            (SQLITE_IOERR | (1 << 8), false),
            (SQLITE_FULL, false),
            (SQLITE_CANTOPEN, false),
            (SQLITE_PERM, false),
            (SQLITE_READONLY, false),
            (SQLITE_INTERRUPT, false),
            (SQLITE_ERROR, false),
            (SQLITE_INTERNAL, false),
            (SQLITE_MISUSE, false),
        ] {
            assert_eq!(sqlite(code).is_snapshot_content_error(), content, "{code}");
        }
    }

    #[test]
    fn snapshot_content_findings_are_separate_from_versions_and_operational_errors() {
        assert!(
            StoreError::InvalidData("generated invalid schema".into()).is_snapshot_content_error()
        );
        assert!(
            StoreError::SchemaCorrupt("generated migration hole".into())
                .is_snapshot_content_error()
        );
        for error in [
            StoreError::Unavailable("generated unavailable storage"),
            StoreError::RestartRequired("generated restart requirement"),
            StoreError::SchemaTooNew {
                found: 2,
                supported: 1,
            },
            StoreError::Migration {
                version: 1,
                source: rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                    None,
                ),
            },
            StoreError::Sqlite(rusqlite::Error::InvalidQuery),
            StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other("generated conversion failure")),
            )),
        ] {
            assert!(!error.is_snapshot_content_error(), "{error:?}");
        }
    }
}
