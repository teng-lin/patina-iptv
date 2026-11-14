//! Paths captured by the application owner, shared with its workers.
pub(crate) fn data_dir() -> std::path::PathBuf {
    super::store_service::service().directory.clone()
}
