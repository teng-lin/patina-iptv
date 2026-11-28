//! Remote input: NSH keystrokes → `iptv_nav` keys → short/long presses →
//! context keymap → actions applied to the app state.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use iptv_nav::{
    Action, BackGuard, Context, Decision, HeldPresses, Keymap, Phase, Press, RemoteKey,
};
use nsh_core::keymap::Keystroke;
use nsh_core::prelude::*;
use nsh_core::{KeyCapture, KeyEvent};

use crate::guide::{Move, Pane};
use crate::{Input, Ui, ViewMode};

mod back;
mod guide_actions;
mod keys;
mod modal_actions;
mod number;
mod player_actions;

#[cfg(feature = "test-support")]
pub use super::input_kit::use_remote_key_phases;
pub(crate) use super::input_kit::{
    DIGITS, KEYS, PROFILE, RemoteFocus, focus_remote_root, holds, keys_now, long_press,
};
pub(crate) use back::swallow_back;
use back::{act, swallow_release};
use keys::RawKeys;

/// Test support: whether the keyboard binding `name` ("enter") fires on
/// the platform's auto-repeats now. A headless keyboard sends no repeats;
/// the platform fires a binding again on one only when it repeats.
#[cfg(feature = "test-support")]
pub fn keyboard_repeats(name: &str) -> Option<bool> {
    keyboard_repeats::get(name)
}

#[cfg(feature = "test-support")]
mod keyboard_repeats {
    use std::collections::HashMap;
    use std::sync::Mutex;

    static REPEATS: Mutex<Option<HashMap<&'static str, bool>>> = Mutex::new(None);

    pub(super) fn set(name: &'static str, repeat: bool) {
        let mut repeats = REPEATS.lock().unwrap_or_else(|e| e.into_inner());
        repeats.get_or_insert_default().insert(name, repeat);
    }

    pub(super) fn get(name: &str) -> Option<bool> {
        let repeats = REPEATS.lock().unwrap_or_else(|e| e.into_inner());
        repeats.as_ref()?.get(name).copied()
    }
}

struct Nav {
    keymap: Keymap,
    presses: HeldPresses<Surface>,
    raw: RawKeys,
    /// Keys whose press a PIN prompt or the remote editor took or cut
    /// short: their repeats and release do nothing until that key goes
    /// down afresh. A signal: a keyboard binding of such a key holds its
    /// auto-repeats meanwhile (`bind`).
    consumed: Signal<Vec<RemoteKey>>,
    /// The capture seen open and its session: one number for as long as
    /// it stays open, a new one for the next (even of the same prompt).
    capture: Option<(Capture, u64)>,
    sessions: u64,
    /// The capture session a Back went down in, until its release:
    /// Android's Back on that release closes only that one.
    back_in: Option<u64>,
}

/// What takes the keys: a PIN prompt (which one) or the remote editor
/// waiting for a key.
#[derive(Clone, PartialEq)]
enum Capture {
    Pin(crate::screens::parental_view::Prompt),
    Editor,
}

/// What takes the keys now, if anything (tracked).
fn capture(ui: crate::ScreensUi) -> Option<Capture> {
    crate::screens::parental_view::prompt(ui)
        .map(Capture::Pin)
        .or_else(|| crate::chrome::remote_editor::capturing(ui).then_some(Capture::Editor))
}

/// What owns input, finer than the keymap's [`Context`]: the info card and
/// the control strip are both `Controls`, the guide's panes both `Guide`.
/// A press that waits to tell a tap from a hold belongs to this surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputOwner {
    Overlay(crate::chrome::overlays::Overlay),
    SettingsPlaylist(i64),
    ChannelList(iptv_presentation::channel_list::ListPane),
    Multiview,
    PlayBar,
    Card,
    Strip,
    Video,
    Pane(Pane),
}

type Surface = (Context, InputOwner);

fn surface(ui: &Ui) -> Surface {
    let owner = if let Some(overlay) = ui.top_overlay() {
        if overlay == crate::chrome::overlays::Overlay::ChannelList {
            ui.playback
                .channels
                .with(|list| list.map(|list| InputOwner::ChannelList(list.pane)))
                .unwrap_or(InputOwner::Overlay(overlay))
        } else if overlay == crate::chrome::overlays::Overlay::Settings
            && let Some(id) = crate::screens::settings_view::focused_playlist(ui.screens)
        {
            InputOwner::SettingsPlaylist(id)
        } else {
            InputOwner::Overlay(overlay)
        }
    } else if crate::playback::multiview::is_open(ui.playback) {
        InputOwner::Multiview
    } else if ui.chrome.mode.get() == ViewMode::Fullscreen {
        if crate::playback::play_bar::captures(*ui) {
            InputOwner::PlayBar
        } else {
            match ui.playback.panels.with(|p| p.shown) {
                iptv_presentation::panels::Shown::Card => InputOwner::Card,
                iptv_presentation::panels::Shown::Controls => InputOwner::Strip,
                _ => InputOwner::Video,
            }
        }
    } else {
        InputOwner::Pane(ui.guide.focus.with(|f| f.pane))
    };
    (context(ui), owner)
}

impl Nav {
    fn new() -> Self {
        Nav {
            keymap: crate::chrome::remote_editor::keymap(),
            presses: HeldPresses::default(),
            raw: RawKeys::default(),
            consumed: signal(Vec::new()),
            capture: None,
            sessions: 0,
            back_in: None,
        }
    }

    /// The session of capture `now` (none when nothing captures), and
    /// whether it just began.
    fn session(&mut self, now: Option<Capture>) -> (Option<u64>, bool) {
        let Some(now) = now else {
            self.capture = None;
            return (None, false);
        };
        match &self.capture {
            Some((seen, id)) if *seen == now => (Some(*id), false),
            _ => {
                self.sessions += 1;
                self.capture = Some((now, self.sessions));
                (Some(self.sessions), true)
            }
        }
    }

    fn is_consumed(&self, key: RemoteKey) -> bool {
        self.consumed.with_untracked(|c| c.contains(&key))
    }

    fn consume(&self, key: RemoteKey) {
        if !self.is_consumed(key) {
            self.consumed.update(|c| c.push(key));
        }
    }

    /// No longer consumed; whether it was.
    fn unconsume(&self, key: RemoteKey) -> bool {
        let was = self.is_consumed(key);
        if was {
            self.consumed.update(|c| c.retain(|k| *k != key));
        }
        was
    }

    /// A prompt or the editor took the keys: every key down now (the
    /// press waiting and any other held) is cut short and stays so until
    /// its release or a fresh press (its repeats, release or timer must
    /// not act once the prompt closes).
    fn cut_short(&mut self) {
        let held = self.presses.held_key().into_iter().chain(self.raw.keys());
        for key in held.collect::<Vec<_>>() {
            self.consume(key);
        }
        self.presses.cancel();
    }
}

#[derive(Clone)]
pub(crate) struct InputScope {
    ui: Ui,
    nav: Rc<RefCell<Nav>>,
    owner: Owner,
}

impl InputScope {
    fn raw_event(&self, event: &KeyEvent, phase: Phase) {
        let key = self.nav.borrow_mut().raw.key(event, phase, long_press());
        if let Some(key) = key {
            self.owner.run(|| key_event(self.ui, &self.nav, key, phase));
        }
    }
}

/// Listen on the window's root so a remote works before native focus exists.
pub(crate) fn window<V: Interactive>(view: V, input: Stored<Option<InputScope>>) -> V {
    let view = if long_press() {
        // Android returns focus to the native container after dismissing the
        // keyboard. A mounted app view keeps the raw listener in its key path.
        let focus = element_ref();
        provide_context(RemoteFocus(focus));
        view.focusable(true).element_ref(&focus)
    } else {
        view
    };
    let capture = if long_press() {
        KeyCapture::keys(
            KEYS.iter()
                .map(|(name, _)| *name)
                .filter(|name| !name.contains('+'))
                .chain(DIGITS)
                .map(|name| Keystroke::parse_with(name, false).expect("TV keyboard binding")),
        )
    } else {
        KeyCapture::None
    };
    view.capture_keys(capture)
        .on_key_down(move |event: &KeyEvent| {
            if let Some(input) = input.get() {
                let phase = if event.repeat {
                    Phase::Repeat
                } else {
                    Phase::Down
                };
                input.raw_event(event, phase);
            }
        })
        .on_key_up(move |event: &KeyEvent| {
            if let Some(input) = input.get() {
                input.raw_event(event, Phase::Up);
            }
        })
}

/// The surface that owns input now.
pub fn context(ui: &Ui) -> Context {
    if let Some(overlay) = ui.top_overlay() {
        return overlay.key_context();
    }
    if crate::playback::multiview::is_open(ui.playback) {
        return Context::Multiview; // Up/Down/Left/Right, OK, Long OK, Menu
    }
    match (ui.chrome.mode.get(), ui.guide.focus.with(|f| f.pane)) {
        (ViewMode::Fullscreen, _)
            if crate::playback::play_bar::captures(*ui)
                || ui.playback.panels.with(|p| p.captures_input()) =>
        {
            Context::Controls
        }
        (ViewMode::Fullscreen, _) => Context::Player,
        (_, Pane::Groups | Pane::Rail) => Context::Groups,
        _ => Context::Guide,
    }
}

/// A named keyboard binding fires again on auto-repeat (a held arrow
/// keeps moving). OK and Menu act once per physical press.
fn named_key_options(key: RemoteKey) -> KeyOptions {
    KeyOptions {
        repeat: !matches!(key, RemoteKey::Menu | RemoteKey::Ok),
        ..Default::default()
    }
}

/// Actions that a held key may activate only once on the current surface.
fn one_shot_action(action: Action, mode: ViewMode, context: Context) -> bool {
    matches!(action, Action::ShowMenu | Action::ToggleFavorite)
        || (action == Action::Select && mode == ViewMode::Guide && context == Context::Guide)
}

pub(crate) fn bind(ui: Ui) -> InputScope {
    focus_remote_root();
    number::bind(ui);
    let input = InputScope {
        ui,
        nav: Rc::new(RefCell::new(Nav::new())),
        owner: Owner::current(),
    };

    let digits = DIGITS
        .into_iter()
        .enumerate()
        .map(|(digit, name)| (name, RemoteKey::Digit(digit as u8)));
    let consumed = input.nav.borrow().consumed;
    // Named bindings omit the repeat phase. Ask NSH to suppress repeats
    // according to the current action, including custom key bindings.
    let one_shot_keys = computed(move || {
        let mode = ui.chrome.mode.get();
        let context = context(&ui);
        ui.app.settings.get();
        let keymap = crate::chrome::remote_editor::keymap();
        KEYS.iter()
            .map(|&(_, key)| key)
            .chain((0..DIGITS.len() as u8).map(RemoteKey::Digit))
            .filter(|&key| {
                named_key_options(key).repeat
                    && keymap
                        .resolve(context, key, Press::Short)
                        .is_some_and(|action| one_shot_action(action, mode, context))
            })
            .collect::<Vec<_>>()
    });
    for (name, key) in KEYS.iter().copied().chain(digits) {
        if !long_press() || name.contains('+') {
            let nav = input.nav.clone();
            // A keyboard binding reports no phase: while a prompt has its
            // key consumed it does not repeat (the platform holds the
            // auto-repeats), so only a fresh press reaches it.
            let held_off = computed(move || {
                consumed.with(|c| c.contains(&key))
                    || one_shot_keys.with(|keys| keys.contains(&key))
            });
            effect(move || {
                let mut options = named_key_options(key);
                options.repeat &= !held_off.get();
                #[cfg(feature = "test-support")]
                keyboard_repeats::set(name, options.repeat);
                let nav = nav.clone();
                on_key_with(name, options, move |_| {
                    key_event(ui, &nav, key, Phase::Down)
                });
            });
        }
    }
    // A prompt or the editor that opens while a key is held (by touch,
    // say) takes that key as one a key event under it would.
    let nav = input.nav.clone();
    effect(move || {
        let now = capture(ui.screens);
        let mut nav = nav.borrow_mut();
        if nav.session(now).1 {
            nav.cut_short();
            drop(nav);
            release_controls(ui);
        }
    });
    if crate::chrome::desktop::DESKTOP {
        on_key("shift+f10", move |_| {
            if context(&ui) == Context::Groups && ui.guide.focus.with(|f| f.pane == Pane::Groups) {
                ui.chrome.input.set(Input::Keys);
                let entry = ui.guide.focus.with_untracked(|f| f.group);
                crate::chrome::menu_view::open_group_options(ui, entry);
            } else {
                apply(ui, RemoteKey::Menu, Press::Short, Action::ShowMenu, false);
            }
        });
    }
    // Back on a PC keyboard: Escape is the platform's (NSH), Backspace ours
    // (it does not fire while a text field edits).
    on_key("backspace", move |_| {
        if number::backspace(ui) {
            return;
        }
        if ui.playback.player_surface != 0
            && crate::playback::pip::exit(ui, iptv_presentation::pip_window::Exit::BackToApp)
        {
            return;
        }
        crate::go_back(ui);
    });
    // Desktop sound keys (remotes and tablets use the system volume). F11
    // (window full screen; Ctrl+Cmd+F on macOS) is the platform's.
    on_key("ctrl+m", move |_| crate::playback::media::toggle_mute(ui));
    on_key("ctrl+up", move |_| {
        crate::playback::media::step_volume(ui, true)
    });
    on_key("ctrl+down", move |_| {
        crate::playback::media::step_volume(ui, false)
    });
    // macOS keeps Ctrl+Up/Down for Mission Control: Cmd+Up/Down there.
    if crate::host::MACOS {
        on_key("cmd+up", move |_| {
            crate::playback::media::step_volume(ui, true)
        });
        on_key("cmd+down", move |_| {
            crate::playback::media::step_volume(ui, false)
        });
    }
    // Spike shortcuts for the view modes.
    if ui.playback.player_surface == 0 {
        on_key("o", move |_| {
            ui.chrome
                .mode
                .apply(crate::chrome::mode::Action::ToggleOverlay)
        });
        on_key("f", move |_| {
            ui.chrome
                .mode
                .apply(crate::chrome::mode::Action::ToggleFullscreen)
        });
    }
    input
}

fn key_event(ui: Ui, nav: &Rc<RefCell<Nav>>, key: RemoteKey, phase: Phase) {
    if ui.playback.player_surface != 0
        && crate::playback::pip::in_window(ui.playback)
        && !matches!(
            key,
            RemoteKey::PlayPause | RemoteKey::Play | RemoteKey::Pause | RemoteKey::Stop
        )
    {
        return;
    }
    if phase != Phase::Up {
        if key != RemoteKey::Back {
            ui.chrome.exit_deadline.set(None);
        }
        crate::playback::browse_playback::activity(ui);
    }
    // Menu has no hold: held, its repeats would close and reopen what its
    // press opened (a menu, the play bar).
    if key == RemoteKey::Menu && phase == Phase::Repeat {
        return;
    }
    // A fresh Back: whatever an earlier one left armed is gone, before a
    // prompt below takes the key (its Back must not be swallowed).
    if key == RemoteKey::Back && phase == Phase::Down {
        ui.chrome.back_guard.update(BackGuard::pressed);
    }
    let captured = untracked(|| capture(ui.screens));
    let session = nav.borrow_mut().session(captured.clone()).0;
    if key == RemoteKey::Back {
        let mut nav = nav.borrow_mut();
        if phase == Phase::Down {
            nav.back_in = None;
        } else if let Some(began) = nav.back_in {
            // A Back that went down in a prompt: its repeats do nothing,
            // and once touch closed that prompt, Android's Back on its
            // release does not act on what it exposed.
            if phase == Phase::Up {
                nav.back_in = None;
                nav.unconsume(key);
                if session != Some(began) {
                    swallow_release(ui);
                }
            }
            return;
        }
    }
    if let (Some(capture), Some(session)) = (captured, session) {
        // A prompt or the editor takes the keys: a press pending from
        // before is cut short, and a key it takes acts nowhere else while
        // held.
        let mut nav = nav.borrow_mut();
        let pending = nav.presses.held_key().is_some();
        nav.cut_short();
        if phase == Phase::Up {
            // A remote's Back released here went down before this capture
            // (one that went down in it is `back_in`'s): Android's Back on
            // it does not dismiss the prompt, consumed or not.
            if key == RemoteKey::Back && long_press() {
                nav.consume(key);
            }
            release_consumed(ui, &nav, key);
        } else if phase == Phase::Down {
            nav.unconsume(key);
            let pin = matches!(capture, Capture::Pin(_));
            if key == RemoteKey::Back && (!pin || long_press()) {
                // The platform's Back (`go_back`) closes the PIN prompt or
                // cancels the editor's capture, once; acting here too
                // would also step back from what the prompt exposes.
                nav.back_in = Some(session);
            } else {
                nav.consume(key);
                drop(nav);
                if pin {
                    crate::screens::parental_view::remote(ui, key);
                } else {
                    crate::chrome::remote_editor::capture(ui.screens, key);
                }
            }
        }
        if pending {
            release_controls(ui);
        }
        return;
    }
    {
        let nav = nav.borrow();
        if phase == Phase::Down {
            nav.unconsume(key);
        } else if nav.is_consumed(key) {
            if phase == Phase::Up {
                release_consumed(ui, &nav, key);
            }
            return;
        }
    }
    // A key going down keeps the panels and the play bar up: a press that
    // waits to tell short from long must not find them gone on release.
    if phase != Phase::Up {
        keep_controls(ui);
    }
    let t = nsh_core::time::Instant::now();
    let here = surface(&ui);
    let ctx = here.0;
    let (decision, deadline) = {
        let mut nav = nav.borrow_mut();
        if phase == Phase::Down {
            nav.keymap = crate::chrome::remote_editor::keymap();
            if key == RemoteKey::Ok
                && matches!(here.1, InputOwner::SettingsPlaylist(_))
                && !crate::chrome::remote_editor::overridden(ctx, key)
            {
                nav.keymap.set_override(iptv_nav::Binding {
                    context: Some(ctx),
                    key,
                    press: Press::Long,
                    action: Action::ShowMenu,
                });
            }
        }
        let now = ui.now_ms();
        // `remote.long_press`, taken up when a key goes down (a new press
        // ends whatever was held).
        let threshold = crate::wiring::settings::long_press_ms();
        if long_press() && phase == Phase::Down && nav.presses.threshold_ms() != threshold {
            nav.presses = HeldPresses::new(threshold);
        }
        // A seek key in a replay has no long press: held, it repeats.
        let has_long = long_press()
            && nav.keymap.has_long(ctx, key)
            && (crate::chrome::remote_editor::overridden(ctx, key)
                || !crate::playback::player_keys::seeks(ui.playback, ctx, key));
        let decision = nav.presses.feed(here, key, phase, now, has_long);
        let deadline = (phase == Phase::Down)
            .then(|| nav.presses.long_deadline_ms())
            .flatten()
            .map(|at| at.saturating_sub(now));
        (decision, deadline)
    };
    // A card or bar on screen outlasts the wait (its own timeout can be
    // shorter than the long press): the release finds what was pressed.
    // Once the press is decided, their own timeout alone counts.
    let waiting = nav.borrow().presses.long_deadline_ms();
    // (Another key's release leaves a waiting press as it is.)
    match waiting {
        Some(at) if phase != Phase::Up => keep_controls_until(ui, at + 50),
        Some(_) => {}
        None => release_controls(ui),
    }
    let action = decision.and_then(|d| resolve(ui, &nav.borrow().keymap, d));
    if let Some((key, press, action)) = action {
        // A hold decided by an auto-repeat is the hold's one action, not a
        // repeat of it.
        act(
            ui,
            key,
            press,
            action,
            phase == Phase::Repeat && press == Press::Short,
        );
    }
    // Stop/Guide can dispose an auxiliary window before this handler returns.
    if untracked(|| ui.chrome.stats.try_get()).is_none() {
        return;
    }
    // IR remotes never auto-repeat: decide long press on a timer too. If the
    // key was released (or a repeat already fired it), the poll finds nothing.
    if let Some(wait_ms) = deadline {
        let nav = nav.clone();
        let _task = set_timeout(Duration::from_millis(wait_ms), move || {
            // The surface of the moment: a press whose surface changed
            // while it was held does nothing (`HeldPresses`), nor does one
            // that a PIN prompt or the remote editor now captures.
            let captured = untracked(|| capture(ui.screens)).is_some();
            let decision = nav.borrow_mut().presses.poll(surface(&ui), ui.now_ms());
            let Some(decision) = decision else {
                return;
            };
            release_controls(ui);
            if captured {
                log::info!("iptv long press by timer dropped: a prompt took the keys");
                nav.borrow_mut().cut_short();
                return;
            }
            let action = resolve(ui, &nav.borrow().keymap, decision);
            if let Some((key, press, action)) = action {
                log::info!("iptv long press by timer: {action:?}");
                act(ui, key, press, action, false);
            }
        });
    }
    let us = t.elapsed().as_micros();
    if phase == Phase::Up && action.is_none() {
        return; // releases are logged only when they act (a deferred short press)
    }
    ui.chrome.stats.update(|s| {
        s.keys += 1;
        s.last_us = us;
        s.max_us = s.max_us.max(us);
    });
    let action = action.map(|(_, _, a)| a);
    log::info!(
        "iptv key={} phase={phase:?} ctx={ctx:?} action={action:?} n={} handler_us={us} mode={:?} panels={:?}",
        key.name(),
        ui.chrome.stats.with(|s| s.keys),
        ui.chrome.mode.get(),
        ui.playback.panels.with(|p| p.shown)
    );
}

/// The action a press decided on, after the player's remapping; none for
/// a press dropped because its surface changed during the hold.
fn resolve(
    ui: Ui,
    keymap: &Keymap,
    decision: Decision<Surface>,
) -> Option<(RemoteKey, Press, Action)> {
    match decision {
        Decision::Act {
            key,
            press,
            context: (ctx, _),
        } => {
            if ctx == Context::Menu
                && (ui.top_overlay() == Some(crate::chrome::overlays::Overlay::Leave)
                    || ui.screens.menu.with(|menu| {
                        menu.as_ref()
                            .is_some_and(|menu| menu.group_offset.is_some())
                    }))
            {
                match key {
                    RemoteKey::Left => return Some((key, press, Action::MoveLeft)),
                    RemoteKey::Right => return Some((key, press, Action::MoveRight)),
                    _ => {}
                }
            }
            let action = keymap.resolve(ctx, key, press);
            let action = if crate::chrome::remote_editor::overridden(ctx, key) {
                action
            } else {
                crate::playback::player_keys::remap(&ui, ctx, key, action)
            };
            action.map(|a| (key, press, a))
        }
        Decision::Dropped {
            key,
            press,
            began,
            now,
        } => {
            log::info!(
                "iptv key={} {press:?} dropped: the surface changed from {:?} ({:?}) to {:?} ({:?}) during the hold",
                key.name(),
                began.0,
                began.1,
                now.0,
                now.1
            );
            // Android's own Back follows the release: it must not step
            // back from the surface the press did not begin on either.
            if key == RemoteKey::Back {
                swallow_release(ui);
            }
            None
        }
    }
}

/// A key going down counts as use of the panels and the play bar on
/// screen, as an applied key does (`apply`).
fn keep_controls(ui: Ui) {
    if ui.playback.panels.with_untracked(|p| p.shown) != iptv_presentation::panels::Shown::Hidden {
        ui.update_panels(|p, now| p.touch(now));
    }
    if untracked(|| crate::playback::play_bar::shows(ui)) {
        crate::playback::play_bar::used(ui);
    }
}

/// The release of a key a prompt took or cut short: it is no longer
/// consumed, and a Back that began before the prompt does not let
/// Android's own Back act on its release either.
fn release_consumed(ui: Ui, nav: &Nav, key: RemoteKey) {
    if nav.unconsume(key) && key == RemoteKey::Back {
        swallow_release(ui);
    }
}

/// The held key no longer keeps the card or the play bar: decided,
/// dropped or taken by a prompt.
fn release_controls(ui: Ui) {
    if ui.playback.panels.with_untracked(|p| p.holding()) {
        ui.playback.panels.update(|p| p.release_hold());
    }
    if untracked(|| ui.playback.play_bar.try_get()).is_some_and(|b| b.holding()) {
        crate::playback::play_bar::release_hold(ui);
    }
}

/// Keeps a shown card or play bar until at least `at` (ms).
fn keep_controls_until(ui: Ui, at: u64) {
    if ui.playback.panels.with_untracked(|p| p.shown) != iptv_presentation::panels::Shown::Hidden {
        ui.playback.panels.update(|p| p.keep_until(at));
    }
    if untracked(|| crate::playback::play_bar::shows(ui)) {
        crate::playback::play_bar::keep_until(ui, at);
    }
}

fn apply(ui: Ui, key: RemoteKey, press: Press, action: Action, repeat: bool) {
    // A held key must not reopen a menu, toggle a favorite repeatedly or
    // turn a guide preview into fullscreen. Remapped keys can repeat too.
    if repeat && one_shot_action(action, ui.chrome.mode.get_untracked(), context(&ui)) {
        return;
    }
    if player_actions::transport(ui, key, press, action) {
        return;
    }
    if action != Action::Ignore {
        crate::playback::external_player::cancel_pending(ui.cast);
    }
    if player_actions::auxiliary(ui, action) {
        return;
    }
    if !matches!(action, Action::ChannelUp | Action::ChannelDown) {
        ui.playback.pending_zap.set(None);
    }
    crate::playback::browse_playback::activity(ui);
    // The first key after touch or a pointer starts from a row on screen.
    if ui.chrome.input.get() != Input::Keys {
        if let Some((first, last)) = ui.guide.visible_rows.get() {
            ui.guide.focus.update(|f| f.adopt_rows(first, last));
        }
        ui.chrome.input.set(Input::Keys);
    }
    // Any key while panels show keeps them up.
    ui.update_panels(|p, now| p.touch(now));
    if modal_actions::apply(ui, action) {
        return;
    }
    if crate::playback::multiview::is_open(ui.playback) {
        crate::playback::multiview::key(ui, action);
        return;
    }
    if number::apply(ui, action) {
        return;
    }
    if player_actions::preview(ui, key, press, action, repeat) {
        return;
    }
    // Any key but Back dismisses the hint card (Back does, as its own step).
    if action != Action::Back && ui.guide.focus.with(|f| f.hint) {
        ui.guide.focus.update(|f| f.hint = false);
    }
    if !player_actions::controls(ui, key, press, action, repeat) {
        guide_actions::apply(ui, key, action);
    }
}

/// Synthetic native journey: the same Enter dispatch as the registered key handler.
#[cfg(feature = "test-support")]
pub(crate) fn synthetic_enter(ui: Ui) {
    let nav = Rc::new(RefCell::new(Nav::new()));
    key_event(ui, &nav, RemoteKey::Ok, Phase::Down);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[component]
    fn PendingHandoff() -> impl IntoElement {
        let directory = tempfile::tempdir().unwrap();
        let runtime = Rc::new(crate::wiring::runtime_owner::new_runtime(
            directory.path().to_owned(),
        ));
        let ui = crate::wiring::runtime_owner::with_runtime(runtime, || {
            Ui::new(
                crate::SharedUi::new(),
                crate::playback::player::Player::new(),
            )
        });
        crate::playback::external_player::cancel_pending(ui.cast);
        view()
            .child(text(move || {
                format!("Request {}", ui.cast.external.request.get())
            }))
            .child(
                view().label("Repeat select").h(px(40.0)).on_press(move || {
                    apply(ui, RemoteKey::Red, Press::Short, Action::Select, true)
                }),
            )
            .child(
                view()
                    .label("Unrelated touch")
                    .h(px(40.0))
                    .on_press(move || ui.touched()),
            )
    }

    #[test]
    fn ignored_select_repeat_keeps_the_external_handoff_pending() {
        let mut app = nsh_testing::Headless::start(nsh_engine::EngineConfig::new(
            nsh_core::element::Root::of::<PendingHandoff>(),
        ))
        .unwrap();
        app.wait_for("text:Request 1").unwrap();
        app.press("label:Repeat select").unwrap();
        assert!(app.screen().matches("text:Request 1").unwrap());
        app.press("label:Unrelated touch").unwrap();
        app.wait_for("text:Request 2").unwrap();
    }

    #[test]
    fn keyboard_menu_and_ok_fire_once_while_held_and_moves_repeat() {
        assert!(!named_key_options(RemoteKey::Menu).repeat, "M: once");
        assert!(!named_key_options(RemoteKey::Ok).repeat, "Enter: once");
        for key in [RemoteKey::Down, RemoteKey::Right, RemoteKey::ChannelUp] {
            assert!(named_key_options(key).repeat, "{key:?}");
        }
    }
}
