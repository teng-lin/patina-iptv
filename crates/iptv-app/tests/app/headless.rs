//! Headless application test wrapper with timeout scaling.
//!
//! Direct wait methods (`wait_for`, `wait_for_within`, `wait_for_gone`, and `wait_until`)
//! scale their timeout deadlines by [`crate::isolated::wait_scale`]. Methods advancing
//! simulated time (`run_for`, `wait`, `fast_forward`) remain unscaled to preserve
//! deterministic test advancement and playback timing.

use nsh_core::{EventKind, EventPayload, KeyEvent, ROOT_ID, TextEngine};
use nsh_engine::EngineConfig;
use nsh_testing::Screen;
use nsh_tv::RemoteKey;
use std::ops::{Deref, DerefMut};
use std::time::Duration;

/// A raw remote key phase ([`Headless::remote_phase`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPhase {
    Down,
    Repeat,
    Up,
}

/// A wrapper around [`nsh_testing::Headless`] that scales scenario wait timeouts.
pub struct Headless(nsh_testing::Headless, bool);

impl Headless {
    /// Observe initial mounts normally, but wait for the splash before input.
    fn ready_for_input(&mut self) -> Result<(), String> {
        if !self.1 {
            self.wait_until("initial app frame", Duration::from_secs(20), |screen| {
                !screen.layout_nodes().is_empty()
            })?;
            self.wait_for_gone("id:startup-splash", Duration::from_secs(20))?;
            self.1 = true;
        }
        Ok(())
    }

    pub fn press(&mut self, selector: &str) -> Result<(), String> {
        self.ready_for_input()?;
        self.0.press(selector)
    }

    pub fn play_bar_option(&mut self, name: &str) -> Result<(), String> {
        self.press("id:play-bar-more")?;
        self.press(&format!("id:chooser-items >> text:{name}"))
    }

    pub fn press_key(&mut self, key: &str) -> Result<(), String> {
        self.ready_for_input()?;
        self.0.press_key(key)
    }

    /// Deliver a hardware-only remote key through the mounted raw listener.
    /// NSH's simulated keyboard accepts named keystrokes only.
    pub fn press_remote(&mut self, key: RemoteKey) -> Result<(), String> {
        self.press_remote_in(key, None)
    }

    /// Route a remote event to the app root or a specific auxiliary root.
    pub fn press_remote_in(&mut self, key: RemoteKey, test_id: Option<&str>) -> Result<(), String> {
        self.ready_for_input()?;
        let mut event = KeyEvent::default();
        if let Some(transported) = nsh_tv::transport::TABLE
            .iter()
            .find(|entry| entry.key == key)
        {
            event.code = transported.dom_code.into();
        } else {
            event.key = match key {
                RemoteKey::Back => "GoBack",
                RemoteKey::Menu => "ContextMenu",
                RemoteKey::Stop => "MediaStop",
                _ => return Err(format!("missing raw event for {key:?}")),
            }
            .into();
        }
        let root = self
            .0
            .screen()
            .layout_nodes()
            .into_iter()
            .find(|node| match test_id {
                Some(id) => node.test_id.as_deref() == Some(id),
                None => node.id != ROOT_ID && node.parent == ROOT_ID,
            })
            .ok_or("missing mounted app root")?;
        self.0.step(format!("press remote {key:?}"));
        let control = self.0.engine().control();
        control.dispatch_event(
            root.id,
            EventKind::KeyDown,
            EventPayload::Key(event.clone()),
        );
        control.dispatch_event(root.id, EventKind::KeyUp, EventPayload::Key(event));
        self.0.settle();
        self.0.wait(Duration::ZERO)
    }

    /// One raw phase of a remote key as Android reports it: a key down, an
    /// auto-repeat (a key down marked `repeat`) or a release. The app takes
    /// them only after `iptv_app::use_remote_key_phases` (before start).
    /// Neither advances the clock: `run_for` between phases holds the key.
    pub fn remote_phase(&mut self, key: RemoteKey, phase: KeyPhase) -> Result<(), String> {
        self.ready_for_input()?;
        let mut event = KeyEvent::default();
        let name = match key {
            RemoteKey::Up => "ArrowUp",
            RemoteKey::Down => "ArrowDown",
            RemoteKey::Left => "ArrowLeft",
            RemoteKey::Right => "ArrowRight",
            RemoteKey::Ok => "Enter",
            RemoteKey::Menu => "ContextMenu",
            RemoteKey::PlayPause => "MediaPlayPause",
            RemoteKey::Previous => "MediaTrackPrevious",
            RemoteKey::Back => "GoBack",
            RemoteKey::Digit(d) => ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"][d as usize],
            _ => return Err(format!("missing raw phase event for {key:?}")),
        };
        event.key = name.into();
        event.code = match key {
            RemoteKey::Digit(d) => format!("Digit{d}").into(),
            _ => name.into(),
        };
        event.repeat = phase == KeyPhase::Repeat;
        let root = self
            .0
            .screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.id != ROOT_ID && node.parent == ROOT_ID)
            .ok_or("missing mounted app root")?;
        self.0.step(format!("remote {key:?} {phase:?}"));
        let kind = if phase == KeyPhase::Up {
            EventKind::KeyUp
        } else {
            EventKind::KeyDown
        };
        self.0
            .engine()
            .control()
            .dispatch_event(root.id, kind, EventPayload::Key(event));
        self.0.settle();
        self.0.wait(Duration::ZERO)
    }

    /// A quick tap of a remote key: down and up at the same instant.
    pub fn remote_tap(&mut self, key: RemoteKey) -> Result<(), String> {
        self.remote_phase(key, KeyPhase::Down)?;
        self.remote_phase(key, KeyPhase::Up)
    }

    pub fn back(&mut self) -> Result<(), String> {
        self.ready_for_input()?;
        self.0.back()
    }

    /// Starts the headless application runner with `config`.
    pub fn start(config: EngineConfig) -> std::io::Result<Self> {
        nsh_testing::Headless::start(config).map(|inner| Self(inner, false))
    }

    /// Starts the headless application runner opening `url`.
    pub fn start_with_link(config: EngineConfig, url: &str) -> std::io::Result<Self> {
        nsh_testing::Headless::start_with_link(config, url).map(|inner| Self(inner, false))
    }

    /// Starts the headless runner using a custom text engine.
    pub fn start_with_text_engine<M, F>(config: EngineConfig, factory: F) -> std::io::Result<Self>
    where
        M: TextEngine + 'static,
        F: FnOnce() -> M + Send + 'static,
    {
        nsh_testing::Headless::start_with_text_engine(config, factory)
            .map(|inner| Self(inner, false))
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
