//! Remote input: NSH keystrokes → `iptv_nav` keys → short/long presses →
//! context keymap → actions applied to the app state.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use iptv_nav::{Action, BackGuard, Context, Keymap, Phase, PressDetector, Profile, RemoteKey};
use nsh_core::keymap::KeyPhase;
use nsh_core::prelude::*;

use crate::guide::{Move, Pane};
use crate::{Input, Ui, ViewMode};

mod back;
mod keys;

use back::act;
pub(crate) use back::swallow_back;
use keys::{ANDROID_SEARCH, KEYS};

/// Long press needs key releases, which NSH (branch `tv-keys`) reports on
/// Android only. Elsewhere every key is a plain press and long bindings are
/// not consulted (a deferred short press would never see its release).
pub(crate) const LONG_PRESS: bool = cfg!(target_os = "android");

/// Whose keys: a TV remote's on Android, a keyboard's on the desktops
/// (`iptv_nav::Profile`: one map; the hints name their keys).
pub(crate) const PROFILE: Profile = if LONG_PRESS {
    Profile::Remote
} else {
    Profile::Keyboard
};

struct Nav {
    keymap: Keymap,
    detector: PressDetector,
    start: Instant,
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
        (ViewMode::Fullscreen, _) if ui.panels.with(|p| p.captures_input()) => Context::Controls,
        (ViewMode::Fullscreen, _) => Context::Player,
        (_, Pane::Groups | Pane::Rail) => Context::Groups,
        _ => Context::Guide,
    }
}

pub fn bind(ui: Ui) {
    let nav = Rc::new(RefCell::new(Nav {
        keymap: crate::remote_editor::keymap(),
        detector: PressDetector::default(),
        start: Instant::now(),
    }));
    let repeat = KeyOptions {
        repeat: true,
        ..Default::default()
    };
    let digits = (0..=9u8).map(|d| (d, RemoteKey::Digit(d)));
    let mut bindings: Vec<(String, RemoteKey)> =
        KEYS.iter().map(|(n, k)| ((*n).to_owned(), *k)).collect();
    bindings.extend(digits.map(|(d, k)| (d.to_string(), k)));
    if cfg!(target_os = "android") {
        bindings.push((ANDROID_SEARCH.to_owned(), RemoteKey::Search));
    }
    for (name, key) in bindings {
        let nav = nav.clone();
        on_key_event(&name, repeat, move |e: &KeyCommand| {
            let phase = match e.phase {
                KeyPhase::Press => Phase::Down,
                KeyPhase::Repeat => Phase::Repeat,
                KeyPhase::Release => Phase::Up,
            };
            key_event(ui, &nav, key, phase)
        });
    }
    // Back on a PC keyboard: Escape is the platform's (NSH), Backspace ours
    // (it does not fire while a text field edits).
    on_key("backspace", move |_| {
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
    on_key("o", move |_| ui.toggle(ViewMode::Overlay));
    on_key("f", move |_| ui.toggle(ViewMode::Fullscreen));
}

fn key_event(ui: Ui, nav: &Rc<RefCell<Nav>>, key: RemoteKey, phase: Phase) {
    if phase != Phase::Up {
        crate::browse_playback::activity(ui);
    }
    if crate::remote_editor::captures(ui) {
        if phase == Phase::Down {
            crate::remote_editor::capture(ui, key);
        }
        return;
    }
    let t = Instant::now();
    let ctx = context(&ui);
    let (action, deadline) = {
        let mut nav = nav.borrow_mut();
        if phase == Phase::Down {
            nav.keymap = crate::remote_editor::keymap();
        }
        let now = nav.start.elapsed().as_millis() as u64;
        // `remote.long_press`, taken up when a key goes down (a new press
        // ends whatever was held).
        let threshold = crate::settings::long_press_ms();
        if LONG_PRESS && phase == Phase::Down && nav.detector.threshold_ms() != threshold {
            nav.detector = PressDetector::new(threshold);
        }
        // A seek key in a replay has no long press: held, it repeats.
        let has_long = LONG_PRESS
            && nav.keymap.has_long(ctx, key)
            && (crate::remote_editor::overridden(ctx, key)
                || !crate::player_keys::seeks(&ui, ctx, key));
        let action = nav
            .detector
            .feed(key, phase, now, has_long)
            .and_then(|(k, press)| {
                let action = nav.keymap.resolve(ctx, k, press);
                let action = if crate::remote_editor::overridden(ctx, k) {
                    action
                } else {
                    crate::player_keys::remap(&ui, ctx, k, action)
                };
                action.map(|a| (k, press, a))
            });
        let deadline = (phase == Phase::Down)
            .then(|| nav.detector.long_deadline_ms())
            .flatten()
            .map(|at| at.saturating_sub(now));
        (action, deadline)
    };
    if key == RemoteKey::Back && phase == Phase::Down {
        ui.back_guard.update(BackGuard::pressed);
    }
    if let Some((key, press, action)) = action {
        act(ui, key, press, action, phase == Phase::Repeat);
    }
    // IR remotes never auto-repeat: decide long press on a timer too. If the
    // key was released (or a repeat already fired it), the poll finds nothing.
    if let Some(wait_ms) = deadline {
        let nav = nav.clone();
        let _task = set_timeout(Duration::from_millis(wait_ms), move || {
            let action = {
                let mut nav = nav.borrow_mut();
                let now = nav.start.elapsed().as_millis() as u64;
                nav.detector
                    .poll(now)
                    .and_then(|(k, press)| nav.keymap.resolve(ctx, k, press).map(|a| (k, press, a)))
            };
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

fn apply(ui: Ui, key: RemoteKey, action: Action, repeat: bool) {
    if !matches!(action, Action::ChannelUp | Action::ChannelDown) {
        ui.pending_zap.set(None);
    }
    crate::browse_playback::activity(ui);
    // The first key after touch starts from a row on screen.
    if ui.input.get() == Input::Touch {
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
    if top == Some(Overlay::Wizard) {
        crate::wizard_view::key(ui, action);
        return;
    }
    // Top-most first: a guide sources screen opens over the playlists
    // screen, which opens over settings.
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
        // The popover has no actions; Back (or Menu) closes it.
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
    // Any key but Back dismisses the hint card (Back does, as its own step).
    if action != Action::Back && ui.focus.with(|f| f.hint) {
        ui.focus.update(|f| f.hint = false);
    }
    // Full screen and its panels: `player_keys.rs`.
    if crate::player_keys::apply(ui, key, action, repeat) {
        return;
    }
    match action {
        // Menu in the groups pane adds a playlist; elsewhere it opens the menu.
        Action::ShowMenu if ui.focus.with(|f| f.pane) == Pane::Groups => {
            crate::wizard_view::open_add(ui)
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
