//! The full guide grid: a time ruler, one row per channel of the list
//! shown, programs as cells sized by duration, and the focused cell's
//! title and time. The focus rules are the guide's own
//! ([`iptv_core::guide::Focus`] over a [`ListView`] of the list), so the
//! grid moves as Patina's guide does on the other platforms.
//!
//! Only the screen is emitted: [`GRID_ROWS`] rows from the first one
//! shown, and per row the programs that overlap the time window (cut to
//! it). Each row carries a signature of what it shows, so the page
//! rebuilds a row's cells only when it changed and otherwise moves the
//! focus mark.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use iptv_core::guide::{Airing, Focus, Move, Pane, Program, Schedule};
use iptv_core::lists::{ChannelList as Rows, ListView};
use serde_json::{Value, json};

/// Channel rows the grid shows at once.
pub(crate) const GRID_ROWS: usize = 8;
/// Minutes across the grid.
pub(crate) const GRID_MINUTES: i32 = 120;

/// What a key in the grid asks of the app.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GridOutcome {
    Stay,
    /// Play catalog `channel`: live, or `archive` from its start.
    Play {
        channel: usize,
        archive: Option<Program>,
    },
    Close,
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

fn is_placeholder(title: &str) -> bool {
    title.is_empty() || title == iptv_guide::NO_INFORMATION || title == iptv_guide::LOADING
}

impl Grid {
    /// The grid of `rows` on `catalog`, focused on row `row`'s program at
    /// guide minute `now`.
    pub(crate) fn open(catalog: Rc<dyn Schedule>, rows: &Rows, row: usize, now: i32) -> Grid {
        let view = ListView::new(catalog, Rc::new(rows.clone()));
        let mut focus = Focus::at(now);
        focus.set_visible(GRID_MINUTES, &view);
        focus.show_channel(row, &view);
        let mut grid = Grid {
            view,
            focus,
            first: 0,
        };
        grid.first = grid.focus.channel.saturating_sub(GRID_ROWS / 2);
        grid.clamp_first();
        grid
    }

    /// The same grid on a new schedule of the same catalog (the guide
    /// arrived): the focused channel is found again, the time stays.
    pub(crate) fn rebase(&mut self, catalog: Rc<dyn Schedule>, rows: &Rows, now: i32) {
        let channel = self.focused_channel();
        let offset = self.focus.channel.saturating_sub(self.first);
        self.view = ListView::new(catalog, Rc::new(rows.clone()));
        self.focus.channel = channel.and_then(|c| rows.row_of(c)).unwrap_or(0);
        self.focus.clamp_to(&self.view);
        self.first = self.focus.channel.saturating_sub(offset);
        self.clamp_first();
        self.tick(now);
    }

    /// The catalog channel of the focused row.
    pub(crate) fn focused_channel(&self) -> Option<usize> {
        self.view.channel(self.focus.channel)
    }

    /// Now moved on (a live focus follows it).
    pub(crate) fn tick(&mut self, now: i32) {
        self.focus.tick(now, &self.view);
    }

    pub(crate) fn key(&mut self, key: &str, now: i32) -> GridOutcome {
        self.tick(now);
        match key {
            "up" => self.focus.apply(Move::Up, &self.view),
            "down" => self.focus.apply(Move::Down, &self.view),
            "left" => {
                self.focus.apply(Move::Left, &self.view);
                // The grid has no groups column beside it: the earliest
                // program stays focused.
                self.focus.pane = Pane::Guide;
            }
            "right" => self.focus.apply(Move::Right, &self.view),
            "chup" | "pageup" => self.page(false),
            "chdown" | "pagedown" => self.page(true),
            "ok" => return self.select(),
            // Back first returns a focus in the past or future to now.
            "back" if !self.focus.return_to_now(&self.view) => return GridOutcome::Close,
            _ => {}
        }
        self.follow_focus();
        GridOutcome::Stay
    }

    /// Channel Up/Down: a screen of rows up or down, the time column kept.
    fn page(&mut self, down: bool) {
        let step = if down { Move::Down } else { Move::Up };
        for _ in 0..GRID_ROWS {
            self.focus.apply(step, &self.view);
        }
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

    /// OK: the focused channel plays; an ended program its archive still
    /// holds plays from its start.
    fn select(&self) -> GridOutcome {
        let Some(channel) = self.focused_channel() else {
            return GridOutcome::Stay;
        };
        let p = self.focus.program(&self.view);
        let now = self.focus.now;
        let archive = (p.airing(now) == Airing::Ended
            && p.can_replay(now, self.view.catchup_days(self.focus.channel)))
        .then_some(p);
        GridOutcome::Play { channel, archive }
    }

    /// The screen: the window, the rows on it with their cells, and the
    /// focused program. `logo` filters a catalog channel's artwork.
    pub(crate) fn json(
        &self,
        playing: Option<usize>,
        logo: &dyn Fn(usize) -> Option<String>,
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
                let mut sig = DefaultHasher::new();
                (channel, from, to, is_playing).hash(&mut sig);
                let mut focus = -1i64;
                let cells: Vec<Value> = s
                    .programs_between(row, from, to - from)
                    .into_iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let title = s.title(row, p);
                        let title = if is_placeholder(&title) { "" } else { &title };
                        let airing = p.airing(f.now);
                        (p.start, p.end, title, airing as u8).hash(&mut sig);
                        if row == f.channel && p == focused {
                            focus = i as i64;
                        }
                        let (a, b) = (p.start.max(from), p.end.min(to));
                        json!({
                            "x": a - from,
                            "w": b - a,
                            "title": title,
                            "airing": airing_name(airing),
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
        let detail = s.channel(f.channel).map(|_| {
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
            })
        });
        json!({
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

    fn grid(len: usize, row: usize) -> Grid {
        let rows = Rows::new(iptv_core::lists::ALL, (0..len).collect());
        Grid::open(Rc::new(SyntheticSchedule), &rows, row, 0)
    }

    #[test]
    fn it_opens_on_the_rows_live_program_with_the_screen_around_it() {
        let g = grid(100, 40);
        assert_eq!(g.focus.channel, 40);
        assert_eq!(g.focus.window, home_window(0));
        assert_eq!(g.focus.visible, GRID_MINUTES);
        assert!((g.first..g.first + GRID_ROWS).contains(&40));
        let v = g.json(None, &|_| None);
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
            g.key("down", 0);
        }
        assert_eq!(g.first, 0, "still on the first screen");
        g.key("down", 0);
        assert_eq!((g.focus.channel, g.first), (GRID_ROWS, 1));
        // A page turns the whole screen and keeps the time column.
        let cursor = g.focus.cursor;
        g.key("chdown", 0);
        assert_eq!(g.focus.channel, 2 * GRID_ROWS);
        assert_eq!(g.first, 1 + GRID_ROWS);
        assert_eq!(g.focus.cursor, cursor);
        g.key("chup", 0);
        g.key("chup", 0);
        assert_eq!((g.focus.channel, g.first), (0, 0));
        // The end of the list clamps.
        for _ in 0..20 {
            g.key("pagedown", 0);
        }
        assert_eq!(g.focus.channel, 99);
        assert_eq!(g.first, 100 - GRID_ROWS);
    }

    #[test]
    fn a_row_signature_changes_only_with_what_the_row_shows() {
        let mut g = grid(100, 2);
        let before = g.json(None, &|_| None);
        g.key("down", 0);
        let after = g.json(None, &|_| None);
        let sig = |v: &Value, i: usize| v["rows"]["items"][i]["sig"].clone();
        assert_eq!(after["rows"]["start"], before["rows"]["start"]);
        for i in 0..GRID_ROWS {
            assert_eq!(sig(&before, i), sig(&after, i), "row {i} unchanged");
        }
        assert_eq!(after["rows"]["focus"], 3);
        // Moving through time scrolls the window: the rows change.
        for _ in 0..10 {
            g.key("right", 0);
        }
        let later = g.json(None, &|_| None);
        assert_ne!(later["start"], after["start"]);
        assert_ne!(sig(&later, 0), sig(&after, 0));
        // Back returns to now, then closes.
        assert_eq!(g.key("back", 0), GridOutcome::Stay);
        assert_eq!(g.focus.cursor, 0);
        assert_eq!(g.key("back", 0), GridOutcome::Close);
    }

    #[test]
    fn ok_plays_the_focused_channel() {
        // Archived programs are covered end to end (`tests.rs`): the
        // synthetic schedule has no listing start, so its past is not
        // browsable.
        let mut g = grid(100, 0);
        g.key("down", 0);
        assert_eq!(
            g.key("ok", 0),
            GridOutcome::Play {
                channel: 1,
                archive: None
            }
        );
        // An empty list plays nothing.
        let mut empty = grid(0, 0);
        assert_eq!(empty.key("ok", 0), GridOutcome::Stay);
        assert!(empty.json(None, &|_| None)["focused"].is_null());
    }
}
