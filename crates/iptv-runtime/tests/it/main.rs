//! Runtime integration scenarios share one executable and independent fixtures.

#[path = "../../../iptv-store-sqlite/tests/common/mod.rs"]
mod common;
#[path = "../support/mod.rs"]
mod support;

mod architecture;
mod command_journeys;
mod command_values;
mod data_startup;
mod foundation_contract;
mod foundation_inspection;
mod foundation_probe;
mod foundation_repair_fences;
mod guide_check_owners;
mod guide_journeys;
mod host_lifecycle;
mod library_journeys;
mod lifecycle_recovery;
mod native_backup;
mod ownership;
mod parental_access;
mod parental_recovery;
mod persistence_safety;
mod read_lane_contention;
mod relative_organization_moves;
mod shell_boundary;
mod vod_import_pipeline;
mod vod_library;
