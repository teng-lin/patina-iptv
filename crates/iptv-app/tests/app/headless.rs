//! Headless application test wrapper with timeout scaling.
//!
//! Direct wait methods (`wait_for`, `wait_for_within`, `wait_for_gone`, and `wait_until`)
//! scale their timeout deadlines by [`crate::isolated::wait_scale`]. Methods advancing
//! simulated time (`run_for`, `wait`, `fast_forward`) remain unscaled to preserve
//! deterministic test advancement and playback timing.

use nsh_core::TextEngine;
use nsh_engine::EngineConfig;
use nsh_testing::Screen;
use std::ops::{Deref, DerefMut};
use std::time::Duration;

/// A wrapper around [`nsh_testing::Headless`] that scales scenario wait timeouts.
pub struct Headless(nsh_testing::Headless);

impl Headless {
    /// Starts the headless application runner with `config`.
    pub fn start(config: EngineConfig) -> std::io::Result<Self> {
        nsh_testing::Headless::start(config).map(Self)
    }

    /// Starts the headless application runner opening `url`.
    pub fn start_with_link(config: EngineConfig, url: &str) -> std::io::Result<Self> {
        nsh_testing::Headless::start_with_link(config, url).map(Self)
    }

    /// Starts the headless runner using a custom text engine.
    pub fn start_with_text_engine<M, F>(config: EngineConfig, factory: F) -> std::io::Result<Self>
    where
        M: TextEngine + 'static,
        F: FnOnce() -> M + Send + 'static,
    {
        nsh_testing::Headless::start_with_text_engine(config, factory).map(Self)
    }

    /// The step `waitFor(<selector>)`: applies batches until a view on
    /// screen matches `selector`, waiting scaled [`nsh_testing::WAIT`] at most.
    pub fn wait_for(&mut self, selector: &str) -> Result<(), String> {
        self.0.wait_for_within(
            selector,
            crate::isolated::scaled_duration(nsh_testing::WAIT),
        )
    }

    /// [`Headless::wait_for`], waiting scaled `timeout` at most.
    pub fn wait_for_within(&mut self, selector: &str, timeout: Duration) -> Result<(), String> {
        self.0
            .wait_for_within(selector, crate::isolated::scaled_duration(timeout))
    }

    /// The step `waitFor(<selector>, gone)`: applies batches until no view
    /// on screen matches `selector`, waiting scaled `timeout` at most.
    pub fn wait_for_gone(&mut self, selector: &str, timeout: Duration) -> Result<(), String> {
        self.0
            .wait_for_gone(selector, crate::isolated::scaled_duration(timeout))
    }

    /// Applies batches until `done` holds for the screen, scaled `timeout` at most.
    pub fn wait_until(
        &mut self,
        what: &str,
        timeout: Duration,
        done: impl Fn(&Screen) -> bool,
    ) -> Result<(), String> {
        self.0
            .wait_until(what, crate::isolated::scaled_duration(timeout), done)
    }
}

impl Deref for Headless {
    type Target = nsh_testing::Headless;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Headless {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
