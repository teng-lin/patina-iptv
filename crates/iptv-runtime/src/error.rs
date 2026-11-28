//! Runtime failures retain their cause without exposing a storage implementation type.

// Store adapters retain their original error until the runtime classifies it.
pub(crate) use iptv_store_sqlite::StoreError as AdapterError;

use iptv_core::recording::{RequestRefusal, SpaceRefusal};

#[derive(Debug)]
#[non_exhaustive]
enum ErrorKind {
    SettingsLocked,
    ProviderCapacity,
    Unavailable(&'static str),
    RestartRequired(&'static str),
    SchemaTooNew {
        found: u32,
        supported: u32,
    },
    CorruptDatabase(iptv_store_sqlite::StoreError),
    WorkerUnavailable,
    #[cfg(feature = "native")]
    RecordingRequest(RequestRefusal),
    #[cfg(feature = "native")]
    RecordingSpace(SpaceRefusal),
    Storage(iptv_store_sqlite::StoreError),
}

const PROVIDER_CAPACITY_REASON: &str =
    iptv_core::message::key("No provider connection available; check Streams at once");

pub(crate) const SETTINGS_LOCKED_REASON: &str =
    iptv_core::message::key("Unlock parental controls before changing settings");

pub struct Error(ErrorKind);

impl Error {
    /// A static application reason, without database diagnostics or provider data.
    pub fn unavailable_reason(&self) -> Option<&'static str> {
        match &self.0 {
            ErrorKind::Unavailable(reason) | ErrorKind::RestartRequired(reason) => Some(reason),
            ErrorKind::SettingsLocked => Some(SETTINGS_LOCKED_REASON),
            ErrorKind::ProviderCapacity => Some(PROVIDER_CAPACITY_REASON),
            _ => None,
        }
    }

    /// A settings-protected write lost its parental authorization.
    pub fn settings_locked(&self) -> bool {
        matches!(self.0, ErrorKind::SettingsLocked)
    }

    /// The provider has no remaining viewing capacity.
    pub fn provider_capacity(&self) -> bool {
        matches!(self.0, ErrorKind::ProviderCapacity)
    }

    /// The typed reason a recording request was refused.
    pub fn recording_request_refusal(&self) -> Option<RequestRefusal> {
        #[cfg(feature = "native")]
        match self.0 {
            ErrorKind::RecordingRequest(refusal) => Some(refusal),
            _ => None,
        }
        #[cfg(not(feature = "native"))]
        {
            let _ = self;
            None
        }
    }

    /// The typed reason available space refused a recording request.
    pub fn recording_space_refusal(&self) -> Option<SpaceRefusal> {
        #[cfg(feature = "native")]
        match self.0 {
            ErrorKind::RecordingSpace(refusal) => Some(refusal),
            _ => None,
        }
        #[cfg(not(feature = "native"))]
        {
            let _ = self;
            None
        }
    }

    /// The database and supported schema versions, if the file is too new.
    pub fn newer_schema(&self) -> Option<(u32, u32)> {
        match self.0 {
            ErrorKind::SchemaTooNew { found, supported } => Some((found, supported)),
            _ => None,
        }
    }

    /// Whether opening the store could succeed after a device or storage change.
    pub fn retryable_open(&self) -> bool {
        match &self.0 {
            ErrorKind::Unavailable(reason) => {
                iptv_store_sqlite::StoreError::Unavailable(reason).retryable_open()
            }
            ErrorKind::WorkerUnavailable => true,
            ErrorKind::Storage(error) => error.retryable_open(),
            _ => false,
        }
    }

    /// The reason, if the host must initialize storage again on the next launch.
    pub fn reopen_reason(&self) -> Option<&'static str> {
        match self.0 {
            ErrorKind::RestartRequired(reason) => Some(reason),
            _ => None,
        }
    }

    /// The store positively identified database or schema corruption.
    pub fn is_corrupt_database(&self) -> bool {
        matches!(self.0, ErrorKind::CorruptDatabase(_))
    }

    /// Re-verification found the saved library file missing, empty, or already healthy.
    pub fn is_library_not_corrupt(&self) -> bool {
        matches!(
            &self.0,
            ErrorKind::Unavailable(reason)
                if iptv_store_sqlite::StoreError::Unavailable(reason).is_library_not_corrupt()
        )
    }

    /// The bytes could not be read as a database with the supplied key (`SQLITE_NOTADB`).
    pub fn is_not_database(&self) -> bool {
        matches!(&self.0, ErrorKind::Storage(error) if error.is_not_database())
    }

    /// Opening failed because the database key or platform credential store was unavailable.
    pub fn is_credential_unavailable(&self) -> bool {
        if self.is_not_database() {
            return true;
        }
        let reason = match &self.0 {
            ErrorKind::Unavailable(reason) | ErrorKind::RestartRequired(reason) => Some(*reason),
            ErrorKind::Storage(iptv_store_sqlite::StoreError::Unavailable(reason))
            | ErrorKind::Storage(iptv_store_sqlite::StoreError::RestartRequired(reason)) => {
                Some(*reason)
            }
            _ => None,
        };
        reason.is_some_and(|text| {
            let bytes = text.as_bytes();
            [
                &b"credential"[..],
                b"database key",
                b"Keystore",
                b"key store",
            ]
            .into_iter()
            .any(|needle| bytes.windows(needle.len()).any(|part| part == needle))
        })
    }

    /// A failure involving host state that is only initialized on launch.
    pub fn restart_required(reason: &'static str) -> Self {
        Self(ErrorKind::RestartRequired(reason))
    }

    pub fn unavailable(reason: &'static str) -> Self {
        match reason {
            PROVIDER_CAPACITY_REASON => Self(ErrorKind::ProviderCapacity),
            SETTINGS_LOCKED_REASON => Self(ErrorKind::SettingsLocked),
            "cannot start store worker" => Self(ErrorKind::WorkerUnavailable),
            _ => Self(ErrorKind::Unavailable(reason)),
        }
    }

    #[cfg(feature = "native")]
    pub(crate) fn from_recording_request_refusal(refusal: RequestRefusal) -> Self {
        Self(ErrorKind::RecordingRequest(refusal))
    }

    #[cfg(feature = "native")]
    pub(crate) fn from_recording_space_refusal(refusal: SpaceRefusal) -> Self {
        Self(ErrorKind::RecordingSpace(refusal))
    }

    pub(crate) fn into_store_error(self) -> iptv_store_sqlite::StoreError {
        match self.0 {
            ErrorKind::Storage(error) => error,
            ErrorKind::Unavailable(reason) => iptv_store_sqlite::StoreError::Unavailable(reason),
            ErrorKind::SettingsLocked => {
                iptv_store_sqlite::StoreError::Unavailable(SETTINGS_LOCKED_REASON)
            }
            ErrorKind::RestartRequired(reason) => {
                iptv_store_sqlite::StoreError::RestartRequired(reason)
            }
            ErrorKind::ProviderCapacity => {
                iptv_store_sqlite::StoreError::Unavailable(PROVIDER_CAPACITY_REASON)
            }
            ErrorKind::SchemaTooNew { found, supported } => {
                iptv_store_sqlite::StoreError::SchemaTooNew { found, supported }
            }
            ErrorKind::CorruptDatabase(error) => error,
            ErrorKind::WorkerUnavailable => {
                iptv_store_sqlite::StoreError::Unavailable("cannot start store worker")
            }
            #[cfg(feature = "native")]
            ErrorKind::RecordingRequest(_) => {
                iptv_store_sqlite::StoreError::Unavailable("recording request refused")
            }
            #[cfg(feature = "native")]
            ErrorKind::RecordingSpace(_) => {
                iptv_store_sqlite::StoreError::Unavailable("recording space refused")
            }
        }
    }
}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            ErrorKind::SettingsLocked => f.write_str("SettingsLocked"),
            ErrorKind::ProviderCapacity => f.write_str("ProviderCapacity"),
            ErrorKind::Unavailable(reason) => f.debug_tuple("Unavailable").field(reason).finish(),
            ErrorKind::RestartRequired(reason) => {
                f.debug_tuple("RestartRequired").field(reason).finish()
            }
            ErrorKind::SchemaTooNew { found, supported } => f
                .debug_struct("SchemaTooNew")
                .field("found", found)
                .field("supported", supported)
                .finish(),
            ErrorKind::CorruptDatabase(_) => f.write_str("CorruptDatabase"),
            ErrorKind::WorkerUnavailable => f.write_str("WorkerUnavailable"),
            #[cfg(feature = "native")]
            ErrorKind::RecordingRequest(refusal) => {
                f.debug_tuple("RecordingRequest").field(refusal).finish()
            }
            #[cfg(feature = "native")]
            ErrorKind::RecordingSpace(refusal) => {
                f.debug_tuple("RecordingSpace").field(refusal).finish()
            }
            ErrorKind::Storage(error) => f.write_str(match error {
                iptv_store_sqlite::StoreError::Sqlite(_) => "Sqlite",
                iptv_store_sqlite::StoreError::Migration { .. } => "Migration",
                iptv_store_sqlite::StoreError::SourceNotFound(_) => "SourceNotFound",
                iptv_store_sqlite::StoreError::CustomGroupNotFound(_) => "CustomGroupNotFound",
                iptv_store_sqlite::StoreError::InvalidData(_) => "InvalidData",
                _ => "Storage",
            }),
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            ErrorKind::SettingsLocked => f.write_str("settings are locked"),
            ErrorKind::ProviderCapacity => f.write_str("provider viewing capacity is full"),
            ErrorKind::Unavailable(reason) => write!(f, "store unavailable: {reason}"),
            ErrorKind::RestartRequired(reason) => write!(f, "store unavailable: {reason}"),
            ErrorKind::SchemaTooNew { found, supported } => write!(
                f,
                "database schema version {found} is newer than this build supports ({supported})"
            ),
            ErrorKind::CorruptDatabase(_) => f.write_str("database is corrupt"),
            ErrorKind::WorkerUnavailable => f.write_str("cannot start store worker"),
            #[cfg(feature = "native")]
            ErrorKind::RecordingRequest(_) => f.write_str("recording request refused"),
            #[cfg(feature = "native")]
            ErrorKind::RecordingSpace(_) => f.write_str("recording space refused"),
            ErrorKind::Storage(_) => f.write_str("storage operation failed"),
        }
    }
}
impl std::error::Error for Error {}
impl From<iptv_store_sqlite::StoreError> for Error {
    fn from(error: iptv_store_sqlite::StoreError) -> Self {
        let kind = match error {
            iptv_store_sqlite::StoreError::Unavailable(reason)
                if reason == SETTINGS_LOCKED_REASON =>
            {
                ErrorKind::SettingsLocked
            }
            iptv_store_sqlite::StoreError::Unavailable(reason)
                if reason == PROVIDER_CAPACITY_REASON =>
            {
                ErrorKind::ProviderCapacity
            }
            iptv_store_sqlite::StoreError::Unavailable("cannot start store worker") => {
                ErrorKind::WorkerUnavailable
            }
            iptv_store_sqlite::StoreError::RestartRequired(reason) => {
                ErrorKind::RestartRequired(reason)
            }
            iptv_store_sqlite::StoreError::SchemaTooNew { found, supported } => {
                ErrorKind::SchemaTooNew { found, supported }
            }
            error if error.is_corrupt_database() => ErrorKind::CorruptDatabase(error),
            iptv_store_sqlite::StoreError::Unavailable(reason) => ErrorKind::Unavailable(reason),
            error => ErrorKind::Storage(error),
        };
        Self(kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corruption_requires_positive_evidence_and_preserves_the_store_cause() {
        use iptv_store_sqlite::StoreError;
        let mismatched_key = StoreError::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_NOTADB),
            None,
        ));
        let error = Error::from(mismatched_key);
        assert!(!error.is_corrupt_database());
        assert!(error.is_not_database());
        assert!(error.is_credential_unavailable());
        assert!(!error.retryable_open());
        assert!(error.into_store_error().is_not_database());

        let locked_broker =
            Error::unavailable("platform credential broker is locked or unavailable");
        assert!(!locked_broker.is_corrupt_database());
        assert!(!locked_broker.is_not_database());
        assert!(locked_broker.is_credential_unavailable());

        let malformed = StoreError::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            None,
        ));
        for error in [malformed, StoreError::SchemaCorrupt("synthetic".to_owned())] {
            let error = Error::from(error);
            assert!(error.is_corrupt_database());
            assert!(error.into_store_error().is_corrupt_database());
        }
    }

    #[test]
    fn storage_details_are_not_exposed_in_runtime_display() {
        let error = Error::from(iptv_store_sqlite::StoreError::Sqlite(
            rusqlite::Error::InvalidParameterName("generated-private-detail".into()),
        ));
        assert_eq!(error.to_string(), "storage operation failed");
        assert!(!format!("{error:?}").contains("generated-private-detail"));
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn classifiers_use_error_kinds_and_keep_catalog_reasons() {
        assert!(
            Error::from(iptv_store_sqlite::StoreError::Unavailable(
                SETTINGS_LOCKED_REASON
            ))
            .settings_locked()
        );
        assert!(
            Error::from(iptv_store_sqlite::StoreError::Unavailable(
                PROVIDER_CAPACITY_REASON
            ))
            .provider_capacity()
        );
        assert!(Error::unavailable(PROVIDER_CAPACITY_REASON).provider_capacity());
        assert!(Error::unavailable("cannot start store worker").retryable_open());
        assert!(
            Error::from(iptv_store_sqlite::StoreError::Unavailable(
                "cannot start store worker"
            ))
            .retryable_open()
        );
        assert_eq!(
            Error::from(iptv_store_sqlite::StoreError::SchemaTooNew {
                found: 7,
                supported: 6,
            })
            .newer_schema(),
            Some((7, 6))
        );
        assert_eq!(
            Error::unavailable("generated catalog key").unavailable_reason(),
            Some("generated catalog key")
        );
    }
}
