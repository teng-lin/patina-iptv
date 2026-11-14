//! The app's integration tests: headless runs of the whole app, one
//! scenario per module, all in this one test binary (each binary links
//! the app, NSH, GTK and libmpv; one link instead of one per scenario).
//!
//! A scenario still runs in a process of its own with a fresh `$HOME`:
//! its `#[test]` starts with `let Some(home) = isolated!() else { return };`
//! (see [`isolated`]). `IPTV_RS_TEST_JOBS` caps how many scenario
//! processes run at once (default 4). Tests that need no app (like
//! `type_drift`) are plain `#[test]`s and run in this process.
//!
//! Adding a scenario: a module here (`mod my_scenario;` below and
//! `tests/app/my_scenario.rs`) whose `#[test]` starts with that line.
//! `cargo test -p iptv-app my_scenario` runs it alone.

#[macro_use]
mod isolated;
/// Settings for the player scenarios that browse in two steps.
mod browse_fixture;
/// The personalization scenarios' start: a stored playlist of favourites.
mod personalization;
/// What the guide sources scenarios share.
mod support;

mod accessibility_visibility;
mod architecture;
mod backup_restore;
mod catalogue_lint;
mod cold_start;
mod combined_playlists;
mod combined_resume;
mod cov_data;
mod cov_data_player;
mod cov_mpv_pip_window;
mod cov_nav;
mod cov_nav_touch;
mod cov_settings_screen;
mod cov_settings_wizard;
mod cov_sources;
mod cov_sources_assign;
mod guide_look;
mod guide_nav;
mod guide_pages;
mod guide_scroll_cost;
mod interface_languages;
mod library_cached_epg;
mod library_management;
mod library_refresh;
mod multiview_menu;
mod personalization_appearance;
mod personalization_groups;
mod personalization_order;
mod personalization_overlay;
mod personalization_playback;
mod personalization_selection;
mod personalization_zapping;
mod pip_control;
mod pointer;
mod screens_look;
mod search_history;
mod stalker_cancel;
mod tablet_rows;
mod tv_player;
mod tv_replay;
mod type_drift;

mod store_latency;
