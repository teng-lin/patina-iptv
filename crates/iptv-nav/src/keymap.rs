//! The layered key → action map: user overrides, then the context's
//! defaults, then global defaults. Defaults are iptv-rs's own choices; the
//! context-specific ones follow the reference UX's documented instructions
//! (Long OK opens the menu in the guide, Left goes programme → groups →
//! search, Long Back returns to the player).
//!
//! One model for a TV remote and a desktop keyboard ([`Profile`]): Left
//! walks back through the programmes a channel can replay and then opens
//! the groups (`iptv_core::guide::floor`); Hold Left (a remote: it has the
//! long presses) or Tab (a keyboard) opens them from anywhere in the grid;
//! Search, `/` and Ctrl + F search. The keyboard's own keys (Home, Tab,
//! Shift/Ctrl + arrows) are bound for both: a remote never sends them.

use crate::{Press, RemoteKey};

/// The surface that owns the remote's input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Context {
    /// The guide grid (also when shown over video).
    Guide,
    /// The group list.
    Groups,
    /// Full-screen playback.
    Player,
    /// The buttons over full-screen video: the action cards under the
    /// info card, or the control strip (Left/Right/OK move and press them;
    /// Up/Down still switch channels).
    Controls,
    /// The channel list over the video (Up/Down move, OK plays, Left shows
    /// the groups, Back closes).
    ChannelList,
    /// Search results and query navigation.
    Search,
    /// An editing mode (favourites): Up/Down move, OK toggles, Left and
    /// Right act on the whole group, Back leaves.
    Editor,
    /// A context menu or popover (Up/Down move, OK selects, Back closes;
    /// Left/Right do nothing, as observed in the reference UX).
    Menu,
    /// Multiview's tiles: the arrows move between them, OK opens the
    /// tile's menu, Long OK enlarges or reduces it.
    Multiview,
}

/// Whose keys: a TV remote's (Android) or a desktop keyboard's. The map
/// is the same; they differ in the long presses only a remote reports and
/// in the keys the hints name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Hold Left shows the groups.
    Remote,
    /// Tab shows the groups; Home, Shift and Ctrl + arrows move in time.
    Keyboard,
}

/// What the app should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    /// OK on the focused item.
    Select,
    Back,
    ShowMenu,
    ShowGroups,
    ShowGuide,
    ShowInfo,
    /// The info card and its action cards over full-screen video.
    ShowPanels,
    /// The full control strip over full-screen video.
    ShowControls,
    /// The recently played channels.
    ShowHistory,
    ChannelUp,
    ChannelDown,
    /// Back to the channel played before the current one (again: return).
    PreviousChannel,
    /// From catch-up back to the live stream of the playing channel.
    GoLive,
    /// The playing group's channels as a list over full-screen video.
    ShowChannels,
    /// The same list beside the video, shrunk to a preview.
    ShowChannelsPreview,
    PageUp,
    PageDown,
    PastPrograms,
    ReturnToNow,
    /// The search screen (Search, `/`, Ctrl + F).
    OpenSearch,
    /// The focused channel's live programme in the default window (Home).
    JumpToNow,
    /// The time column an hour back/forward (Shift + Left/Right).
    HourBack,
    HourForward,
    /// The same time a day back/forward (Ctrl + Left/Right).
    DayBack,
    DayForward,
    /// The groups column, or back to the grid from it (Tab).
    ToggleGroups,
    ReturnToPlayer,
    PlayPause,
    Stop,
    SeekBack,
    SeekForward,
    /// One step back/forward along a replayed programme's time bar.
    TimeBarBack,
    TimeBarForward,
    /// Multiview: enlarge the focused tile, or reduce it.
    ToggleEnlarge,
    Record,
    ToggleFavorite,
    Digit(u8),
    /// Bound explicitly to do nothing (a user override).
    Ignore,
}

/// One mapping. `context: None` applies everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub context: Option<Context>,
    pub key: RemoteKey,
    pub press: Press,
    pub action: Action,
}

const fn b(context: Option<Context>, key: RemoteKey, press: Press, action: Action) -> Binding {
    Binding {
        context,
        key,
        press,
        action,
    }
}

/// Layered navigation bindings. [`Default`] installs the built-in map with
/// no user overrides, just like [`Keymap::defaults`].
#[derive(Clone, Debug)]
pub struct Keymap {
    defaults: Vec<Binding>,
    overrides: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::defaults()
    }
}

impl Keymap {
    /// The map of `profile`'s keys: the same for both ([`Profile`]).
    pub fn for_profile(_profile: Profile) -> Keymap {
        Keymap::defaults()
    }

    /// iptv-rs's default map: the remote's ([`Profile::Remote`]).
    /// Equivalent to [`Keymap::default`], with no user overrides.
    pub fn defaults() -> Keymap {
        use Action as A;
        use Context::{Controls, Groups, Guide, Menu, Multiview, Player};
        use Press::{Long, Short};
        use RemoteKey as K;
        let g = None;
        let mut defaults = vec![
            b(g, K::Up, Short, A::MoveUp),
            b(g, K::Down, Short, A::MoveDown),
            b(g, K::Left, Short, A::MoveLeft),
            b(g, K::Right, Short, A::MoveRight),
            b(g, K::Ok, Short, A::Select),
            b(g, K::Back, Short, A::Back),
            b(g, K::Menu, Short, A::ShowMenu),
            b(g, K::Guide, Short, A::ShowGuide),
            b(g, K::Info, Short, A::ShowInfo),
            b(g, K::ChannelUp, Short, A::ChannelUp),
            b(g, K::ChannelDown, Short, A::ChannelDown),
            b(g, K::PageUp, Short, A::PageUp),
            b(g, K::PageDown, Short, A::PageDown),
            b(g, K::PlayPause, Short, A::PlayPause),
            b(g, K::Play, Short, A::PlayPause),
            b(g, K::Pause, Short, A::PlayPause),
            b(g, K::Stop, Short, A::Stop),
            b(g, K::Rewind, Short, A::SeekBack),
            b(g, K::FastForward, Short, A::SeekForward),
            b(g, K::Record, Short, A::Record),
            b(g, K::Red, Short, A::ToggleFavorite),
            b(g, K::Search, Short, A::OpenSearch),
            // Previous/Next (⏮/⏭): last channel and back to live. Free on
            // every remote with media keys; no seeking uses them yet.
            b(g, K::Previous, Short, A::PreviousChannel),
            b(g, K::Next, Short, A::GoLive),
            // Guide: the reference UX's documented long presses; Hold Left
            // shows the groups from anywhere in the grid.
            b(Some(Guide), K::Ok, Long, A::ShowMenu),
            b(Some(Guide), K::Left, Long, A::ShowGroups),
            b(Some(Guide), K::Back, Long, A::ReturnToPlayer),
            b(Some(Groups), K::Back, Long, A::ReturnToPlayer),
            // Ch+/Ch- page the guide's channel list (observed); over video they zap.
            b(Some(Guide), K::ChannelUp, Short, A::PageUp),
            b(Some(Guide), K::ChannelDown, Short, A::PageDown),
            // Full screen, the reference's player defaults: OK shows the
            // info card and its action cards, Long OK and Menu the control
            // strip (as observed in the reference), Up/Down zap, Left the
            // channel list over the video (Long Left beside the video),
            // Right the most recent channel (Long Right the history), Long
            // Back stops playback.
            b(Some(Player), K::Ok, Short, A::ShowPanels),
            b(Some(Player), K::Ok, Long, A::ShowControls),
            b(Some(Player), K::Menu, Short, A::ShowControls),
            b(Some(Player), K::Up, Short, A::ChannelUp),
            b(Some(Player), K::Down, Short, A::ChannelDown),
            b(Some(Player), K::Left, Short, A::ShowChannels),
            b(Some(Player), K::Left, Long, A::ShowChannelsPreview),
            b(Some(Player), K::Right, Short, A::PreviousChannel),
            b(Some(Player), K::Right, Long, A::ShowHistory),
            b(Some(Player), K::Back, Long, A::Stop),
            // Remotes without media keys: hold 0 for the last channel.
            b(Some(Player), K::Digit(0), Long, A::PreviousChannel),
            // With the buttons up: Left/Right/OK move and press them (the
            // global moves); Up/Down still zap, the card following; Long OK
            // opens the strip, Menu opens or closes it.
            b(Some(Controls), K::Up, Short, A::ChannelUp),
            b(Some(Controls), K::Down, Short, A::ChannelDown),
            b(Some(Controls), K::Ok, Long, A::ShowControls),
            b(Some(Controls), K::Menu, Short, A::ShowControls),
            b(Some(Controls), K::Back, Long, A::Stop),
            // Over video there is no list to page: Page Up/Down (CH+/CH- on
            // many remotes, and a PC keyboard's) zap, as in the guide's rows.
            b(Some(Player), K::PageUp, Short, A::ChannelUp),
            b(Some(Player), K::PageDown, Short, A::ChannelDown),
            b(Some(Controls), K::PageUp, Short, A::ChannelUp),
            b(Some(Controls), K::PageDown, Short, A::ChannelDown),
            // Menus: Left and Right do nothing (observed); Back closes.
            b(Some(Menu), K::Left, Short, A::Ignore),
            b(Some(Menu), K::Right, Short, A::Ignore),
            // Multiview: Long OK enlarges the tile (the reference's).
            b(Some(Multiview), K::Ok, Long, A::ToggleEnlarge),
            // A keyboard's own keys in the guide: now, hours, days, groups.
            b(Some(Guide), K::Home, Short, A::JumpToNow),
            b(Some(Groups), K::Home, Short, A::JumpToNow),
            b(Some(Guide), K::ShiftLeft, Short, A::HourBack),
            b(Some(Guide), K::ShiftRight, Short, A::HourForward),
            b(Some(Guide), K::CtrlLeft, Short, A::DayBack),
            b(Some(Guide), K::CtrlRight, Short, A::DayForward),
            b(Some(Guide), K::Tab, Short, A::ToggleGroups),
            b(Some(Guide), K::ShiftTab, Short, A::ToggleGroups),
            b(Some(Groups), K::Tab, Short, A::ToggleGroups),
            b(Some(Groups), K::ShiftTab, Short, A::ToggleGroups),
            // Shift + Left/Right stand in for Long Left/Right over video
            // (a keyboard has no long presses; Menu, `M`, for Long OK).
            b(Some(Player), K::ShiftLeft, Short, A::ShowChannelsPreview),
            b(Some(Player), K::ShiftRight, Short, A::ShowHistory),
        ];
        defaults.extend((0..=9).map(|d| b(g, K::Digit(d), Short, A::Digit(d))));
        Keymap {
            defaults,
            overrides: Vec::new(),
        }
    }

    /// Adds or replaces a user override (same context, key and press).
    pub fn set_override(&mut self, binding: Binding) {
        self.overrides.retain(|o| {
            (o.context, o.key, o.press) != (binding.context, binding.key, binding.press)
        });
        self.overrides.push(binding);
    }

    pub fn clear_overrides(&mut self) {
        self.overrides.clear();
    }

    pub fn overrides(&self) -> &[Binding] {
        &self.overrides
    }

    /// The action for `key` pressed `press` in `context`: overrides before
    /// defaults, the context's before global ones.
    pub fn resolve(&self, context: Context, key: RemoteKey, press: Press) -> Option<Action> {
        let find = |list: &[Binding], ctx: Option<Context>| {
            list.iter()
                .find(|b| b.context == ctx && b.key == key && b.press == press)
                .map(|b| b.action)
        };
        find(&self.overrides, Some(context))
            .or_else(|| find(&self.overrides, None))
            .or_else(|| find(&self.defaults, Some(context)))
            .or_else(|| find(&self.defaults, None))
            .filter(|a| *a != Action::Ignore)
    }

    /// Whether `key` has a long action in `context` (the press detector then
    /// defers its short action until release).
    pub fn has_long(&self, context: Context, key: RemoteKey) -> bool {
        self.resolve(context, key, Press::Long).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Context::{ChannelList, Controls, Groups, Guide, Menu, Player};

    #[test]
    fn default_installs_the_same_bindings_as_the_explicit_constructors() {
        let default: Keymap = Default::default();
        assert_eq!(
            default.resolve(Guide, RemoteKey::Up, Press::Short),
            Some(Action::MoveUp)
        );
        assert_eq!(
            default.resolve(Player, RemoteKey::Ok, Press::Short),
            Some(Action::ShowPanels)
        );
        assert!(default.has_long(Player, RemoteKey::Ok));
        assert!(default.overrides().is_empty());
        for map in [
            Keymap::defaults(),
            Keymap::for_profile(Profile::Remote),
            Keymap::for_profile(Profile::Keyboard),
        ] {
            assert!(map.overrides().is_empty());
            for context in [
                Guide,
                Groups,
                Player,
                Controls,
                ChannelList,
                Context::Search,
                Context::Editor,
                Menu,
                Context::Multiview,
            ] {
                for binding in &map.defaults {
                    assert_eq!(
                        default.resolve(context, binding.key, binding.press),
                        map.resolve(context, binding.key, binding.press),
                        "{context:?}: {binding:?}"
                    );
                    assert_eq!(
                        default.has_long(context, binding.key),
                        map.has_long(context, binding.key)
                    );
                }
            }
        }
    }

    #[test]
    fn the_keyboard_and_the_remote_share_one_left() {
        let remote = Keymap::for_profile(Profile::Remote);
        let keyboard = Keymap::for_profile(Profile::Keyboard);
        let left = |k: &Keymap, ctx| k.resolve(ctx, RemoteKey::Left, Press::Short);
        // Left is a move everywhere in the guide screen: the focus model
        // walks the replays, then opens the groups, then the rail.
        for k in [&remote, &keyboard] {
            for ctx in [Guide, Groups, ChannelList, Controls, Context::Editor] {
                assert_eq!(left(k, ctx), Some(Action::MoveLeft), "{ctx:?}");
            }
            assert_eq!(left(k, Player), Some(Action::ShowChannels));
            // Hold Left: the groups from anywhere in the grid.
            assert_eq!(
                k.resolve(Guide, RemoteKey::Left, Press::Long),
                Some(Action::ShowGroups)
            );
            assert_eq!(
                k.resolve(Guide, RemoteKey::Right, Press::Short),
                Some(Action::MoveRight)
            );
            assert_eq!(
                k.resolve(Guide, RemoteKey::Back, Press::Short),
                Some(Action::Back)
            );
        }
    }

    #[test]
    fn a_late_release_opens_controls_when_the_long_press_timer_has_not_run() {
        use crate::{Phase, PressDetector};
        let keys = Keymap::for_profile(Profile::Remote);
        for context in [Player, Controls] {
            let short = if context == Player {
                Action::ShowPanels
            } else {
                Action::Select
            };
            for (repeat_at, release_at, expected) in [
                (Some(399), 404, Action::ShowControls),
                (None, 400, Action::ShowControls),
                (None, 399, short),
            ] {
                let mut detector = PressDetector::new(400);
                assert_eq!(detector.feed(RemoteKey::Ok, Phase::Down, 0, true), None);
                // Native Android can report its repeat just before the threshold,
                // with release queued ahead of the UI timer under rendering load.
                if let Some(at) = repeat_at {
                    assert_eq!(detector.feed(RemoteKey::Ok, Phase::Repeat, at, true), None);
                }
                let released = detector.feed(RemoteKey::Ok, Phase::Up, release_at, true);
                assert_eq!(
                    released.and_then(|(key, press)| keys.resolve(context, key, press)),
                    Some(expected),
                    "{context:?}: release at {release_at} respects the hold threshold"
                );
                assert_eq!(detector.poll(500), None, "late timer cannot fire twice");
                assert_eq!(detector.feed(RemoteKey::Ok, Phase::Up, 501, true), None);
            }
        }
    }

    #[test]
    fn long_ok_opens_controls_once_even_after_the_context_changes() {
        use crate::{Phase, PressDetector};
        let keys = Keymap::for_profile(Profile::Remote);
        for repeated in [false, true] {
            let mut detector = PressDetector::new(400);
            assert_eq!(
                detector.feed(
                    RemoteKey::Ok,
                    Phase::Down,
                    0,
                    keys.has_long(Player, RemoteKey::Ok)
                ),
                None
            );
            let long = if repeated {
                detector.feed(RemoteKey::Ok, Phase::Repeat, 400, true)
            } else {
                detector.poll(400)
            };
            assert_eq!(
                long.and_then(|(key, press)| keys.resolve(Player, key, press)),
                Some(Action::ShowControls)
            );
            // Opening the bar changes Player to Controls before release.
            assert_eq!(
                detector.feed(
                    RemoteKey::Ok,
                    Phase::Repeat,
                    600,
                    keys.has_long(Controls, RemoteKey::Ok)
                ),
                None
            );
            assert_eq!(
                detector.feed(
                    RemoteKey::Ok,
                    Phase::Up,
                    700,
                    keys.has_long(Controls, RemoteKey::Ok)
                ),
                None
            );
            assert_eq!(detector.poll(800), None);
        }
    }

    #[test]
    fn search_opens_from_everywhere() {
        let k = Keymap::defaults();
        for ctx in [Guide, Groups, Player, Controls, ChannelList, Menu] {
            assert_eq!(
                k.resolve(ctx, RemoteKey::Search, Press::Short),
                Some(Action::OpenSearch),
                "{ctx:?}"
            );
        }
        assert!(!k.has_long(Guide, RemoteKey::Search));
    }

    #[test]
    fn a_keyboards_own_keys_move_in_time_and_open_the_groups() {
        let k = Keymap::for_profile(Profile::Keyboard);
        let r = |ctx, key| k.resolve(ctx, key, Press::Short);
        assert_eq!(r(Guide, RemoteKey::Home), Some(Action::JumpToNow));
        assert_eq!(r(Groups, RemoteKey::Home), Some(Action::JumpToNow));
        assert_eq!(r(Guide, RemoteKey::ShiftLeft), Some(Action::HourBack));
        assert_eq!(r(Guide, RemoteKey::ShiftRight), Some(Action::HourForward));
        assert_eq!(r(Guide, RemoteKey::CtrlLeft), Some(Action::DayBack));
        assert_eq!(r(Guide, RemoteKey::CtrlRight), Some(Action::DayForward));
        for ctx in [Guide, Groups] {
            assert_eq!(r(ctx, RemoteKey::Tab), Some(Action::ToggleGroups));
            assert_eq!(r(ctx, RemoteKey::ShiftTab), Some(Action::ToggleGroups));
        }
        assert_eq!(r(Guide, RemoteKey::PageDown), Some(Action::PageDown));
        assert_eq!(r(Guide, RemoteKey::PageUp), Some(Action::PageUp));
        // Over video Shift + Left is Long Left's channel preview.
        assert_eq!(
            r(Player, RemoteKey::ShiftLeft),
            Some(Action::ShowChannelsPreview)
        );
        // Nothing elsewhere, and no long presses.
        assert_eq!(r(Menu, RemoteKey::Tab), None);
        assert!(!k.has_long(Guide, RemoteKey::Home));
        // The remote's map has them too (a remote never sends them).
        let remote = Keymap::for_profile(Profile::Remote);
        assert_eq!(
            remote.resolve(Guide, RemoteKey::CtrlLeft, Press::Short),
            Some(Action::DayBack)
        );
    }

    #[test]
    fn the_players_remote_defaults() {
        let k = Keymap::defaults();
        let r = |key, press| k.resolve(Player, key, press);
        use Press::{Long, Short};
        // OK: the info card and its action cards; held, or Menu: the strip.
        assert_eq!(r(RemoteKey::Ok, Short), Some(Action::ShowPanels));
        assert_eq!(r(RemoteKey::Ok, Long), Some(Action::ShowControls));
        assert_eq!(r(RemoteKey::Menu, Short), Some(Action::ShowControls));
        assert_eq!(r(RemoteKey::Back, Short), Some(Action::Back));
        assert_eq!(r(RemoteKey::Back, Long), Some(Action::Stop));
        assert_eq!(r(RemoteKey::Left, Short), Some(Action::ShowChannels));
        assert_eq!(r(RemoteKey::Left, Long), Some(Action::ShowChannelsPreview));
        assert_eq!(r(RemoteKey::Right, Short), Some(Action::PreviousChannel));
        assert_eq!(r(RemoteKey::Right, Long), Some(Action::ShowHistory));
        assert_eq!(r(RemoteKey::Up, Short), Some(Action::ChannelUp));
        assert_eq!(r(RemoteKey::Down, Short), Some(Action::ChannelDown));
        assert_eq!(r(RemoteKey::ChannelUp, Short), Some(Action::ChannelUp));
        assert_eq!(r(RemoteKey::ChannelDown, Short), Some(Action::ChannelDown));
        // Held Up/Down zap on repeat: no long action defers them.
        assert!(!k.has_long(Player, RemoteKey::Up));
        assert!(!k.has_long(Player, RemoteKey::Down));
        // OK, Left and Right tell a tap from a hold (they act on release).
        for key in [
            RemoteKey::Ok,
            RemoteKey::Left,
            RemoteKey::Right,
            RemoteKey::Back,
        ] {
            assert!(k.has_long(Player, key), "{key:?}");
        }
    }

    #[test]
    fn a_keyboard_stands_in_for_the_players_long_presses() {
        let k = Keymap::for_profile(Profile::Keyboard);
        let r = |key| k.resolve(Player, key, Press::Short);
        assert_eq!(r(RemoteKey::ShiftLeft), Some(Action::ShowChannelsPreview));
        assert_eq!(r(RemoteKey::ShiftRight), Some(Action::ShowHistory));
        assert_eq!(r(RemoteKey::Menu), Some(Action::ShowControls));
        // In the guide Shift + arrows still move by the hour.
        assert_eq!(
            k.resolve(Guide, RemoteKey::ShiftRight, Press::Short),
            Some(Action::HourForward)
        );
    }

    #[test]
    fn long_back_returns_to_the_player_or_stops_and_short_back_stays_back() {
        let k = Keymap::defaults();
        for ctx in [Guide, Groups] {
            assert_eq!(
                k.resolve(ctx, RemoteKey::Back, Press::Long),
                Some(Action::ReturnToPlayer)
            );
        }
        for ctx in [Player, Controls] {
            assert_eq!(
                k.resolve(ctx, RemoteKey::Back, Press::Long),
                Some(Action::Stop)
            );
        }
        // Short Back is Back everywhere (the platform's Back handler acts on
        // it); where nothing is bound long it comes at once.
        for ctx in [
            Guide,
            Groups,
            Player,
            Controls,
            ChannelList,
            Context::Editor,
            Menu,
        ] {
            assert_eq!(
                k.resolve(ctx, RemoteKey::Back, Press::Short),
                Some(Action::Back)
            );
        }
        for ctx in [ChannelList, Context::Editor, Menu] {
            assert!(!k.has_long(ctx, RemoteKey::Back));
        }
    }

    #[test]
    fn the_channel_list_moves_and_selects_with_the_global_moves() {
        let k = Keymap::defaults();
        let r = |key| k.resolve(ChannelList, key, Press::Short);
        assert_eq!(r(RemoteKey::Up), Some(Action::MoveUp));
        assert_eq!(r(RemoteKey::Left), Some(Action::MoveLeft));
        assert_eq!(r(RemoteKey::Ok), Some(Action::Select));
        assert_eq!(r(RemoteKey::Back), Some(Action::Back));
        assert_eq!(r(RemoteKey::PageDown), Some(Action::PageDown));
        assert!(!k.has_long(ChannelList, RemoteKey::Left));
    }

    #[test]
    fn context_defaults_win_over_global_ones() {
        let k = Keymap::defaults();
        assert_eq!(
            k.resolve(Guide, RemoteKey::Up, Press::Short),
            Some(Action::MoveUp)
        );
        assert_eq!(
            k.resolve(Player, RemoteKey::Up, Press::Short),
            Some(Action::ChannelUp)
        );
        assert_eq!(
            k.resolve(Player, RemoteKey::Ok, Press::Short),
            Some(Action::ShowPanels)
        );
        assert_eq!(
            k.resolve(Groups, RemoteKey::Ok, Press::Short),
            Some(Action::Select)
        );
    }

    #[test]
    fn the_buttons_move_with_left_right_and_up_down_still_zap() {
        let k = Keymap::defaults();
        let r = |key, press| k.resolve(Controls, key, press);
        assert_eq!(r(RemoteKey::Left, Press::Short), Some(Action::MoveLeft));
        assert_eq!(r(RemoteKey::Right, Press::Short), Some(Action::MoveRight));
        assert_eq!(r(RemoteKey::Ok, Press::Short), Some(Action::Select));
        assert_eq!(r(RemoteKey::Up, Press::Short), Some(Action::ChannelUp));
        assert_eq!(r(RemoteKey::Down, Press::Short), Some(Action::ChannelDown));
        // Long OK from the card, and Menu, open the strip.
        assert_eq!(r(RemoteKey::Ok, Press::Long), Some(Action::ShowControls));
        assert_eq!(r(RemoteKey::Menu, Press::Short), Some(Action::ShowControls));
        assert!(
            !k.has_long(Controls, RemoteKey::Left),
            "the focus moves at once, and on repeat"
        );
    }

    #[test]
    fn page_keys_page_the_guide_and_zap_over_video() {
        let k = Keymap::defaults();
        assert_eq!(
            k.resolve(Context::Guide, RemoteKey::PageDown, Press::Short),
            Some(Action::PageDown)
        );
        assert_eq!(
            k.resolve(Context::Guide, RemoteKey::ChannelUp, Press::Short),
            Some(Action::PageUp),
            "Ch+ pages the guide"
        );
        assert_eq!(
            k.resolve(Context::Guide, RemoteKey::ChannelDown, Press::Short),
            Some(Action::PageDown)
        );
        assert_eq!(
            k.resolve(Context::Player, RemoteKey::ChannelUp, Press::Short),
            Some(Action::ChannelUp),
            "and zaps over video"
        );
        for ctx in [Context::Player, Controls] {
            assert_eq!(
                k.resolve(ctx, RemoteKey::PageUp, Press::Short),
                Some(Action::ChannelUp)
            );
            assert_eq!(
                k.resolve(ctx, RemoteKey::PageDown, Press::Short),
                Some(Action::ChannelDown)
            );
        }
    }

    #[test]
    fn menus_move_vertically_and_ignore_left_right() {
        let k = Keymap::defaults();
        assert_eq!(
            k.resolve(Menu, RemoteKey::Down, Press::Short),
            Some(Action::MoveDown)
        );
        assert_eq!(k.resolve(Menu, RemoteKey::Left, Press::Short), None);
        assert_eq!(k.resolve(Menu, RemoteKey::Right, Press::Short), None);
        assert!(!k.has_long(Menu, RemoteKey::Ok));
    }

    #[test]
    fn the_editor_gets_plain_moves_and_select() {
        let k = Keymap::defaults();
        assert_eq!(
            k.resolve(Context::Editor, RemoteKey::Left, Press::Short),
            Some(Action::MoveLeft)
        );
        assert_eq!(
            k.resolve(Context::Editor, RemoteKey::Ok, Press::Short),
            Some(Action::Select)
        );
        assert!(!k.has_long(Context::Editor, RemoteKey::Ok));
    }

    #[test]
    fn multiview_enlarges_on_long_ok() {
        let k = Keymap::defaults();
        let mv = Context::Multiview;
        assert_eq!(
            k.resolve(mv, RemoteKey::Ok, Press::Short),
            Some(Action::Select)
        );
        assert_eq!(
            k.resolve(mv, RemoteKey::Ok, Press::Long),
            Some(Action::ToggleEnlarge)
        );
        assert_eq!(
            k.resolve(mv, RemoteKey::Up, Press::Short),
            Some(Action::MoveUp)
        );
        assert!(!k.has_long(mv, RemoteKey::Back));
    }

    #[test]
    fn long_presses_exist_only_where_bound() {
        let k = Keymap::defaults();
        assert!(k.has_long(Guide, RemoteKey::Ok));
        assert!(k.has_long(Guide, RemoteKey::Left));
        assert!(
            !k.has_long(Groups, RemoteKey::Ok),
            "OK is immediate in the group list"
        );
        assert!(
            !k.has_long(Guide, RemoteKey::Down),
            "directions scroll on repeat"
        );
        assert_eq!(
            k.resolve(Guide, RemoteKey::Back, Press::Long),
            Some(Action::ReturnToPlayer)
        );
    }

    #[test]
    fn overrides_win_and_ignore_unbinds() {
        let mut k = Keymap::defaults();
        k.set_override(Binding {
            context: None,
            key: RemoteKey::Red,
            press: Press::Short,
            action: Action::Record,
        });
        assert_eq!(
            k.resolve(Guide, RemoteKey::Red, Press::Short),
            Some(Action::Record)
        );
        k.set_override(Binding {
            context: Some(Guide),
            key: RemoteKey::Ok,
            press: Press::Long,
            action: Action::Ignore,
        });
        assert!(!k.has_long(Guide, RemoteKey::Ok));
        assert!(
            k.has_long(Player, RemoteKey::Ok),
            "the override is scoped to the guide"
        );
        k.set_override(Binding {
            context: None,
            key: RemoteKey::Red,
            press: Press::Short,
            action: Action::ShowInfo,
        });
        assert_eq!(k.overrides().len(), 2, "same slot replaced, not duplicated");
        k.clear_overrides();
        assert_eq!(
            k.resolve(Guide, RemoteKey::Red, Press::Short),
            Some(Action::ToggleFavorite)
        );
    }

    #[test]
    fn previous_channel_and_live_have_their_own_keys() {
        let k = Keymap::defaults();
        for ctx in [Guide, Groups, Player, Controls] {
            assert_eq!(
                k.resolve(ctx, RemoteKey::Previous, Press::Short),
                Some(Action::PreviousChannel)
            );
            assert_eq!(
                k.resolve(ctx, RemoteKey::Next, Press::Short),
                Some(Action::GoLive)
            );
        }
        assert_eq!(
            k.resolve(Player, RemoteKey::Digit(0), Press::Long),
            Some(Action::PreviousChannel)
        );
        assert_eq!(
            k.resolve(Player, RemoteKey::Digit(0), Press::Short),
            Some(Action::Digit(0)),
            "a short 0 is still a digit"
        );
        assert!(!k.has_long(Guide, RemoteKey::Digit(0)));
    }

    #[test]
    fn digits_and_unknown_combinations() {
        let k = Keymap::defaults();
        assert_eq!(
            k.resolve(Guide, RemoteKey::Digit(7), Press::Short),
            Some(Action::Digit(7))
        );
        assert_eq!(k.resolve(Groups, RemoteKey::Digit(7), Press::Long), None);
    }
}
