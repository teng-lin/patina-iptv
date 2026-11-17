//! Stable binding tokens and safe user-editable context actions.
use crate::{Action, Binding, Context, Press, RemoteKey};
impl Context {
    pub fn token(self) -> &'static str {
        match self {
            Self::Guide => "guide",
            Self::Groups => "groups",
            Self::Player => "player",
            Self::Controls => "controls",
            Self::ChannelList => "channel_list",
            Self::Search => "search",
            Self::Editor => "editor",
            Self::Menu => "menu",
            Self::Multiview => "multiview",
        }
    }
    pub fn from_token(s: &str) -> Option<Self> {
        Some(match s {
            "guide" => Self::Guide,
            "groups" => Self::Groups,
            "player" => Self::Player,
            "controls" => Self::Controls,
            "channel_list" => Self::ChannelList,
            "search" => Self::Search,
            "editor" => Self::Editor,
            "menu" => Self::Menu,
            "multiview" => Self::Multiview,
            _ => return None,
        })
    }
}
impl Action {
    pub fn token(self) -> &'static str {
        match self {
            Self::MoveUp => "move_up",
            Self::MoveDown => "move_down",
            Self::MoveLeft => "move_left",
            Self::MoveRight => "move_right",
            Self::Select => "select",
            Self::Back => "back",
            Self::ShowMenu => "show_menu",
            Self::ShowGroups => "show_groups",
            Self::ShowGuide => "show_guide",
            Self::ShowInfo => "show_info",
            Self::ShowPanels => "show_panels",
            Self::ShowControls => "show_controls",
            Self::ShowHistory => "show_history",
            Self::ChannelUp => "channel_up",
            Self::ChannelDown => "channel_down",
            Self::PreviousChannel => "previous_channel",
            Self::GoLive => "go_live",
            Self::ShowChannels => "show_channels",
            Self::ShowChannelsPreview => "show_channels_preview",
            Self::PageUp => "page_up",
            Self::PageDown => "page_down",
            Self::PastPrograms => "past_programs",
            Self::ReturnToNow => "return_to_now",
            Self::OpenSearch => "open_search",
            Self::JumpToNow => "jump_to_now",
            Self::HourBack => "hour_back",
            Self::HourForward => "hour_forward",
            Self::DayBack => "day_back",
            Self::DayForward => "day_forward",
            Self::ToggleGroups => "toggle_groups",
            Self::ReturnToPlayer => "return_to_player",
            Self::PlayPause => "play_pause",
            Self::Stop => "stop",
            Self::SeekBack => "seek_back",
            Self::SeekForward => "seek_forward",
            Self::TimeBarBack => "time_bar_back",
            Self::TimeBarForward => "time_bar_forward",
            Self::ToggleEnlarge => "toggle_enlarge",
            Self::Record => "record",
            Self::ToggleFavorite => "toggle_favorite",
            Self::Ignore => "ignore",
            Self::Digit(_) => "digit",
        }
    }
    pub fn from_token(s: &str) -> Option<Self> {
        Some(match s {
            "move_up" => Self::MoveUp,
            "move_down" => Self::MoveDown,
            "move_left" => Self::MoveLeft,
            "move_right" => Self::MoveRight,
            "select" => Self::Select,
            "back" => Self::Back,
            "show_menu" => Self::ShowMenu,
            "show_groups" => Self::ShowGroups,
            "show_guide" => Self::ShowGuide,
            "show_info" => Self::ShowInfo,
            "show_panels" => Self::ShowPanels,
            "show_controls" => Self::ShowControls,
            "show_history" => Self::ShowHistory,
            "channel_up" => Self::ChannelUp,
            "channel_down" => Self::ChannelDown,
            "previous_channel" => Self::PreviousChannel,
            "go_live" => Self::GoLive,
            "show_channels" => Self::ShowChannels,
            "show_channels_preview" => Self::ShowChannelsPreview,
            "page_up" => Self::PageUp,
            "page_down" => Self::PageDown,
            "past_programs" => Self::PastPrograms,
            "return_to_now" => Self::ReturnToNow,
            "open_search" => Self::OpenSearch,
            "jump_to_now" => Self::JumpToNow,
            "hour_back" => Self::HourBack,
            "hour_forward" => Self::HourForward,
            "day_back" => Self::DayBack,
            "day_forward" => Self::DayForward,
            "toggle_groups" => Self::ToggleGroups,
            "return_to_player" => Self::ReturnToPlayer,
            "play_pause" => Self::PlayPause,
            "stop" => Self::Stop,
            "seek_back" => Self::SeekBack,
            "seek_forward" => Self::SeekForward,
            "time_bar_back" => Self::TimeBarBack,
            "time_bar_forward" => Self::TimeBarForward,
            "toggle_enlarge" => Self::ToggleEnlarge,
            "record" => Self::Record,
            "toggle_favorite" => Self::ToggleFavorite,
            "ignore" => Self::Ignore,
            _ => return None,
        })
    }
}

/// Editing surfaces always keep default input, so a bad binding cannot trap the user.
pub fn allowed(context: Context, key: RemoteKey, press: Press, action: Action) -> bool {
    if matches!(
        context,
        Context::Menu | Context::Editor | Context::Multiview
    ) {
        return false;
    }
    if key == RemoteKey::Back && press == Press::Short {
        return false;
    }
    if matches!(action, Action::Ignore | Action::Back) {
        return true;
    }
    match context {
        Context::Guide | Context::Groups => matches!(
            action,
            Action::MoveUp
                | Action::MoveDown
                | Action::MoveLeft
                | Action::MoveRight
                | Action::Select
                | Action::OpenSearch
                | Action::ShowGuide
                | Action::ShowMenu
                | Action::ShowGroups
                | Action::PageUp
                | Action::PageDown
                | Action::JumpToNow
                | Action::HourBack
                | Action::HourForward
                | Action::DayBack
                | Action::DayForward
                | Action::ReturnToPlayer
                | Action::ToggleFavorite
                | Action::PastPrograms
                | Action::ReturnToNow
                | Action::ToggleGroups
        ),
        Context::Player | Context::Controls => matches!(
            action,
            Action::MoveLeft
                | Action::MoveRight
                | Action::Select
                | Action::OpenSearch
                | Action::ShowGuide
                | Action::ChannelUp
                | Action::ChannelDown
                | Action::PreviousChannel
                | Action::ShowInfo
                | Action::ShowPanels
                | Action::ShowControls
                | Action::ShowChannels
                | Action::ShowChannelsPreview
                | Action::ShowHistory
                | Action::PlayPause
                | Action::Stop
                | Action::GoLive
                | Action::SeekBack
                | Action::SeekForward
                | Action::ToggleFavorite
        ),
        Context::ChannelList => matches!(
            action,
            Action::MoveUp
                | Action::MoveDown
                | Action::MoveLeft
                | Action::MoveRight
                | Action::Select
                | Action::PageUp
                | Action::PageDown
        ),
        Context::Search => matches!(
            action,
            Action::MoveUp
                | Action::MoveDown
                | Action::MoveLeft
                | Action::MoveRight
                | Action::Select
        ),
        _ => false,
    }
}
pub fn checked_binding(context: &str, key: &str, long: bool, action: &str) -> Option<Binding> {
    let context = Context::from_token(context)?;
    let key = RemoteKey::from_name(key)?;
    let press = if long { Press::Long } else { Press::Short };
    let action = Action::from_token(action)?;
    allowed(context, key, press, action).then_some(Binding {
        context: Some(context),
        key,
        press,
        action,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bindings_validate_context_and_preserve_escape() {
        assert!(checked_binding("player", "red", false, "show_controls").is_some());
        assert!(checked_binding("guide", "red", false, "channel_up").is_none());
        assert!(checked_binding("player", "back", false, "ignore").is_none());
        assert!(checked_binding("editor", "ok", false, "stop").is_none());
        assert!(checked_binding("player", "unknown", false, "stop").is_none());
    }
}

pub const ACTIONS: &[Action] = &[
    Action::MoveUp,
    Action::MoveDown,
    Action::MoveLeft,
    Action::MoveRight,
    Action::Select,
    Action::Back,
    Action::ShowMenu,
    Action::ShowGroups,
    Action::ShowGuide,
    Action::ShowInfo,
    Action::ShowPanels,
    Action::ShowControls,
    Action::ShowHistory,
    Action::ChannelUp,
    Action::ChannelDown,
    Action::PreviousChannel,
    Action::GoLive,
    Action::ShowChannels,
    Action::ShowChannelsPreview,
    Action::PageUp,
    Action::PageDown,
    Action::PastPrograms,
    Action::ReturnToNow,
    Action::OpenSearch,
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
    Action::Ignore,
];

pub const CONTEXTS: &[Context] = &[
    Context::Guide,
    Context::Groups,
    Context::Player,
    Context::Controls,
    Context::ChannelList,
    Context::Search,
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Context,
    Key,
    Press,
    Action,
    Capture,
    Save,
    Clear,
    Reset,
}
pub const FIELDS: &[Field] = &[
    Field::Context,
    Field::Key,
    Field::Press,
    Field::Action,
    Field::Capture,
    Field::Save,
    Field::Clear,
    Field::Reset,
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirm {
    Replace,
    Reset,
}
/// A draft is separate from saved bindings; cancellation cannot alter the map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    pub context: Context,
    pub key: RemoteKey,
    pub press: Press,
    pub action: Action,
    pub focus: Field,
    pub capturing: bool,
    pub confirm: Option<Confirm>,
    pub accept: bool,
}
impl Default for Editor {
    fn default() -> Self {
        Self {
            context: Context::Guide,
            key: RemoteKey::Red,
            press: Press::Short,
            action: Action::ToggleFavorite,
            focus: Field::Context,
            capturing: false,
            confirm: None,
            accept: false,
        }
    }
}
impl Editor {
    /// Moves focus by `by` fields, clamping at the first and last field.
    /// While confirming, toggles acceptance instead.
    pub fn step(&mut self, by: isize) {
        if self.confirm.is_some() {
            self.accept = !self.accept;
            return;
        }
        let at = FIELDS.iter().position(|v| *v == self.focus).unwrap_or(0);
        self.focus = FIELDS[at.saturating_add_signed(by).min(FIELDS.len() - 1)];
    }
    pub fn binding(&self) -> Option<Binding> {
        allowed(self.context, self.key, self.press, self.action).then_some(Binding {
            context: Some(self.context),
            key: self.key,
            press: self.press,
            action: self.action,
        })
    }
    pub fn actions(&self) -> Vec<Action> {
        ACTIONS
            .iter()
            .copied()
            .filter(|&a| allowed(self.context, self.key, self.press, a))
            .collect()
    }
    pub fn back(&mut self) -> bool {
        if self.capturing {
            self.capturing = false;
            return true;
        }
        self.confirm.take().is_some()
    }
}

#[cfg(test)]
mod editor_tests {
    use super::*;

    #[test]
    fn focus_steps_clamp_even_for_extreme_offsets() {
        for &focus in FIELDS {
            let mut editor = Editor {
                focus,
                ..Editor::default()
            };
            editor.step(0);
            assert_eq!(editor.focus, focus);
            editor.step(isize::MAX);
            assert_eq!(editor.focus, Field::Reset);
            editor.focus = focus;
            editor.step(isize::MIN);
            assert_eq!(editor.focus, Field::Context);
            editor.step(-1);
            assert_eq!(editor.focus, Field::Context);
            editor.step(1);
            assert_eq!(editor.focus, Field::Key);
        }
    }

    #[test]
    fn draft_and_escape_are_safe_and_search_is_isolated() {
        let mut e = Editor {
            context: Context::Search,
            action: Action::Select,
            ..Editor::default()
        };
        let b = e.binding().unwrap();
        let mut map = crate::Keymap::defaults();
        map.set_override(b);
        assert_eq!(
            map.resolve(Context::Search, RemoteKey::Red, Press::Short),
            Some(Action::Select)
        );
        assert_eq!(
            map.resolve(Context::Guide, RemoteKey::Red, Press::Short),
            Some(Action::ToggleFavorite)
        );
        e.key = RemoteKey::Back;
        assert!(e.binding().is_none());
        assert!(e.actions().is_empty());
        e.capturing = true;
        assert!(e.back());
        assert!(!e.back());
        e.confirm = Some(Confirm::Replace);
        e.accept = false;
        assert!(e.back());
        assert!(!e.back());
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    #[test]
    fn persisted_tokens_round_trip_and_unknown_data_is_rejected() {
        for context in
            CONTEXTS
                .iter()
                .copied()
                .chain([Context::Menu, Context::Editor, Context::Multiview])
        {
            assert_eq!(Context::from_token(context.token()), Some(context));
            assert!(!allowed(
                context,
                RemoteKey::Back,
                Press::Short,
                Action::Ignore
            ));
        }
        for action in ACTIONS.iter().copied() {
            assert_eq!(Action::from_token(action.token()), Some(action));
        }
        assert_eq!(Action::Digit(2).token(), "digit");
        assert_eq!(Action::from_token("digit"), None);
        assert_eq!(Context::from_token("unsupported"), None);
        assert_eq!(Action::from_token("unsupported"), None);
        for (context, action) in [
            ("player", "seek_forward"),
            ("controls", "move_right"),
            ("channel_list", "select"),
            ("search", "move_left"),
            ("groups", "show_groups"),
            ("guide", "day_back"),
        ] {
            let binding = checked_binding(context, "green", true, action).unwrap();
            assert_eq!(binding.press, Press::Long);
            assert_eq!(binding.key, RemoteKey::Green);
        }
        assert!(checked_binding("menu", "green", true, "select").is_none());
        assert!(checked_binding("multiview", "green", true, "select").is_none());
        assert!(checked_binding("search", "green", true, "stop").is_none());
        assert!(checked_binding("channel_list", "green", false, "stop").is_none());
        assert!(checked_binding("player", "back", true, "stop").is_some());
        assert!(checked_binding("unknown", "green", false, "stop").is_none());
        assert!(checked_binding("player", "green", false, "unknown").is_none());
    }
    #[test]
    fn editor_boundaries_confirmation_and_capture_keep_saved_bindings_separate() {
        let mut editor = Editor::default();
        let draft = editor.binding().unwrap();
        editor.step(-100);
        assert_eq!(editor.focus, Field::Context);
        editor.step(100);
        assert_eq!(editor.focus, Field::Reset);
        editor.step(-1);
        assert_eq!(editor.focus, Field::Clear);
        editor.confirm = Some(Confirm::Reset);
        assert!(!editor.accept);
        editor.step(1);
        assert!(editor.accept);
        editor.step(-1);
        assert!(!editor.accept);
        assert!(editor.back());
        assert_eq!(editor.binding(), Some(draft));
        editor.capturing = true;
        assert!(editor.back());
        assert!(!editor.capturing);
        assert!(!editor.back());
        assert!(editor.actions().contains(&Action::ToggleFavorite));
        assert!(!editor.actions().contains(&Action::Stop));
    }
}
