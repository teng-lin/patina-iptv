//! Runtime, device, settings, and navigation wiring.

pub(crate) mod capabilities;
pub(crate) mod data;
pub(crate) mod device;
pub(crate) mod device_wiring;
#[cfg(feature = "test-support")]
pub(crate) mod guide_window_walkthrough;
pub(crate) mod listen;
pub(crate) mod navigation;
#[cfg(feature = "test-support")]
pub(crate) mod performance_probe;
pub(crate) mod request_ui;
pub(crate) mod runtime_events;
pub(crate) mod runtime_owner;
pub(crate) mod runtime_wiring;
pub(crate) mod settings;
pub(crate) mod settings_wiring;
pub(crate) mod tasks;
