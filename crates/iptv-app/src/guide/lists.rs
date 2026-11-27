//! The list the guide shows (`iptv_core::lists`) and the guide screen's
//! keys across the rail, the groups column and the grid
//! (`iptv_presentation::browse`).
//!
//! `crate::wiring::runtime_owner::schedule()` stays the full catalog: `Ui::playing`, `Ui::play`,
//! search and the player use catalog channels. The focus works on rows of
//! [`Ui::list`] through [`view`]; [`focused_channel`] maps back. The list
//! is rebuilt when the selection, the catalog or the favorites change,
//! and the focus stays on its channel by key.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use iptv_core::guide::{GroupName, Schedule};
use iptv_core::lists::group_of;
use iptv_presentation::browse::{Columns, Outcome, RailItem, VodAvailability, rail_items};
use iptv_presentation::channel_list::ChannelList as PlayerList;
use iptv_presentation::guide::{Focus, Move, Pane};
use iptv_presentation::lists::{
    ALL, ChannelList, Entry, ListId, ListView, VisibleEntries, entry_count, resume_entry,
};
use iptv_settings::{Settings, Target, keys};
use nsh_core::prelude::*;

use crate::Ui;

/// How long the hint card stays after a group was chosen.
const HINT: Duration = Duration::from_secs(6);

thread_local! {
    /// Bumped per hint shown: an older timer leaves a newer hint alone.
    static HINT_SHOWN: Cell<u64> = const { Cell::new(0) };
}

/// The list on screen as a schedule of rows.
pub(crate) fn view(guide: crate::GuideUi) -> ListView {
    ListView::new(
        crate::wiring::runtime_owner::schedule(),
        guide.list.with(Rc::clone),
    )
}

/// The catalog channel on the focused row.
pub(crate) fn focused_channel(guide: crate::GuideUi) -> Option<usize> {
    let row = guide.focus.with(|f| f.channel);
    guide.list.with(|l| l.channel(row))
}

/// The number catalog `channel` shows (the player card, History): its
/// place in the list on screen, as in the guide, else the catalog's.
pub(crate) fn number(guide: crate::GuideUi, channel: usize) -> u32 {
    let catalog = crate::wiring::runtime_owner::schedule();
    guide.list.with(|l| l.number_of(channel, &*catalog))
}

/// Catalogs stay visible unless every enabled playlist is M3U; discovery
/// shelves follow enabled playlists with imported movies or shows.
pub fn rail(ui: Ui) -> Vec<RailItem> {
    ui.app.library.version.get();
    rail_of(
        VodAvailability {
            shown: ui.app.has_vod.get(),
            any: ui.app.any_vod.get(),
            live_only: live_only(),
        },
        crate::wiring::capabilities::recordings_library(ui),
    )
}

fn rail_untracked(ui: Ui) -> Vec<RailItem> {
    rail_of(
        VodAvailability {
            shown: ui.app.has_vod.get_untracked(),
            any: ui.app.any_vod.get_untracked(),
            live_only: live_only(),
        },
        crate::wiring::capabilities::recordings_library_untracked(ui),
    )
}

/// Recordings only where the device shows the library
/// (`capabilities::recordings_library`).
fn rail_of(vod: VodAvailability, recordings: bool) -> Vec<RailItem> {
    let mut items = rail_items(vod);
    if !recordings {
        items.retain(|&item| item != RailItem::Recordings);
    }
    items
}

/// Whether every enabled playlist is M3U ([`iptv_presentation::browse::live_only`]),
/// as the library last loaded them (none while it loads). Callers that
/// follow it track `Library::version`.
pub fn live_only() -> bool {
    let playlists = crate::wiring::runtime_owner::runtime()
        .playlists()
        .unwrap_or_default();
    iptv_presentation::browse::live_only(playlists.iter().map(|p| (p.kind, p.policy.enabled)))
}

/// Localize built-in lists by identity; provider group names are opaque.
pub fn entry_name(entry: usize, catalog: &dyn Schedule) -> String {
    crate::guide::listing_labels::group(iptv_presentation::lists::entry_name(entry, catalog))
}

/// The name of the list on screen.
pub fn list_name(ui: Ui) -> String {
    ui.app.data_version.get();
    let entry = ui.guide.list.with(|l| l.entry());
    entry_name(entry, &*crate::wiring::runtime_owner::schedule())
}

pub(crate) fn category_preferences(settings: &Settings) -> [bool; 3] {
    [
        keys::SHOW_FAVORITES,
        keys::SHOW_RECENT,
        keys::SHOW_ALL_CHANNELS,
    ]
    .map(|key| settings.bool(key, &Target::Global))
}

pub(crate) fn entries_in(ui: Ui, catalog: &dyn Schedule) -> VisibleEntries {
    let mut visible = category_preferences(&crate::wiring::settings_wiring::read(ui));
    // The combined view has its own aggregate, All playlists.
    visible[ALL] |= catalog.all_channels_label() == GroupName::AllPlaylists;
    VisibleEntries::new(entry_count(catalog), visible)
}

pub(crate) fn visible_entries(ui: Ui) -> VisibleEntries {
    ui.guide_data();
    entries_in(ui, &*crate::wiring::runtime_owner::schedule())
}

/// Builds the list of the selected entry. With `keep`, the focus stays on
/// its channel (the content changed under it); a newly selected list
/// starts where the core put the focus (its top).
pub fn rebuild(ui: Ui, keep: bool) {
    rebuild_in(ui, keep, None);
}

fn rebuild_in(ui: Ui, keep: bool, explicit_entry: Option<usize>) {
    let key = if keep {
        let v = view(ui.guide);
        ui.guide.focus.with(|f| v.channel_key(f.channel))
    } else {
        None
    };
    let catalog = crate::wiring::runtime_owner::schedule();
    let player = keep_player_list(ui, &*catalog);
    let entries = entries_in(ui, &*catalog);
    let entry = explicit_entry.unwrap_or_else(|| {
        ui.guide
            .focus
            .with(|f| entries.retain_selection(f.group, &*catalog, key.as_deref()))
    });
    let list = ChannelList::build(
        entry,
        &*catalog,
        &crate::guide::favorites::favorite_channels(),
        &crate::guide::favorites::recent_channels(),
    );
    let list = Rc::new(list);
    let view = ListView::new(catalog, list.clone());
    let row = key.and_then(|k| view.find_channel(&k));
    batch(|| {
        ui.guide.list.set(list);
        ui.guide.focus.update(|f| {
            f.group = entry;
            f.restore(row, &view);
        });
        restore_player_list(ui, player, &view, entries);
    });
}

/// The favorites changed: the Favorites list follows if it is shown.
pub fn favorites_changed(ui: Ui) {
    let entry = Entry::at(ui.guide.focus.with_untracked(|f| f.group));
    if matches!(entry, Entry::Favorites | Entry::Recent)
        || matches!(entry, Entry::Group(group) if crate::wiring::runtime_owner::schedule().group_preferences(group).favorites_only)
    {
        rebuild(ui, true);
    }
}

/// What the guide shows, by content, to find again in a new catalog.
pub struct Kept {
    list: ListId,
    key: Option<String>,
    focus: Focus,
    player: Option<KeptPlayerList>,
}

/// Resolve old rows against the catalog that built the list, which the
/// runtime may have replaced already.
pub(crate) fn keep_in(ui: Ui, catalog: Rc<dyn Schedule>) -> Kept {
    let player = keep_player_list(ui, &*catalog);
    let mut kept = keep_list(
        ui.guide.focus.get_untracked(),
        ui.guide.list.get_untracked(),
        catalog,
    );
    kept.player = player;
    kept
}

fn keep_list(focus: Focus, list: Rc<ChannelList>, catalog: Rc<dyn Schedule>) -> Kept {
    let view = ListView::new(catalog, list);
    Kept {
        list: ListId::of(focus.group, &**view.catalog()),
        key: view.channel_key(focus.channel),
        focus,
        player: None,
    }
}

/// After a catalog swap: the same list and channel by identity, at the new
/// catalog's now, with the same column focused.
pub(crate) fn restore(ui: Ui, mut kept: Kept, catalog: Rc<dyn Schedule>) {
    let player = kept.player.take();
    let now = crate::guide::clock::now_minute_in(&*catalog);
    let entries = entries_in(ui, &*catalog);
    let (mut focus, list) = restored_list(
        kept,
        catalog.clone(),
        &crate::guide::favorites::favorite_channels(),
        &crate::guide::favorites::recent_channels(),
        now,
        entries,
    );
    focus.retain_rail(&rail_untracked(ui));
    let view = ListView::new(catalog, list.clone());
    batch(|| {
        ui.guide.list.set(list);
        ui.guide.focus.set(focus);
        restore_player_list(ui, player, &view, entries);
    });
}

fn restored_list(
    kept: Kept,
    catalog: Rc<dyn Schedule>,
    favorites: &[usize],
    recent: &[usize],
    now: i32,
    entries: VisibleEntries,
) -> (Focus, Rc<ChannelList>) {
    let mut focus = Focus::at(now);
    focus.visible = kept.focus.visible;
    focus.pane = kept.focus.pane;
    focus.rail = kept.focus.rail;
    focus.group =
        entries.retain_selection(kept.list.find(&*catalog), &*catalog, kept.key.as_deref());
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

struct KeptPlayerList {
    state: PlayerList,
    entry: ListId,
    key: Option<String>,
}

fn keep_player_list(ui: Ui, catalog: &dyn Schedule) -> Option<KeptPlayerList> {
    let state = ui.playback.channels.get_untracked()?;
    let key = ui
        .guide
        .list
        .with_untracked(|list| list.channel(state.row))
        .and_then(|channel| catalog.channel_key(channel));
    Some(KeptPlayerList {
        state,
        entry: ListId::of(state.entry, catalog),
        key,
    })
}

// A missing group falls back to All channels. A missing channel starts at the
// first row of the restored list, or has no selectable row when it is empty.
fn restore_player_list(
    ui: Ui,
    kept: Option<KeptPlayerList>,
    view: &ListView,
    entries: VisibleEntries,
) {
    if let Some(kept) = kept {
        let mut state = kept.state;
        let entry = kept.entry.find(&**view.catalog());
        state.entry = if entries.position(entry).is_some() {
            entry
        } else {
            view.list().entry()
        };
        state.row = kept
            .key
            .and_then(|key| view.find_channel(&key))
            .unwrap_or(0);
        ui.playback.channels.set(Some(state));
    }
}

/// The row of catalog `channel`, selecting the channel's own group first
/// when the list on screen does not have it.
fn listed(ui: Ui, channel: usize) -> usize {
    if let Some(row) = ui.guide.list.with_untracked(|l| l.row_of(channel)) {
        return row;
    }
    let catalog = crate::wiring::runtime_owner::schedule();
    let entry = group_of(&*catalog, channel).map_or(ALL, |g| Entry::Group(g).index());
    // A direct selection (for example from Search) can reach an ungrouped
    // channel through the aggregate even when its category tab is hidden.
    rebuild_in(ui, false, Some(entry));
    ui.guide
        .list
        .with_untracked(|l| l.row_of(channel))
        .unwrap_or(0)
}

/// Shows what plays on catalog `channel` (a zap, a resumed or previous
/// channel): its row, its live program, the default window.
pub fn show_channel(ui: Ui, channel: usize) {
    let row = listed(ui, channel);
    let view = view(ui.guide);
    ui.guide.focus.update(|f| f.show_channel(row, &view));
}

/// The channel resumed at startup (`player::source_ready`,
/// `resume::adopt`), once it plays: shown in the list the core puts it in
/// ([`resume_entry`]), so Back from full screen lands on it; else where
/// [`show_channel`] shows any channel. Decided once: later favorite
/// edits and the viewer's own choices are left alone.
pub fn show_resumed(ui: Ui, channel: usize) {
    if !into_resume_list(ui, channel) {
        show_channel(ui, channel);
    }
}

/// The combined playlists' resumed channel, once it plays: as
/// [`show_resumed`], except that its usual place keeps the focus where it
/// was.
pub fn resumed(ui: Ui, channel: usize) {
    into_resume_list(ui, channel);
}

/// Selects the list the core puts a resumed `channel` in and focuses the
/// channel when that list has its row. False when the core leaves the
/// usual place.
fn into_resume_list(ui: Ui, channel: usize) -> bool {
    let catalog = crate::wiring::runtime_owner::schedule();
    let Some(entry) = resume_entry(
        channel,
        &*catalog,
        &crate::guide::favorites::favorite_channels(),
        crate::wiring::settings::flag(iptv_settings::keys::SHOW_FAVORITES),
    ) else {
        return false;
    };
    if ui.guide.focus.with_untracked(|f| f.group) != entry {
        ui.guide.focus.update(|f| f.group = entry);
        rebuild(ui, false);
    }
    if ui
        .guide
        .list
        .with_untracked(|l| l.row_of(channel))
        .is_some()
    {
        show_channel(ui, channel);
    }
    true
}

/// Focuses catalog `channel`'s program at `minute` where it is (a search
/// result, a closed menu).
pub fn tap(ui: Ui, channel: usize, minute: i32) {
    let row = listed(ui, channel);
    let view = view(ui.guide);
    ui.guide.focus.update(|f| f.tap(row, minute, &view));
}

/// Runs a focus step with the columns and the list on screen.
fn step(ui: Ui, f: impl FnOnce(&mut Focus, &Columns, &ListView) -> Outcome) -> Outcome {
    let rail = rail_untracked(ui);
    let view = view(ui.guide);
    let cols = Columns {
        entries: entries_in(ui, &**view.catalog()),
        rail: &rail,
        // Always: `guide.back_returns_to_now` is retired (#281).
        back_to_now: true,
    };
    let mut outcome = Outcome::None;
    ui.guide
        .focus
        .update(|focus| outcome = f(focus, &cols, &view));
    outcome
}

/// Reconcile the selected category in each window when visibility changes.
pub(crate) fn bind_categories(ui: Ui) {
    let categories =
        computed(move || category_preferences(&crate::wiring::settings_wiring::read(ui)));
    effect(move || {
        categories.get();
        untracked(|| {
            let catalog = crate::wiring::runtime_owner::schedule();
            // The real catalog's restore chooses the initial category.
            if catalog.channel_count() == 0 {
                return;
            }
            let entries = entries_in(ui, &*catalog);
            let invalid = |entry| entries.retain(entry) != entry;
            if ui.guide.focus.with(|f| invalid(f.group))
                || ui
                    .playback
                    .channels
                    .with(|list| list.is_some_and(|list| invalid(list.entry)))
            {
                rebuild(ui, true);
            }
        });
    });
}

/// Keep preview focus on a visible, playing surface across stop/layout changes.
pub fn bind(ui: Ui) {
    bind_categories(ui);
    effect(move || {
        let available = (ui.playback.playing.get().is_some()
            || crate::playback::on_demand::active(ui))
            && matches!(
                ui.chrome.mode.get(),
                crate::ViewMode::Guide | crate::ViewMode::Fullscreen
            );
        if ui.guide.focus.with(|f| f.pane == Pane::Preview) && !available {
            let view = view(ui.guide);
            ui.guide.focus.update(|f| {
                f.retain_preview(false);
                f.restore(None, &view);
            });
        }
    });
}

/// A direction on the guide screen.
pub fn navigate(ui: Ui, m: Move) {
    if m == Move::Up
        && matches!(
            nsh_core::prelude::idiom(),
            nsh_core::platform::Idiom::Tv | nsh_core::platform::Idiom::Desktop
        )
        && ui.chrome.mode.get_untracked() == crate::ViewMode::Guide
    {
        let mut moved = false;
        ui.guide.focus.update(|f| {
            moved = f.focus_preview(
                ui.playback.playing.get_untracked().is_some()
                    || crate::playback::on_demand::active(ui),
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
    if ui.guide.focus.with_untracked(|f| f.pane) == Pane::Groups
        && toggle_playlist(ui.guide.focus.with_untracked(|f| f.group))
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

/// Shows the groups column (the Groups key or the guide's corner Back control).
pub fn open_groups(ui: Ui) {
    ui.guide.focus.update(Focus::open_groups);
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
    if o != Outcome::None {
        toggle_playlist(entry);
    }
}

/// Activates a group from the desktop's pointer. Playlist section
/// headings fold or unfold, as they do on Enter.
pub fn activate_entry(ui: Ui, entry: usize) {
    let outcome = step(ui, |f, cols, _| f.tap_entry(entry, cols));
    follow(ui, outcome);
    ok(ui);
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
        Outcome::Committed if ui.guide.focus.with_untracked(|f| f.hint) => arm_hint(ui),
        Outcome::Watch => ui.watch(),
        Outcome::ReturnToPlayer
            if ui.playback.playing.get_untracked().is_some()
                || crate::playback::on_demand::active(ui) =>
        {
            ui.chrome
                .mode
                .apply(crate::chrome::mode::Action::EnterFullscreen);
            ui.show_playing();
        }
        Outcome::Open(item) => open(ui, item),
        _ => {}
    }
}

fn open(ui: Ui, item: RailItem) {
    match item {
        RailItem::Search => crate::screens::search_view::open(ui),
        RailItem::Continue => {
            crate::screens::vod_view::open_shelf(ui, iptv_core::vod::Shelf::ContinueWatching)
        }
        RailItem::Recent => {
            crate::screens::vod_view::open_shelf(ui, iptv_core::vod::Shelf::RecentlyAdded)
        }
        RailItem::Movies => crate::screens::vod_view::open(ui, iptv_core::vod::Kind::Movie),
        RailItem::Shows => crate::screens::vod_view::open(ui, iptv_core::vod::Kind::Series),
        RailItem::Recordings => crate::screens::recordings_view::open(ui),
        RailItem::Settings => crate::screens::settings_view::open(ui),
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
        if HINT_SHOWN.with(Cell::get) == shown && ui.guide.focus.with_untracked(|f| f.hint) {
            ui.guide.focus.update(|f| f.hint = false);
        }
    });
}

/// Whether the rail and groups show beside the guide.
pub fn columns_shown(ui: Ui) -> bool {
    ui.guide.focus.with(|f| f.columns_shown())
}

fn toggle_playlist(entry: usize) -> bool {
    match Entry::at(entry) {
        Entry::Group(group) => {
            crate::wiring::data::toggle_playlist(&crate::wiring::runtime_owner::runtime(), group)
        }
        _ => false,
    }
}

/// Expansion state of a playlist heading in a combined guide.
pub(crate) fn playlist_expanded(entry: usize, schedule: &dyn Schedule) -> Option<bool> {
    match Entry::at(entry) {
        Entry::Group(group) if schedule.group_is_playlist(group) => {
            Some(!schedule.group_is_collapsed(group))
        }
        _ => None,
    }
}

/// The disclosure triangle of a playlist heading: ▶ folded, ▼ open.
pub(crate) fn playlist_arrow(expanded: Option<bool>) -> &'static str {
    match expanded {
        Some(false) => "▶",
        Some(true) => "▼",
        None => "",
    }
}

pub(crate) fn browser_entry_name(entry: usize, schedule: &dyn Schedule) -> String {
    let name = entry_name(entry, schedule);
    if let Entry::Group(group) = Entry::at(entry)
        && schedule.group_depth(group) > 0
    {
        return format!("    {name}");
    }
    name
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use iptv_guide::{EpgSchedule, GuideChannel, Organization};
    use iptv_presentation::lists::{FAVORITES, FIRST_GROUP, RECENT};

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
        // Both catalog row indices and the group's position change. The
        // remaining focused channel is retained; a hidden member disappears.
        let after = catalog(&[("Sport", &["c"]), ("News", &["b", "a"])]);
        after.set_organization(Organization {
            channels: vec![("a".into(), "Generated a".into(), true)],
            ..Default::default()
        });
        let after: Rc<dyn Schedule> = Rc::new(after);
        let (focus, list) = restored_list(
            kept,
            after.clone(),
            &[],
            &[],
            60,
            VisibleEntries::all(entry_count(&*after)),
        );
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
            let (focus, list) = restored_list(
                kept,
                catalog.clone(),
                &changed,
                &changed,
                0,
                VisibleEntries::all(entry_count(&*catalog)),
            );
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
            let (_, list) = restored_list(
                kept,
                catalog.clone(),
                &[2],
                &[2],
                0,
                VisibleEntries::all(entry_count(&*catalog)),
            );
            assert_eq!(keys(catalog.clone(), list), ["c"]);
        }
    }

    #[test]
    fn preview_bookmark_restores_search_and_channel_by_identity() {
        let before: Rc<dyn Schedule> = Rc::new(catalog(&[("Generated", &["a", "b"])]));
        let mut focus = Focus::at(0);
        focus.pane = Pane::Rail;
        focus.rail = RailItem::Search;
        focus.channel = 1;
        let list = Rc::new(ChannelList::build(focus.group, &*before, &[0, 1], &[]));
        let key = ListView::new(before.clone(), list.clone()).channel_key(focus.channel);
        assert!(focus.focus_preview(true));
        let kept = keep_list(focus, list, before);
        let after: Rc<dyn Schedule> = Rc::new(catalog(&[("Generated", &["b", "a"])]));
        let (mut restored, list) = restored_list(
            kept,
            after.clone(),
            &[0, 1],
            &[],
            60,
            VisibleEntries::all(entry_count(&*after)),
        );
        assert_eq!(restored.pane, Pane::Preview);
        assert_eq!(restored.layout_pane(), Pane::Rail);
        assert_eq!(
            ListView::new(after, list).channel_key(restored.channel),
            key
        );
        restored.retain_preview(false);
        assert_eq!(restored.pane, Pane::Rail, "Stop after a catalog refresh");
        assert_eq!(restored.rail, RailItem::Search);
    }

    #[test]
    fn a_catalog_bookmark_retains_rail_action_identity_as_vod_rows_change() {
        for item in [RailItem::Recordings, RailItem::Settings, RailItem::Movies] {
            let before: Rc<dyn Schedule> = Rc::new(catalog(&[("Generated", &["a", "b"])]));
            let mut focus = Focus::at(0);
            focus.pane = Pane::Rail;
            focus.rail = item;
            focus.channel = 1;
            let list = Rc::new(ChannelList::build(focus.group, &*before, &[], &[]));
            let kept = keep_list(focus, list, before);
            let after: Rc<dyn Schedule> = Rc::new(catalog(&[("Generated", &["b", "a"])]));
            let (mut restored, list) = restored_list(
                kept,
                after.clone(),
                &[],
                &[],
                60,
                VisibleEntries::all(entry_count(&*after)),
            );
            assert_eq!(
                restored.rail, item,
                "bookmark stores the action, not its old position"
            );
            restored.retain_rail(&rail_items(VodAvailability {
                shown: true,
                any: true,
                live_only: false,
            }));
            assert_eq!(restored.rail, item);
            restored.retain_rail(&rail_items(VodAvailability::default()));
            assert_eq!(restored.rail, item, "catalogs remain available without VOD");
            assert_eq!(restored.pane, Pane::Rail);
            assert_eq!(
                ListView::new(after, list)
                    .channel_key(restored.channel)
                    .as_deref(),
                Some("b")
            );
        }
    }
}
