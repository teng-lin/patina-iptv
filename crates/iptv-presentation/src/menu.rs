//! Context menus (Long OK / Menu): rows of section headings and actions,
//! focus that skips headings and disabled actions, and the content the menu
//! was opened on so focus returns there when it closes.
//!
//! [`program_menu`] follows the structure observed in the reference UX's
//! program menu:
//! global actions, then the program's, the channel's and the group's
//! sections. Program actions that cannot apply are **omitted** (not shown
//! disabled); which actions are enabled is the app's call (what it has
//! built). Labels are the shell's own (i18n), not stored here.

use crate::lists::ListId;
use iptv_core::guide::{Airing, Program};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuItem {
    // Global.
    Search,
    Settings,
    // Program.
    StartFromBeginning,
    WatchInNewWindow,
    Record,
    ScheduleRecording,
    RecordNow,
    CancelRecording,
    StopRecording,
    CustomRecording,
    Description,
    // Channel.
    ToggleFavorite,
    BlockChannel,
    BlockGroup,
    HideChannel,
    AssignEpg,
    ChannelOptions,
    ChannelActions,
    ProtectChannel,
    // Group.
    ManageFavorites,
    ManageBlocked,
    ManageHidden,
    ReorderChannels,
    SortChannels,
    CopyChannels,
    CreateGroup,
    GroupOptions,
    SortGroups,
    ReorderGroups,
    ManageGroups,
    RenameGroup,
    ResetGroupName,
    ShowFavoritesOnly,
    GroupExternalPlayer,
    /// Turns the group's external player off, offered when a handoff found
    /// no player to take it.
    StopExternalPlayer,
    GroupEpgOffset,
    DecreaseEpgOffset,
    IncreaseEpgOffset,
    ResetEpgOffset,
    SaveEpgOffset,
    HideGroup,
    ProtectGroup,
    ManageParentalBlocks,
}

/// Section headings (never focused).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heading {
    Program,
    Channel,
    Group,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Heading(Heading),
    Action { item: MenuItem, enabled: bool },
}

impl Row {
    fn focusable(&self) -> bool {
        matches!(self, Row::Action { enabled: true, .. })
    }
}

/// A resolved program position in the current catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    pub channel: usize,
    pub program_start: i32,
}

/// The content addressed by the menu, independent of catalog order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Program {
        channel: String,
        start: i32,
    },
    Channel {
        channel: String,
        program: Program,
        offset: i32,
    },
    Group,
}

impl Target {
    pub fn channel_key(&self) -> Option<&str> {
        match self {
            Self::Program { channel, .. } | Self::Channel { channel, .. } => Some(channel),
            Self::Group => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    pub rows: Vec<Row>,
    pub focus: usize,
    pub target: Target,
    pub list: ListId,
    pub group_offset: Option<i32>,
}

impl Menu {
    /// A menu focused on its first enabled action.
    pub fn new(rows: Vec<Row>, target: Target, list: ListId) -> Menu {
        let focus = rows.iter().position(Row::focusable).unwrap_or(0);
        Menu {
            rows,
            focus,
            target,
            list,
            group_offset: None,
        }
    }

    pub fn focused(&self) -> Option<MenuItem> {
        self.action_at(self.focus)
    }

    /// Moves to the next focusable row in `step`'s direction; stays at the
    /// ends (no wrap).
    pub fn move_focus(&mut self, step: isize) {
        if step == 0 {
            return;
        }
        let mut at = self.focus as isize;
        loop {
            at += step.signum();
            if at < 0 {
                return;
            }
            match self.rows.get(at as usize) {
                None => return,
                Some(r) if r.focusable() => {
                    self.focus = at as usize;
                    return;
                }
                Some(_) => {}
            }
        }
    }

    /// The action under a pointer or touch, if it is enabled.
    pub fn action_at(&self, index: usize) -> Option<MenuItem> {
        match self.rows.get(index) {
            Some(Row::Action {
                item,
                enabled: true,
            }) => Some(*item),
            _ => None,
        }
    }
}

/// The program actions that apply, in order (`now`: guide minutes;
/// `catchup_days`: the channel's archive window, 0 = none).
pub fn program_actions(program: Program, catchup_days: u8, now: i32) -> Vec<MenuItem> {
    use MenuItem::*;
    let in_archive = program.can_replay(now, catchup_days);
    let mut items = match program.airing(now) {
        Airing::Live if in_archive => vec![StartFromBeginning, Record],
        Airing::Live => vec![Record],
        Airing::Ended if in_archive => vec![StartFromBeginning],
        Airing::Ended => vec![],
        // Recording a future program is retired (#273): no Schedule recording
        // or Schedule a recording… here. `Record` starts recording now.
        Airing::Upcoming => vec![],
    };
    items.push(Description);
    items
}

const CHANNEL_ACTIONS: [MenuItem; 6] = [
    MenuItem::ToggleFavorite,
    MenuItem::BlockChannel,
    MenuItem::HideChannel,
    MenuItem::AssignEpg,
    MenuItem::ChannelOptions,
    MenuItem::ProtectChannel,
];

/// Actions shared by the guide's program menu and a group's own menu.
const GROUP_ACTIONS: [MenuItem; 12] = [
    MenuItem::ManageFavorites,
    MenuItem::ManageBlocked,
    MenuItem::ManageHidden,
    MenuItem::ReorderChannels,
    MenuItem::SortChannels,
    MenuItem::CopyChannels,
    MenuItem::CreateGroup,
    MenuItem::BlockGroup,
    MenuItem::HideGroup,
    MenuItem::GroupOptions,
    MenuItem::ProtectGroup,
    MenuItem::ManageParentalBlocks,
];

/// The channel's fixed actions, with optional recording and desktop playback.
pub fn channel_menu(
    recording: Option<MenuItem>,
    new_window: Option<bool>,
    can_reorder: bool,
) -> Vec<Row> {
    use MenuItem::*;
    let action = |item| Row::Action {
        item,
        enabled: true,
    };
    let mut rows = vec![action(ToggleFavorite)];
    rows.extend(recording.map(action));
    if let Some(enabled) = new_window {
        rows.push(Row::Action {
            item: WatchInNewWindow,
            enabled,
        });
    }
    if can_reorder {
        rows.push(action(ReorderChannels));
    }
    rows.extend(
        [
            AssignEpg,
            ChannelOptions,
            BlockChannel,
            HideChannel,
            ProtectChannel,
        ]
        .map(action),
    );
    rows
}

/// The focused list's settings and group management shortcuts.
pub fn group_options(available: impl Fn(MenuItem) -> bool) -> Vec<Row> {
    use MenuItem::*;
    standalone(
        [
            RenameGroup,
            ResetGroupName,
            SortChannels,
            ReorderChannels,
            ShowFavoritesOnly,
            GroupExternalPlayer,
            GroupEpgOffset,
            BlockGroup,
            HideGroup,
            ReorderGroups,
            ManageGroups,
        ],
        available,
    )
}

/// Edits a guide time correction in fifteen-minute increments.
pub fn group_offset_menu(list: ListId, minutes: i32) -> Menu {
    use MenuItem::*;
    let rows = [
        DecreaseEpgOffset,
        IncreaseEpgOffset,
        ResetEpgOffset,
        SaveEpgOffset,
    ]
    .into_iter()
    .map(|item| Row::Action {
        item,
        enabled: true,
    })
    .collect();
    let mut menu = Menu::new(rows, Target::Group, list);
    menu.group_offset = Some(minutes);
    menu
}

impl Menu {
    pub fn step_group_offset(&mut self, delta: i32) {
        use iptv_core::organization::{GROUP_EPG_OFFSET_MAX, GROUP_EPG_OFFSET_MIN};
        if let Some(offset) = &mut self.group_offset {
            *offset = (*offset + delta * 15).clamp(GROUP_EPG_OFFSET_MIN, GROUP_EPG_OFFSET_MAX);
        }
    }
}

fn standalone(
    actions: impl IntoIterator<Item = MenuItem>,
    available: impl Fn(MenuItem) -> bool,
) -> Vec<Row> {
    actions
        .into_iter()
        .filter(|&item| available(item))
        .map(|item| Row::Action {
            item,
            enabled: true,
        })
        .collect()
}

/// The guide's full context menu for a program. `enabled` says which
/// actions the app can perform.
pub fn program_menu(
    program: Program,
    catchup_days: u8,
    now: i32,
    enabled: impl Fn(MenuItem) -> bool,
) -> Vec<Row> {
    use MenuItem::*;
    let action = |item| Row::Action {
        item,
        enabled: enabled(item),
    };
    let mut rows = vec![
        action(Search),
        action(Settings),
        Row::Heading(Heading::Program),
    ];
    rows.extend(
        program_actions(program, catchup_days, now)
            .into_iter()
            .map(action),
    );
    rows.push(Row::Heading(Heading::Channel));
    rows.extend(CHANNEL_ACTIONS.map(action));
    rows.push(action(ChannelActions));
    rows.push(Row::Heading(Heading::Group));
    rows.extend(GROUP_ACTIONS.map(action));
    rows
}

#[cfg(test)]
mod tests {
    use super::MenuItem::*;
    use super::*;

    const DAY: i32 = 24 * 60;
    const LIVE: Program = Program {
        start: -10,
        end: 50,
    };
    const ENDED: Program = Program {
        start: -120,
        end: -60,
    };
    const FUTURE: Program = Program { start: 60, end: 90 };

    #[test]
    fn standalone_menus_focus_the_first_applicable_action() {
        let rows = channel_menu(Some(RecordNow), Some(false), true);
        let mut menu = Menu::new(
            rows,
            Target::Channel {
                channel: "channel-a".into(),
                program: LIVE,
                offset: 0,
            },
            ListId::All,
        );
        assert_eq!(menu.focused(), Some(ToggleFavorite));
        menu.move_focus(1);
        assert_eq!(menu.focused(), Some(RecordNow));
        menu.move_focus(1);
        assert_eq!(
            menu.focused(),
            Some(ReorderChannels),
            "skips the unavailable window"
        );
        assert!(!menu.rows.iter().any(|row| matches!(
            row,
            Row::Action {
                item: Description | ManageFavorites,
                ..
            }
        )));

        let rows = group_options(|item| item != RenameGroup && item != ResetGroupName);
        let menu = Menu::new(rows, Target::Group, ListId::All);
        assert_eq!(menu.focused(), Some(SortChannels));
    }

    #[test]
    fn program_actions_follow_the_observed_matrix() {
        assert_eq!(
            program_actions(LIVE, 3, 0),
            [StartFromBeginning, Record, Description]
        );
        assert_eq!(program_actions(LIVE, 0, 0), [Record, Description]);
        assert_eq!(
            program_actions(ENDED, 3, 0),
            [StartFromBeginning, Description]
        );
        assert_eq!(program_actions(ENDED, 0, 0), [Description]);
        assert_eq!(program_actions(FUTURE, 3, 0), [Description]);
    }

    #[test]
    fn no_program_action_schedules_a_future_recording() {
        for program in [LIVE, ENDED, FUTURE] {
            for days in [0, 3] {
                let items = program_actions(program, days, 0);
                assert!(!items.contains(&ScheduleRecording), "{program:?}");
                assert!(!items.contains(&CustomRecording), "{program:?}");
            }
        }
    }

    #[test]
    fn actions_follow_the_guide_now() {
        let p = Program { start: 30, end: 60 };
        assert_eq!(program_actions(p, 3, 0), [Description], "upcoming");
        assert_eq!(program_actions(p, 3, 45)[0], StartFromBeginning);
        assert!(program_actions(p, 0, 45).contains(&Record));
        assert_eq!(
            program_actions(p, 0, 90),
            [Description],
            "ended, no archive"
        );
    }

    #[test]
    fn programs_older_than_the_archive_lose_playback() {
        let old = Program {
            start: -4 * DAY,
            end: -4 * DAY + 30,
        };
        assert!(!program_actions(old, 3, 0).contains(&StartFromBeginning));
    }

    #[test]
    fn sections_come_in_order_and_headings_never_take_focus() {
        let rows = program_menu(LIVE, 3, 0, |_| true);
        let headings: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, Row::Heading(_)))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(headings.len(), 3);
        assert_eq!(
            rows[0],
            Row::Action {
                item: Search,
                enabled: true
            }
        );
        assert_eq!(rows[2], Row::Heading(Heading::Program));
        let mut m = Menu::new(
            rows,
            Target::Program {
                channel: "channel-a".into(),
                start: -10,
            },
            ListId::All,
        );
        assert_eq!(m.focused(), Some(Search));
        m.move_focus(1);
        m.move_focus(1);
        assert_eq!(
            m.focused(),
            Some(StartFromBeginning),
            "skipped the program heading"
        );
    }

    #[test]
    fn focus_skips_disabled_actions_and_stops_at_the_ends() {
        let only = |i| matches!(i, Description | ToggleFavorite);
        let mut m = Menu::new(
            program_menu(FUTURE, 0, 0, only),
            Target::Program {
                channel: "channel-a".into(),
                start: 60,
            },
            ListId::All,
        );
        assert_eq!(m.focused(), Some(Description));
        m.move_focus(-1);
        assert_eq!(m.focused(), Some(Description), "nothing enabled above");
        m.move_focus(1);
        assert_eq!(m.focused(), Some(ToggleFavorite));
        m.move_focus(1);
        assert_eq!(m.focused(), Some(ToggleFavorite), "nothing enabled below");
        assert_eq!(m.action_at(0), None, "disabled");
        assert_eq!(m.action_at(2), None, "a heading");
    }
    #[test]
    fn zero_movement_is_a_noop_for_nonfocusable_and_ordinary_menus() {
        for rows in [
            vec![Row::Heading(Heading::Program)],
            vec![Row::Action {
                item: MenuItem::Search,
                enabled: false,
            }],
            vec![Row::Action {
                item: MenuItem::Search,
                enabled: true,
            }],
        ] {
            let mut menu = Menu::new(rows, Target::Group, ListId::All);
            let before = menu.clone();
            menu.move_focus(0);
            assert_eq!(menu, before);
        }
    }
}
