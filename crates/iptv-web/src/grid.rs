//! The guide screen below the preview: the navigation rail, the groups
//! column and the guide grid (a time ruler, one row per channel of the list
//! shown, programs as cells sized by duration). The focus rules are the
//! core's ([`iptv_core::guide::Focus`] over a [`ListView`] of the list, its
//! columns in [`iptv_core::browse`]), so the screen moves as Patina's guide
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

use iptv_core::browse::{self, Columns, RailItem};
use iptv_core::guide::{Airing, Focus, Move, Pane, Program, Schedule};
use iptv_core::lists::{ChannelList as Rows, ListView};
use serde_json::{Value, json};

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
    /// Watch catalog `channel`: live, or `archive` from its start.
    Watch {
        channel: usize,
        archive: Option<Program>,
    },
    /// OK on a program that has not started (its start, Unix time).
    Upcoming(i64),
    /// OK on an ended program the archive no longer has.
    Unavailable,
    /// OK on the preview: the playing video full screen again.
    Full,
    /// The rail's Search.
    Search,
    /// OK or Right on an empty list.
    Empty,
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

fn is_placeholder(title: &str) -> bool {
    title.is_empty() || title == iptv_guide::NO_INFORMATION || title == iptv_guide::LOADING
}

fn columns(entries: usize) -> Columns<'static> {
    Columns {
        entries,
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
    /// arrived): the focused channel is found again, the time stays. A
    /// schedule made in a later hour counts minutes from that hour: the
    /// focus's minutes are converted through Unix time first.
    pub(crate) fn rebase(&mut self, catalog: Rc<dyn Schedule>, rows: Rc<Rows>, now: i32) {
        let channel = self.focused_channel();
        let offset = self.focus.channel.saturating_sub(self.first);
        let row = channel.and_then(|c| rows.row_of(c));
        let shift = (self.view.epoch_unix() - catalog.epoch_unix()).div_euclid(60);
        let shift = i32::try_from(shift).unwrap_or(0);
        let f = &mut self.focus;
        f.cursor = f.cursor.saturating_add(shift);
        f.window = f.window.saturating_add(shift);
        f.now = f.now.saturating_add(shift);
        self.view = ListView::new(catalog, rows);
        self.focus.restore(Some(row.unwrap_or(0)), &self.view);
        self.first = self.focus.channel.saturating_sub(offset);
        self.clamp_first();
        self.tick(now);
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

    /// Shows the groups column with the focus on the selected entry.
    pub(crate) fn open_groups(&mut self) {
        self.focus.open_groups();
    }

    /// The video stopped: a focused preview hands the focus back.
    pub(crate) fn preview_gone(&mut self) {
        self.focus.retain_preview(false);
        self.focus.restore(None, &self.view);
    }

    /// A key on the guide screen. `entries` is the groups column's length;
    /// `playing` whether a video plays (the preview can take the focus).
    pub(crate) fn key(
        &mut self,
        key: &str,
        now: i32,
        entries: usize,
        playing: bool,
    ) -> GridOutcome {
        self.tick(now);
        let cols = columns(entries);
        let pane = self.focus.pane;
        let empty = self.view.channel_count() == 0;
        let outcome = match key {
            "up" if self.focus.focus_preview(playing) => browse::Outcome::Moved,
            "up" => self.focus.navigate(Move::Up, &cols, &self.view),
            "down" => self.focus.navigate(Move::Down, &cols, &self.view),
            "left" => self.focus.navigate(Move::Left, &cols, &self.view),
            "right" => self.focus.navigate(Move::Right, &cols, &self.view),
            "chup" | "pageup" if pane == Pane::Guide => {
                self.page(false);
                browse::Outcome::Moved
            }
            "chdown" | "pagedown" if pane == Pane::Guide => {
                self.page(true);
                browse::Outcome::Moved
            }
            "ok" => self.focus.ok(&cols, &self.view),
            "back" => self.focus.back(&cols, &self.view),
            // A held Left (the page sends it once per hold): the groups
            // column from anywhere in the grid, as on Android TV.
            "groups" if pane == Pane::Guide => {
                self.focus.open_groups();
                browse::Outcome::Moved
            }
            _ => browse::Outcome::None,
        };
        // No hint card here: Back would first dismiss what is not shown.
        self.focus.hint = false;
        self.follow_focus();
        match outcome {
            browse::Outcome::Preview => GridOutcome::Preview,
            browse::Outcome::Watch => self.watch(),
            browse::Outcome::ReturnToPlayer => GridOutcome::Full,
            browse::Outcome::Open(RailItem::Search) => GridOutcome::Search,
            browse::Outcome::None
                if pane == Pane::Groups && empty && matches!(key, "ok" | "right") =>
            {
                GridOutcome::Empty
            }
            // Back on the rail: the app stays (the remote's Exit key leaves).
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
        let row = self.focus.channel;
        let to = if down {
            (row + GRID_ROWS).min(len - 1)
        } else {
            row.saturating_sub(GRID_ROWS)
        };
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

    /// OK on a program: a live one plays live, an ended one its archive
    /// still holds plays from its start; the others cannot play.
    fn watch(&self) -> GridOutcome {
        let Some(channel) = self.focused_channel() else {
            return GridOutcome::Stay;
        };
        let p = self.focus.program(&self.view);
        let now = self.focus.now;
        match p.airing(now) {
            Airing::Live => GridOutcome::Watch {
                channel,
                archive: None,
            },
            Airing::Ended if p.can_replay(now, self.view.catchup_days(self.focus.channel)) => {
                GridOutcome::Watch {
                    channel,
                    archive: Some(p),
                }
            }
            Airing::Ended => GridOutcome::Unavailable,
            Airing::Upcoming => {
                GridOutcome::Upcoming(self.view.epoch_unix() + i64::from(p.start) * 60)
            }
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
                        let title = s.title(row, p);
                        let title = if is_placeholder(&title) { "" } else { &title };
                        // An ended program the archive has is not dimmed.
                        let airing = match p.airing(f.now) {
                            Airing::Ended if p.can_replay(f.now, archive_days) => "archive",
                            a => airing_name(a),
                        };
                        (p.start, p.end, title, airing).hash(&mut sig);
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
            let title = s.title(f.channel, focused);
            let airing = focused.airing(f.now);
            json!({
                "number": Rows::number(f.channel),
                "name": s.channel_name(f.channel),
                "title": if is_placeholder(&title) { None } else { Some(title) },
                "start": unix(focused.start),
                "end": unix(focused.end),
                "airing": airing_name(airing),
                "catchup": airing == Airing::Ended
                    && focused.can_replay(f.now, s.catchup_days(f.channel)),
                "favorite": favorite(channel),
            })
        });
        json!({
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
    use iptv_core::guide::{SyntheticSchedule, home_window};

    const ENTRIES: usize = 10;

    fn grid(len: usize, row: usize) -> Grid {
        let rows = Rows::new(iptv_core::lists::ALL, (0..len).collect());
        let mut g = Grid::open(Rc::new(SyntheticSchedule), Rc::new(rows), 0);
        g.show_channel(row);
        g
    }

    fn json(g: &Grid) -> Value {
        g.json(None, &|_| None, &|_| false)
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
            g.key("down", 0, ENTRIES, false);
        }
        assert_eq!(g.first, 0, "still on the first screen");
        g.key("down", 0, ENTRIES, false);
        assert_eq!((g.focus.channel, g.first), (GRID_ROWS, 1));
        // A page turns the whole screen and keeps the time column.
        let cursor = g.focus.cursor;
        g.key("chdown", 0, ENTRIES, false);
        assert_eq!(g.focus.channel, 2 * GRID_ROWS);
        assert_eq!(g.first, 1 + GRID_ROWS);
        assert_eq!(g.focus.cursor, cursor);
        g.key("chup", 0, ENTRIES, false);
        g.key("chup", 0, ENTRIES, false);
        assert_eq!((g.focus.channel, g.first), (0, 0));
        // The end of the list clamps.
        for _ in 0..20 {
            g.key("pagedown", 0, ENTRIES, false);
        }
        assert_eq!(g.focus.channel, 99);
        assert_eq!(g.first, 100 - GRID_ROWS);
    }

    #[test]
    fn a_row_signature_changes_only_with_what_the_row_shows() {
        let mut g = grid(100, 2);
        let before = json(&g);
        g.key("down", 0, ENTRIES, false);
        let after = json(&g);
        let sig = |v: &Value, i: usize| v["rows"]["items"][i]["sig"].clone();
        assert_eq!(after["rows"]["start"], before["rows"]["start"]);
        for i in 0..GRID_ROWS {
            assert_eq!(sig(&before, i), sig(&after, i), "row {i} unchanged");
        }
        assert_eq!(after["rows"]["focus"], 3);
        // Moving through time scrolls the window: the rows change.
        for _ in 0..10 {
            g.key("right", 0, ENTRIES, false);
        }
        let later = json(&g);
        assert_ne!(later["start"], after["start"]);
        assert_ne!(sig(&later, 0), sig(&after, 0));
        // Back returns to now, then shows the groups, then the rail; on the
        // rail it stays.
        assert_eq!(g.key("back", 0, ENTRIES, false), GridOutcome::Stay);
        assert_eq!(g.focus.cursor, 0);
        g.key("back", 0, ENTRIES, false);
        assert_eq!(g.pane(), Pane::Groups);
        assert_eq!(json(&g)["columns"], true);
        g.key("back", 0, ENTRIES, false);
        assert_eq!(g.pane(), Pane::Rail);
        assert_eq!(g.key("back", 0, ENTRIES, false), GridOutcome::Stay);
        assert_eq!(g.pane(), Pane::Rail);
    }

    #[test]
    fn ok_watches_live_programs_and_refuses_future_ones() {
        // Archived programs are covered end to end (`tests.rs`): the
        // synthetic schedule has no listing start, so its past is not
        // browsable.
        let mut g = grid(100, 0);
        g.key("down", 0, ENTRIES, false);
        assert_eq!(
            g.key("ok", 0, ENTRIES, false),
            GridOutcome::Watch {
                channel: 1,
                archive: None
            }
        );
        g.key("right", 0, ENTRIES, false);
        let start = json(&g)["focused"]["start"].as_i64().unwrap();
        assert_eq!(g.key("ok", 0, ENTRIES, false), GridOutcome::Upcoming(start));
        // An empty list plays nothing and cannot be entered.
        let mut empty = grid(0, 0);
        assert_eq!(empty.pane(), Pane::Groups);
        assert_eq!(empty.key("ok", 0, ENTRIES, false), GridOutcome::Empty);
        assert!(json(&empty)["focused"].is_null());
    }

    #[test]
    fn up_from_the_first_row_reaches_the_preview_only_while_playing() {
        let mut g = grid(100, 0);
        g.key("up", 0, ENTRIES, false);
        assert_eq!(g.pane(), Pane::Guide);
        g.key("up", 0, ENTRIES, true);
        assert_eq!(g.pane(), Pane::Preview);
        assert_eq!(g.key("ok", 0, ENTRIES, true), GridOutcome::Full);
        g.key("down", 0, ENTRIES, true);
        assert_eq!((g.pane(), g.row()), (Pane::Guide, 0));
        // Back on a live program shows the groups; Left there is the
        // rail's Search, which opens the search screen.
        g.key("back", 0, ENTRIES, true);
        assert_eq!(g.pane(), Pane::Groups);
        g.key("left", 0, ENTRIES, true);
        assert_eq!(json(&g)["rail"], "search");
        assert_eq!(g.key("ok", 0, ENTRIES, true), GridOutcome::Search);
    }
}
