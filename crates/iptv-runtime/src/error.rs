//! Runtime failures retain their cause without exposing a storage implementation type.
pub struct Error(iptv_store_sqlite::StoreError);

impl Error {
    /// A static application reason, without database diagnostics or provider data.
    pub fn unavailable_reason(&self) -> Option<&'static str> {
        match &self.0 {
            iptv_store_sqlite::StoreError::Unavailable(reason) => Some(reason),
            _ => None,
        }
    }
    /// The database and supported schema versions, if the file is too new.
    pub fn newer_schema(&self) -> Option<(u32, u32)> {
        match self.0 {
            iptv_store_sqlite::StoreError::SchemaTooNew { found, supported } => {
                Some((found, supported))
            }
            _ => None,
        }
    }

    /// Whether opening the store could succeed after a device or storage change.
    pub fn retryable_open(&self) -> bool {
        self.0.retryable_open()
    }

    pub fn unavailable(reason: &'static str) -> Self {
        Self(iptv_store_sqlite::StoreError::Unavailable(reason))
    }
}
impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}
impl From<iptv_store_sqlite::StoreError> for Error {
    fn from(error: iptv_store_sqlite::StoreError) -> Self {
        Self(error)
    }
}
impl From<Error> for iptv_store_sqlite::StoreError {
    fn from(error: Error) -> Self {
        error.0
    }
}
