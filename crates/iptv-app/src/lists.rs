//! The list the guide shows (`iptv_core::lists`) and the guide screen's
//! keys across the rail, the groups column and the grid
//! (`iptv_core::browse`).
//!
//! `crate::schedule()` stays the full catalog: `Ui::playing`, `Ui::play`,
//! search and the player use catalog channels. The focus works on rows of
//! [`Ui::list`] through [`view`]; [`focused_channel`] maps back. The list
//! is rebuilt when the selection, the catalog or the favourites change,
//! and the focus stays on its channel by key.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use iptv_core::browse::{Columns, Outcome, RailItem, rail_items};
use iptv_core::guide::{Focus, Move, Pane, Schedule};
use iptv_core::lists::{
    ALL, ChannelList, Entry, FAVORITES, ListId, ListView, entry_count, group_of,
};
use nsh_core::prelude::*;

use crate::Ui;
use crate::i18n::tr;

/// How long the hint card stays after a group was chosen.
const HINT: Duration = Duration::from_secs(6);

thread_local! {
    /// Bumped per hint shown: an older timer leaves a newer hint alone.
    static HINT_SHOWN: Cell<u64> = const { Cell::new(0) };
}

/// The list on screen as a schedule of rows.
pub fn view(ui: Ui) -> ListView {
    ListView::new(crate::schedule(), ui.list.with(Rc::clone))
}

/// The catalog channel on the focused row.
pub fn focused_channel(ui: Ui) -> Option<usize> {
    let row = ui.focus.with(|f| f.channel);
    ui.list.with(|l| l.channel(row))
}

/// The number catalog `channel` shows (the player card, History): its
/// place in the list on screen, as in the guide, else the catalog's.
pub fn number(ui: Ui, channel: usize) -> u32 {
    let catalog = crate::schedule();
    ui.list.with(|l| l.number_of(channel, &*catalog))
}

/// The rail's items for the source on screen.
pub fn rail(ui: Ui) -> Vec<RailItem> {
    rail_items(ui.has_vod.get())
}

/// Localize built-in lists by identity; provider group names are opaque.
pub fn entry_name(entry: usize, catalog: &dyn Schedule) -> String {
    let name = iptv_core::lists::entry_name(entry, catalog);
    match Entry::at(entry) {
        Entry::Group(_) => name,
        _ => tr(&name).to_owned(),
    }
}

/// The name of the list on screen.
pub fn list_name(ui: Ui) -> String {
    ui.data_version.get();
    let entry = ui.list.with(|l| l.entry());
    entry_name(entry, &*crate::schedule())
}

/// Builds the list of the selected entry. With `keep`, the focus stays on
/// its channel (the content changed under it); a newly selected list
/// starts where the core put the focus (its top).
pub fn rebuild(ui: Ui, keep: bool) {
    let key = if keep {
        let v = view(ui);
        ui.focus.with(|f| v.channel_key(f.channel))
    } else {
        None
    };
    let catalog = crate::schedule();
    let entry = ui.focus.with(|f| f.group);
    let list = ChannelList::build(
        entry,
        &*catalog,
        &crate::favorites::favorite_channels(),
        &crate::favorites::recent_channels(),
    );
    ui.list.set(Rc::new(list));
    let view = view(ui);
    let row = key.and_then(|k| row_of_key(&view, &k));
    ui.focus.update(|f| f.restore(row, &view));
}

fn row_of_key(view: &ListView, key: &str) -> Option<usize> {
    view.list().row_of(crate::favorites::channel_of(key)?)
}

/// The favourites changed: the Favourites list follows if it is shown.
pub fn favorites_changed(ui: Ui) {
    if ui.focus.with_untracked(|f| f.group) == FAVORITES {
        rebuild(ui, true);
    }
}

/// What the guide shows, by content, to find again in a new catalog.
pub struct Kept {
    list: ListId,
    key: Option<String>,
    focus: Focus,
}

/// Before a catalog swap (`set_schedule`).
pub fn keep(ui: Ui) -> Kept {
    let view = view(ui);
    let focus = ui.focus.get_untracked();
    Kept {
        list: ListId::of(focus.group, &**view.catalog()),
        key: view.channel_key(focus.channel),
        focus,
    }
}

/// After a catalog swap: the same list and channel by identity, at the new
/// catalog's now, with the same column focused.
pub fn restore(ui: Ui, kept: Kept) {
    let catalog = crate::schedule();
    let mut focus = Focus::at(crate::time::now_minute());
    focus.visible = kept.focus.visible;
    focus.pane = kept.focus.pane;
    focus.rail = kept.focus.rail;
    focus.group = kept.list.find(&*catalog);
    ui.focus.set(focus);
    rebuild(ui, false);
    let view = view(ui);
    let row = kept.key.and_then(|k| row_of_key(&view, &k));
    ui.focus.update(|f| f.restore(row, &view));
}

/// The row of catalog `channel`, selecting the channel's own group first
/// when the list on screen does not have it.
fn listed(ui: Ui, channel: usize) -> usize {
    if let Some(row) = ui.list.with_untracked(|l| l.row_of(channel)) {
        return row;
    }
    let catalog = crate::schedule();
    let entry = group_of(&*catalog, channel).map_or(ALL, |g| Entry::Group(g).index());
    ui.focus.update(|f| f.group = entry);
    rebuild(ui, false);
    ui.list.with_untracked(|l| l.row_of(channel)).unwrap_or(0)
}

/// Shows what plays on catalog `channel` (a zap, a resumed or previous
/// channel): its row, its live programme, the default window.
pub fn show_channel(ui: Ui, channel: usize) {
    let row = listed(ui, channel);
    let view = view(ui);
    ui.focus.update(|f| f.show_channel(row, &view));
}

/// Focuses catalog `channel`'s programme at `minute` where it is (a search
/// result, a closed menu).
pub fn tap(ui: Ui, channel: usize, minute: i32) {
    let row = listed(ui, channel);
    let view = view(ui);
    ui.focus.update(|f| f.tap(row, minute, &view));
}

/// Runs a focus step with the columns and the list on screen.
fn step(ui: Ui, f: impl FnOnce(&mut Focus, &Columns, &ListView) -> Outcome) -> Outcome {
    let rail = rail_items(ui.has_vod.get_untracked());
    let view = view(ui);
    let cols = Columns {
        entries: entry_count(&**view.catalog()),
        rail: &rail,
        back_to_now: crate::settings::flag(iptv_settings::keys::BACK_RETURNS_TO_NOW),
    };
    let mut outcome = Outcome::None;
    ui.focus.update(|focus| outcome = f(focus, &cols, &view));
    outcome
}

/// A direction on the guide screen.
pub fn navigate(ui: Ui, m: Move) {
    let o = step(ui, |f, cols, view| f.navigate(m, cols, view));
    follow(ui, o);
}

/// OK on the guide screen.
pub fn ok(ui: Ui) {
    if ui.focus.with_untracked(|f| f.pane) == Pane::Groups
        && toggle_playlist(ui.focus.with_untracked(|f| f.group))
    {
        return;
    }

    let o = step(ui, |f, cols, view| f.ok(cols, view));
    follow(ui, o);
}

/// Back on the guide screen; `false` where Back leaves the app.
pub fn back(ui: Ui) -> bool {
    let o = step(ui, |f, cols, view| f.back(cols, view));
    follow(ui, o);
    o != Outcome::Leave
}

/// Shows the groups column (the Groups key, a tap on the list's name).
pub fn open_groups(ui: Ui) {
    ui.focus.update(Focus::open_groups);
}

/// Opens the rail with its names (an iPad's ‹ in the groups column).
pub fn open_rail(ui: Ui) {
    step(ui, |f, cols, _| {
        f.open_rail(cols);
        Outcome::Moved
    });
}

/// A tap on a groups column entry.
pub fn tap_entry(ui: Ui, entry: usize) {
    let o = step(ui, |f, cols, _| f.tap_entry(entry, cols));
    follow(ui, o);
    // A parent previews its source before changing section visibility.
    toggle_playlist(entry);
}

/// The player's channel list chose groups column `entry`: the guide shows
/// its list as its own groups column would (focus at the top, now).
pub fn preview(ui: Ui, entry: usize) {
    let changed = step(ui, |f, cols, _| {
        if f.preview_list(entry, cols) {
            Outcome::Preview
        } else {
            Outcome::None
        }
    });
    if changed == Outcome::Preview {
        rebuild(ui, false);
    }
}

/// A tap on a rail item.
pub fn tap_rail(ui: Ui, index: usize) {
    let o = step(ui, |f, cols, view| f.tap_rail(index, cols, view));
    follow(ui, o);
}

fn follow(ui: Ui, outcome: Outcome) {
    match outcome {
        Outcome::Preview => rebuild(ui, false),
        Outcome::Committed if ui.focus.with_untracked(|f| f.hint) => arm_hint(ui),
        Outcome::Watch => ui.watch(),
        Outcome::Open(item) => open(ui, item),
        _ => {}
    }
}

fn open(ui: Ui, item: RailItem) {
    match item {
        RailItem::Search => crate::search_view::open(ui),
        RailItem::Playlists => crate::playlists_view::open(ui),
        RailItem::Settings => crate::settings_view::open(ui),
        other => log::info!("iptv rail: {other:?} not implemented yet"),
    }
}

/// Hides the hint card after a while, unless a newer one shows by then.
fn arm_hint(ui: Ui) {
    let shown = HINT_SHOWN.with(|h| {
        h.set(h.get() + 1);
        h.get()
    });
    let _task = set_timeout(HINT, move || {
        if HINT_SHOWN.with(Cell::get) == shown && ui.focus.with_untracked(|f| f.hint) {
            ui.focus.update(|f| f.hint = false);
        }
    });
}

/// Whether the rail and groups show beside the guide.
pub fn columns_shown(ui: Ui) -> bool {
    ui.focus.with(|f| f.pane) != Pane::Guide
}

fn toggle_playlist(entry: usize) -> bool {
    match Entry::at(entry) {
        Entry::Group(group) => crate::data::combined::toggle_playlist(group),
        _ => false,
    }
}

pub fn browser_entry_name(entry: usize) -> String {
    let schedule = crate::schedule();
    let name = entry_name(entry, &*schedule);
    if let Entry::Group(g) = Entry::at(entry) {
        if schedule.group_is_playlist(g) {
            return format!(
                "{}  {}",
                if schedule.group_is_collapsed(g) {
                    "▸"
                } else {
                    "▾"
                },
                name
            );
        }
        if schedule.group_depth(g) > 0 {
            return format!("    {name}");
        }
    }
    name
}
