//! The guide screen below the preview: the navigation rail, the groups
//! column and the guide grid (a time ruler, one row per channel of the list
//! shown, programs as cells sized by duration). The focus rules are the
//! core's ([`iptv_presentation::guide::Focus`] over a [`ListView`] of the list, its
//! columns in [`iptv_presentation::browse`]), so the screen moves as Patina's guide
//! does on the other platforms. The bridge shows no hint card after a list
//! is chosen, so the focus never keeps one.
//!
//! Only the screen is emitted: [`GRID_ROWS`] rows from the first one
//! shown, and per row the programs that overlap the time window (cut to
//! it). Each row carries a signature of what it shows, so the page
//! rebuilds a row's cells only when it changed and otherwise moves the
//! focus mark.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use iptv_core::guide::{Airing, Listing, Program, Schedule};
use iptv_nav::Action;
use iptv_presentation::browse::{self, Columns, RailItem};
use iptv_presentation::guide::{Focus, Move, Pane, page_row};
use iptv_presentation::lists::{ChannelList as Rows, ListView};
use serde_json::{Value, json};

use crate::presentation::{Presentation, timing_label};

/// Channel rows the grid shows at once.
pub(crate) const GRID_ROWS: usize = 8;
/// Minutes across the grid.
pub(crate) const GRID_MINUTES: i32 = iptv_core::guide::VISIBLE;
/// The rail's items: this bridge has Search and the guide itself.
pub(crate) const RAIL: [RailItem; 2] = [RailItem::Search, RailItem::Tv];

/// What a key on the guide screen asks of the app.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GridOutcome {
    Stay,
    /// The groups column selected another entry ([`Grid::entry`]): show its
    /// list.
    Preview,
    /// Select a program on the focused catalog channel.
    Watch {
        channel: usize,
        program: Program,
    },
    /// OK on the preview: the playing video full screen again.
    Full,
    /// The rail's Search.
    Search,
    /// OK or Right on an empty list.
    Empty,
    /// Back on the rail: the page asks before the app leaves (Samsung's
    /// exit confirmation on Return at the app's first screen).
    Leave,
}

pub(crate) struct Grid {
    view: ListView,
    focus: Focus,
    /// The first row on screen: it moves only when the focus leaves the
    /// screen (or a page turns), so most keys leave the rows in place.
    first: usize,
}

fn airing_name(a: Airing) -> &'static str {
    match a {
        Airing::Ended => "ended",
        Airing::Live => "live",
        Airing::Upcoming => "upcoming",
    }
}

fn pane_name(p: Pane) -> &'static str {
    match p {
        Pane::Rail => "rail",
        Pane::Groups => "groups",
        Pane::Guide => "guide",
        Pane::Preview => "preview",
    }
}

/// The legacy bridge leaves placeholders and empty titles blank.
pub(crate) fn stored_title(listing: Option<Listing>) -> Option<String> {
    match listing? {
        Listing::Title(title) if !title.is_empty() => Some(title),
        Listing::Title(_) | Listing::Gap | Listing::Loading => None,
    }
}

fn columns(entries: usize) -> Columns<'static> {
    Columns {
        entries: iptv_presentation::lists::VisibleEntries::all(entries),
        rail: &RAIL,
        back_to_now: true,
    }
}

impl Grid {
    /// The guide of `rows` on `catalog` at guide minute `now`, on its first
    /// row's live program.
    pub(crate) fn open(catalog: Rc<dyn Schedule>, rows: Rc<Rows>, now: i32) -> Grid {
        let mut focus = Focus::at(now);
        focus.group = rows.entry();
        let view = ListView::new(catalog, rows);
        focus.set_visible(GRID_MINUTES, &view);
        let mut grid = Grid {
            view,
            focus,
            first: 0,
        };
        grid.focus.restore(None, &grid.view);
        grid
    }

    /// The same screen on a new schedule of the same catalog (the guide
    /// arrived): the focused channel is found again, the time stays where
    /// the new guide still has listings. A
    /// schedule made in a later hour counts minutes from that hour: the
    /// focus's minutes are converted through Unix time first.
    pub(crate) fn rebase(&mut self, catalog: Rc<dyn Schedule>, rows: Rc<Rows>, now: i32) {
        let channel = self.view.channel_key(self.focus.channel);
        let offset = self.focus.channel.saturating_sub(self.first);
        let shift = (self.view.epoch_unix() - catalog.epoch_unix()).div_euclid(60);
        let shift = i32::try_from(shift).unwrap_or(0);
        let f = &mut self.focus;
        f.cursor = f.cursor.saturating_add(shift);
        f.window = f.window.saturating_add(shift);
        f.now = f.now.saturating_add(shift);
        self.view = ListView::new(catalog, rows);
        let row = channel
            .as_deref()
            .and_then(|key| self.view.find_channel(key));
        self.focus.restore(Some(row.unwrap_or(0)), &self.view);
        self.first = self.focus.channel.saturating_sub(offset);
        self.clamp_first();
        self.tick(now);
        // The new guide may not reach as far back (or on) as the old one,
        // and `tick` changes nothing while now is the same: the time is
        // brought inside the row's listings and the guide's bounds as a tap
        // there is, the window then to the focused program.
        let (pane, row, cursor) = (self.focus.pane, self.focus.channel, self.focus.cursor);
        self.focus.tap(row, cursor, &self.view);
        self.focus.pane = pane;
        self.focus.set_visible(self.focus.visible, &self.view);
    }

    /// Another list (a group previewed, Favorites reordered), with the
    /// focus on `row` (else where it is, kept inside the list).
    pub(crate) fn set_list(&mut self, rows: Rc<Rows>, row: Option<usize>) {
        self.view = ListView::new(self.view.catalog().clone(), rows);
        self.focus.restore(row, &self.view);
        self.follow_focus();
    }

    /// The groups column entry whose list the guide shows (or previews).
    pub(crate) fn entry(&self) -> usize {
        self.focus.group
    }

    pub(crate) fn pane(&self) -> Pane {
        self.focus.pane
    }

    /// The focused rail item.
    pub(crate) fn rail(&self) -> RailItem {
        self.focus.rail
    }

    /// The focused row.
    pub(crate) fn row(&self) -> usize {
        self.focus.channel
    }

    /// The catalog channel of the focused row.
    pub(crate) fn focused_channel(&self) -> Option<usize> {
        self.view.channel(self.focus.channel)
    }

    /// Now moved on (a live focus follows it).
    pub(crate) fn tick(&mut self, now: i32) {
        self.focus.tick(now, &self.view);
    }

    /// The grid on `row`'s live program (the playing channel after full
    /// screen), centered on screen when it was off it.
    pub(crate) fn show_channel(&mut self, row: usize) {
        self.focus.show_channel(row, &self.view);
        // An empty list keeps the focus in the groups column.
        self.focus.restore(None, &self.view);
        if !(self.first..self.first + GRID_ROWS).contains(&self.focus.channel) {
            self.first = self.focus.channel.saturating_sub(GRID_ROWS / 2);
        }
        self.clamp_first();
    }

    /// The grid has the focus (the Guide key on this screen), when the list
    /// has rows.
    pub(crate) fn focus_guide(&mut self) {
        if self.view.channel_count() > 0 {
            self.focus.pane = Pane::Guide;
        }
    }

    /// Selects groups column `entry` from elsewhere (Blue: Favorites), at its
    /// first row, at now. Whether the selection changed.
    pub(crate) fn select_entry(&mut self, entry: usize, entries: usize) -> bool {
        self.focus.preview_list(entry, &columns(entries))
    }

    /// Shows the groups column with the focus on the selected entry (an
    /// empty Favorites; a Left held in the grid, as on Android TV).
    pub(crate) fn open_groups(&mut self) {
        self.focus.open_groups();
    }

    /// The video stopped: a focused preview hands the focus back.
    pub(crate) fn preview_gone(&mut self) {
        self.focus.retain_preview(false);
        self.focus.restore(None, &self.view);
    }

    /// A key on the guide screen. `entries` is the groups column's length;
    /// `preview` whether video is playing and Up from Search may focus it.
    pub(crate) fn key(
        &mut self,
        action: Action,
        now: i32,
        entries: usize,
        preview: bool,
    ) -> GridOutcome {
        self.tick(now);
        let cols = columns(entries);
        let pane = self.focus.pane;
        let empty = self.view.channel_count() == 0;
        let outcome = match action {
            Action::MoveUp if self.focus.focus_preview(preview) => browse::Outcome::Moved,
            Action::MoveUp => self.focus.navigate(Move::Up, &cols, &self.view),
            Action::MoveDown => self.focus.navigate(Move::Down, &cols, &self.view),
            Action::MoveLeft => self.focus.navigate(Move::Left, &cols, &self.view),
            Action::MoveRight => self.focus.navigate(Move::Right, &cols, &self.view),
            Action::PageUp if pane == Pane::Guide => {
                self.page(false);
                browse::Outcome::Moved
            }
            Action::PageDown if pane == Pane::Guide => {
                self.page(true);
                browse::Outcome::Moved
            }
            Action::Select => self.focus.ok(&cols, &self.view),
            Action::Back => self.focus.back(&cols, &self.view),
            _ => browse::Outcome::None,
        };
        // No hint card here: Back would first dismiss what is not shown.
        self.focus.hint = false;
        self.follow_focus();
        match outcome {
            browse::Outcome::Preview => GridOutcome::Preview,
            browse::Outcome::Watch => {
                self.focused_channel()
                    .map_or(GridOutcome::Stay, |channel| GridOutcome::Watch {
                        channel,
                        program: self.focus.program(&self.view),
                    })
            }
            browse::Outcome::ReturnToPlayer => GridOutcome::Full,
            browse::Outcome::Open(RailItem::Search) => GridOutcome::Search,
            browse::Outcome::None
                if pane == Pane::Groups
                    && empty
                    && matches!(action, Action::Select | Action::MoveRight) =>
            {
                GridOutcome::Empty
            }
            browse::Outcome::Leave => GridOutcome::Leave,
            _ => GridOutcome::Stay,
        }
    }

    /// Channel Up/Down: a screen of rows up or down, the time column kept
    /// (within the destination row's listings: rows between do not move
    /// it).
    fn page(&mut self, down: bool) {
        let len = self.view.channel_count();
        if len == 0 {
            return;
        }
        let step = if down {
            GRID_ROWS as isize
        } else {
            -(GRID_ROWS as isize)
        };
        let to = page_row(self.focus.channel, len, step);
        // As a tap there: the time clamped to that row's floor and the
        // guide's bounds; then the window follows the focused program.
        let cursor = self.focus.cursor;
        self.focus.tap(to, cursor, &self.view);
        self.focus.set_visible(self.focus.visible, &self.view);
        self.first = if down {
            self.first + GRID_ROWS
        } else {
            self.first.saturating_sub(GRID_ROWS)
        };
        self.clamp_first();
    }

    fn follow_focus(&mut self) {
        let row = self.focus.channel;
        if row < self.first {
            self.first = row;
        } else if row >= self.first + GRID_ROWS {
            self.first = row + 1 - GRID_ROWS;
        }
        self.clamp_first();
    }

    fn clamp_first(&mut self) {
        let len = self.view.channel_count();
        self.first = self.first.min(len.saturating_sub(GRID_ROWS));
        let row = self.focus.channel;
        if row < self.first || row >= self.first + GRID_ROWS {
            self.first = row
                .saturating_sub(GRID_ROWS - 1)
                .min(len.saturating_sub(GRID_ROWS));
        }
    }

    /// The screen: the panes, the window, the rows on it with their cells,
    /// and the focused program. `logo` filters a catalog channel's artwork;
    /// `favorite` says whether a catalog channel is a favorite.
    pub(crate) fn json(
        &self,
        playing: Option<usize>,
        logo: &dyn Fn(usize) -> Option<String>,
        favorite: &dyn Fn(usize) -> bool,
        presentation: &Presentation,
    ) -> Value {
        let s = &self.view;
        let f = &self.focus;
        let epoch = s.epoch_unix();
        let unix = |m: i32| epoch + i64::from(m) * 60;
        let (from, to) = (f.window, f.window + f.visible);
        let len = s.channel_count();
        let focused = f.program(s);
        let rows: Vec<Value> = (self.first..(self.first + GRID_ROWS).min(len))
            .filter_map(|row| {
                let channel = s.channel(row)?;
                let is_playing = playing == Some(channel);
                let archive_days = s.catchup_days(row);
                let mut sig = DefaultHasher::new();
                // The row's number too: a slot can show the same channel at
                // another row (another list), numbered differently.
                (row, channel, from, to, is_playing).hash(&mut sig);
                let mut focus = -1i64;
                let cells: Vec<Value> = s
                    .programs_between(row, from, to - from)
                    .into_iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let title = stored_title(s.listing(row, p)).unwrap_or_default();
                        // An ended program the archive has is not dimmed.
                        let airing = match p.airing(f.now) {
                            Airing::Ended if p.can_replay(f.now, archive_days) => "archive",
                            a => airing_name(a),
                        };
                        (p.start, p.end, &title, airing).hash(&mut sig);
                        if row == f.channel && p == focused {
                            focus = i as i64;
                        }
                        let (a, b) = (p.start.max(from), p.end.min(to));
                        json!({
                            "x": a - from,
                            "w": b - a,
                            "title": title,
                            "airing": airing,
                        })
                    })
                    .collect();
                Some(json!({
                    "number": Rows::number(row),
                    "name": s.channel_name(row),
                    "logo": logo(channel),
                    "playing": is_playing,
                    "sig": format!("{:016x}", sig.finish()),
                    "focus": focus,
                    "cells": cells,
                }))
            })
            .collect();
        let detail = s.channel(f.channel).map(|channel| {
            let title = stored_title(s.listing(f.channel, focused));
            let (start, end) = (unix(focused.start), unix(focused.end));
            let timing = presentation.timing(start, end);
            let airing = focused.airing(f.now);
            json!({
                "number": Rows::number(f.channel),
                "name": s.channel_name(f.channel),
                "title": title,
                "start": start,
                "end": end,
                "timeLabel": presentation.span(start, end, " – "),
                "timingLabel": timing_label(timing),
                "progress": timing.progress,
                "airing": airing_name(airing),
                "catchup": airing == Airing::Ended
                    && focused.can_replay(f.now, s.catchup_days(f.channel)),
                "favorite": favorite(channel),
            })
        });
        let start = unix(from);
        let first_mark = start + (-start).rem_euclid(1800);
        let marks: Vec<_> = (first_mark..unix(to))
            .step_by(1800)
            .map(|at| {
                json!({
                    "at": at,
                    "time": presentation.time(at),
                })
            })
            .collect();
        json!({
            "clock": presentation.clock(),
            "marks": marks,
            "pane": pane_name(f.pane),
            // The rail and the groups column show while one has the focus
            // (or the preview, entered from one of them).
            "columns": f.columns_shown(),
            "layout": pane_name(f.layout_pane()),
            "rail": if f.rail == RailItem::Search { "search" } else { "tv" },
            "start": unix(from),
            "minutes": f.visible,
            "now": unix(f.now),
            "rows": { "start": self.first, "focus": f.channel, "total": len, "items": rows },
            "focused": detail,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_core::guide::SyntheticSchedule;
    use iptv_presentation::guide::home_window;

    const ENTRIES: usize = 10;

    fn grid(len: usize, row: usize) -> Grid {
        let rows = Rows::new(iptv_presentation::lists::ALL, (0..len).collect());
        let mut g = Grid::open(Rc::new(SyntheticSchedule), Rc::new(rows), 0);
        g.show_channel(row);
        g
    }

    fn json(g: &Grid) -> Value {
        g.json(
            None,
            &|_| None,
            &|_| false,
            &Presentation::utc(iptv_core::guide::SYNTHETIC_EPOCH_UNIX),
        )
    }

    #[test]
    fn it_opens_on_the_rows_live_program_with_the_screen_around_it() {
        let g = grid(100, 40);
        assert_eq!(g.focus.channel, 40);
        assert_eq!(g.focus.window, home_window(0));
        assert_eq!(g.focus.visible, GRID_MINUTES);
        assert!((g.first..g.first + GRID_ROWS).contains(&40));
        let v = json(&g);
        assert_eq!(v["pane"], "guide");
        assert_eq!(v["columns"], false);
        assert_eq!(v["rows"]["items"].as_array().unwrap().len(), GRID_ROWS);
        assert_eq!(v["focused"]["airing"], "live");
        // Cells are cut to the window and tile it.
        for row in v["rows"]["items"].as_array().unwrap() {
            let cells = row["cells"].as_array().unwrap();
            let width: i64 = cells.iter().map(|c| c["w"].as_i64().unwrap()).sum();
            assert_eq!(width, i64::from(GRID_MINUTES), "{row}");
            assert_eq!(cells[0]["x"], 0);
        }
        let focused_row = &v["rows"]["items"][40 - g.first];
        assert!(focused_row["focus"].as_i64().unwrap() >= 0);
    }

    #[test]
    fn rows_stay_until_the_focus_leaves_the_screen() {
        let mut g = grid(100, 0);
        assert_eq!(g.first, 0);
        for _ in 0..GRID_ROWS - 1 {
            g.key(Action::MoveDown, 0, ENTRIES, false);
        }
        assert_eq!(g.first, 0, "still on the first screen");
        g.key(Action::MoveDown, 0, ENTRIES, false);
        assert_eq!((g.focus.channel, g.first), (GRID_ROWS, 1));
        // A page turns the whole screen and keeps the time column.
        let cursor = g.focus.cursor;
        g.key(Action::PageDown, 0, ENTRIES, false);
        assert_eq!(g.focus.channel, 2 * GRID_ROWS);
        assert_eq!(g.first, 1 + GRID_ROWS);
        assert_eq!(g.focus.cursor, cursor);
        g.key(Action::PageUp, 0, ENTRIES, false);
        g.key(Action::PageUp, 0, ENTRIES, false);
        assert_eq!((g.focus.channel, g.first), (0, 0));
        // The end of the list clamps.
        for _ in 0..20 {
            g.key(Action::PageDown, 0, ENTRIES, false);
        }
        assert_eq!(g.focus.channel, 99);
        assert_eq!(g.first, 100 - GRID_ROWS);
    }

    #[test]
    fn a_row_signature_changes_only_with_what_the_row_shows() {
        let mut g = grid(100, 2);
        let before = json(&g);
        g.key(Action::MoveDown, 0, ENTRIES, false);
        let after = json(&g);
        let sig = |v: &Value, i: usize| v["rows"]["items"][i]["sig"].clone();
        assert_eq!(after["rows"]["start"], before["rows"]["start"]);
        for i in 0..GRID_ROWS {
            assert_eq!(sig(&before, i), sig(&after, i), "row {i} unchanged");
        }
        assert_eq!(after["rows"]["focus"], 3);
        // Moving through time scrolls the window: the rows change.
        for _ in 0..10 {
            g.key(Action::MoveRight, 0, ENTRIES, false);
        }
        let later = json(&g);
        assert_ne!(later["start"], after["start"]);
        assert_ne!(sig(&later, 0), sig(&after, 0));
        // Back returns to now, then shows the groups, then the rail; on the
        // rail it asks to leave, and stays there.
        assert_eq!(g.key(Action::Back, 0, ENTRIES, false), GridOutcome::Stay);
        assert_eq!(g.focus.cursor, 0);
        g.key(Action::Back, 0, ENTRIES, false);
        assert_eq!(g.pane(), Pane::Groups);
        assert_eq!(json(&g)["columns"], true);
        g.key(Action::Back, 0, ENTRIES, false);
        assert_eq!(g.pane(), Pane::Rail);
        assert_eq!(g.key(Action::Back, 0, ENTRIES, false), GridOutcome::Leave);
        assert_eq!(g.pane(), Pane::Rail);
    }

    #[test]
    fn ok_selects_the_focused_channel_and_program() {
        let mut g = grid(20, 0);
        let now = g.focus.now;
        for at in [now, now + 60] {
            g.focus.tap(0, at, &g.view);
            let program = g.focus.program(&g.view);
            assert_eq!(
                g.key(Action::Select, now, ENTRIES, false),
                GridOutcome::Watch {
                    channel: 0,
                    program
                },
            );
        }
    }

    #[test]
    fn rows_wrap_while_playing_and_search_can_focus_the_preview() {
        let mut g = grid(100, 0);
        g.key(Action::MoveUp, 0, ENTRIES, true);
        assert_eq!((g.pane(), g.row()), (Pane::Guide, 99));
        g.key(Action::MoveDown, 0, ENTRIES, true);
        assert_eq!((g.pane(), g.row()), (Pane::Guide, 0));
        // Back on a live program shows the groups; Left there is the
        // rail's Search, which opens the search screen.
        g.key(Action::Back, 0, ENTRIES, true);
        assert_eq!(g.pane(), Pane::Groups);
        g.key(Action::MoveLeft, 0, ENTRIES, true);
        assert_eq!(json(&g)["rail"], "search");
        g.key(Action::MoveUp, 0, ENTRIES, false);
        assert_eq!(g.pane(), Pane::Rail);
        g.key(Action::MoveUp, 0, ENTRIES, true);
        assert_eq!(g.pane(), Pane::Preview);
        assert_eq!(g.key(Action::Select, 0, ENTRIES, true), GridOutcome::Full);
        g.key(Action::MoveDown, 0, ENTRIES, true);
        assert_eq!(g.pane(), Pane::Rail);
        assert_eq!(g.key(Action::Select, 0, ENTRIES, true), GridOutcome::Search);
    }
}
