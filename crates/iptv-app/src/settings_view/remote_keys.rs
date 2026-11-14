//! The remote's keys and actions by name, for the bindings editor
//! (`remote_editor.rs`).

use crate::i18n::{tr, trf};

use iptv_nav::{Action, Context, RemoteKey};

pub(crate) const KEYS: [RemoteKey; 27] = [
    RemoteKey::Up,
    RemoteKey::Down,
    RemoteKey::Left,
    RemoteKey::Right,
    RemoteKey::Ok,
    RemoteKey::Back,
    RemoteKey::Menu,
    RemoteKey::Guide,
    RemoteKey::Info,
    RemoteKey::ChannelUp,
    RemoteKey::ChannelDown,
    RemoteKey::PageUp,
    RemoteKey::PageDown,
    RemoteKey::PlayPause,
    RemoteKey::Play,
    RemoteKey::Pause,
    RemoteKey::Stop,
    RemoteKey::Rewind,
    RemoteKey::FastForward,
    RemoteKey::Record,
    RemoteKey::Next,
    RemoteKey::Previous,
    RemoteKey::Red,
    RemoteKey::Green,
    RemoteKey::Yellow,
    RemoteKey::Blue,
    RemoteKey::Search,
];

/// A desktop keyboard's own keys (`iptv_nav::Profile::Keyboard`).
pub(crate) const KEYBOARD_KEYS: [RemoteKey; 7] = [
    RemoteKey::Home,
    RemoteKey::Tab,
    RemoteKey::ShiftTab,
    RemoteKey::ShiftLeft,
    RemoteKey::ShiftRight,
    RemoteKey::CtrlLeft,
    RemoteKey::CtrlRight,
];

pub(crate) fn key_name(key: RemoteKey) -> String {
    let name = match key {
        RemoteKey::Up => tr("Up"),
        RemoteKey::Down => tr("Down"),
        RemoteKey::Left => tr("Left"),
        RemoteKey::Right => tr("Right"),
        RemoteKey::Ok => tr("OK"),
        RemoteKey::Back => tr("Back"),
        RemoteKey::Menu => tr("Menu"),
        RemoteKey::Guide => tr("Guide"),
        RemoteKey::Info => tr("Info"),
        RemoteKey::ChannelUp => tr("Channel +"),
        RemoteKey::ChannelDown => tr("Channel −"),
        RemoteKey::PageUp => tr("Page up"),
        RemoteKey::PageDown => tr("Page down"),
        RemoteKey::PlayPause => tr("Play/pause"),
        RemoteKey::Play => tr("Play"),
        RemoteKey::Pause => tr("Pause"),
        RemoteKey::Stop => tr("Stop"),
        RemoteKey::Rewind => tr("Rewind"),
        RemoteKey::FastForward => tr("Fast-forward"),
        RemoteKey::Record => tr("Record"),
        RemoteKey::Next => tr("Next"),
        RemoteKey::Previous => tr("Previous"),
        RemoteKey::Red => tr("Red"),
        RemoteKey::Green => tr("Green"),
        RemoteKey::Yellow => tr("Yellow"),
        RemoteKey::Blue => tr("Blue"),
        RemoteKey::Search => tr("Search"),
        RemoteKey::Digit(_) => "0–9",
        RemoteKey::Home => tr("Home"),
        RemoteKey::Tab => tr("Tab"),
        RemoteKey::ShiftTab => tr("Shift+Tab"),
        RemoteKey::ShiftLeft => tr("Shift+Left"),
        RemoteKey::ShiftRight => tr("Shift+Right"),
        RemoteKey::CtrlLeft if cfg!(target_os = "macos") => tr("Cmd+Left"),
        RemoteKey::CtrlRight if cfg!(target_os = "macos") => tr("Cmd+Right"),
        RemoteKey::CtrlLeft => tr("Ctrl+Left"),
        RemoteKey::CtrlRight => tr("Ctrl+Right"),
    };
    // The keyboard stands in for a remote on the desktop (`input.rs`).
    let keyboard = match key {
        RemoteKey::Ok => tr("Enter"),
        RemoteKey::Back => tr("Esc"),
        RemoteKey::Menu => "M",
        RemoteKey::Guide => "G",
        RemoteKey::Info => "I",
        RemoteKey::ChannelUp => "=",
        RemoteKey::ChannelDown => "-",
        RemoteKey::PlayPause => tr("Space"),
        RemoteKey::Next => "N",
        RemoteKey::Previous => "P",
        RemoteKey::Search if cfg!(target_os = "macos") => tr("/ or Cmd+F"),
        RemoteKey::Search => tr("/ or Ctrl+F"),
        _ => "",
    };
    if cfg!(target_os = "android") || keyboard.is_empty() {
        name.to_owned()
    } else {
        trf(
            "{name} ({keyboard})",
            &[("name", name.into()), ("keyboard", keyboard.into())],
        )
    }
}

/// The screen or panel a binding applies in.
pub(crate) fn context_name(context: Context) -> &'static str {
    match context {
        Context::Guide => tr("Guide"),
        Context::Groups => tr("Groups"),
        Context::Player => tr("Player"),
        Context::Controls => tr("Player controls"),
        Context::ChannelList => tr("Channel list"),
        Context::Search => tr("Search"),
        Context::Editor => tr("Editors"),
        Context::Menu => tr("Menu"),
        Context::Multiview => tr("Multiview"),
    }
}

pub(crate) fn action_name(action: Action) -> &'static str {
    match action {
        Action::MoveUp => tr("Move up"),
        Action::MoveDown => tr("Move down"),
        Action::MoveLeft => tr("Move left"),
        Action::MoveRight => tr("Move right"),
        Action::Select => tr("Choose"),
        Action::Back => tr("Back"),
        Action::ShowMenu => tr("Menu"),
        Action::ShowGroups => tr("Groups"),
        Action::OpenSearch => tr("Search"),
        Action::ShowGuide => tr("Guide"),
        Action::ShowChannels => tr("Channel list over the video"),
        Action::ShowChannelsPreview => tr("Channel list beside the video"),
        Action::ShowInfo => tr("Programme information"),
        Action::ShowPanels => tr("Programme info and actions"),
        Action::ShowControls => tr("All player controls"),
        Action::ShowHistory => tr("Recently watched"),
        Action::ChannelUp => tr("Channel up"),
        Action::ChannelDown => tr("Channel down"),
        Action::PreviousChannel => tr("Last channel"),
        Action::GoLive => tr("Back to live"),
        Action::PageUp => tr("Page up"),
        Action::PageDown => tr("Page down"),
        Action::PastPrograms => tr("Earlier programmes"),
        Action::ReturnToNow => tr("Back to now"),
        Action::JumpToNow => tr("Now on the channel"),
        Action::HourBack => tr("An hour earlier"),
        Action::HourForward => tr("An hour later"),
        Action::DayBack => tr("A day earlier"),
        Action::DayForward => tr("A day later"),
        Action::ToggleGroups => tr("Groups, or back to the guide"),
        Action::ReturnToPlayer => tr("Back to the player"),
        Action::PlayPause => tr("Play or pause"),
        Action::Stop => tr("Stop"),
        Action::SeekBack => tr("Rewind (replays)"),
        Action::SeekForward => tr("Fast-forward (replays)"),
        Action::TimeBarBack => tr("Time bar back"),
        Action::TimeBarForward => tr("Time bar forward"),
        Action::ToggleEnlarge => tr("Enlarge or reduce"),
        Action::Record => tr("Record"),
        Action::ToggleFavorite => tr("Favourite on or off"),
        Action::Digit(_) => tr("Channel number"),
        Action::Ignore => tr("Nothing"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::in_language;

    const ACTIONS: [Action; 42] = [
        Action::MoveUp,
        Action::MoveDown,
        Action::MoveLeft,
        Action::MoveRight,
        Action::Select,
        Action::Back,
        Action::ShowMenu,
        Action::ShowGroups,
        Action::OpenSearch,
        Action::ShowGuide,
        Action::ShowChannels,
        Action::ShowChannelsPreview,
        Action::ShowInfo,
        Action::ShowPanels,
        Action::ShowControls,
        Action::ShowHistory,
        Action::ChannelUp,
        Action::ChannelDown,
        Action::PreviousChannel,
        Action::GoLive,
        Action::PageUp,
        Action::PageDown,
        Action::PastPrograms,
        Action::ReturnToNow,
        Action::JumpToNow,
        Action::HourBack,
        Action::HourForward,
        Action::DayBack,
        Action::DayForward,
        Action::ToggleGroups,
        Action::ReturnToPlayer,
        Action::PlayPause,
        Action::Stop,
        Action::SeekBack,
        Action::SeekForward,
        Action::TimeBarBack,
        Action::TimeBarForward,
        Action::ToggleEnlarge,
        Action::Record,
        Action::ToggleFavorite,
        Action::Digit(0),
        Action::Ignore,
    ];

    const CONTEXTS: [Context; 9] = [
        Context::Guide,
        Context::Groups,
        Context::Player,
        Context::Controls,
        Context::ChannelList,
        Context::Search,
        Context::Editor,
        Context::Menu,
        Context::Multiview,
    ];

    /// The editor names every action and every screen in the interface
    /// language, each by a name of its own.
    #[test]
    fn actions_and_screens_have_their_own_names_in_every_language() {
        for language in iptv_core::i18n::LANGUAGES {
            in_language(language.tag, || {
                let actions: std::collections::BTreeSet<_> =
                    ACTIONS.iter().map(|action| action_name(*action)).collect();
                assert_eq!(actions.len(), ACTIONS.len(), "{}", language.tag);
                let contexts: std::collections::BTreeSet<_> = CONTEXTS
                    .iter()
                    .map(|context| context_name(*context))
                    .collect();
                assert_eq!(contexts.len(), CONTEXTS.len(), "{}", language.tag);
            });
        }
        assert_eq!(context_name(Context::Controls), "Player controls");
        in_language("de", || {
            assert_eq!(
                action_name(Action::ShowChannels),
                "Senderliste über dem Video"
            );
            assert_eq!(action_name(Action::Ignore), "Nichts");
            assert_eq!(context_name(Context::Guide), "Programm");
            assert_eq!(context_name(Context::ChannelList), "Senderliste");
        });
        in_language("ja", || {
            assert_eq!(context_name(Context::Guide), "番組表");
            assert_eq!(context_name(Context::Editor), "編集画面");
        });
    }

    /// A remote's keys are named in the interface language. A desktop adds
    /// the keyboard key that stands in for one; Android has the remote.
    #[test]
    fn keys_are_named_in_the_interface_language() {
        let remote_only = |name: &str| name.split(" (").next().unwrap().to_owned();
        assert_eq!(key_name(RemoteKey::Green), "Green");
        assert_eq!(remote_only(&key_name(RemoteKey::Back)), "Back");
        in_language("de", || {
            assert_eq!(key_name(RemoteKey::Green), "Grün");
            assert_eq!(key_name(RemoteKey::Up), "Auf");
            assert_eq!(remote_only(&key_name(RemoteKey::Back)), "Zurück");
        });
        in_language("ru", || assert_eq!(key_name(RemoteKey::Green), "Зелёная"));
        for language in iptv_core::i18n::LANGUAGES {
            in_language(language.tag, || {
                let names: std::collections::BTreeSet<_> =
                    KEYS.iter().map(|key| key_name(*key)).collect();
                assert_eq!(names.len(), KEYS.len(), "{}", language.tag);
            });
        }
    }
}
