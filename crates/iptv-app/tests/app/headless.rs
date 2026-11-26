//! Headless application test wrapper with timeout scaling.
//!
//! Direct wait methods (`wait_for`, `wait_for_within`, `wait_for_gone`, and `wait_until`)
//! scale their timeout deadlines by [`crate::isolated::wait_scale`]. Methods advancing
//! simulated time (`run_for`, `wait`, `fast_forward`) remain unscaled to preserve
//! deterministic test advancement and playback timing.

use nsh_core::PayloadValue;
use nsh_core::keymap::{KeyMatch, Keymap, Keystroke};
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

    /// Open groups through the retained navigation, corner Back or keyboard route.
    pub fn open_guide_groups(&mut self) -> Result<(), String> {
        self.ready_for_input()?;
        if self.screen().layout_nodes().iter().any(|node| {
            node.test_id.as_deref() == Some("guide-groups") && node.bounds.size.width.0 > 0.0
        }) {
            // Live TV is the second rail destination in every language. It
            // restores group focus and activates the main window without tuning.
            return self.press("id:navigation-rail >> role:tab >> nth=1");
        }
        let back = "id:guide-corner-back >> label:Groups";
        if self.screen().matches(back)? {
            self.press(back)
        } else {
            self.press_key("tab")
        }
    }

    /// Settings → Playlists (the rail's Settings, then its Playlists page):
    /// every stored playlist with its check box, then adding, refreshing
    /// and ordering them. The playlists replaced the rail's own entry.
    pub fn open_settings_playlists(&mut self) -> Result<(), String> {
        self.ready_for_input()?;
        let page = "id:settings-content >> role:tab[Playlists]";
        if !self.screen().matches(page)? {
            let item = "id:navigation-rail >> label:Settings";
            if !self.screen().matches(item)? {
                self.open_guide_groups()?;
                self.wait_for(item)?;
            }
            self.press(item)?;
            self.wait_for(page)?;
        }
        self.press(page)?;
        // Its rows: the playlists first (many push the commands below).
        self.wait_until("the Playlists page", Duration::from_secs(5), |screen| {
            screen
                .matches("id:settings-content >> text:Add a playlist")
                .unwrap_or(false)
                || screen
                    .matches("id:settings-content >> role:checkbox")
                    .unwrap_or(false)
        })
    }

    /// Open a source's full management controls in Settings → Playlists.
    pub fn open_playlist_actions(&mut self, name: &str) -> Result<(), String> {
        if !self.screen().matches("id:playlists-screen")? {
            self.open_settings_playlists()?;
            let manage = "id:settings-content >> text:Manage playlists";
            if !self.screen().matches(manage)? {
                self.press_key("right")?;
                while !self.screen().matches(manage)? {
                    let previous = self.panel_texts("settings-content");
                    self.press_key("pagedown")?;
                    if previous == self.panel_texts("settings-content") {
                        break;
                    }
                }
            }
            self.press(manage)?;
        }
        let list = "id:playlists-list";
        let source = format!("{list} >> text:{name}");
        self.wait_for(list)?;
        if !self.screen().matches(&source)? {
            let half_page = self
                .screen()
                .layout_nodes()
                .into_iter()
                .find(|node| node.test_id.as_deref() == Some("playlists-list"))
                .map(|node| nsh_core::prelude::px(node.bounds.size.height.0 / 2.0))
                .ok_or_else(|| "the playlist picker has no scroll viewport".to_owned())?;
            while !self.screen().matches(&source)? {
                self.0
                    .scroll(list, half_page)
                    .map_err(|err| format!("could not find playlist {name}: {err}"))?;
            }
        }
        self.press(&source)?;
        self.wait_for("id:playlist-actions-list")
    }

    pub fn panel_texts(&self, panel_id: &str) -> Vec<String> {
        let nodes = self.screen().layout_nodes();
        let Some(panel) = nodes
            .iter()
            .find(|n| n.test_id.as_deref() == Some(panel_id))
            .map(|n| n.id)
        else {
            return Vec::new();
        };
        let parent: std::collections::HashMap<_, _> =
            nodes.iter().map(|n| (n.id, n.parent)).collect();
        let inside = |mut id| {
            while let Some(&up) = parent.get(&id) {
                if up == panel {
                    return true;
                }
                id = up;
            }
            false
        };
        let mut found: Vec<(String, f32)> = nodes
            .iter()
            .filter(|n| inside(n.id))
            .filter_map(|n| Some((n.text.as_ref()?.text.text.to_string(), n.bounds.origin.y.0)))
            .collect();
        found.sort_by(|a, b| a.1.total_cmp(&b.1));
        found.into_iter().map(|t| t.0).collect()
    }

    /// Closes the settings (after a playlist's task there ended): their
    /// Close button where shown, else Back until they are gone.
    pub fn close_settings(&mut self) -> Result<(), String> {
        self.wait_for_gone("id:playlists-task", Duration::from_secs(10))?;
        let close = "id:settings-content >> text:Close";
        if self.screen().matches(close)? {
            self.press(close)?;
        } else {
            for _ in 0..3 {
                if !self.screen().matches("id:settings-screen")? {
                    break;
                }
                self.back()?;
            }
        }
        self.wait_for_gone("id:settings-screen", Duration::from_secs(5))
    }

    pub fn play_bar_option(&mut self, name: &str) -> Result<(), String> {
        let direct = format!("id:play-bar-layer >> label:{name}");
        if self.screen().matches(&direct)? {
            return self.press(&direct);
        }
        self.press("id:play-bar-more")?;
        self.press(&format!("id:chooser-items >> text:{name}"))
    }

    pub fn press_key(&mut self, key: &str) -> Result<(), String> {
        self.ready_for_input()?;
        self.0.press_key(key)
    }

    /// Auto-repeat a named shortcut in the main window, outside text input.
    /// Match the latest observed `MountItem::Keymap` and dispatch the public
    /// binding as native hosts do; `press_key` is always a fresh press.
    pub fn repeat_key_binding(&mut self, key: &str, keymap: &Keymap) -> Result<KeyMatch, String> {
        self.ready_for_input()?;
        let stroke = Keystroke::parse_with(key, false).map_err(|e| e.to_string())?;
        let found = keymap.find(stroke, false, true);
        self.0
            .step(format!("repeat named shortcut {key}: {found:?}"));
        if let Some(id) = found.fired() {
            self.engine().control().dispatch_event(
                ROOT_ID,
                EventKind::KeyCommand,
                EventPayload::Value(PayloadValue::Number(f64::from(id))),
            );
        }
        self.0.settle();
        self.0.wait(Duration::ZERO)?;
        Ok(found)
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
            _ => nsh_tv::transport::TABLE
                .iter()
                .find(|entry| entry.key == key)
                .map(|entry| entry.dom_code)
                .ok_or_else(|| format!("missing raw phase event for {key:?}"))?,
        };
        event.key = if nsh_tv::transport::TABLE
            .iter()
            .any(|entry| entry.key == key)
        {
            "Unidentified"
        } else {
            name
        }
        .into();
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

    /// Pumps work from other threads while leaving the manual clock fixed.
    #[track_caller]
    pub fn wait_until_fixed_clock(
        &mut self,
        what: &str,
        timeout: Duration,
        done: impl Fn(&Screen) -> bool,
    ) {
        assert!(self.clock().is_manual());
        let deadline = std::time::Instant::now() + crate::isolated::scaled_duration(timeout);
        loop {
            self.catch_up();
            if done(self.screen()) {
                return;
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "{what} did not happen before the timeout"
            );
            self.engine().control().frame();
            self.next_batch(remaining);
        }
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
