#![forbid(unsafe_code)]
//! The guide schedule built from real data: imported channels and groups
//! (`iptv-providers`) plus the EPG store (`iptv-epg`), behind
//! `iptv_core::guide::Schedule` so the focus model and the UI work on it
//! exactly as on the synthetic schedule.
//!
//! Guide minutes count from the schedule's epoch: the import's `now`
//! floored to the hour (UTC), fixed for the schedule's lifetime. "Now" is
//! the wall clock ([`Schedule::follows_wall_clock`]), so the app advances it
//! with [`iptv_core::guide::now_minute`] without rebuilding the schedule.
//! Where a channel has no programme, the
//! gap is filled with hour-aligned placeholder slots clipped to the
//! neighbouring programmes, so navigation always has a cell to land on.
//!
//! A stored guide can be held in part: the store passed in covers a recent
//! window, and older days are paged in on demand ([`pages`],
//! [`EpgSchedule::with_pages`]). Until a day arrives its cells are
//! hour slots titled [`LOADING`]. A cold start's first schedule may hold
//! even less, only the hours of its first screen
//! ([`EpgSchedule::held_only`]): the rest shows as loading until a schedule
//! with the whole window replaces it.

pub mod combined;
mod held;
pub mod pages;
pub mod playback;

use pages::{PageLoader, PageRequest, PagedWindow, Pages};

use iptv_core::guide::{Program, Schedule};
use iptv_epg::{ChannelKey, GuideStore, UnixTime};
use iptv_providers::model::{Channel, Group};

/// Gap slots, and the epoch, are aligned to the hour.
const SLOT: i64 = 3600;
/// Minutes the guide can be browsed past its last programme.
pub const PAST_END: i32 = 60;
/// Title of a gap slot.
pub const NO_INFORMATION: &str = "No information";
/// Title of a slot whose day is still being read from the store.
pub const LOADING: &str = "Loading…";
/// A channel in no group (`EpgSchedule::group_index`).
const NO_GROUP: u32 = u32::MAX;

/// A channel as the guide needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuideChannel {
    /// The key the EPG store matched the channel under (its stable id).
    pub key: ChannelKey,
    pub name: String,
    /// The number shown before the name (`tvg-chno`, or the playlist position).
    pub number: u32,
    pub catchup_days: u8,
}

/// User customization in stable provider/content identity order.
#[derive(Clone, Debug, Default)]
pub struct Organization {
    pub channels: Vec<(String, String, bool)>,
    pub groups: Vec<(String, String, bool)>,
}

#[derive(Clone, Default)]
struct OrganizationState {
    names: Vec<String>,
    hidden: Vec<bool>,
    order: Vec<usize>,
    groups: Vec<(usize, String)>,
    lists: Option<Vec<(String, String, Vec<usize>)>>,
}
#[derive(Clone)]
pub struct EpgSchedule {
    organization: std::cell::RefCell<Option<std::sync::Arc<OrganizationState>>>,
    /// Each channel by key, made on the first lookup: channels never change.
    key_index: std::rc::Rc<std::cell::OnceCell<std::collections::HashMap<String, usize>>>,
    /// Each channel's group (`NO_GROUP` for none), made on the first lookup
    /// after the groups last changed ([`Self::set_organization`],
    /// [`Self::set_group_lists`]).
    group_index: std::cell::RefCell<Option<Vec<u32>>>,
    group_keys: std::sync::Arc<Vec<String>>,
    channels: std::sync::Arc<Vec<GuideChannel>>,
    /// The imported channel behind each guide channel (stream and catch-up).
    sources: std::sync::Arc<Vec<Option<Channel>>>,
    group_names: std::sync::Arc<Vec<String>>,
    /// The index of each group's first channel; channels are stored grouped.
    group_starts: std::sync::Arc<Vec<usize>>,
    /// The channels in the playlist's order (empty: as stored).
    playlist_order: std::sync::Arc<Vec<usize>>,
    store: std::sync::Arc<GuideStore>,
    /// Unix time of guide minute 0.
    epoch: UnixTime,
    /// Reads a description the in-memory store left out.
    describe: Option<std::rc::Rc<DescriptionLookup>>,
    /// Older days than `store` holds, read in on demand.
    pages: std::rc::Rc<Pages>,
    /// When the first programme in `store` starts and the last one ends.
    data_start: Option<UnixTime>,
    data_end: Option<UnixTime>,
    /// The only times `store` holds, when it holds less than its window
    /// (a cold start's first screen): `[from, to)`.
    held: Option<(UnixTime, UnixTime)>,
    /// The only channels `store` holds then (`None`: every bound one).
    held_keys: Option<std::collections::HashSet<ChannelKey>>,
    /// The source's account or session, for streams that need one.
    /// Shared with the next schedule of the same source (a refreshed guide),
    /// so a Stalker session renewed by one is the other's too.
    playback: Option<std::rc::Rc<playback::PlaybackContext>>,
}

/// Each channel's group slot (its first group's position in `groups`;
/// `groups.len()` for none) with its index, in playlist order.
fn group_slots<'a>(
    groups: &'a [Group],
    channels: &'a [Channel],
) -> impl Iterator<Item = (usize, usize)> + 'a {
    let at: std::collections::HashMap<&iptv_providers::GroupId, usize> = groups
        .iter()
        .enumerate()
        .rev()
        .map(|(i, g)| (&g.id, i))
        .collect();
    channels.iter().enumerate().map(move |(i, c)| {
        let slot = c.groups.first().and_then(|gid| at.get(gid).copied());
        (slot.unwrap_or(groups.len()), i)
    })
}

/// Reads the description of the programme starting at the given time on a
/// channel, e.g. from the SQLite store (`Store::epg_description`).
pub type DescriptionLookup = Box<dyn Fn(&ChannelKey, UnixTime) -> Option<String>>;

impl EpgSchedule {
    /// Display lists refer to the existing catalog by stable channel key.
    /// Provider groups can have independent order; custom groups share the
    /// same channels, guide data and player streams without duplication.
    pub fn set_group_lists(&self, lists: Vec<(String, String, Vec<String>)>) {
        self.group_index.take();
        if let Some(s) = self.organization.borrow_mut().as_mut() {
            std::sync::Arc::make_mut(s).lists = Some(self.shape().lists(lists));
        }
    }
    /// `groups`: names with their channels, in display order. The epoch is
    /// `now` rounded down to the hour. A group's name is its key.
    pub fn new(groups: Vec<(String, Vec<GuideChannel>)>, store: GuideStore, now: UnixTime) -> Self {
        let keyed = groups
            .into_iter()
            .map(|(name, members)| (name.clone(), name, members));
        Self::from_parts(Catalog::keyed(keyed, store, now))
    }

    /// A schedule of `catalog`, with nothing that cannot cross threads
    /// added yet (playback, pages, descriptions).
    fn from_parts(catalog: Catalog) -> Self {
        let Catalog {
            organization,
            group_keys,
            channels,
            sources,
            group_names,
            group_starts,
            playlist_order,
            store,
            epoch,
            data_start,
            data_end,
        } = catalog;
        EpgSchedule {
            organization: std::cell::RefCell::new(organization.map(std::sync::Arc::new)),
            key_index: Default::default(),
            group_index: Default::default(),
            group_keys: std::sync::Arc::new(group_keys),
            channels: std::sync::Arc::new(channels),
            sources: std::sync::Arc::new(sources),
            group_names: std::sync::Arc::new(group_names),
            group_starts: std::sync::Arc::new(group_starts),
            playlist_order: std::sync::Arc::new(playlist_order),
            store: std::sync::Arc::new(store),
            epoch,
            describe: None,
            pages: std::rc::Rc::new(Pages::default()),
            data_start,
            data_end,
            held: None,
            held_keys: None,
            playback: None,
        }
    }

    fn shape(&self) -> Shape<'_> {
        Shape {
            channels: &self.channels,
            sources: &self.sources,
            playlist_order: &self.playlist_order,
            group_keys: &self.group_keys,
            group_names: &self.group_names,
        }
    }

    /// `store` holds the guide from `window.loaded_from` on; earlier days
    /// are read in when first needed: `loader` reads one off the UI thread
    /// and hands it to [`Self::insert_page`].
    pub fn with_pages(
        mut self,
        window: PagedWindow,
        loader: impl Fn(PageRequest) + 'static,
    ) -> Self {
        let loader: PageLoader = Box::new(loader);
        self.pages = std::rc::Rc::new(Pages::new(window, loader));
        self
    }

    /// Where `store` starts holding the guide (`None`: it holds all of it).
    pub fn loaded_from(&self) -> Option<UnixTime> {
        let from = self.pages.loaded_from();
        (from != UnixTime::MIN).then_some(from)
    }

    /// A page read for `request`; false when it is no longer wanted (the
    /// user went back to now) or is not this schedule's.
    pub fn insert_page(&self, request: PageRequest, store: GuideStore) -> bool {
        self.pages.insert(request, store)
    }

    /// Whether the programmes around `t` are in memory; asks for their day
    /// when they are not.
    pub fn ensure_loaded(&self, t: UnixTime) -> bool {
        self.with_store(t, |_| ()).is_some()
    }

    /// Drops the older days read in (except one pinned with
    /// [`Self::pin_page`]); returns how many went.
    pub fn evict_pages(&self) -> usize {
        self.pages.evict()
    }

    /// Keeps the day holding `t` in memory (the programme playing from the
    /// archive); `None` keeps none.
    pub fn pin_page(&self, t: Option<UnixTime>) {
        self.pages.pin(t);
    }

    /// Older days held in memory.
    pub fn paged_days(&self) -> usize {
        self.pages.loaded()
    }

    /// `f` on the store holding the programmes around `t`; `None` while
    /// its day is being read.
    fn with_store<R>(&self, t: UnixTime, f: impl FnOnce(&GuideStore) -> R) -> Option<R> {
        if let Some((from, to)) = self.held
            && !(from..to).contains(&t)
        {
            return None;
        }
        if self.pages.in_window(t) {
            Some(f(&self.store))
        } else {
            self.pages.with(t, f)
        }
    }

    /// Descriptions not held in memory (a guide loaded from the store
    /// without them) are read with `lookup(key, programme start)`.
    pub fn with_description_lookup(
        mut self,
        lookup: impl Fn(&ChannelKey, UnixTime) -> Option<String> + 'static,
    ) -> Self {
        self.describe = Some(std::rc::Rc::new(Box::new(lookup)));
        self
    }

    /// From an imported catalog: each channel in its first group (in the
    /// catalog's group order), channels without a group last under "Other".
    /// A group id listed twice is one group, the first listed: its
    /// channels are there, and the repeat holds none.
    pub fn from_catalog(
        groups: &[Group],
        channels: &[Channel],
        store: GuideStore,
        now: UnixTime,
    ) -> Self {
        Self::from_parts(Catalog::from_catalog(groups, channels, store, now))
    }

    /// [`Self::from_catalog`] with `organization` (and its group lists)
    /// applied, as data that can cross threads: for a large playlist this
    /// is the costly part of showing it, so it is made off the UI thread
    /// and turned into the schedule there with [`Self::from_prepared`].
    pub fn prepare(
        groups: &[Group],
        channels: &[Channel],
        store: GuideStore,
        now: UnixTime,
        organization: Option<(Organization, Vec<GroupList>)>,
    ) -> Prepared {
        let mut catalog = Catalog::from_catalog(groups, channels, store, now);
        if let Some((organization, lists)) = organization {
            let mut state = catalog.shape().organization(organization);
            state.lists = Some(catalog.shape().lists(lists));
            catalog.organization = Some(state);
        }
        Prepared(catalog)
    }

    /// The schedule [`Self::prepare`] made; cheap (it moves the data).
    pub fn from_prepared(prepared: Prepared) -> Self {
        Self::from_parts(prepared.0)
    }

    /// Cheap immutable input for preparing an edit on a blocking worker.
    pub fn same_catalogue(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.channels, &other.channels)
    }
    pub fn organization_builder(&self) -> OrganizationBuilder {
        OrganizationBuilder {
            channels: self.channels.clone(),
            sources: self.sources.clone(),
            playlist_order: self.playlist_order.clone(),
            group_keys: self.group_keys.clone(),
            group_names: self.group_names.clone(),
        }
    }
    /// Copies display state without mutating a schedule retained by combined mode.
    pub fn with_prepared_organization(&self, prepared: PreparedOrganization) -> Self {
        let next = self.clone();
        *next.organization.borrow_mut() = Some(std::sync::Arc::new(prepared.0));
        next.group_index.take();
        next
    }

    pub fn set_organization(&self, organization: Organization) {
        self.group_index.take();
        let state = self.shape().organization(organization);
        *self.organization.borrow_mut() = Some(std::sync::Arc::new(state));
    }

    /// Each channel's group as [`Schedule::group_channels`] lists them:
    /// the first group (in display order) a channel is a member of.
    fn group_index(&self) -> Vec<u32> {
        let mut index = vec![NO_GROUP; self.channels.len()];
        let mut place = |channel: usize, group: usize| {
            if let Some(slot) = index.get_mut(channel)
                && *slot == NO_GROUP
            {
                *slot = u32::try_from(group).unwrap_or(NO_GROUP);
            }
        };
        let organization = self.organization.borrow();
        if let Some(lists) = organization.as_ref().and_then(|s| s.lists.as_ref()) {
            for (group, (_, _, members)) in lists.iter().enumerate() {
                for &channel in members {
                    place(channel, group);
                }
            }
        } else {
            drop(organization);
            // Channels are stored grouped: a group's members are its range.
            for (group, &stored) in self.visible_groups().iter().enumerate() {
                let start = self.group_starts[stored];
                let end = self
                    .group_starts
                    .get(stored + 1)
                    .copied()
                    .unwrap_or(self.channels.len());
                for channel in start..end {
                    place(channel, group);
                }
            }
        }
        index
    }

    fn visible_groups(&self) -> Vec<usize> {
        self.organization.borrow().as_ref().map_or_else(
            || (0..self.group_keys.len()).collect(),
            |s| s.groups.iter().map(|(g, _)| *g).collect(),
        )
    }

    /// Attaches the source's playback context (Xtream credentials and
    /// account, Stalker session, M3U playlist headers). Without one, only
    /// ready URLs play and they carry only the channel's own headers.
    pub fn with_playback(mut self, context: Option<playback::PlaybackContext>) -> Self {
        self.playback = context.map(std::rc::Rc::new);
        self
    }

    /// [`Self::with_playback`] with a context another schedule holds.
    pub fn with_shared_playback(
        mut self,
        context: Option<std::rc::Rc<playback::PlaybackContext>>,
    ) -> Self {
        self.playback = context;
        self
    }

    pub fn playback(&self) -> Option<&playback::PlaybackContext> {
        self.playback.as_deref()
    }

    /// The context, to share with a schedule that replaces this one.
    pub fn shared_playback(&self) -> Option<std::rc::Rc<playback::PlaybackContext>> {
        self.playback.clone()
    }

    /// The imported channel behind a guide row.
    pub fn source_channel(&self, channel: usize) -> Option<&Channel> {
        self.sources.get(channel)?.as_ref()
    }

    /// What the player should open for `channel`: live, or `archive` (a
    /// programme in guide minutes) from its start. `clock.now` is the real
    /// time, which can be later than the schedule's minute 0.
    pub fn resolve(
        &self,
        channel: usize,
        archive: Option<Program>,
        clock: playback::Clock,
    ) -> playback::Resolution {
        self.resolve_from(channel, archive, 0, clock)
    }

    /// [`Self::resolve`], an archived programme from `offset` seconds after
    /// its start (a seek: archive addresses are time-based, so a seek opens
    /// the archive again from the new time).
    pub fn resolve_from(
        &self,
        channel: usize,
        archive: Option<Program>,
        offset: i64,
        clock: playback::Clock,
    ) -> playback::Resolution {
        let Some(c) = self.source_channel(channel) else {
            return playback::Resolution::Unavailable(playback::Unavailable::NoChannel);
        };
        let archive = archive.map(|p| playback::Archive {
            start: self.unix(p.start) + offset.max(0),
            end: self.unix(p.end),
        });
        playback::resolve(self.playback.as_deref(), c, archive, clock)
    }

    /// The clock for the trait's URL methods: guide minute `now`, UTC.
    fn clock_at(&self, now: i32) -> playback::Clock {
        playback::Clock {
            now: self.unix(now),
            local_offset_secs: 0,
        }
    }

    /// Unix time of guide minute 0.
    pub fn epoch(&self) -> UnixTime {
        self.epoch
    }

    /// The guide held with the schedule (from [`Self::loaded_from`] on).
    pub fn store(&self) -> &GuideStore {
        &self.store
    }

    pub fn channel(&self, channel: usize) -> Option<&GuideChannel> {
        self.channels.get(channel)
    }

    /// The guide minute `t` falls in.
    pub fn minute(&self, t: UnixTime) -> i32 {
        (t - self.epoch).div_euclid(60) as i32
    }

    /// Unix time of guide `minute`.
    pub fn unix(&self, minute: i32) -> UnixTime {
        self.epoch + i64::from(minute) * 60
    }

    fn key(&self, channel: usize) -> Option<&ChannelKey> {
        self.channels.get(channel).map(|c| &c.key)
    }

    /// The description of `program`, when the EPG has one: from memory,
    /// else from the description lookup.
    pub fn description(&self, channel: usize, program: Program) -> Option<String> {
        let key = self.key(channel)?;
        let t = self.probe(program);
        let (desc, start) = self
            .channel_store(Some(key), t, |s| {
                let p = s.programme_at(key, t)?;
                Some((p.desc().map(str::to_owned), p.start()))
            })
            .flatten()?;
        match desc {
            Some(d) => Some(d),
            None => self.describe.as_ref()?(key, start),
        }
    }

    /// A time inside `program` (its first second, or its middle if short).
    /// One reaching into the window held with the schedule is read there
    /// (it is held whole), not from the older day it started in.
    fn probe(&self, program: Program) -> UnixTime {
        let (a, b) = (self.unix(program.start), self.unix(program.end));
        let t = (a + 59).min((a + b) / 2).max(a);
        let held_from = self.held.map_or(UnixTime::MIN, |(from, _)| from);
        let from = self.pages.loaded_from().max(held_from);
        if b > from { t.max(from) } else { t }
    }

    /// The placeholder slot around `t` for a channel with no programme then
    /// (in `store`, which holds `t`).
    fn gap(store: &GuideStore, key: Option<&ChannelKey>, t: UnixTime) -> (UnixTime, UnixTime) {
        let slot_start = t - t.rem_euclid(SLOT);
        let mut start = slot_start;
        let mut end = slot_start + SLOT;
        if let Some(key) = key {
            if let Some(prev_end) = store
                .programmes(key, slot_start, t + 1)
                .map(|p| p.stop())
                .filter(|&s| s <= t)
                .max()
            {
                start = start.max(prev_end);
            }
            if let Some(next) = store.next_after(key, t)
                && next.start() > t
            {
                end = end.min(next.start());
            }
        }
        (start, end)
    }
}

/// A schedule's plain data: no closures and no `Rc`, so it can be made on
/// another thread ([`EpgSchedule::prepare`]).
struct Catalog {
    organization: Option<OrganizationState>,
    group_keys: Vec<String>,
    channels: Vec<GuideChannel>,
    sources: Vec<Option<Channel>>,
    group_names: Vec<String>,
    group_starts: Vec<usize>,
    playlist_order: Vec<usize>,
    store: GuideStore,
    epoch: UnixTime,
    data_start: Option<UnixTime>,
    data_end: Option<UnixTime>,
}

/// A display list by channel key: its key, its name and its members
/// ([`EpgSchedule::set_group_lists`]).
pub type GroupList = (String, String, Vec<String>);

/// A schedule's data with its organisation applied, made off the UI thread
/// ([`EpgSchedule::prepare`], [`EpgSchedule::from_prepared`]).
pub struct Prepared(Catalog);

impl Catalog {
    /// `groups`: each group's stable key, its name and its channels, in
    /// display order. Groups without channels are left out. A group's key,
    /// name and first channel are recorded together here and nowhere
    /// else, so the three lists always have one entry per group
    /// ([`Shape::organization`] indexes one by another's positions).
    fn keyed(
        groups: impl IntoIterator<Item = (String, String, Vec<GuideChannel>)>,
        store: GuideStore,
        now: UnixTime,
    ) -> Self {
        let mut channels = Vec::new();
        let mut group_keys = Vec::new();
        let mut group_names = Vec::new();
        let mut group_starts = Vec::new();
        for (key, name, members) in groups {
            if members.is_empty() {
                continue;
            }
            group_keys.push(key);
            group_names.push(name);
            group_starts.push(channels.len());
            channels.extend(members);
        }
        Catalog {
            organization: None,
            data_start: store.first_start(),
            data_end: store.last_stop(),
            sources: vec![None; channels.len()],
            group_keys,
            channels,
            group_names,
            group_starts,
            playlist_order: Vec::new(),
            store,
            epoch: now - now.rem_euclid(SLOT),
        }
    }

    /// [`EpgSchedule::from_catalog`]'s data.
    fn from_catalog(
        groups: &[Group],
        channels: &[Channel],
        store: GuideStore,
        now: UnixTime,
    ) -> Self {
        // With their playlist positions (1-based), for channels without a
        // provider number.
        let mut buckets: Vec<Vec<(u32, &Channel)>> = vec![Vec::new(); groups.len() + 1];
        for (slot, i) in group_slots(groups, channels) {
            buckets[slot].push((i as u32 + 1, &channels[i]));
        }
        // One key and name per bucket: the catalog's groups, then "Other".
        let labels = groups
            .iter()
            .map(|g| (g.id.as_str().to_owned(), g.name.clone()))
            .chain([("__other__".to_owned(), "Other".to_owned())]);
        // Where each playlist position lands once grouped.
        let mut stored_at = vec![0; channels.len()];
        for (at, &(position, _)) in buckets.iter().flatten().enumerate() {
            stored_at[position as usize - 1] = at;
        }
        let mut ordered = Vec::new();
        let grouped: Vec<_> = labels
            .zip(buckets)
            .map(|((key, name), members)| {
                let guide = members
                    .iter()
                    .map(|&(position, c)| GuideChannel {
                        key: ChannelKey::new(c.id.as_str()),
                        name: c.name.clone(),
                        number: c.number.unwrap_or(position),
                        catchup_days: c.catch_up.as_ref().map_or(0, |cu| {
                            cu.window_days(iptv_providers::m3u::DEFAULT_CATCHUP_WINDOW_DAYS)
                                .min(u32::from(u8::MAX)) as u8
                        }),
                    })
                    .collect::<Vec<_>>();
                ordered.extend(members.into_iter().map(|(_, c)| c.clone()));
                (key, name, guide)
            })
            .collect();
        let mut catalog = Self::keyed(grouped, store, now);
        catalog.sources = ordered.into_iter().map(Some).collect();
        catalog.playlist_order = stored_at;
        catalog
    }

    fn shape(&self) -> Shape<'_> {
        Shape {
            channels: &self.channels,
            sources: &self.sources,
            playlist_order: &self.playlist_order,
            group_keys: &self.group_keys,
            group_names: &self.group_names,
        }
    }
}

/// What organising a schedule reads, from a schedule or its [`Catalog`].
struct Shape<'a> {
    channels: &'a [GuideChannel],
    sources: &'a [Option<Channel>],
    playlist_order: &'a [usize],
    group_keys: &'a [String],
    group_names: &'a [String],
}

impl Shape<'_> {
    /// The user's names, hidden channels and groups, and order.
    fn organization(&self, organization: Organization) -> OrganizationState {
        use std::collections::HashMap;
        let channel_prefs: HashMap<_, _> = organization
            .channels
            .iter()
            .enumerate()
            .map(|(i, (id, name, hidden))| (id.as_str(), (i, name, *hidden)))
            .collect();
        let group_prefs: HashMap<_, _> = organization
            .groups
            .iter()
            .enumerate()
            .map(|(i, (id, name, hidden))| (id.as_str(), (i, name, *hidden)))
            .collect();
        let mut state = OrganizationState::default();
        for (i, c) in self.channels.iter().enumerate() {
            let pref = channel_prefs.get(c.key.as_str());
            state
                .names
                .push(pref.map_or_else(|| c.name.clone(), |(_, name, _)| (*name).clone()));
            let group_hidden = self
                .sources
                .get(i)
                .and_then(Option::as_ref)
                .and_then(|c| c.groups.first())
                .and_then(|g| group_prefs.get(g.as_str()))
                .is_some_and(|(_, _, hidden)| *hidden);
            state
                .hidden
                .push(pref.is_some_and(|(_, _, hidden)| *hidden) || group_hidden);
        }
        state.order = self.playlist_order.to_vec();
        if state.order.len() != self.channels.len() {
            state.order = (0..self.channels.len()).collect();
        }
        state.order.sort_by_key(|&c| {
            channel_prefs
                .get(self.channels[c].key.as_str())
                .map_or(usize::MAX, |(i, _, _)| *i)
        });
        let mut groups: Vec<_> = (0..self.group_keys.len())
            .filter(|&g| {
                !group_prefs
                    .get(self.group_keys[g].as_str())
                    .is_some_and(|(_, _, hidden)| *hidden)
            })
            .collect();
        groups.sort_by_key(|&g| {
            group_prefs
                .get(self.group_keys[g].as_str())
                .map_or(usize::MAX, |(i, _, _)| *i)
        });
        state.groups = groups
            .into_iter()
            .map(|g| {
                (
                    g,
                    group_prefs.get(self.group_keys[g].as_str()).map_or_else(
                        || self.group_names[g].clone(),
                        |(_, name, _)| (*name).clone(),
                    ),
                )
            })
            .collect();
        state
    }

    /// Display lists by channel key, as channel positions: unknown keys
    /// left out, a channel listed twice in a list kept once.
    fn lists(
        &self,
        lists: Vec<(String, String, Vec<String>)>,
    ) -> Vec<(String, String, Vec<usize>)> {
        let index: std::collections::HashMap<_, _> = self
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| (c.key.as_str(), i))
            .collect();
        lists
            .into_iter()
            .map(|(key, name, members)| {
                let mut seen = std::collections::HashSet::new();
                let members = members
                    .iter()
                    .filter_map(|id| index.get(id.as_str()).copied())
                    .filter(|c| seen.insert(*c))
                    .collect();
                (key, name, members)
            })
            .collect()
    }
}

impl Schedule for EpgSchedule {
    fn epoch_unix(&self) -> i64 {
        self.epoch
    }

    fn follows_wall_clock(&self) -> bool {
        true
    }

    /// From the earliest programme stored (the oldest day kept: pages'
    /// [`pages::PagedWindow::oldest`], else the first programme in memory)
    /// to [`PAST_END`] past the last one; without guide data, from now's
    /// default window to the default horizon. Nothing before the stored
    /// data can be browsed, as nothing after it.
    fn browsable(&self, now: i32) -> (i32, i32) {
        let home = iptv_core::guide::home_window(now);
        let first = match self.pages.oldest() {
            UnixTime::MIN => self.data_start.map_or(home, |t| self.minute(t)),
            oldest => self.minute(oldest),
        };
        let end = self.data_end.map_or(now + iptv_core::guide::HORIZON, |t| {
            self.minute(t) + PAST_END
        });
        let end = end.max(now + iptv_core::guide::VISIBLE);
        (first.min(home), end)
    }

    fn channel_count(&self) -> usize {
        self.channels.len()
    }

    fn group_count(&self) -> usize {
        if let Some(lists) = self
            .organization
            .borrow()
            .as_ref()
            .and_then(|s| s.lists.as_ref())
        {
            return lists.len();
        }
        self.visible_groups().len()
    }

    fn group_start(&self, group: usize) -> usize {
        if let Some(lists) = self
            .organization
            .borrow()
            .as_ref()
            .and_then(|s| s.lists.as_ref())
        {
            return lists
                .get(group)
                .and_then(|l| l.2.first())
                .copied()
                .unwrap_or(0);
        }
        self.visible_groups()
            .get(group)
            .and_then(|&g| self.group_starts.get(g))
            .copied()
            .unwrap_or(0)
    }

    fn program_at(&self, channel: usize, minute: i32) -> Program {
        // What airs at the minute's last second: a programme starting partway
        // through a minute (its start rounds down to it) owns that minute, so
        // the minute a cell starts at finds that cell's programme, not the one
        // before it, and cells never overlap.
        let t = self.unix(minute) + 59;
        let key = self.key(channel);
        let found = self
            .channel_store(key, t, |s| match key.and_then(|k| s.programme_at(k, t)) {
                Some(p) => (p.start(), p.stop()),
                None => Self::gap(s, key, t),
            })
            .or_else(|| self.held_across(key, t));
        // Its day is loading: the hour slot.
        let (start, end) = found.unwrap_or_else(|| self.loading_slot(key, t));
        let (start, end) = (self.minute(start), self.minute(end));
        // Sub-minute programmes still get a one-minute cell.
        Program {
            start,
            end: end.max(start + 1),
        }
    }

    fn programs_between(&self, channel: usize, from: i32, minutes: i32) -> Vec<Program> {
        let to = from + minutes;
        let mut out = Vec::new();
        let mut at = from;
        while at < to {
            let p = self.program_at(channel, at);
            out.push(p);
            at = p.end.max(at + 1);
        }
        out
    }

    fn catchup_days(&self, channel: usize) -> u8 {
        self.channels.get(channel).map_or(0, |c| c.catchup_days)
    }

    fn channel_name(&self, channel: usize) -> String {
        self.organization
            .borrow()
            .as_ref()
            .and_then(|s| s.names.get(channel))
            .cloned()
            .unwrap_or_else(|| {
                self.channels
                    .get(channel)
                    .map_or_else(String::new, |c| c.name.clone())
            })
    }
    fn channel_hidden(&self, channel: usize) -> bool {
        self.organization
            .borrow()
            .as_ref()
            .and_then(|s| s.hidden.get(channel))
            .copied()
            .unwrap_or(false)
    }

    fn channel_number(&self, channel: usize) -> u32 {
        self.channels
            .get(channel)
            .map_or(channel as u32 + 1, |c| c.number)
    }

    fn group_key(&self, group: usize) -> String {
        if let Some(lists) = self
            .organization
            .borrow()
            .as_ref()
            .and_then(|s| s.lists.as_ref())
        {
            return lists.get(group).map_or_else(String::new, |l| l.0.clone());
        }
        self.visible_groups()
            .get(group)
            .and_then(|&g| self.group_keys.get(g))
            .cloned()
            .unwrap_or_default()
    }
    fn group_name(&self, group: usize) -> String {
        if let Some(lists) = self
            .organization
            .borrow()
            .as_ref()
            .and_then(|s| s.lists.as_ref())
        {
            return lists.get(group).map_or_else(String::new, |l| l.1.clone());
        }
        if let Some(state) = self.organization.borrow().as_ref() {
            return state
                .groups
                .get(group)
                .map_or_else(String::new, |(_, name)| name.clone());
        }
        self.group_names.get(group).cloned().unwrap_or_default()
    }
    fn group_channels(&self, group: usize) -> Vec<usize> {
        if let Some(lists) = self
            .organization
            .borrow()
            .as_ref()
            .and_then(|s| s.lists.as_ref())
        {
            return lists.get(group).map_or_else(Vec::new, |l| l.2.clone());
        }
        let Some(g) = self.visible_groups().get(group).copied() else {
            return Vec::new();
        };
        let start = self.group_starts[g];
        let end = self
            .group_starts
            .get(g + 1)
            .copied()
            .unwrap_or(self.channels.len());
        self.playlist_order()
            .into_iter()
            .filter(|c| (start..end).contains(c))
            .collect()
    }

    fn channel_logo(&self, channel: usize) -> Option<String> {
        let logo = self.source_channel(channel)?.logo.as_deref()?.trim();
        (!logo.is_empty()).then(|| logo.to_owned())
    }

    fn playlist_order(&self) -> Vec<usize> {
        if let Some(state) = self.organization.borrow().as_ref() {
            return state.order.clone();
        }
        if self.playlist_order.len() == self.channels.len() {
            self.playlist_order.to_vec()
        } else {
            (0..self.channels.len()).collect()
        }
    }

    /// Ready URLs only (Stalker `create_link` needs a round trip: use
    /// [`EpgSchedule::resolve`]). Times are the schedule's epoch.
    fn stream_url(&self, channel: usize) -> Option<String> {
        let source = self.resolve(channel, None, self.clock_at(0)).ready()?;
        Some(source.url)
    }

    fn catchup_url(&self, channel: usize, program: Program, now: i32) -> Option<String> {
        let source = self
            .resolve(channel, Some(program), self.clock_at(now))
            .ready()?;
        Some(source.url)
    }

    fn channel_key(&self, channel: usize) -> Option<String> {
        self.channels
            .get(channel)
            .map(|c| c.key.as_str().to_owned())
    }

    fn find_channel(&self, key: &str) -> Option<usize> {
        self.key_index
            .get_or_init(|| {
                let mut index = std::collections::HashMap::with_capacity(self.channels.len());
                for (at, channel) in self.channels.iter().enumerate() {
                    // The first channel with a key wins, as a scan finds it.
                    index.entry(channel.key.as_str().to_owned()).or_insert(at);
                }
                index
            })
            .get(key)
            .copied()
    }

    fn channel_group(&self, channel: usize) -> Option<usize> {
        if self.group_index.borrow().is_none() {
            let index = self.group_index();
            *self.group_index.borrow_mut() = Some(index);
        }
        let group = self.group_index.borrow().as_ref()?.get(channel).copied()?;
        (group != NO_GROUP).then_some(group as usize)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn title(&self, channel: usize, program: Program) -> String {
        let Some(key) = self.key(channel) else {
            return NO_INFORMATION.to_owned();
        };
        let t = self.probe(program);
        let found = self.channel_store(Some(key), t, |s| {
            s.programme_at(key, t)
                .filter(|p| self.minute(p.start()) == program.start)
                // A guide stored before badges were folded at import.
                .map(|p| iptv_epg::fold_superscripts(p.title()).into_owned())
        });
        match found {
            None => LOADING.to_owned(),
            Some(title) => title.unwrap_or_else(|| NO_INFORMATION.to_owned()),
        }
    }
}

#[cfg(test)]
mod held_tests;
#[cfg(test)]
mod index_tests;
#[cfg(test)]
mod pages_tests;
#[cfg(test)]
mod tests;

/// Sendable immutable catalogue shape for organization work.
pub struct OrganizationBuilder {
    channels: std::sync::Arc<Vec<GuideChannel>>,
    sources: std::sync::Arc<Vec<Option<Channel>>>,
    playlist_order: std::sync::Arc<Vec<usize>>,
    group_keys: std::sync::Arc<Vec<String>>,
    group_names: std::sync::Arc<Vec<String>>,
}
pub struct PreparedOrganization(OrganizationState);
impl OrganizationBuilder {
    pub fn prepare(
        self,
        organization: Organization,
        lists: Vec<GroupList>,
    ) -> PreparedOrganization {
        let shape = Shape {
            channels: &self.channels,
            sources: &self.sources,
            playlist_order: &self.playlist_order,
            group_keys: &self.group_keys,
            group_names: &self.group_names,
        };
        let mut state = shape.organization(organization);
        state.lists = Some(shape.lists(lists));
        PreparedOrganization(state)
    }
}
