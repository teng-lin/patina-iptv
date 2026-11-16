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
        Entry::Group(group) => catalog.group_name_in(group, crate::i18n::language()),
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
    keep_in(ui, crate::schedule())
}

/// A window can observe a catalog replacement after the runtime installed it.
/// Resolve its old row identities against the catalog that built its list.
pub(super) fn keep_in(ui: Ui, catalog: Rc<dyn Schedule>) -> Kept {
    keep_list(ui.focus.get_untracked(), ui.list.get_untracked(), catalog)
}

fn keep_list(focus: Focus, list: Rc<ChannelList>, catalog: Rc<dyn Schedule>) -> Kept {
    let view = ListView::new(catalog, list);
    Kept {
        list: ListId::of(focus.group, &**view.catalog()),
        key: view.channel_key(focus.channel),
        focus,
    }
}

/// After a catalog swap: the same list and channel by identity, at the new
/// catalog's now, with the same column focused.
pub fn restore(ui: Ui, kept: Kept) {
    let (focus, list) = restored_list(
        kept,
        crate::schedule(),
        &crate::favorites::favorite_channels(),
        &crate::favorites::recent_channels(),
        crate::time::now_minute(),
    );
    batch(|| {
        ui.list.set(list);
        ui.focus.set(focus);
    });
}

fn restored_list(
    kept: Kept,
    catalog: Rc<dyn Schedule>,
    favorites: &[usize],
    recent: &[usize],
    now: i32,
) -> (Focus, Rc<ChannelList>) {
    let mut focus = Focus::at(now);
    focus.visible = kept.focus.visible;
    focus.pane = kept.focus.pane;
    focus.rail = kept.focus.rail;
    focus.group = kept.list.find(&*catalog);
    let list = Rc::new(ChannelList::build(
        focus.group,
        &*catalog,
        favorites,
        recent,
    ));
    let view = ListView::new(catalog, list.clone());
    let row = kept.key.and_then(|key| view.find_channel(&key));
    focus.restore(row, &view);
    (focus, list)
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

/// Keep preview focus on a visible, playing surface across stop/layout changes.
pub fn bind(ui: Ui) {
    effect(move || {
        let available = (ui.playing.get().is_some() || crate::on_demand::active(ui))
            && matches!(
                ui.mode.get(),
                crate::ViewMode::Guide | crate::ViewMode::Fullscreen
            );
        if ui.focus.with(|f| f.pane == Pane::Preview) && !available {
            ui.focus.update(|f| f.retain_preview(false));
        }
    });
}

/// A direction on the guide screen.
pub fn navigate(ui: Ui, m: Move) {
    if m == Move::Up
        && nsh_core::prelude::idiom() == nsh_core::platform::Idiom::Tv
        && ui.mode.get_untracked() == crate::ViewMode::Guide
    {
        let mut moved = false;
        ui.focus.update(|f| {
            moved = f.focus_preview(
                ui.playing.get_untracked().is_some() || crate::on_demand::active(ui),
            );
        });
        if moved {
            return;
        }
    }
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
        Outcome::ReturnToPlayer
            if ui.playing.get_untracked().is_some() || crate::on_demand::active(ui) =>
        {
            ui.mode.set(crate::ViewMode::Fullscreen);
            ui.show_playing();
        }
        Outcome::Open(item) => open(ui, item),
        _ => {}
    }
}

fn open(ui: Ui, item: RailItem) {
    match item {
        RailItem::Search => crate::search_view::open(ui),
        RailItem::Movies => crate::vod_view::open(ui, iptv_core::vod::Kind::Movie),
        RailItem::Shows => crate::vod_view::open(ui, iptv_core::vod::Kind::Series),
        RailItem::Recordings => crate::recordings_view::open(ui),
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
    ui.focus.with(|f| f.columns_shown())
}

fn toggle_playlist(entry: usize) -> bool {
    match Entry::at(entry) {
        Entry::Group(group) => crate::data::combined::toggle_playlist(&crate::runtime(), group),
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

#[cfg(test)]
mod identity_tests {
    use super::*;
    use iptv_core::lists::{FIRST_GROUP, RECENT};
    use iptv_guide::{EpgSchedule, GuideChannel, Organization};

    fn catalog(groups: &[(&str, &[&str])]) -> EpgSchedule {
        EpgSchedule::new(
            groups
                .iter()
                .map(|(group, keys)| {
                    (
                        (*group).to_owned(),
                        keys.iter()
                            .map(|key| GuideChannel {
                                key: (*key).into(),
                                name: format!("Generated {key}"),
                                number: 1,
                                catchup_days: 0,
                            })
                            .collect(),
                    )
                })
                .collect(),
            iptv_epg::GuideStore::default(),
            1_800_000_000,
        )
    }

    fn keys(catalog: Rc<dyn Schedule>, list: Rc<ChannelList>) -> Vec<String> {
        let view = ListView::new(catalog, list);
        (0..view.channel_count())
            .map(|row| view.channel_key(row).unwrap())
            .collect()
    }

    #[test]
    fn popup_list_restores_group_and_channel_identity_after_reorder_and_hide() {
        let before: Rc<dyn Schedule> =
            Rc::new(catalog(&[("News", &["a", "b"]), ("Sport", &["c"])]));
        let mut focus = Focus::at(0);
        focus.group = FIRST_GROUP;
        focus.channel = 1;
        let list = Rc::new(ChannelList::build(focus.group, &*before, &[], &[]));
        let kept = keep_list(focus, list, before);
        // Both catalogue row indices and the group's position change. The
        // remaining focused channel is retained; a hidden member disappears.
        let after = catalog(&[("Sport", &["c"]), ("News", &["b", "a"])]);
        after.set_organization(Organization {
            channels: vec![("a".into(), "Generated a".into(), true)],
            ..Default::default()
        });
        let after: Rc<dyn Schedule> = Rc::new(after);
        let (focus, list) = restored_list(kept, after.clone(), &[], &[], 60);
        assert_eq!(focus.group, FIRST_GROUP + 1);
        assert_eq!(focus.now, 60);
        assert_eq!(keys(after.clone(), list.clone()), ["b"]);
        let view = ListView::new(after, list);
        assert_eq!(view.channel_key(focus.channel).as_deref(), Some("b"));
    }

    #[test]
    fn popup_dynamic_lists_follow_edits_without_changing_selected_content() {
        let catalog: Rc<dyn Schedule> = Rc::new(catalog(&[("Generated", &["a", "b", "c"])]));
        for (entry, changed) in [(FAVORITES, vec![2, 1]), (RECENT, vec![2, 0, 1])] {
            let mut focus = Focus::at(0);
            focus.group = entry;
            focus.channel = 1;
            let list = Rc::new(ChannelList::build(entry, &*catalog, &[0, 1], &[0, 1]));
            let kept = keep_list(focus, list, catalog.clone());
            let (focus, list) = restored_list(kept, catalog.clone(), &changed, &changed, 0);
            let view = ListView::new(catalog.clone(), list.clone());
            assert_eq!(view.channel_key(focus.channel).as_deref(), Some("b"));
            assert_eq!(
                keys(catalog.clone(), list.clone())
                    .first()
                    .map(String::as_str),
                Some("c")
            );
            // Removing the focused item must remove its stale list row too.
            let kept = keep_list(focus, list, catalog.clone());
            let (_, list) = restored_list(kept, catalog.clone(), &[2], &[2], 0);
            assert_eq!(keys(catalog.clone(), list), ["c"]);
        }
    }
}
