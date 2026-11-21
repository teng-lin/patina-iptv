//! The store's integration tests.
//!
//! Ordinary scenarios share this executable, with one module per topic.
//! Native initialization runs in fresh child processes. Allocation scenarios
//! share the separate `memory` executable and also isolate each measurement
//! in a child, so global allocator and SQLite counters cannot overlap.

#[path = "../common/mod.rs"]
mod common;
#[path = "../epg_synth/mod.rs"]
mod epg_synth;
#[path = "../initialization_support/mod.rs"]
mod initialization_support;
#[path = "../staged/mod.rs"]
mod staged;
#[path = "../stream_support/mod.rs"]
mod stream_support;

mod backup;
mod catalog;
mod child_output;
mod cipher_initialization;
mod concurrency;
mod custom_group_reset_scale;
mod epg;
mod epg_stream;
mod guide_assign;
mod library_preferences;
mod migrations;
mod organize;
mod overlays;
mod perf;
mod personalization;
mod playlists;
mod redaction;
mod relative_moves;
mod search;

mod catalog_stage;
