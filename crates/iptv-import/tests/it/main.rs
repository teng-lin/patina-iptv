//! The import pipeline's integration tests.
//!
//! One test binary for the crate's integration tests (`tests/it/`, one
//! module per topic): each extra binary costs a link. Tests here share
//! a process, so none may rely on process-wide state (environment,
//! working directory, a global allocator); such a test is a binary of its
//! own in `tests/`.

mod common;

mod epg;
mod m3u;
#[cfg(feature = "stalker")]
mod on_demand;
#[cfg(feature = "stalker")]
mod stalker;
#[cfg(not(feature = "stalker"))]
mod stalker_unsupported;
mod xtream;

mod limits;

mod parser_properties;
