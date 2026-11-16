//! Context menus (Long OK / Menu): rows of section headings and actions,
//! focus that skips headings and disabled actions, and the content the menu
//! was opened on so focus returns there when it closes.
//!
//! [`program_menu`] follows the structure observed in the reference UX's
//! programme menu:
//! global actions, then the programme's, the channel's and the group's
//! sections. Programme actions that cannot apply are **omitted** (not shown
//! disabled); which actions are enabled is the app's call (what it has
//! built). Labels are the shell's own (i18n), not stored here.

use crate::guide::{Airing, Program};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuItem {
    // Global.
    Search,
    Settings,
    // Programme.
    StartFromBeginning,
    ExternalPlayer,
    WatchInNewWindow,
    Reminder,
    Record,
    ScheduleRecording,
    CustomRecording,
    SaveToList,
    Description,
    // Channel.
    ToggleFavorite,
    BlockChannel,
    BlockGroup,
    HideChannel,
    AssignEpg,
    ChannelOptions,
    // Group.
    ManageFavorites,
    ManageBlocked,
    ManageHidden,
    ReorderChannels,
    CopyChannels,
    CreateGroup,
    GroupOptions,
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

/// What the menu was opened on: focus returns here on close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    pub channel: usize,
    pub program_start: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    pub rows: Vec<Row>,
    pub focus: usize,
    pub origin: Origin,
}

impl Menu {
    /// A menu focused on its first enabled action.
    pub fn new(rows: Vec<Row>, origin: Origin) -> Menu {
        let focus = rows.iter().position(Row::focusable).unwrap_or(0);
        Menu {
            rows,
            focus,
            origin,
        }
    }

    pub fn focused(&self) -> Option<MenuItem> {
        match self.rows.get(self.focus) {
            Some(Row::Action {
                item,
                enabled: true,
            }) => Some(*item),
            _ => None,
        }
    }

    /// Moves to the next focusable row in `step`'s direction; stays at the
    /// ends (no wrap).
    pub fn move_focus(&mut self, step: isize) {
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

    /// A tap: focus the row and return its action if it is enabled.
    pub fn tap(&mut self, index: usize) -> Option<MenuItem> {
        match self.rows.get(index) {
            Some(Row::Action {
                item,
                enabled: true,
            }) => {
                self.focus = index;
                Some(*item)
            }
            _ => None,
        }
    }
}

/// The programme actions that apply, in order (`now`: guide minutes;
/// `catchup_days`: the channel's archive window, 0 = none).
pub fn program_actions(program: Program, catchup_days: u8, now: i32) -> Vec<MenuItem> {
    use MenuItem::*;
    let in_archive = catchup_days > 0 && program.start >= now - i32::from(catchup_days) * 24 * 60;
    let mut items = match program.airing(now) {
        Airing::Live if catchup_days > 0 => vec![StartFromBeginning, ExternalPlayer, Record],
        Airing::Live => vec![ExternalPlayer, Record],
        Airing::Ended if in_archive => vec![StartFromBeginning, ExternalPlayer],
        Airing::Ended => vec![],
        Airing::Upcoming => vec![Reminder, ScheduleRecording],
    };
    if program.airing(now) != Airing::Upcoming {
        items.push(CustomRecording);
    }
    items.extend([SaveToList, Description]);
    items
}

/// The guide's full context menu for a programme. `enabled` says which
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
    rows.extend(
        [
            ToggleFavorite,
            BlockChannel,
            HideChannel,
            AssignEpg,
            ChannelOptions,
        ]
        .map(action),
    );
    rows.push(Row::Heading(Heading::Group));
    rows.extend(
        [
            ManageFavorites,
            ManageBlocked,
            BlockGroup,
            ManageHidden,
            ReorderChannels,
            CopyChannels,
            CreateGroup,
            GroupOptions,
        ]
        .map(action),
    );
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
    fn programme_actions_follow_the_observed_matrix() {
        assert_eq!(
            program_actions(LIVE, 3, 0),
            [
                StartFromBeginning,
                ExternalPlayer,
                Record,
                CustomRecording,
                SaveToList,
                Description
            ]
        );
        assert_eq!(
            program_actions(LIVE, 0, 0),
            [
                ExternalPlayer,
                Record,
                CustomRecording,
                SaveToList,
                Description
            ]
        );
        assert_eq!(
            program_actions(ENDED, 3, 0),
            [
                StartFromBeginning,
                ExternalPlayer,
                CustomRecording,
                SaveToList,
                Description
            ]
        );
        assert_eq!(
            program_actions(ENDED, 0, 0),
            [CustomRecording, SaveToList, Description]
        );
        assert_eq!(
            program_actions(FUTURE, 3, 0),
            [Reminder, ScheduleRecording, SaveToList, Description]
        );
    }

    #[test]
    fn actions_follow_the_guide_now() {
        let p = Program { start: 30, end: 60 };
        assert_eq!(program_actions(p, 3, 0)[0], Reminder);
        assert_eq!(program_actions(p, 3, 45)[0], StartFromBeginning);
        assert!(program_actions(p, 0, 45).contains(&Record));
        assert_eq!(
            program_actions(p, 0, 90),
            [CustomRecording, SaveToList, Description],
            "ended, no archive"
        );
    }

    #[test]
    fn programmes_older_than_the_archive_lose_playback() {
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
            Origin {
                channel: 0,
                program_start: -10,
            },
        );
        assert_eq!(m.focused(), Some(Search));
        m.move_focus(1);
        m.move_focus(1);
        assert_eq!(
            m.focused(),
            Some(StartFromBeginning),
            "skipped the programme heading"
        );
    }

    #[test]
    fn focus_skips_disabled_actions_and_stops_at_the_ends() {
        let only = |i| matches!(i, Description | ToggleFavorite);
        let mut m = Menu::new(
            program_menu(FUTURE, 0, 0, only),
            Origin {
                channel: 4,
                program_start: 60,
            },
        );
        assert_eq!(m.focused(), Some(Description));
        m.move_focus(-1);
        assert_eq!(m.focused(), Some(Description), "nothing enabled above");
        m.move_focus(1);
        assert_eq!(m.focused(), Some(ToggleFavorite));
        m.move_focus(1);
        assert_eq!(m.focused(), Some(ToggleFavorite), "nothing enabled below");
        assert_eq!(m.tap(0), None, "disabled");
        assert_eq!(m.tap(2), None, "a heading");
    }
}
