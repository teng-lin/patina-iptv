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

/// The Android remote profile distinguishes a tap from a held key.
const ANDROID_KEYS: bool = cfg!(target_os = "android");

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

/// Whether keys arrive as a remote's raw phases and tell a tap from a hold.
pub(crate) fn long_press() -> bool {
    #[cfg(feature = "test-support")]
    if REMOTE_PHASES.load(std::sync::atomic::Ordering::Relaxed) {
        return true;
    }
    ANDROID_KEYS
}

/// Whose keys: a TV remote's on Android, a keyboard's on the desktops
/// (`iptv_nav::Profile`: one map; the hints name their keys).
pub(crate) const PROFILE: Profile = if ANDROID_KEYS {
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
    /// down afresh.
    consumed: Vec<RemoteKey>,
}

/// What owns input, finer than the keymap's [`Context`]: the info card and
/// the control strip are both `Controls`, the guide's panes both `Guide`.
/// A press that waits to tell a tap from a hold belongs to this surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputOwner {
    Overlay(crate::overlays::Overlay),
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
        InputOwner::Overlay(overlay)
    } else if crate::multiview::is_open(*ui) {
        InputOwner::Multiview
    } else if ui.mode.get() == ViewMode::Fullscreen {
        if crate::play_bar::captures(*ui) {
            InputOwner::PlayBar
        } else {
            match ui.panels.with(|p| p.shown) {
                iptv_core::panels::Shown::Card => InputOwner::Card,
                iptv_core::panels::Shown::Controls => InputOwner::Strip,
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
            consumed: Vec::new(),
        }
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
/// keeps moving), except Menu (`M`): held, it opens its menu once.
fn named_key_options(key: RemoteKey) -> KeyOptions {
    KeyOptions {
        repeat: key != RemoteKey::Menu,
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
    for (name, key) in KEYS.iter().copied().chain(digits) {
        if !long_press() || name.contains('+') {
            let nav = input.nav.clone();
            on_key_with(name, named_key_options(key), move |_| {
                key_event(ui, &nav, key, Phase::Down)
            });
        }
    }
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
        if ui.player_surface != 0 && crate::pip::exit(ui, iptv_core::pip_window::Exit::BackToApp) {
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
    if cfg!(target_os = "macos") {
        on_key("cmd+up", move |_| crate::media::step_volume(ui, true));
        on_key("cmd+down", move |_| crate::media::step_volume(ui, false));
    }
    // Spike shortcuts for the view modes.
    if ui.player_surface == 0 {
        on_key("o", move |_| ui.toggle(ViewMode::Overlay));
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
    let pin = crate::parental_view::is_open(ui);
    if pin || crate::remote_editor::captures(ui) {
        // A prompt or the editor takes the keys: a press pending from
        // before is forgotten (its release or timer must not act once the
        // prompt closes), and a key it takes acts nowhere else while held.
        let mut nav = nav.borrow_mut();
        // The cut-short key stays consumed until its release or a fresh
        // press: a later repeat must not start a new hold of it.
        if let Some(held) = nav.presses.held_key()
            && !nav.consumed.contains(&held)
        {
            nav.consumed.push(held);
        }
        nav.presses.cancel();
        if phase == Phase::Up {
            release_consumed(ui, &mut nav, key);
            return;
        }
        if phase == Phase::Down {
            nav.consumed.retain(|k| *k != key);
            if key == RemoteKey::Back && (!pin || long_press()) {
                // The platform's Back (`go_back`) closes the PIN prompt or
                // cancels the editor's capture, once; acting here too
                // would also step back from what the prompt exposes.
                return;
            }
            nav.consumed.push(key);
            drop(nav);
            if pin {
                crate::parental_view::remote(ui, key);
            } else {
                crate::remote_editor::capture(ui, key);
            }
        }
        return;
    }
    {
        let mut nav = nav.borrow_mut();
        if phase == Phase::Down {
            nav.consumed.retain(|k| *k != key);
        } else if nav.consumed.contains(&key) {
            if phase == Phase::Up {
                release_consumed(ui, &mut nav, key);
            }
            return;
        }
    }
    // A key going down keeps the panels and the play bar up: a press that
    // waits to tell short from long must not find them gone on release.
    if phase != Phase::Up {
        keep_controls(ui);
    }
    let t = std::time::Instant::now();
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
    // Each event of the press renews this (a repeat's touch alone would
    // pull the deadline back in).
    if phase != Phase::Up
        && let Some(at) = nav.borrow().presses.long_deadline_ms()
    {
        keep_controls_until(ui, at + 50);
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
            let captured = crate::parental_view::is_open(ui) || crate::remote_editor::captures(ui);
            let decision = nav.borrow_mut().presses.poll(surface(&ui), ui.now_ms());
            let Some(decision) = decision else {
                return;
            };
            if captured {
                log::info!("iptv long press by timer dropped: a prompt took the keys");
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
    if ui.panels.with_untracked(|p| p.shown) != iptv_core::panels::Shown::Hidden {
        ui.update_panels(|p, now| p.touch(now));
    }
    if untracked(|| crate::play_bar::shows(ui)) {
        crate::play_bar::used(ui);
    }
}

/// The release of a key a prompt took or cut short: it is no longer
/// consumed, and a Back that began before the prompt does not let
/// Android's own Back act on its release either.
fn release_consumed(ui: Ui, nav: &mut Nav, key: RemoteKey) {
    if nav.consumed.contains(&key) {
        nav.consumed.retain(|k| *k != key);
        if key == RemoteKey::Back {
            swallow_release(ui);
        }
    }
}

/// Keeps a shown card or play bar until at least `at` (ms).
fn keep_controls_until(ui: Ui, at: u64) {
    if ui.panels.with_untracked(|p| p.shown) != iptv_core::panels::Shown::Hidden {
        ui.panels.update(|p| p.keep_until(at));
    }
    if untracked(|| crate::play_bar::shows(ui)) {
        crate::play_bar::keep_until(ui, at);
    }
}

fn apply(ui: Ui, key: RemoteKey, action: Action, repeat: bool) {
    // A held Menu opens a menu once; its repeats would close and reopen it.
    if repeat && action == Action::ShowMenu {
        return;
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
    if top == Some(Overlay::Vod) {
        crate::vod_view::key(ui, action);
        return;
    }
    if top == Some(Overlay::Parental) {
        crate::parental_view::key(ui, action);
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
        match action {
            Action::MoveUp => crate::fav_editor::move_focus(ui, -1),
            Action::MoveDown => crate::fav_editor::move_focus(ui, 1),
            Action::Select => crate::fav_editor::toggle(ui),
            Action::MoveLeft => crate::fav_editor::clear_group(ui),
            Action::MoveRight => crate::fav_editor::set_group(ui),
            Action::Back => crate::fav_editor::close(ui),
            _ => {}
        }
        return;
    }
    if top == Some(Overlay::Chooser) {
        match action {
            Action::MoveUp => crate::chooser::move_focus(ui, -1),
            Action::MoveDown => crate::chooser::move_focus(ui, 1),
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
        if ui.player_surface == 0
            && matches!(action, Action::Select | Action::Record)
            && let Some(origin) = ui.description.get_untracked()
        {
            crate::recordings_view::programme(ui, origin, false);
            return;
        }
        // Back (or Menu) closes programme details.
        if matches!(action, Action::Back | Action::ShowMenu) {
            crate::menu_view::close(ui);
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
            Action::Select => {
                if let Some(item) = ui.menu.with(|m| m.as_ref().and_then(|m| m.focused())) {
                    crate::menu_view::perform(ui, item);
                }
            }
            Action::Back | Action::ShowMenu => crate::menu_view::close(ui),
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
    // programme/list commands from acting on a different channel.
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
            if (key == RemoteKey::Menu && entry == iptv_core::lists::ALL)
                || (!crate::menu_view::open_group_options(ui, entry)
                    && ui.app.library.shown.get_untracked().is_none())
            {
                crate::wizard_view::open_add(ui);
            }
        }
        Action::ShowMenu if ui.focus.with(|f| f.pane) == Pane::Rail => {}
        Action::ShowMenu => crate::menu_view::open(ui),
        Action::MoveUp => mv(Move::Up),
        Action::MoveDown => mv(Move::Down),
        Action::MoveLeft => mv(Move::Left),
        Action::MoveRight => mv(Move::Right),
        Action::Select => crate::lists::ok(ui),
        Action::ShowGuide => ui.mode.set(match ui.mode.get() {
            ViewMode::Fullscreen => ViewMode::Overlay,
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
        let to = (f.channel as isize + dir * rows as isize).clamp(0, (len as isize - 1).max(0));
        f.channel = to as usize;
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

    #[test]
    fn keyboard_menu_fires_once_while_held_and_moves_repeat() {
        assert!(!named_key_options(RemoteKey::Menu).repeat, "M: once");
        for key in [
            RemoteKey::Down,
            RemoteKey::Right,
            RemoteKey::Ok,
            RemoteKey::ChannelUp,
        ] {
            assert!(named_key_options(key).repeat, "{key:?}");
        }
    }
}
