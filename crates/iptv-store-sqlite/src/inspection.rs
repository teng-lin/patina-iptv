//! Bounded semantic query reads. Physical import row IDs never enter cursors.
use std::time::Instant;

use iptv_epg::{ChannelKey, FeedId};
use iptv_import::CancelToken;
use iptv_library::inspection::*;
use iptv_library::{Choice, SourceId};
use iptv_providers::ContentId;
use rusqlite::{
    OptionalExtension, params, params_from_iter,
    types::{Value, ValueRef},
};
use sha2::{Digest, Sha256};

use crate::{EpgOverride, Store};

/// Effective live-content access supplied by the accepting runtime owner.
/// The adapter translates this captured authority into pre-limit predicates.
#[derive(Clone, Debug)]
pub struct InspectionAccess {
    pub unrestricted: bool,
    pub grants: Vec<iptv_core::parental::Target>,
}

type Result<T> = crate::Result<InspectionResult<T>>;

pub fn digest(parts: impl IntoIterator<Item = impl AsRef<[u8]>>) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        let part = part.as_ref();
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part);
    }
    format!("{:x}", hash.finalize())
}

fn choice(overlay: Option<EpgOverride>) -> Choice {
    match overlay {
        Some(EpgOverride::Disabled) => Choice::NoGuide,
        Some(value @ EpgOverride::Xmltv { .. }) => match value.feed() {
            Some(feed) => match value {
                EpgOverride::Xmltv { epg_channel, .. } => Choice::Channel { feed, epg_channel },
                _ => unreachable!(),
            },
            None => Choice::Automatic,
        },
        None => Choice::Automatic,
    }
}

fn info(row: crate::ChannelRow) -> ChannelInfo {
    ChannelInfo {
        key: ChannelRef {
            source: row.source_id,
            content: row.channel.id,
        },
        name: row.name,
        number: row.number,
        hidden: row.hidden,
        blocked: row.blocked,
        epg_id: row.channel.epg_id,
        epg_name: row.channel.epg_name,
        choice: choice(row.epg_override),
    }
}

fn scope(store: &Store, query: &Query) -> crate::Result<InspectionResult<Vec<SourceId>>> {
    if !query.valid() {
        return Ok(Err(InspectionError::InvalidRequest));
    }
    let all = matches!(query.scope, SourceScope::AllEnabled);
    let mut ids = match &query.scope {
        SourceScope::Source(id) => vec![*id],
        SourceScope::Selected(ids) => ids.clone(),
        SourceScope::AllEnabled => store.sources()?.into_iter().map(|s| s.id).collect(),
    };
    ids.sort();
    ids.dedup();
    let mut enabled = Vec::new();
    for id in ids {
        if store.source(id)?.is_none() {
            return Ok(Err(InspectionError::SourceMissing(id)));
        }
        if !store.source_policy(id)?.enabled {
            if !all {
                return Ok(Err(InspectionError::SourceDisabled(id)));
            }
        } else {
            enabled.push(id);
        }
    }
    if enabled.len() > MAX_SOURCES {
        return Ok(Err(InspectionError::LimitExceeded));
    }
    Ok(Ok(enabled))
}

fn query_context(query: &Query, mode: &str) -> String {
    digest([
        format!("{:?}", query.scope),
        query.text.clone(),
        query.include_hidden.to_string(),
        mode.to_owned(),
    ])
}

/// Canonical typed fields are borrowed directly from SQLite and batched into
/// the digest. Formatting/allocating twelve owned Values per channel made a
/// 100k-channel page exceed the deadline on Apple debug builds.
struct SemanticHash {
    hash: Sha256,
    buffer: Vec<u8>,
}
impl SemanticHash {
    fn new() -> Self {
        Self {
            hash: Sha256::new(),
            buffer: Vec::with_capacity(64 * 1024),
        }
    }
    fn bytes(&mut self, tag: u8, bytes: &[u8]) {
        self.buffer.push(tag);
        self.buffer
            .extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        if self.buffer.len() + bytes.len() > 64 * 1024 {
            self.hash.update(&self.buffer);
            self.buffer.clear();
            self.hash.update(bytes);
        } else {
            self.buffer.extend_from_slice(bytes);
        }
    }
    fn row(&mut self, row: &rusqlite::Row<'_>, columns: usize) -> crate::Result<()> {
        for column in 0..columns {
            match row.get_ref(column)? {
                ValueRef::Null => self.bytes(0, &[]),
                ValueRef::Integer(value) => self.bytes(1, &value.to_le_bytes()),
                ValueRef::Real(value) => self.bytes(2, &value.to_bits().to_le_bytes()),
                ValueRef::Text(value) => self.bytes(3, value),
                ValueRef::Blob(value) => self.bytes(4, value),
            }
        }
        Ok(())
    }
    fn finish(mut self) -> String {
        self.hash.update(&self.buffer);
        format!("{:x}", self.hash.finalize())
    }
}
fn snapshot(store: &Store, ids: &[SourceId]) -> crate::Result<String> {
    let mut hash = SemanticHash::new();
    for id in ids {
        hash.bytes(1, &id.0.to_le_bytes());
        hash.bytes(3, store.source_incarnation(*id)?.as_bytes());
        let mut statement = store.conn.prepare_cached(
            "SELECT c.id,c.name,c.epg_id,c.epg_name,c.number,
                o.custom_name,o.custom_number,COALESCE(o.hidden,0),COALESCE(o.blocked,0),
                o.epg_mode,o.epg_source,o.epg_channel
             FROM channels c LEFT JOIN channel_overlays o
                ON o.source_id=c.source_id AND o.channel_id=c.id
             WHERE c.source_id=?1 ORDER BY c.id",
        )?;
        let mut rows = statement.query([id.0])?;
        while let Some(row) = rows.next()? {
            hash.row(row, 12)?;
        }
    }
    Ok(hash.finish())
}

fn name_keys(subject: &ChannelInfo) -> Vec<String> {
    let mut keys = Vec::new();
    for name in std::iter::once(subject.name.as_str()).chain(subject.epg_name.as_deref()) {
        for name in std::iter::once(name).chain(iptv_epg::strip_tag_prefix(name)) {
            let normalized = iptv_epg::normalize_name(name);
            if !normalized.is_empty() && !keys.contains(&normalized) {
                keys.push(normalized);
            }
        }
    }
    keys
}

fn check_cursor(query: &Query, context: &str, snapshot: &str, columns: usize) -> bool {
    query.cursor.as_ref().is_none_or(|c| {
        c.context == context
            && c.snapshot == snapshot
            && c.after.len() == columns
            && c.after.iter().all(|s| s.len() <= 4096)
    })
}

fn page<T>(
    mut items: Vec<T>,
    limit: u32,
    context: String,
    snapshot: String,
    key: impl Fn(&T) -> Vec<String>,
) -> Page<T> {
    let more = items.len() > limit as usize;
    items.truncate(limit as usize);
    let next = more.then(|| Cursor {
        context,
        snapshot,
        after: key(items.last().expect("nonzero page size")),
    });
    Page { items, next }
}

impl Store {
    /// Scope trusted runtime access to one coherent read; restore prior context on every exit.
    pub fn inspection_with_access<T>(
        &self,
        access: InspectionAccess,
        read: impl FnOnce(&Self) -> crate::Result<T>,
    ) -> crate::Result<T> {
        struct Restore<'a> {
            slot: &'a std::cell::RefCell<Option<InspectionAccess>>,
            previous: Option<InspectionAccess>,
        }
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.slot.replace(self.previous.take());
            }
        }
        let restore = Restore {
            slot: &self.inspection_access,
            previous: self.inspection_access.replace(Some(access)),
        };
        let result = self.read_snapshot(read);
        drop(restore);
        result
    }

    fn inspection_live_filter(
        &self,
        channel: &str,
        overlay: &str,
        args: &mut Vec<Value>,
    ) -> String {
        let access = self.inspection_access.borrow();
        if access.as_ref().is_some_and(|access| access.unrestricted) {
            return "1".into();
        }
        // No trusted runtime context means restricted, with no implicit grants.
        let grants: Vec<_> = access
            .as_ref()
            .into_iter()
            .flat_map(|access| &access.grants)
            .map(|target| {
                serde_json::json!([
                    target.source,
                    crate::parental::kind_slug(target.kind),
                    target.id
                ])
            })
            .collect();
        args.push(Value::Text(
            serde_json::to_string(&grants).expect("target tuples serialize"),
        ));
        let grant_arg = args.len();
        let granted = |kind: &str, id: &str| {
            format!(
                "EXISTS(SELECT 1 FROM json_each(?{grant_arg}) granted
                 WHERE json_extract(granted.value,'$[0]')={channel}.source_id
                 AND json_extract(granted.value,'$[1]')='{kind}'
                 AND json_extract(granted.value,'$[2]')={id})"
            )
        };
        let own_grant = granted("live", &format!("{channel}.id"));
        let parent_grant = granted("group", "membership.group_id");
        format!("({own_grant} OR (COALESCE({overlay}.blocked,0)=0
            AND NOT EXISTS(SELECT 1 FROM parental_blocks block WHERE block.source_id={channel}.source_id AND block.kind='live' AND block.content_id={channel}.id AND block.blocked=1)
            AND NOT EXISTS(SELECT 1 FROM (
                SELECT group_id FROM channel_groups WHERE source_id={channel}.source_id AND channel_id={channel}.id
                UNION SELECT 'custom:'||g.id FROM custom_groups g JOIN custom_group_members m ON m.group_id=g.id WHERE g.source_id={channel}.source_id AND m.channel_id={channel}.id
            ) membership WHERE NOT ({parent_grant}) AND (
                EXISTS(SELECT 1 FROM parental_blocks block WHERE block.source_id={channel}.source_id AND block.kind='group' AND block.content_id=membership.group_id AND block.blocked=1)
                OR EXISTS(SELECT 1 FROM groups adult WHERE adult.source_id={channel}.source_id AND adult.id=membership.group_id AND adult.kind IN ('live','radio') AND adult.adult=1
                    AND NOT EXISTS(SELECT 1 FROM parental_blocks allow WHERE allow.source_id={channel}.source_id
                        AND allow.kind='group' AND allow.content_id=adult.id AND allow.blocked=0))
            ))))")
    }

    fn inspection_live_allowed(&self, key: &ChannelRef) -> crate::Result<bool> {
        let mut args = vec![
            Value::Integer(key.source.0),
            Value::Text(key.content.as_str().to_owned()),
        ];
        let access = self.inspection_live_filter("c", "o", &mut args);
        self.conn
            .query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM channels c LEFT JOIN channel_overlays o
                ON o.source_id=c.source_id AND o.channel_id=c.id
                WHERE c.source_id=?1 AND c.id=?2 AND ({access}))"
                ),
                params_from_iter(args),
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    /// Incarnation of the cached evidence, only while it still describes the
    /// current enabled configured feed. A reused ID cannot borrow old rows.
    pub fn inspection_stored_feed_incarnation(
        &self,
        source: SourceId,
        feed: FeedId,
    ) -> crate::Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT i.incarnation FROM guide_feed_incarnations i JOIN guide_sources g
             ON g.source_id=i.source_id AND g.feed_id=i.feed_id
             JOIN epg_feeds f ON f.source_id=g.source_id AND f.feed_id=g.feed_id
             JOIN store_meta m ON m.key='inspection.feed.' || g.source_id || '.' || g.feed_id
             WHERE g.source_id=?1 AND g.feed_id=?2 AND g.enabled=1
             AND m.value=i.incarnation AND f.url=g.url AND f.shift_minutes=g.shift_minutes",
                params![source.0, feed.0],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Install an effective SQLite instruction deadline/cancellation handler for
    /// this one analysis job, including sorts and semantic-fingerprint scans.
    pub fn inspection_read<T>(
        &mut self,
        cancel: CancelToken,
        deadline: Instant,
        operation: impl FnOnce(&Self) -> crate::Result<InspectionResult<T>>,
    ) -> crate::Result<InspectionResult<T>> {
        self.inspection_read_observed(cancel, deadline, || {}, operation)
    }

    /// `Instant` panics on wasm32-unknown-unknown: the browser build has no
    /// monotonic clock to bound an inspection with (yet), so it refuses one.
    #[cfg(target_family = "wasm")]
    #[doc(hidden)]
    pub fn inspection_read_observed<T>(
        &mut self,
        _cancel: CancelToken,
        _deadline: Instant,
        _progress: impl FnMut() + Send + 'static,
        _operation: impl FnOnce(&Self) -> crate::Result<InspectionResult<T>>,
    ) -> crate::Result<InspectionResult<T>> {
        Err(crate::StoreError::Unavailable(
            "bounded inspection needs a monotonic clock the browser build does not have",
        ))
    }

    /// Adapter instrumentation for an actual SQLite scan, not a queued-lane barrier.
    #[cfg(not(target_family = "wasm"))]
    #[doc(hidden)]
    pub fn inspection_read_observed<T>(
        &mut self,
        cancel: CancelToken,
        deadline: Instant,
        mut progress: impl FnMut() + Send + 'static,
        operation: impl FnOnce(&Self) -> crate::Result<InspectionResult<T>>,
    ) -> crate::Result<InspectionResult<T>> {
        if cancel.is_canceled() {
            return Ok(Err(InspectionError::Canceled));
        }
        if Instant::now() >= deadline {
            return Ok(Err(InspectionError::Deadline));
        }
        let flag = cancel.clone();
        self.conn.progress_handler(
            1000,
            Some(move || {
                if flag.is_canceled() || Instant::now() >= deadline {
                    return true;
                }
                progress();
                flag.is_canceled() || Instant::now() >= deadline
            }),
        )?;
        let result = operation(self);
        self.conn.progress_handler(0, None::<fn() -> bool>)?;
        if cancel.is_canceled() {
            return Ok(Err(InspectionError::Canceled));
        }
        if Instant::now() >= deadline {
            return Ok(Err(InspectionError::Deadline));
        }
        result
    }

    pub fn inspection_channel(&self, key: &ChannelRef) -> Result<ChannelInfo> {
        self.read_snapshot(|store| store.inspection_channel_inner(key))
    }

    fn inspection_channel_inner(&self, key: &ChannelRef) -> Result<ChannelInfo> {
        let query = Query::source(key.source);
        if let Err(error) = scope(self, &query)? {
            return Ok(Err(error));
        }
        let Some(channel) = self.channel(key.source, &key.content)? else {
            return Ok(Err(InspectionError::ChannelMissing));
        };
        if !self.inspection_live_allowed(key)? {
            return Ok(Err(InspectionError::PermissionDenied));
        }
        Ok(Ok(info(channel)))
    }

    /// Stable ordering is `(source, content)`; search matches displayed/provider
    /// names, with the same literal behavior on FTS and non-FTS hosts.
    pub fn inspection_channels(&self, query: &Query) -> Result<Page<ChannelInfo>> {
        let tx = self.begin_read_snapshot()?;
        let result = self.inspection_channels_inner(query);
        if let Some(tx) = tx {
            tx.commit()?;
        }
        result
    }

    fn inspection_channels_inner(&self, query: &Query) -> Result<Page<ChannelInfo>> {
        let ids = match scope(self, query)? {
            Ok(ids) => ids,
            Err(e) => return Ok(Err(e)),
        };
        let context = query_context(query, "channels");
        let snapshot = snapshot(self, &ids)?;
        if !check_cursor(query, &context, &snapshot, 2) {
            return Ok(Err(InspectionError::StaleCursor));
        }
        let after = match query.cursor.as_ref() {
            Some(c) => match c.after[0].parse::<i64>() {
                Ok(source) => (source, c.after[1].as_str()),
                Err(_) => return Ok(Err(InspectionError::StaleCursor)),
            },
            None => (i64::MIN, ""),
        };
        let mut items = Vec::new();
        let words: Vec<_> = query
            .text
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        for source in ids {
            if source.0 < after.0 {
                continue;
            }
            let mut args = vec![
                Value::Integer(source.0),
                Value::Text(if source.0 == after.0 { after.1 } else { "" }.to_owned()),
                Value::Integer(i64::from(query.limit + 1)),
            ];
            let mut filters = String::new();
            for word in &words {
                args.push(Value::Text(crate::search::like_pattern(word)));
                let at = args.len();
                filters.push_str(&format!(
                    " AND (lower(c.name) LIKE ?{at} ESCAPE '\\'
                    OR lower(COALESCE(o.custom_name,'')) LIKE ?{at} ESCAPE '\\')"
                ));
            }
            let visible = if query.include_hidden {
                ""
            } else {
                " AND COALESCE(o.hidden,0)=0"
            };
            let access = self.inspection_live_filter("c", "o", &mut args);
            let sql = format!(
                "SELECT {} FROM channels c {} WHERE c.source_id=?1
                AND c.id>?2 AND ({access}) {visible} {filters}
                ORDER BY c.id LIMIT ?3",
                crate::channels::ROW_COLUMNS,
                crate::channels::OVERLAY_JOINS
            );
            let mut statement = self.conn.prepare(&sql)?;
            let mut rows = statement.query(params_from_iter(args))?;
            while let Some(row) = rows.next()? {
                items.push(info(crate::channels::decode(row)?));
            }
            if items.len() > query.limit as usize {
                break;
            }
        }
        Ok(Ok(page(items, query.limit, context, snapshot, |c| {
            vec![
                c.key.source.0.to_string(),
                c.key.content.as_str().to_owned(),
            ]
        })))
    }

    pub fn inspection_programs(
        &self,
        query: &Query,
        window: TimeWindow,
        channel: Option<&ChannelRef>,
    ) -> Result<Page<ProgramInfo>> {
        if !window.valid() {
            return Ok(Err(InspectionError::InvalidRequest));
        }
        let tx = self.begin_read_snapshot()?;
        let ids = match scope(self, query)? {
            Ok(ids) => ids,
            Err(e) => return Ok(Err(e)),
        };
        if channel.is_some_and(|c| !ids.contains(&c.source)) {
            return Ok(Err(InspectionError::InvalidRequest));
        }
        if let Some(channel) = channel
            && let Err(e) = self.inspection_channel(channel)?
        {
            return Ok(Err(e));
        }
        let context = query_context(query, &format!("programs:{window:?}:{channel:?}"));
        let mut hash = Sha256::new();
        hash.update(snapshot(self, &ids)?.as_bytes());
        let mut items = Vec::new();
        for source in &ids {
            let incarnation = self.source_incarnation(*source)?;
            let mut args = vec![
                Value::Integer(source.0),
                Value::Integer(window.from),
                Value::Integer(window.to),
                Value::Integer(i64::from(query.include_hidden)),
            ];
            let access = self.inspection_live_filter("ch", "o", &mut args);
            let mut statement = self.conn.prepare_cached(&format!(
                "SELECT b.channel_key,c.feed_id,c.epg_channel,p.start,p.stop,p.title
                 FROM epg_bindings b JOIN epg_channels c ON c.id=b.epg_channel_id
                 JOIN epg_programs p ON p.channel_id=c.id
                 JOIN channels ch ON ch.source_id=b.source_id AND ch.id=b.channel_key
                 LEFT JOIN channel_overlays o ON o.source_id=b.source_id AND o.channel_id=b.channel_key
                 WHERE b.source_id=?1 AND p.start<?3 AND p.stop>?2
                   AND ({access}) AND (?4 OR COALESCE(o.hidden,0)=0)
                 ORDER BY b.channel_key,p.start"))?;
            let mut rows = statement.query(params_from_iter(args))?;
            while let Some(row) = rows.next()? {
                let key: String = row.get(0)?;
                if channel.is_some_and(|ch| ch.source != *source || ch.content.as_str() != key) {
                    continue;
                }
                let title: String = row.get(5)?;
                if !query
                    .text
                    .split_whitespace()
                    .all(|w| title.to_lowercase().contains(&w.to_lowercase()))
                {
                    continue;
                }
                let feed = FeedId(row.get(1)?);
                let Some(feed_incarnation) =
                    self.inspection_stored_feed_incarnation(*source, feed)?
                else {
                    continue;
                };
                let candidate = CandidateRef {
                    source: *source,
                    feed,
                    incarnation: feed_incarnation,
                    epg_channel: row.get(2)?,
                };
                let (start, stop): (i64, i64) = (row.get(3)?, row.get(4)?);
                hash.update(
                    format!("{source:?}:{key}:{candidate:?}:{start}:{stop}:{title}").as_bytes(),
                );
                let cursor_key = vec![source.0.to_string(), key.clone(), start.to_string()];
                let beyond = query.cursor.as_ref().is_none_or(|c| {
                    c.after.len() == 3
                        && c.after[0]
                            .parse::<i64>()
                            .ok()
                            .zip(c.after[2].parse::<i64>().ok())
                            .is_some_and(|(s, t)| {
                                (source.0, key.as_str(), start) > (s, c.after[1].as_str(), t)
                            })
                });
                if beyond && items.len() <= query.limit as usize {
                    let _ = cursor_key;
                    items.push(ProgramInfo {
                        channel: ChannelRef {
                            source: *source,
                            content: ContentId::from_stored(key),
                        },
                        candidate,
                        source_incarnation: incarnation.clone(),
                        evidence_revision: String::new(),
                        provider_event_id: None,
                        start,
                        stop,
                        title,
                    });
                }
            }
        }
        let snapshot = format!("{:x}", hash.finalize());
        if !check_cursor(query, &context, &snapshot, 3) {
            return Ok(Err(InspectionError::StaleCursor));
        }
        for item in &mut items {
            item.evidence_revision = snapshot.clone();
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(Ok(page(items, query.limit, context, snapshot, |p| {
            vec![
                p.channel.source.0.to_string(),
                p.channel.content.as_str().to_owned(),
                p.start.to_string(),
            ]
        })))
    }

    pub fn inspection_candidates(
        &self,
        key: &ChannelRef,
        query: &Query,
    ) -> Result<Page<Candidate>> {
        let tx = self.begin_read_snapshot()?;
        let subject = match self.inspection_channel(key)? {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        };
        if query.scope != SourceScope::Source(key.source) || !query.valid() {
            return Ok(Err(InspectionError::InvalidRequest));
        }
        let context = query_context(query, &format!("candidates:{key:?}"));
        let channels = self.guide_channels(key.source)?;
        let mut rows = Vec::new();
        for channel in channels {
            let Some(incarnation) =
                self.inspection_stored_feed_incarnation(key.source, channel.feed)?
            else {
                continue;
            };
            let mut evidence = Vec::new();
            if matches!(&subject.choice,Choice::Channel{feed,epg_channel}
                if *feed==channel.feed && *epg_channel==channel.epg_channel)
            {
                evidence.push(Evidence::Manual);
            }
            if let Some(id) = &subject.epg_id {
                if id.trim() == channel.epg_channel {
                    evidence.push(Evidence::ExactId);
                } else if id.trim().to_lowercase() == channel.epg_channel.to_lowercase() {
                    evidence.push(Evidence::CaseInsensitiveId);
                }
            }
            if let Some(name) = &channel.name
                && name_keys(&subject).contains(&iptv_epg::normalize_name(name))
            {
                evidence.push(Evidence::NormalizedName);
            }
            if !query.text.split_whitespace().all(|w| {
                channel
                    .name
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&w.to_lowercase())
                    || channel
                        .epg_channel
                        .to_lowercase()
                        .contains(&w.to_lowercase())
            }) {
                continue;
            }
            if evidence.is_empty() {
                evidence.push(Evidence::Alternative);
            }
            rows.push(Candidate {
                key: CandidateRef {
                    source: key.source,
                    feed: channel.feed,
                    incarnation,
                    epg_channel: channel.epg_channel,
                },
                names: channel.name.clone().into_iter().collect(),
                name: channel.name,
                metadata: Vec::new(),
                assigned: channel.assigned,
                evidence,
                binding: None,
            });
        }
        rows.sort_by(|a, b| {
            (&a.key.feed, &a.key.epg_channel).cmp(&(&b.key.feed, &b.key.epg_channel))
        });
        let snapshot = digest(rows.iter().map(|r| format!("{r:?}")));
        if !check_cursor(query, &context, &snapshot, 2) {
            return Ok(Err(InspectionError::StaleCursor));
        }
        if let Some(cursor) = &query.cursor {
            let Ok(feed) = cursor.after[0].parse::<u32>() else {
                return Ok(Err(InspectionError::StaleCursor));
            };
            rows.retain(|c| {
                (c.key.feed.0, c.key.epg_channel.as_str()) > (feed, cursor.after[1].as_str())
            });
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(Ok(page(rows, query.limit, context, snapshot, |c| {
            vec![c.key.feed.0.to_string(), c.key.epg_channel.clone()]
        })))
    }

    pub fn inspection_coverage(&self, query: &Query, window: TimeWindow) -> Result<Page<Coverage>> {
        if !window.valid() {
            return Ok(Err(InspectionError::InvalidRequest));
        }
        let tx = self.begin_read_snapshot()?;
        let ids = match scope(self, query)? {
            Ok(ids) => ids,
            Err(e) => return Ok(Err(e)),
        };
        let context = query_context(query, &format!("audit:{window:?}"));
        let channel_snapshot = snapshot(self, &ids)?;
        let mut hash = SemanticHash::new();
        hash.bytes(3, channel_snapshot.as_bytes());
        let mut name_counts = std::collections::HashMap::new();
        let mut suspicious = std::collections::HashSet::new();
        let mut failed_feeds = std::collections::HashSet::new();
        for source in &ids {
            let revisions = self.guide_revisions(*source)?;
            hash.bytes(1, &source.0.to_le_bytes());
            hash.bytes(1, &revisions.mapping.to_le_bytes());
            hash.bytes(1, &revisions.configuration.to_le_bytes());
            let results = self.epg_feed_results(*source)?;
            for guide in self.guide_sources(*source)? {
                if guide.enabled
                    && results
                        .iter()
                        .any(|r| r.feed == guide.feed && r.error.is_some())
                {
                    failed_feeds.insert((*source, guide.feed));
                }
                hash.bytes(1, &guide.feed.0.to_le_bytes());
                hash.bytes(
                    3,
                    self.feed_incarnation(*source, guide.feed)?
                        .as_deref()
                        .unwrap_or("")
                        .as_bytes(),
                );
                if guide.shift_minutes.unsigned_abs() > 24 * 60 {
                    suspicious.insert((*source, guide.feed));
                }
            }
            for channel in self.guide_channels(*source)? {
                if self
                    .inspection_stored_feed_incarnation(*source, channel.feed)?
                    .is_none()
                {
                    continue;
                }
                hash.bytes(1, &channel.feed.0.to_le_bytes());
                hash.bytes(3, channel.epg_channel.as_bytes());
                if let Some(name) = channel.name {
                    hash.bytes(3, name.as_bytes());
                    *name_counts
                        .entry((*source, channel.feed, iptv_epg::normalize_name(&name)))
                        .or_insert(0usize) += 1;
                }
            }
            for (sql,columns) in [
                ("SELECT last_error_kind FROM epg_guides WHERE source_id=?1",1),
                ("SELECT feed_id,error_kind FROM epg_feed_results WHERE source_id=?1 ORDER BY feed_id",2),
                ("SELECT b.channel_key,c.feed_id,c.epg_channel,b.match_kind FROM epg_bindings b
                   JOIN epg_channels c ON c.id=b.epg_channel_id WHERE b.source_id=?1 ORDER BY b.channel_key",4),
                ("SELECT c.feed_id,c.epg_channel,MIN(p.start),MAX(p.stop) FROM epg_programs p
                   JOIN epg_channels c ON c.id=p.channel_id WHERE c.source_id=?1
                   GROUP BY c.feed_id,c.epg_channel ORDER BY c.feed_id,c.epg_channel",4),
            ] {
                let mut statement=self.conn.prepare_cached(sql)?;
                let mut rows=statement.query([source.0])?;
                while let Some(row)=rows.next()?{hash.row(row,columns)?;}
            }
            let mut statement = self.conn.prepare_cached(
                "SELECT c.feed_id,c.epg_channel,p.start,p.stop,p.title FROM epg_programs p
                 JOIN epg_channels c ON c.id=p.channel_id WHERE c.source_id=?1
                 AND p.start<?3 AND p.stop>?2 ORDER BY c.feed_id,c.epg_channel,p.start",
            )?;
            let mut rows = statement.query(params![source.0, window.from, window.to])?;
            while let Some(row) = rows.next()? {
                hash.row(row, 5)?;
            }
        }
        let snapshot = hash.finish();
        if !check_cursor(query, &context, &snapshot, 2) {
            return Ok(Err(InspectionError::StaleCursor));
        }
        let mut channel_query = query.clone();
        if let Some(cursor) = &mut channel_query.cursor {
            cursor.context = query_context(query, "channels");
            cursor.snapshot = channel_snapshot;
        }
        let channels = match self.inspection_channels_inner(&channel_query)? {
            Ok(p) => p,
            Err(e) => return Ok(Err(e)),
        };
        let mut items = Vec::new();
        for channel in channels.items {
            let mut binding = self.epg_binding(
                channel.key.source,
                &ChannelKey::new(channel.key.content.as_str()),
            )?;
            let stale_binding = binding
                .as_ref()
                .map(|(feed, _, _)| {
                    self.inspection_stored_feed_incarnation(channel.key.source, *feed)
                })
                .transpose()?
                .is_some_and(|incarnation| incarnation.is_none());
            if stale_binding {
                binding = None;
            }
            let guide_error: Option<String> = self
                .conn
                .query_row(
                    "SELECT last_error_kind FROM epg_guides WHERE source_id=?1",
                    [channel.key.source.0],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            let (mut covered, mut count, mut latest, mut earliest, mut previous) =
                (0u64, 0, None, None, window.from);
            if let Some((feed, ref epg, _)) = binding {
                let mut statement=self.conn.prepare_cached(
                    "SELECT p.start,p.stop FROM epg_programs p JOIN epg_channels c ON c.id=p.channel_id
                     WHERE c.source_id=?1 AND c.feed_id=?2 AND c.epg_channel=?3 ORDER BY p.start")?;
                let mut rows = statement.query(params![channel.key.source.0, feed.0, epg])?;
                while let Some(row) = rows.next()? {
                    let (start, stop): (i64, i64) = (row.get(0)?, row.get(1)?);
                    earliest = Some(earliest.map_or(start, |old: i64| old.min(start)));
                    latest = Some(latest.map_or(stop, |old: i64| old.max(stop)));
                    let from = start.max(window.from);
                    let to = stop.min(window.to);
                    if to > from {
                        count += 1;
                    }
                    let from = from.max(previous);
                    if to > from {
                        covered += (to - from) as u64;
                        previous = to;
                    }
                }
            }
            let ambiguous = binding.as_ref().is_some_and(|(feed, _, kind)| {
                *kind == iptv_epg::MatchKind::Name
                    && name_keys(&channel).iter().any(|name| {
                        name_counts
                            .get(&(channel.key.source, *feed, name.clone()))
                            .copied()
                            .unwrap_or(0)
                            > 1
                    })
            });
            let failed = match &binding {
                Some((feed, _, _)) => failed_feeds.contains(&(channel.key.source, *feed)),
                None => failed_feeds
                    .iter()
                    .any(|(source, _)| *source == channel.key.source),
            };
            let timing = binding
                .as_ref()
                .is_some_and(|(feed, _, _)| suspicious.contains(&(channel.key.source, *feed)))
                || earliest.is_some_and(|start| start > window.to.saturating_add(7 * 86_400));
            let diagnosis = if channel.choice == Choice::NoGuide {
                Diagnosis::Disabled
            } else if guide_error.is_some() || failed {
                Diagnosis::InputFailure
            } else if stale_binding {
                Diagnosis::Stale
            } else if binding.is_none() && matches!(channel.choice, Choice::Channel { .. }) {
                Diagnosis::UnresolvedOverride
            } else if binding.is_none()
                && channel.name.trim().is_empty()
                && channel
                    .epg_id
                    .as_deref()
                    .is_none_or(|id| id.trim().is_empty())
                && channel
                    .epg_name
                    .as_deref()
                    .is_none_or(|name| name.trim().is_empty())
            {
                Diagnosis::MissingMetadata
            } else if binding.is_none() {
                Diagnosis::Unmatched
            } else if ambiguous {
                Diagnosis::Ambiguous
            } else if timing {
                Diagnosis::SuspiciousTiming
            } else if latest.is_some_and(|t| t <= window.from) {
                Diagnosis::Stale
            } else if count == 0 {
                Diagnosis::Empty
            } else if covered < (window.to - window.from) as u64 {
                Diagnosis::Partial
            } else {
                Diagnosis::Covered
            };
            let binding = binding
                .map(|(feed, epg_channel, _)| -> crate::Result<_> {
                    Ok(CandidateRef {
                        source: channel.key.source,
                        feed,
                        incarnation: self
                            .inspection_stored_feed_incarnation(channel.key.source, feed)?
                            .expect("validated incarnation"),
                        epg_channel,
                    })
                })
                .transpose()?;
            items.push(Coverage {
                channel,
                diagnosis,
                window,
                covered_seconds: covered,
                program_count: count,
                binding,
                identity_verified: false,
            });
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(Ok(Page {
            items,
            next: channels.next.map(|mut cursor| {
                cursor.context = context.clone();
                cursor.snapshot = snapshot.clone();
                cursor
            }),
        }))
    }
}
