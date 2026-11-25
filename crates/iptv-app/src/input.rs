//! Remote input: NSH keystrokes → `iptv_nav` keys → short/long presses →
//! context keymap → actions applied to the app state.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use iptv_nav::{
    Action, BackGuard, Context, Decision, HeldPresses, Keymap, Phase, Press, Profile, RemoteKey,
};
use nsh_core::keymap::Keystroke;
use nsh_core::prelude::*;
use nsh_core::{KeyCapture, KeyEvent};

use crate::guide::{Move, Pane};
use crate::{Input, Ui, ViewMode};

mod back;
mod keys;
mod number;

pub(crate) use back::swallow_back;
use back::{act, swallow_release};
use keys::{DIGITS, KEYS, RawKeys};

/// A TV remote's profile (Android, a browser TV) distinguishes a tap from
/// a held key.
const REMOTE_KEYS: bool = crate::host::REMOTE_KEYS;

/// Headless scenarios that drive a remote's raw Down/Repeat/Up phases
/// (`use_remote_key_phases`), set before the app starts.
#[cfg(feature = "test-support")]
static REMOTE_PHASES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Test support: take keys as the Android remote does (raw key phases on
/// the window's root, short and long presses) in this process. Call it
/// before the app starts.
#[cfg(feature = "test-support")]
pub fn use_remote_key_phases() {
    REMOTE_PHASES.store(true, std::sync::atomic::Ordering::Relaxed);
}

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

/// Whether keys arrive as a remote's raw phases and tell a tap from a hold.
pub(crate) fn long_press() -> bool {
    #[cfg(feature = "test-support")]
    if REMOTE_PHASES.load(std::sync::atomic::Ordering::Relaxed) {
        return true;
    }
    REMOTE_KEYS
}

/// Whether `key` tells a tap from a hold: where keys report their
/// phases ([`long_press`]), but not a keyboard shortcut with a modifier
/// (`shift+left`), which reports its press only.
pub(crate) fn holds(key: RemoteKey) -> bool {
    long_press()
        && (KEYS.iter().all(|&(_, k)| k != key)
            || KEYS
                .iter()
                .any(|&(name, k)| k == key && !name.contains('+')))
}

/// Whose keys: a TV remote's on Android, a keyboard's on the desktops
/// (`iptv_nav::Profile`: one map; the hints name their keys).
pub(crate) const PROFILE: Profile = if REMOTE_KEYS {
    Profile::Remote
} else {
    Profile::Keyboard
};

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
    Pin(crate::parental_view::Prompt),
    Editor,
}

/// What takes the keys now, if anything (tracked).
fn capture(ui: Ui) -> Option<Capture> {
    crate::parental_view::prompt(ui)
        .map(Capture::Pin)
        .or_else(|| crate::remote_editor::capturing(ui).then_some(Capture::Editor))
}

/// What owns input, finer than the keymap's [`Context`]: the info card and
/// the control strip are both `Controls`, the guide's panes both `Guide`.
/// A press that waits to tell a tap from a hold belongs to this surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputOwner {
    Overlay(crate::overlays::Overlay),
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
        if overlay == crate::overlays::Overlay::ChannelList {
            ui.channels
                .with(|list| list.map(|list| InputOwner::ChannelList(list.pane)))
                .unwrap_or(InputOwner::Overlay(overlay))
        } else {
            InputOwner::Overlay(overlay)
        }
    } else if crate::multiview::is_open(*ui) {
        InputOwner::Multiview
    } else if ui.mode.get() == ViewMode::Fullscreen {
        if crate::play_bar::captures(*ui) {
            InputOwner::PlayBar
        } else {
            match ui.panels.with(|p| p.shown) {
                iptv_presentation::panels::Shown::Card => InputOwner::Card,
                iptv_presentation::panels::Shown::Controls => InputOwner::Strip,
                _ => InputOwner::Video,
            }
        }
    } else {
        InputOwner::Pane(ui.focus.with(|f| f.pane))
    };
    (context(ui), owner)
}

impl Nav {
    fn new() -> Self {
        Nav {
            keymap: crate::remote_editor::keymap(),
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

#[derive(Clone, Copy)]
struct RemoteFocus(ElementRef);

fn focus_remote_root() {
    if long_press()
        && let Some(focus) = use_context::<RemoteFocus>()
    {
        focus.0.focus();
    }
}

/// Give remote focus back to the app when native editing ends.
pub(crate) fn dismiss_keyboard() {
    nsh_core::element::dismiss_keyboard();
    focus_remote_root();
}

/// Keep a field in sync with the app's editing mode.
pub(crate) fn bind_field_focus(field: ElementRef, editing: impl Fn() -> bool + 'static) {
    let was_editing = Cell::new(false);
    effect(move || {
        let editing = editing();
        let leaving = was_editing.replace(editing) && !editing;
        if editing {
            field.focus();
        } else {
            field.blur();
            if leaving {
                focus_remote_root();
            }
        }
    });
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
    if crate::multiview::is_open(*ui) {
        return Context::Multiview; // Up/Down/Left/Right, OK, Long OK, Menu
    }
    match (ui.mode.get(), ui.focus.with(|f| f.pane)) {
        (ViewMode::Fullscreen, _)
            if crate::play_bar::captures(*ui) || ui.panels.with(|p| p.captures_input()) =>
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
    let guide_active =
        computed(move || ui.mode.get() == ViewMode::Guide && context(&ui) == Context::Guide);
    // Named bindings omit the repeat phase. Ask NSH to suppress repeats
    // of remapped Select keys while they activate the guide's preview.
    let guide_select_keys = computed(move || {
        if !guide_active.get() {
            return Vec::new();
        }
        ui.app.settings.get();
        let keymap = crate::remote_editor::keymap();
        KEYS.iter()
            .map(|&(_, key)| key)
            .chain((0..DIGITS.len() as u8).map(RemoteKey::Digit))
            .filter(|&key| {
                named_key_options(key).repeat
                    && keymap.resolve(Context::Guide, key, Press::Short) == Some(Action::Select)
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
                    || guide_select_keys.with(|keys| keys.contains(&key))
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
        let now = capture(ui);
        let mut nav = nav.borrow_mut();
        if nav.session(now).1 {
            nav.cut_short();
            drop(nav);
            release_controls(ui);
        }
    });
    if crate::desktop::DESKTOP {
        on_key("shift+f10", move |_| {
            if context(&ui) == Context::Groups && ui.focus.with(|f| f.pane == Pane::Groups) {
                ui.input.set(Input::Keys);
                let entry = ui.focus.with_untracked(|f| f.group);
                crate::menu_view::open_group_options(ui, entry);
            } else {
                apply(ui, RemoteKey::Menu, Action::ShowMenu, false);
            }
        });
    }
    // Back on a PC keyboard: Escape is the platform's (NSH), Backspace ours
    // (it does not fire while a text field edits).
    on_key("backspace", move |_| {
        if number::backspace(ui) {
            return;
        }
        if ui.player_surface != 0
            && crate::pip::exit(ui, iptv_presentation::pip_window::Exit::BackToApp)
        {
            return;
        }
        crate::go_back(ui);
    });
    // Desktop sound keys (remotes and tablets use the system volume). F11
    // (window full screen; Ctrl+Cmd+F on macOS) is the platform's.
    on_key("ctrl+m", move |_| crate::media::toggle_mute(ui));
    on_key("ctrl+up", move |_| crate::media::step_volume(ui, true));
    on_key("ctrl+down", move |_| crate::media::step_volume(ui, false));
    // macOS keeps Ctrl+Up/Down for Mission Control: Cmd+Up/Down there.
    if crate::host::MACOS {
        on_key("cmd+up", move |_| crate::media::step_volume(ui, true));
        on_key("cmd+down", move |_| crate::media::step_volume(ui, false));
    }
    // Spike shortcuts for the view modes.
    if ui.player_surface == 0 {
        on_key("o", move |_| ui.toggle(crate::settings::guide_over_video()));
        on_key("f", move |_| ui.toggle(ViewMode::Fullscreen));
    }
    input
}

fn key_event(ui: Ui, nav: &Rc<RefCell<Nav>>, key: RemoteKey, phase: Phase) {
    if ui.player_surface != 0
        && crate::pip::in_window(ui)
        && !matches!(
            key,
            RemoteKey::PlayPause | RemoteKey::Play | RemoteKey::Pause | RemoteKey::Stop
        )
    {
        return;
    }
    if phase != Phase::Up {
        crate::browse_playback::activity(ui);
    }
    // Menu has no hold: held, its repeats would close and reopen what its
    // press opened (a menu, the play bar).
    if key == RemoteKey::Menu && phase == Phase::Repeat {
        return;
    }
    // A fresh Back: whatever an earlier one left armed is gone, before a
    // prompt below takes the key (its Back must not be swallowed).
    if key == RemoteKey::Back && phase == Phase::Down {
        ui.back_guard.update(BackGuard::pressed);
    }
    let captured = untracked(|| capture(ui));
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
                    crate::parental_view::remote(ui, key);
                } else {
                    crate::remote_editor::capture(ui, key);
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
            nav.keymap = crate::remote_editor::keymap();
        }
        let now = ui.now_ms();
        // `remote.long_press`, taken up when a key goes down (a new press
        // ends whatever was held).
        let threshold = crate::settings::long_press_ms();
        if long_press() && phase == Phase::Down && nav.presses.threshold_ms() != threshold {
            nav.presses = HeldPresses::new(threshold);
        }
        // A seek key in a replay has no long press: held, it repeats.
        let has_long = long_press()
            && nav.keymap.has_long(ctx, key)
            && (crate::remote_editor::overridden(ctx, key)
                || !crate::player_keys::seeks(&ui, ctx, key));
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
    if untracked(|| ui.stats.try_get()).is_none() {
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
            let captured = untracked(|| capture(ui)).is_some();
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
    ui.stats.update(|s| {
        s.keys += 1;
        s.last_us = us;
        s.max_us = s.max_us.max(us);
    });
    let action = action.map(|(_, _, a)| a);
    log::info!(
        "iptv key={} phase={phase:?} ctx={ctx:?} action={action:?} n={} handler_us={us} mode={:?} panels={:?}",
        key.name(),
        ui.stats.with(|s| s.keys),
        ui.mode.get(),
        ui.panels.with(|p| p.shown)
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
                && (ui.top_overlay() == Some(crate::overlays::Overlay::Leave)
                    || ui.menu.with(|menu| {
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
            let action = if crate::remote_editor::overridden(ctx, key) {
                action
            } else {
                crate::player_keys::remap(&ui, ctx, key, action)
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
    if ui.panels.with_untracked(|p| p.shown) != iptv_presentation::panels::Shown::Hidden {
        ui.update_panels(|p, now| p.touch(now));
    }
    if untracked(|| crate::play_bar::shows(ui)) {
        crate::play_bar::used(ui);
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
    if ui.panels.with_untracked(|p| p.holding()) {
        ui.panels.update(|p| p.release_hold());
    }
    if untracked(|| ui.play_bar.try_get()).is_some_and(|b| b.holding()) {
        crate::play_bar::release_hold(ui);
    }
}

/// Keeps a shown card or play bar until at least `at` (ms).
fn keep_controls_until(ui: Ui, at: u64) {
    if ui.panels.with_untracked(|p| p.shown) != iptv_presentation::panels::Shown::Hidden {
        ui.panels.update(|p| p.keep_until(at));
    }
    if untracked(|| crate::play_bar::shows(ui)) {
        crate::play_bar::keep_until(ui, at);
    }
}

fn apply(ui: Ui, key: RemoteKey, action: Action, repeat: bool) {
    // A held key must not reopen a menu, toggle a favorite repeatedly or
    // turn a guide preview into fullscreen. Remapped keys can repeat too.
    if repeat
        && (matches!(action, Action::ShowMenu | Action::ToggleFavorite)
            || (action == Action::Select
                && ui.mode.get_untracked() == ViewMode::Guide
                && context(&ui) == Context::Guide))
    {
        return;
    }
    if action == Action::Stop && ui.player_surface == 0 && crate::external_player::is_playing(ui) {
        crate::player_keys::stop_playback(ui);
        return;
    }
    if action != Action::Ignore {
        crate::external_player::cancel_pending(ui);
    }
    if ui.player_surface != 0 {
        match action {
            // The guide is in the main window; dismissing this player lets
            // its owner stop the correct surface. Search has no local screen.
            Action::Stop | Action::ShowGuide => {
                ui.mode.set(crate::settings::browse_mode());
                return;
            }
            Action::OpenSearch | Action::Record => return,
            _ => {}
        }
    }
    if !matches!(action, Action::ChannelUp | Action::ChannelDown) {
        ui.pending_zap.set(None);
    }
    crate::browse_playback::activity(ui);
    // The first key after touch or a pointer starts from a row on screen.
    if ui.input.get() != Input::Keys {
        if let Some((first, last)) = ui.visible_rows.get() {
            ui.focus.update(|f| f.adopt_rows(first, last));
        }
        ui.input.set(Input::Keys);
    }
    // Any key while panels show keeps them up.
    ui.update_panels(|p, now| p.touch(now));
    use crate::overlays::Overlay;
    let top = ui.top_overlay();
    let mv = |m: Move| crate::lists::navigate(ui, m);
    if top == Some(Overlay::QuitConfirm) {
        crate::desktop::quit_confirm::key(ui, action);
        return;
    }
    if top == Some(Overlay::StoreStartupFailure) {
        crate::startup::store_failure_key(ui, action);
        return;
    }
    if top == Some(Overlay::Vod) {
        crate::vod_view::key(ui, action);
        return;
    }
    if top == Some(Overlay::Parental) {
        crate::parental_view::key(ui, action);
        return;
    }
    if top == Some(Overlay::RestoreReview) {
        crate::backup::key(ui, action);
        return;
    }
    if top == Some(Overlay::Recordings) {
        crate::recordings_view::key(ui, action);
        return;
    }
    if top == Some(Overlay::Wizard) {
        crate::wizard_view::key(ui, action);
        return;
    }
    // Top-most first: a guide sources screen opens over the playlists
    // screen, which opens over settings.
    if top == Some(Overlay::GuideRepair) {
        crate::guide_repair_view::key(ui, action);
        return;
    }
    if top == Some(Overlay::GuideInspection) {
        crate::guide_inspect_view::key(ui, action);
        return;
    }
    if top == Some(Overlay::GuideSources) {
        crate::epg_sources_view::key(ui, action);
        return;
    }
    if top == Some(Overlay::RemoteEditor) {
        crate::remote_editor::key(ui, action);
        return;
    }
    if top == Some(Overlay::Organization) {
        crate::organization_view::key(ui, action);
        return;
    }
    if top == Some(Overlay::CustomGroups) {
        crate::custom_groups::key(ui, action);
        return;
    }
    if top == Some(Overlay::Playlists) {
        crate::playlists_view::key(ui, action);
        return;
    }
    if top == Some(Overlay::Settings) {
        crate::settings_view::key(ui, action);
        return;
    }
    if top == Some(Overlay::GuideAssign) {
        crate::guide_assign_view::key(ui, action);
        return;
    }
    if top == Some(Overlay::FavoritesEditor) {
        crate::fav_editor::key(ui, action);
        return;
    }
    if matches!(top, Some(Overlay::Chooser | Overlay::Leave)) {
        match action {
            Action::MoveUp => crate::chooser::move_focus(ui, -1),
            Action::MoveDown => crate::chooser::move_focus(ui, 1),
            Action::MoveLeft if top == Some(Overlay::Leave) => {
                crate::chooser::focus_leave(ui, false)
            }
            Action::MoveRight if top == Some(Overlay::Leave) => {
                crate::chooser::focus_leave(ui, true)
            }
            Action::Select => {
                if let Some(i) = ui.chooser.with(|c| c.as_ref().map(|c| c.list.focus)) {
                    crate::chooser::pick(ui, i);
                }
            }
            Action::Back | Action::ShowMenu => crate::chooser::close(ui),
            _ => {}
        }
        return;
    }
    if top == Some(Overlay::Description) {
        // Select and Record press the details' Record button, which only a
        // program on now shows (`recordings_view::record_details`).
        if matches!(action, Action::Select | Action::Record)
            && let Some(origin) = ui.description.get_untracked()
        {
            crate::recordings_view::record_details(ui, origin);
            return;
        }
        // Back (or Menu) closes program details.
        match action {
            Action::MoveUp => crate::menu_view::scroll_description(ui, -1.0),
            Action::MoveDown => crate::menu_view::scroll_description(ui, 1.0),
            Action::Back => crate::menu_view::back(ui),
            Action::ShowMenu => crate::menu_view::close(ui),
            _ => {}
        }
        return;
    }
    if top == Some(Overlay::Menu) {
        match action {
            Action::MoveUp => ui
                .menu
                .update(|m| m.as_mut().into_iter().for_each(|m| m.move_focus(-1))),
            Action::MoveDown => ui
                .menu
                .update(|m| m.as_mut().into_iter().for_each(|m| m.move_focus(1))),
            Action::MoveLeft | Action::MoveRight => ui.menu.update(|menu| {
                if let Some(menu) = menu {
                    menu.step_group_offset(if action == Action::MoveLeft { -1 } else { 1 });
                }
            }),
            Action::Select => {
                if let Some(item) = ui.menu.with(|m| m.as_ref().and_then(|m| m.focused())) {
                    crate::menu_view::perform(ui, item);
                }
            }
            Action::Back => crate::menu_view::back(ui),
            Action::ShowMenu => crate::menu_view::close(ui),
            _ => {}
        }
        return;
    }
    if top == Some(Overlay::ChannelPicker) {
        crate::multiview_picker::key(ui, action);
        return;
    }
    if top == Some(Overlay::ChannelList) {
        crate::channels_view::key(ui, action);
        return;
    }
    if top == Some(Overlay::Search) {
        crate::search_view::key(ui, action);
        return;
    }
    if crate::multiview::is_open(ui) {
        crate::multiview::key(ui, action);
        return;
    }
    if number::apply(ui, action) {
        return;
    }
    // The preview owns focus, not the guide row left underneath it. Keep
    // program/list commands from acting on a different channel.
    if ui.mode.get() == ViewMode::Guide && ui.focus.with(|f| f.pane == Pane::Preview) {
        match action {
            Action::MoveUp => mv(Move::Up),
            Action::MoveDown => mv(Move::Down),
            Action::MoveLeft => mv(Move::Left),
            Action::MoveRight => mv(Move::Right),
            Action::Select | Action::ReturnToPlayer => crate::lists::ok(ui),
            Action::ShowMenu | Action::ShowControls
                if ui.playing.get().is_some() || crate::on_demand::active(ui) =>
            {
                ui.mode.set(ViewMode::Fullscreen);
                crate::play_bar::remote(ui);
            }
            Action::Back => {
                crate::go_back(ui);
            }
            Action::PlayPause | Action::Stop => {
                crate::player_keys::apply(ui, key, action, repeat);
            }
            _ => {}
        }
        return;
    }
    // Any key but Back dismisses the hint card (Back does, as its own step).
    if action != Action::Back && ui.focus.with(|f| f.hint) {
        ui.focus.update(|f| f.hint = false);
    }
    if crate::play_bar::key(ui, key, action, repeat) {
        return;
    }
    // Full screen and its panels: `player_keys.rs`.
    if crate::player_keys::apply(ui, key, action, repeat) {
        return;
    }
    match action {
        Action::Record => crate::recordings_view::current(ui),
        Action::ShowMenu if ui.focus.with(|f| f.pane) == Pane::Groups => {
            let entry = ui.focus.with_untracked(|f| f.group);
            let combined = crate::data::combined::active(&crate::runtime());
            if (key == RemoteKey::Menu && !combined && entry == iptv_presentation::lists::ALL)
                || (!crate::menu_view::open_group_options(ui, entry)
                    && ui.app.library.shown.get_untracked().is_none()
                    && !combined)
            {
                crate::wizard_view::open_add(ui);
            }
        }
        Action::ShowMenu if ui.focus.with(|f| f.pane) == Pane::Rail => {}
        Action::ShowMenu
            if ui.mode.get() != ViewMode::Fullscreen
                && ui.focus.with(|f| f.pane) == Pane::Guide =>
        {
            crate::menu_view::open_timeline_context(ui);
        }
        Action::ShowMenu => crate::menu_view::open(ui),
        Action::MoveUp => mv(Move::Up),
        Action::MoveDown => mv(Move::Down),
        Action::MoveLeft => mv(Move::Left),
        Action::MoveRight => mv(Move::Right),
        Action::Select => crate::lists::ok(ui),
        Action::ShowGuide => ui.mode.set(match ui.mode.get() {
            ViewMode::Fullscreen => crate::settings::guide_over_video(),
            _ => crate::settings::browse_mode(),
        }),
        Action::ShowGroups => crate::lists::open_groups(ui),
        Action::OpenSearch => crate::search_view::open(ui),
        a @ (Action::JumpToNow
        | Action::HourBack
        | Action::HourForward
        | Action::DayBack
        | Action::DayForward
        | Action::ToggleGroups) => crate::grid::keys::apply(ui, a),
        Action::ReturnToPlayer if ui.playing.get().is_some() => ui.mode.set(ViewMode::Fullscreen),
        Action::ToggleFavorite => {
            if let Some(channel) = if ui.mode.get() == ViewMode::Fullscreen {
                ui.playing.get()
            } else {
                crate::lists::focused_channel(ui)
            } {
                crate::favorites::toggle(ui, channel);
            }
        }
        Action::PageUp => page(ui, -1),
        Action::PageDown => page(ui, 1),
        other => log::info!("iptv action {other:?} not implemented yet"),
    }
}

/// Page up/down: moves the focus by the rows on screen.
fn page(ui: Ui, dir: isize) {
    let rows = ui.visible_rows.get().map_or(8, |(a, b)| (b - a).max(1));
    let len = ui.list.with(|l| l.len());
    ui.focus.update(|f| {
        f.channel = iptv_presentation::guide::page_row(f.channel, len, dir * rows as isize);
    });
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
        let runtime = Rc::new(crate::new_runtime(directory.path().to_owned()));
        let ui = crate::with_runtime(runtime, || {
            Ui::new(crate::SharedUi::new(), crate::player::Player::new())
        });
        crate::external_player::cancel_pending(ui);
        view()
            .child(text(move || {
                format!("Request {}", ui.external_request.get())
            }))
            .child(
                view()
                    .label("Repeat select")
                    .h(px(40.0))
                    .on_press(move || apply(ui, RemoteKey::Red, Action::Select, true)),
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
