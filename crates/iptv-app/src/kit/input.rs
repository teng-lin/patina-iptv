//! Shared input helpers: remote/keyboard capability queries, key-profile
//! metadata, and native text-field focus management.

use std::cell::Cell;

use iptv_core::capabilities::{FormFactor, Host, Platform};
use iptv_nav::{Profile, RemoteKey};
use iptv_presentation::capabilities::Keys;
use nsh_core::prelude::*;

/// A TV remote's profile (Android, a browser TV) distinguishes a tap from
/// a held key.
pub(crate) const REMOTE_KEYS: bool = crate::host::REMOTE_KEYS;

/// Named keyboard bindings, including the keyboard stand-ins for a remote.
pub(crate) const KEYS: &[(&str, RemoteKey)] = &[
    ("up", RemoteKey::Up),
    ("down", RemoteKey::Down),
    ("left", RemoteKey::Left),
    ("right", RemoteKey::Right),
    ("enter", RemoteKey::Ok),
    ("pageup", RemoteKey::PageUp),
    ("pagedown", RemoteKey::PageDown),
    // Keyboard stand-ins.
    ("m", RemoteKey::Menu),
    ("g", RemoteKey::Guide),
    ("i", RemoteKey::Info),
    ("=", RemoteKey::ChannelUp),
    ("-", RemoteKey::ChannelDown),
    ("space", RemoteKey::PlayPause),
    ("p", RemoteKey::Previous),
    ("n", RemoteKey::Next),
    ("[", RemoteKey::Rewind),
    ("]", RemoteKey::FastForward),
    // A desktop keyboard's own (Ctrl + arrows: Cmd on macOS, where Ctrl
    // switches spaces).
    ("home", RemoteKey::Home),
    ("tab", RemoteKey::Tab),
    ("shift+tab", RemoteKey::ShiftTab),
    ("shift+left", RemoteKey::ShiftLeft),
    ("shift+right", RemoteKey::ShiftRight),
    ("mod+left", RemoteKey::CtrlLeft),
    ("mod+right", RemoteKey::CtrlRight),
    ("/", RemoteKey::Search),
    ("mod+f", RemoteKey::Search),
];

pub(crate) const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

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

/// What the connected keys are set up as. iPadOS reports changes through
/// GCKeyboard, so keyboard settings follow attachment without a rebuild.
pub(crate) fn keys_for(host: Host) -> Option<Keys> {
    iptv_presentation::capabilities::keys(host)
}

/// [`keys_for`] now (reactive).
pub(crate) fn keys_now() -> Option<Keys> {
    keys_for(crate::wiring::device::host())
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

/// Whether double-Back exit confirmation is offered on `host` (Android TV).
pub(crate) fn double_back_offered(host: Host) -> bool {
    host.platform == Platform::Android && host.form_factor == FormFactor::Tv
}

#[derive(Clone, Copy)]
pub(crate) struct RemoteFocus(pub(crate) ElementRef);

pub(crate) fn focus_remote_root() {
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
