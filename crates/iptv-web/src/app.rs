//! Live TV: the import, the channel list beside the preview, the full-screen
//! player and channel zapping. The state is the core's (`iptv_core`'s list
//! and focus models over an `iptv_guide` schedule); the host renders the
//! views this emits and plays what it is told to.

use std::cell::RefCell;
use std::rc::Rc;

use iptv_core::channel_list::{ChannelList as Focus, ListMode, ListPane, Outcome};
use iptv_core::favorites::{Favorites, KeyIndex, Recent};
use iptv_core::guide::{Schedule, now_minute};
use iptv_core::lists::{self, ChannelList as Rows, Entry};
use iptv_epg::{GuideStore, Overrides, StoreOptions};
use iptv_guide::EpgSchedule;
use iptv_guide::playback::{Clock, PlaybackContext, Resolution};
use iptv_import::{
    CancelToken, Catalog, Decision, EpgSourceConfig, GuideState, ImportOptions, MemoryGuide, Phase,
    Progress, RefreshPolicy, SourceMeta, Trigger, decide, matching_fingerprint, plan_startup,
};
use iptv_providers::model::{M3uSourceConfig, SourceConfig, XtreamCredentials};
use iptv_providers::xtream::StreamFormat;
use iptv_providers::{Secret, SecretUrl};
use serde_json::{Value, json};

use crate::exec::{HostFetch, spawn};
use crate::host::host;
use crate::store;

/// Rows the channel column shows at once.
pub(crate) const VISIBLE_ROWS: usize = 9;
/// Entries the groups column shows at once.
pub(crate) const VISIBLE_ENTRIES: usize = 11;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// The list beside the video, shrunk to a preview.
    List,
    /// Full-screen video.
    Full,
}

pub(crate) struct App {
    config: Option<SourceConfig>,
    schedule: Option<Rc<EpgSchedule>>,
    rows: Rows,
    focus: Focus,
    mode: Mode,
    playing: Option<usize>,
    /// The playlist's identity for stored data (no credentials).
    source_id: Option<String>,
    /// The user's lists by channel key, so they survive refreshes.
    favorites: Favorites,
    recent: Recent,
    keys: KeyIndex,
    /// The import's progress for the status line.
    status: Value,
    guide_loaded: bool,
    /// The current playlist's guide refresh inputs and stored state.
    guide: Option<Rc<GuideInputs>>,
    guide_state: Option<GuideState>,
    /// Bumped to stop a waiting guide loop (a manual refresh replaces it).
    guide_generation: u32,
    cancel: CancelToken,
    generation: u32,
    /// The viewer's UTC offset, from the host.
    local_offset_secs: i32,
}

thread_local! {
    static APP: RefCell<App> = RefCell::new(App::new());
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> R {
    APP.with(|a| f(&mut a.borrow_mut()))
}

fn wall_secs() -> i64 {
    (host().wall_ms() / 1000.0) as i64
}

impl App {
    fn new() -> Self {
        let rows = Rows::default();
        App {
            config: None,
            schedule: None,
            focus: Focus::open(ListMode::Preview, None, &rows),
            rows,
            mode: Mode::List,
            playing: None,
            source_id: None,
            favorites: Favorites::default(),
            recent: Recent::default(),
            keys: KeyIndex::default(),
            status: json!({ "phase": "idle" }),
            guide_loaded: false,
            guide: None,
            guide_state: None,
            guide_generation: 0,
            cancel: CancelToken::new(),
            generation: 0,
            local_offset_secs: 0,
        }
    }

    fn schedule(&self) -> Option<&dyn Schedule> {
        self.schedule.as_deref().map(|s| s as &dyn Schedule)
    }

    fn build_rows(&self, entry: usize, s: &dyn Schedule) -> Rows {
        let favorites = self.favorites.channels(&self.keys);
        let recent = self.recent.channels(&self.keys);
        Rows::build(entry, s, &favorites, &recent)
    }

    /// Shows the list of groups column `entry`.
    fn show_entry(&mut self, entry: usize) {
        let Some(s) = self.schedule.clone() else {
            return;
        };
        self.rows = self.build_rows(entry, &*s);
        self.focus.shown(&self.rows);
    }

    fn key_of(&self, channel: usize) -> Option<String> {
        self.schedule()?.channel_key(channel)
    }

    fn save_lists(&self) {
        if let Some(id) = &self.source_id {
            store::save_lists(id, &self.favorites, &self.recent);
        }
    }

    /// Adds or removes the focused channel (the playing one in full screen)
    /// from Favorites. The Favorites list itself updates when shown again.
    fn toggle_favorite(&mut self) {
        let channel = match self.mode {
            Mode::Full => self.playing,
            Mode::List if self.focus.pane == ListPane::Channels => {
                self.rows.channel(self.focus.row)
            }
            Mode::List => None,
        };
        let Some(key) = channel.and_then(|c| self.key_of(c)) else {
            return;
        };
        let on = !self.favorites.contains(&key);
        self.favorites.set(&key, on);
        self.save_lists();
        let text = if on {
            "Added to Favorites"
        } else {
            "Removed from Favorites"
        };
        host().emit(&json!({ "type": "notice", "text": text, "kind": "info" }).to_string());
    }

    /// A new schedule for the same catalog (the guide arrived): the list and
    /// the focused channel stay, found again by catalog channel.
    fn replace_schedule(&mut self, schedule: EpgSchedule) {
        let focused = self.rows.channel(self.focus.row);
        let entry = self.rows.entry();
        let first = self.schedule.is_none();
        self.keys = KeyIndex::build(&schedule);
        self.schedule = Some(Rc::new(schedule));
        let s = self.schedule.clone().expect("just set");
        let entry = if first { lists::ALL } else { entry };
        self.rows = self.build_rows(entry, &*s);
        let (pane, group_entry) = (self.focus.pane, self.focus.entry);
        self.focus = Focus::open(ListMode::Preview, focused.or(self.playing), &self.rows);
        if !first {
            self.focus.pane = pane;
            self.focus.entry = group_entry;
        }
    }

    fn play(&mut self, channel: usize) {
        let Some(s) = self.schedule.clone() else {
            return;
        };
        let clock = Clock {
            now: wall_secs(),
            local_offset_secs: self.local_offset_secs,
        };
        match s.resolve(channel, None, clock) {
            Resolution::Play(source) => {
                self.playing = Some(channel);
                if let Some(key) = s.channel_key(channel) {
                    self.recent.played(&key);
                    self.save_lists();
                }
                let user_agent = source
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
                    .map(|(_, v)| v.clone());
                host().emit(
                    &json!({
                        "type": "play",
                        "url": source.url,
                        "userAgent": user_agent,
                        "channel": self.channel_json(&*s, channel, None),
                    })
                    .to_string(),
                );
            }
            Resolution::StalkerLink(_) | Resolution::Unavailable(_) => {
                host().emit(
                    &json!({ "type": "notice", "text": "This channel cannot be played." })
                        .to_string(),
                );
            }
        }
    }

    /// Channel up/down from the playing channel, within the list shown.
    fn zap(&mut self, step: isize) {
        let row = self
            .playing
            .and_then(|c| self.rows.row_of(c))
            .unwrap_or(self.focus.row);
        if let Some(to) = self.rows.zap(row, step).and_then(|r| self.rows.channel(r)) {
            self.play(to);
            self.focus.follow(to, &self.rows);
        }
    }

    fn outcome(&mut self, outcome: Outcome) -> bool {
        match outcome {
            Outcome::Stay => {}
            Outcome::Play(c) => self.play(c),
            Outcome::Preview(entry) => self.show_entry(entry),
            Outcome::Close => {
                if self.playing.is_some() {
                    self.mode = Mode::Full;
                } else {
                    return false;
                }
            }
        }
        true
    }

    /// One remote key. Returns false when the app should close.
    fn key(&mut self, key: &str) -> bool {
        if self.schedule.is_none() {
            return key != "back";
        }
        if key == "favorite" {
            self.toggle_favorite();
            return true;
        }
        let entries = self.schedule().map_or(0, lists::entry_count);
        match self.mode {
            Mode::Full => match key {
                "up" | "chup" => self.zap(-1),
                "down" | "chdown" => self.zap(1),
                "ok" | "left" => {
                    self.mode = Mode::List;
                    self.focus = Focus::open(ListMode::Preview, self.playing, &self.rows);
                }
                // Back from full screen leaves the app (the host confirms first);
                // the list is one OK or Left away.
                "back" => return false,
                _ => {}
            },
            Mode::List => {
                let page = VISIBLE_ROWS as isize;
                let outcome = match key {
                    "up" => self.focus.move_focus(-1, &self.rows, entries),
                    "down" => self.focus.move_focus(1, &self.rows, entries),
                    "pageup" => self.focus.move_focus(-page, &self.rows, entries),
                    "pagedown" => self.focus.move_focus(page, &self.rows, entries),
                    "left" => {
                        self.focus.left(&self.rows);
                        Outcome::Stay
                    }
                    "right" => {
                        self.focus.right(&self.rows);
                        Outcome::Stay
                    }
                    "ok" => self.focus.select(self.playing, &self.rows),
                    "back" => self.focus.back(),
                    "chup" => {
                        self.zap(-1);
                        Outcome::Stay
                    }
                    "chdown" => {
                        self.zap(1);
                        Outcome::Stay
                    }
                    _ => Outcome::Stay,
                };
                return self.outcome(outcome);
            }
        }
        true
    }

    fn program_json(s: &dyn Schedule, channel: usize, minute: i32) -> Option<Value> {
        let p = s.program_at(channel, minute);
        let title = s.title(channel, p);
        if title.is_empty() || title == iptv_guide::NO_INFORMATION || title == iptv_guide::LOADING {
            return None;
        }
        let epoch = s.epoch_unix();
        Some(json!({
            "title": title,
            "start": epoch + i64::from(p.start) * 60,
            "end": epoch + i64::from(p.end) * 60,
        }))
    }

    fn channel_json(&self, s: &dyn Schedule, channel: usize, number: Option<u32>) -> Value {
        let minute = now_minute(s, wall_secs(), 0);
        let now = Self::program_json(s, channel, minute);
        let next = now
            .as_ref()
            .and_then(|_| Self::program_json(s, channel, s.program_at(channel, minute).end));
        json!({
            "id": channel,
            "number": number.unwrap_or_else(|| self.rows.number_of(channel, s)),
            "name": s.channel_name(channel),
            "logo": s.channel_logo(channel),
            "now": now,
            "next": next,
            "playing": self.playing == Some(channel),
        })
    }

    fn entry_name(s: &dyn Schedule, entry: usize) -> String {
        match Entry::at(entry) {
            Entry::Favorites => "Favorites".into(),
            Entry::Recent => "Recently watched".into(),
            Entry::All => "All channels".into(),
            Entry::Group(g) => s.group_name(g),
        }
    }

    fn window(focus: usize, len: usize, shown: usize) -> usize {
        focus
            .saturating_sub(shown / 2)
            .min(len.saturating_sub(shown))
    }

    fn view(&self) -> Value {
        let Some(s) = self.schedule() else {
            return json!({ "type": "view", "ready": false, "status": self.status });
        };
        let len = self.rows.len();
        let start = Self::window(self.focus.row, len, VISIBLE_ROWS);
        let rows: Vec<Value> = (start..(start + VISIBLE_ROWS).min(len))
            .filter_map(|r| {
                self.rows
                    .channel(r)
                    .map(|c| self.channel_json(s, c, Some(Rows::number(r))))
            })
            .collect();
        let entries = lists::entry_count(s);
        let estart = Self::window(self.focus.entry, entries, VISIBLE_ENTRIES);
        let groups: Vec<Value> = (estart..(estart + VISIBLE_ENTRIES).min(entries))
            .map(|e| json!({ "name": Self::entry_name(s, e), "shown": e == self.rows.entry() }))
            .collect();
        json!({
            "type": "view",
            "ready": true,
            "mode": match self.mode { Mode::List => "list", Mode::Full => "full" },
            "pane": match self.focus.pane { ListPane::Channels => "channels", ListPane::Groups => "groups" },
            "title": Self::entry_name(s, self.rows.entry()),
            "rows": { "start": start, "focus": self.focus.row, "total": len, "items": rows },
            "groups": { "start": estart, "focus": self.focus.entry, "total": entries, "items": groups },
            "playing": self.playing.map(|c| self.channel_json(s, c, None)),
            "guide": self.guide_loaded,
            "status": self.status,
        })
    }
}

fn emit_view() {
    let v = with_app(|a| a.view());
    host().emit(&v.to_string());
}

fn phase_name(p: &Progress) -> &'static str {
    match p.phase {
        Phase::Connecting => "connecting",
        Phase::Authenticating => "authenticating",
        Phase::Account => "account",
        Phase::Playlist => "playlist",
        Phase::Groups(_) => "groups",
        Phase::Items(_) => "channels",
        Phase::Guide(_) => "guide",
        Phase::Matching => "matching",
        _ => "done",
    }
}

fn report(generation: u32, p: &Progress) {
    let status = json!({
        "phase": phase_name(p),
        "items": p.items,
        "bytes": p.bytes,
        "totalBytes": p.total_bytes,
    });
    let current = with_app(|a| {
        if a.generation == generation {
            a.status = status.clone();
            true
        } else {
            false
        }
    });
    if current {
        host().emit(&json!({ "type": "progress", "status": status }).to_string());
    }
}

fn playback_context(config: &SourceConfig, catalog: &Catalog) -> Option<PlaybackContext> {
    match (config, &catalog.source) {
        // Raw TS starts and zaps fastest on the TV's native player.
        (SourceConfig::Xtream(c), SourceMeta::Xtream { account, .. }) => {
            PlaybackContext::xtream(c, account.as_ref(), StreamFormat::Ts).ok()
        }
        (SourceConfig::M3u(m), _) => Some(PlaybackContext::m3u(m)),
        _ => None,
    }
}

fn schedule(config: &SourceConfig, catalog: &Catalog, store: GuideStore, now: i64) -> EpgSchedule {
    let s = EpgSchedule::from_catalog(&catalog.groups, &catalog.channels, store, now);
    s.with_playback(playback_context(config, catalog))
}

/// How the TV keeps guides: without descriptions (the list does not show
/// them, and they are most of a guide's size) and one past day.
pub(crate) const GUIDE_STORE: StoreOptions = StoreOptions {
    store_descriptions: false,
    past_days_to_keep: Some(1),
};

/// What the guide refresh reads besides the stored state.
pub(crate) struct GuideInputs {
    config: SourceConfig,
    catalog: Rc<Catalog>,
    source_id: String,
    feeds: Vec<EpgSourceConfig>,
    fingerprint: u64,
}

/// The latest stored program end of any bound channel.
fn ends_at(guide: &GuideStore) -> Option<i64> {
    guide
        .bindings()
        .filter_map(|(key, _)| guide.schedule(key))
        .filter_map(|s| s.get(s.len().checked_sub(1)?).map(|p| p.stop()))
        .max()
}

fn set_status(status: Value) {
    with_app(|a| a.status = status);
    emit_view();
}

async fn import(generation: u32, config: SourceConfig, cancel: CancelToken) {
    let fetch = HostFetch::new();
    let now = wall_secs();
    let current = move || with_app(|a| a.generation == generation);
    let mut progress = move |p: &Progress| report(generation, p);
    let catalog = match iptv_import::import_source(
        &fetch,
        &config,
        &ImportOptions::new(now),
        &mut progress,
        &cancel,
    )
    .await
    {
        Ok(c) => Rc::new(c),
        Err(e) => {
            if current() {
                set_status(json!({ "phase": "failed", "error": e.to_string() }));
            }
            return;
        }
    };
    let source_id = store::source_id(&config);
    store::make_current(&source_id).await;
    let (favorites, recent) = store::load_lists(&source_id).await;
    let state = store::load_guide_state(&source_id).await;
    if !current() {
        return;
    }
    let mut feeds = catalog.epg_sources(1).unwrap_or_default();
    for feed in &mut feeds {
        feed.store = GUIDE_STORE;
    }
    let fingerprint = matching_fingerprint(&catalog.channels, &Overrides::default());
    let policy = RefreshPolicy::default();
    // The stored guide shows at once, before any guide download.
    let plan = plan_startup(&policy, &feeds, fingerprint, state.as_ref(), now);
    let stored = if plan.use_stored {
        store::load_guide(&source_id)
            .await
            .and_then(|bytes| GuideStore::from_snapshot(&bytes, GUIDE_STORE, now).ok())
    } else {
        None
    };
    if !current() {
        return;
    }
    // A state without its guide (cleared storage) is no guide at all.
    let (state, decision) = match (&stored, plan.use_stored) {
        (None, true) => (
            None,
            decide(&policy, Trigger::Startup, &feeds, fingerprint, None, now),
        ),
        _ => (state, plan.decision),
    };
    let has_guide = stored.is_some();
    let first = schedule(&config, &catalog, stored.unwrap_or_default(), now);
    let inputs = Rc::new(GuideInputs {
        config,
        catalog,
        source_id: source_id.clone(),
        feeds,
        fingerprint,
    });
    with_app(|a| {
        a.source_id = Some(source_id);
        a.favorites = favorites;
        a.recent = recent;
        a.replace_schedule(first);
        a.guide_loaded = has_guide;
        a.guide = Some(inputs.clone());
        a.guide_state = state;
        a.status = json!({ "phase": "done", "guide": has_guide });
    });
    emit_view();
    guide_loop(generation, inputs, decision, fetch, cancel).await;
}

/// Keeps the guide as the refresh policy decides, for as long as this
/// playlist and guide loop are the current ones.
async fn guide_loop(
    generation: u32,
    inputs: Rc<GuideInputs>,
    mut decision: Decision,
    fetch: HostFetch,
    cancel: CancelToken,
) {
    let guide_generation = with_app(|a| a.guide_generation);
    let current =
        move || with_app(|a| a.generation == generation && a.guide_generation == guide_generation);
    let policy = RefreshPolicy::default();
    let mut progress = move |p: &Progress| report(generation, p);
    loop {
        let wait_until = match decision {
            Decision::Discard => {
                store::discard_guide(&inputs.source_id);
                with_app(|a| a.guide_state = None);
                set_status(json!({ "phase": "done", "guide": false }));
                return;
            }
            Decision::Keep { next_check } => next_check,
            Decision::Backoff { retry_at } => Some(retry_at),
            Decision::Refresh(_) => {
                let updating = with_app(|a| a.guide_loaded);
                set_status(json!({ "phase": "guide", "updating": updating }));
                let now = wall_secs();
                let mut sink = MemoryGuide::new();
                let result = iptv_import::import_epg(
                    &fetch,
                    &inputs.feeds,
                    &inputs.catalog.channels,
                    &Overrides::default(),
                    now,
                    &mut sink,
                    &mut progress,
                    &cancel,
                )
                .await;
                if !current() {
                    return;
                }
                let previous = with_app(|a| a.guide_state.clone()).unwrap_or_default();
                let guide = result.as_ref().ok().map(|epg| sink.into_guide(epg));
                // An empty result never replaces a guide that works.
                let usable = guide
                    .as_ref()
                    .filter(|g| g.bound_count() > 0 || previous.fetched_at.is_none());
                let state = match usable {
                    Some(guide) => {
                        let state = GuideState {
                            fetched_at: Some(now),
                            attempted_at: Some(now),
                            last_attempt_failed: false,
                            feeds: inputs.feeds.clone(),
                            match_fingerprint: Some(inputs.fingerprint),
                            ends_at: ends_at(guide),
                            bound_channels: guide.bound_count(),
                        };
                        store::save_guide(&inputs.source_id, &guide.to_snapshot(), &state);
                        state
                    }
                    None => {
                        let state = GuideState {
                            attempted_at: Some(now),
                            last_attempt_failed: true,
                            ..previous
                        };
                        store::save_guide_state(&inputs.source_id, &state);
                        state
                    }
                };
                let error = result.as_ref().err().map(ToString::to_string);
                let bound = state.bound_channels;
                if let Some(guide) = guide.filter(|_| !state.last_attempt_failed) {
                    let with_guide = schedule(&inputs.config, &inputs.catalog, guide, now);
                    with_app(|a| {
                        a.replace_schedule(with_guide);
                        a.guide_loaded = true;
                    });
                }
                with_app(|a| a.guide_state = Some(state));
                let loaded = with_app(|a| a.guide_loaded);
                set_status(
                    json!({ "phase": "done", "guide": loaded, "bound": bound, "error": error }),
                );
                let state = with_app(|a| a.guide_state.clone());
                decision = decide(
                    &policy,
                    Trigger::Timer,
                    &inputs.feeds,
                    inputs.fingerprint,
                    state.as_ref(),
                    wall_secs(),
                );
                continue;
            }
        };
        let Some(at) = wait_until else {
            return; // Only on request ("refreshGuide").
        };
        let wait = (at - wall_secs()).max(1) as u64;
        iptv_import::Fetch::sleep(&fetch, std::time::Duration::from_secs(wait)).await;
        if !current() {
            return;
        }
        let state = with_app(|a| a.guide_state.clone());
        decision = decide(
            &policy,
            Trigger::Timer,
            &inputs.feeds,
            inputs.fingerprint,
            state.as_ref(),
            wall_secs(),
        );
    }
}

/// "Update the guide now": a manual refresh replaces the waiting loop.
fn refresh_guide() {
    let Some((generation, inputs, state, cancel)) = with_app(|a| {
        a.guide_generation += 1;
        Some((
            a.generation,
            a.guide.clone()?,
            a.guide_state.clone(),
            a.cancel.clone(),
        ))
    }) else {
        return;
    };
    let decision = decide(
        &RefreshPolicy::default(),
        Trigger::Manual,
        &inputs.feeds,
        inputs.fingerprint,
        state.as_ref(),
        wall_secs(),
    );
    spawn(guide_loop(
        generation,
        inputs,
        decision,
        HostFetch::new(),
        cancel,
    ));
}

/// The source from the host's settings JSON.
fn source_config(v: &Value) -> Result<SourceConfig, String> {
    let text = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
    };
    match text("kind") {
        "xtream" => {
            if text("server").is_empty()
                || text("username").is_empty()
                || text("password").is_empty()
            {
                return Err("Enter the server, username and password.".into());
            }
            Ok(SourceConfig::Xtream(XtreamCredentials {
                server_url: text("server").to_owned(),
                username: Secret::new(text("username")),
                password: Secret::new(text("password")),
                user_agent: None,
            }))
        }
        "m3u" => {
            if text("url").is_empty() {
                return Err("Enter the playlist address.".into());
            }
            Ok(SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(text("url")),
                user_agent: None,
                referrer: None,
            }))
        }
        _ => Err("Choose Xtream or an M3U playlist.".into()),
    }
}

/// One command from the host (JSON). Replies are emitted.
pub(crate) fn command(text: &str) {
    let Ok(cmd) = serde_json::from_str::<Value>(text) else {
        host().emit(&json!({ "type": "notice", "text": "bad command" }).to_string());
        return;
    };
    match cmd.get("cmd").and_then(Value::as_str).unwrap_or("") {
        "source" => match source_config(cmd.get("source").unwrap_or(&Value::Null)) {
            Ok(config) => {
                if let Some(offset) = cmd.get("utcOffsetSecs").and_then(Value::as_i64) {
                    with_app(|a| a.local_offset_secs = offset as i32);
                }
                let (generation, cancel) = with_app(|a| {
                    a.cancel.cancel();
                    a.cancel = CancelToken::new();
                    a.generation += 1;
                    *a = App {
                        generation: a.generation,
                        cancel: a.cancel.clone(),
                        local_offset_secs: a.local_offset_secs,
                        config: Some(config.clone()),
                        status: json!({ "phase": "connecting" }),
                        ..App::new()
                    };
                    (a.generation, a.cancel.clone())
                });
                emit_view();
                spawn(import(generation, config, cancel));
            }
            Err(text) => host().emit(&json!({ "type": "notice", "text": text }).to_string()),
        },
        "key" => {
            let key = cmd.get("key").and_then(Value::as_str).unwrap_or("");
            if with_app(|a| a.key(key)) {
                emit_view();
            } else {
                host().emit(&json!({ "type": "exit" }).to_string());
            }
        }
        "view" => emit_view(),
        "refreshGuide" => refresh_guide(),
        "reload" => {
            let config = with_app(|a| a.config.clone());
            if let Some(c) = config {
                let source = match &c {
                    SourceConfig::Xtream(x) => {
                        json!({ "kind": "xtream", "server": x.server_url, "username": x.username.expose(), "password": x.password.expose() })
                    }
                    SourceConfig::M3u(m) => json!({ "kind": "m3u", "url": m.location.expose() }),
                    SourceConfig::Stalker(_) => Value::Null,
                };
                command(&json!({ "cmd": "source", "source": source }).to_string());
            }
        }
        "playFailed" => {
            with_app(|a| a.playing = None);
            emit_view();
        }
        other => host().emit(
            &json!({ "type": "notice", "text": format!("unknown command {other}") }).to_string(),
        ),
    }
}
