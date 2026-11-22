//! The store's integration tests.
//!
//! Ordinary scenarios share this executable, with one module per topic.
//! Native initialization runs in fresh child processes. Allocation scenarios
//! share the separate `memory` executable and also isolate each measurement
//! in a child, so global allocator and SQLite counters cannot overlap.
//!
//! The same executable builds for `wasm32-unknown-unknown` (the browser's
//! SQLite) and runs under `tools/wasm-test.mjs`; scenarios that need a file
//! system, threads or child processes are native only.

// The browser has no temporary directories: `tempfile::tempdir()` names an
// unused prefix in SQLite's default memory VFS instead (see `wasm`).
#[cfg(target_family = "wasm")]
extern crate self as tempfile;
#[cfg(target_family = "wasm")]
mod wasm;
#[cfg(target_family = "wasm")]
pub use wasm::{TempDir, tempdir};

#[path = "../common/mod.rs"]
mod common;
#[path = "../epg_synth/mod.rs"]
mod epg_synth;
#[cfg(not(target_family = "wasm"))]
#[path = "../initialization_support/mod.rs"]
mod initialization_support;
#[path = "../staged/mod.rs"]
mod staged;
#[path = "../stream_support/mod.rs"]
mod stream_support;

#[cfg(not(target_family = "wasm"))]
mod backup;
#[cfg(target_family = "wasm")]
mod browser;
mod catalog;
#[cfg(not(target_family = "wasm"))]
mod child_output;
#[cfg(not(target_family = "wasm"))]
mod cipher_initialization;
#[cfg(not(target_family = "wasm"))]
mod concurrency;
mod custom_group_reset_scale;
mod epg;
mod epg_stream;
mod guide_assign;
mod library_preferences;
#[cfg(not(target_family = "wasm"))]
mod migrations;
mod organize;
mod overlays;
#[cfg(not(target_family = "wasm"))]
mod perf;
mod personalization;
mod playlists;
mod redaction;
mod relative_moves;
mod search;

mod catalog_stage;
