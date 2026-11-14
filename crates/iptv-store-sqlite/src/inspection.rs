//! Bounded semantic query reads. Physical import row IDs never enter cursors.
use std::time::Instant;

use iptv_epg::{ChannelKey, FeedId};
use iptv_import::CancelToken;
use iptv_library::inspection::*;
use iptv_library::{Choice, SourceId};
use iptv_providers::ContentId;
use rusqlite::{OptionalExtension, params, params_from_iter, types::Value};
use sha2::{Digest, Sha256};

use crate::{EpgOverride, Store};

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
    if ids.len() > MAX_SOURCES {
        return Ok(Err(InspectionError::LimitExceeded));
    }
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
    Ok(Ok(enabled))
}

fn context(query: &Query, mode: &str) -> String {
    digest([
        format!("{:?}", query.scope),
        query.text.clone(),
        query.include_hidden.to_string(),
        mode.to_owned(),
    ])
}

fn snapshot(store: &Store, ids: &[SourceId]) -> crate::Result<String> {
    let mut hash = Sha256::new();
    for id in ids {
        hash.update(id.0.to_le_bytes());
        hash.update(store.source_incarnation(*id)?.as_bytes());
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
            for i in 0..12 {
                let value: Value = row.get(i)?;
                let bytes = format!("{value:?}");
                hash.update((bytes.len() as u64).to_le_bytes());
                hash.update(bytes.as_bytes());
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
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
    /// Install an effective SQLite instruction deadline/cancellation handler for
    /// this one analysis job, including sorts and semantic-fingerprint scans.
    pub fn inspection_read<T>(
        &mut self,
        cancel: CancelToken,
        deadline: Instant,
        operation: impl FnOnce(&Self) -> crate::Result<InspectionResult<T>>,
    ) -> crate::Result<InspectionResult<T>> {
        if cancel.is_cancelled() {
            return Ok(Err(InspectionError::Cancelled));
        }
        if Instant::now() >= deadline {
            return Ok(Err(InspectionError::Deadline));
        }
        let flag = cancel.clone();
        self.conn.progress_handler(
            1000,
            Some(move || flag.is_cancelled() || Instant::now() >= deadline),
        )?;
        let result = operation(self);
        self.conn.progress_handler(0, None::<fn() -> bool>)?;
        if cancel.is_cancelled() {
            return Ok(Err(InspectionError::Cancelled));
        }
        if Instant::now() >= deadline {
            return Ok(Err(InspectionError::Deadline));
        }
        result
    }

    pub fn inspection_channel(&self, key: &ChannelRef) -> Result<ChannelInfo> {
        let query = Query::source(key.source);
        if let Err(error) = scope(self, &query)? {
            return Ok(Err(error));
        }
        let Some(channel) = self.channel(key.source, &key.content)? else {
            return Ok(Err(InspectionError::ChannelMissing));
        };
        if channel.blocked {
            return Ok(Err(InspectionError::PermissionDenied));
        }
        Ok(Ok(info(channel)))
    }

    /// Stable ordering is `(source, content)`; search matches displayed/provider
    /// names, with the same literal behavior on FTS and non-FTS hosts.
    pub fn inspection_channels(&self, query: &Query) -> Result<Page<ChannelInfo>> {
        let ids = match scope(self, query)? {
            Ok(ids) => ids,
            Err(e) => return Ok(Err(e)),
        };
        let tx = self.conn.unchecked_transaction()?;
        let context = context(query, "channels");
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
            let sql = format!(
                "SELECT {} FROM channels c {} WHERE c.source_id=?1
                AND c.id>?2 AND COALESCE(o.blocked,0)=0 {visible} {filters}
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
        tx.commit()?;
        Ok(Ok(page(items, query.limit, context, snapshot, |c| {
            vec![
                c.key.source.0.to_string(),
                c.key.content.as_str().to_owned(),
            ]
        })))
    }

    pub fn inspection_programmes(
        &self,
        query: &Query,
        window: TimeWindow,
        channel: Option<&ChannelRef>,
    ) -> Result<Page<ProgrammeInfo>> {
        if !window.valid() {
            return Ok(Err(InspectionError::InvalidRequest));
        }
        let ids = match scope(self, query)? {
            Ok(ids) => ids,
            Err(e) => return Ok(Err(e)),
        };
        if channel.is_some_and(|c| !ids.contains(&c.source)) {
            return Ok(Err(InspectionError::InvalidRequest));
        }
        if let Some(channel) = channel {
            if let Err(e) = self.inspection_channel(channel)? {
                return Ok(Err(e));
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        let context = context(query, &format!("programmes:{window:?}:{channel:?}"));
        let mut hash = Sha256::new();
        hash.update(snapshot(self, &ids)?.as_bytes());
        let mut items = Vec::new();
        for source in &ids {
            let incarnation = self.source_incarnation(*source)?;
            let mut statement = self.conn.prepare_cached(
                "SELECT b.channel_key,c.feed_id,c.epg_channel,p.start,p.stop,p.title
                 FROM epg_bindings b JOIN epg_channels c ON c.id=b.epg_channel_id
                 JOIN epg_programmes p ON p.channel_id=c.id
                 JOIN channels ch ON ch.source_id=b.source_id AND ch.id=b.channel_key
                 LEFT JOIN channel_overlays o ON o.source_id=b.source_id AND o.channel_id=b.channel_key
                 WHERE b.source_id=?1 AND p.start<?3 AND p.stop>?2
                   AND COALESCE(o.blocked,0)=0 AND (?4 OR COALESCE(o.hidden,0)=0)
                 ORDER BY b.channel_key,p.start")?;
            let mut rows = statement.query(params![
                source.0,
                window.from,
                window.to,
                query.include_hidden
            ])?;
            while let Some(row) = rows.next()? {
                let key: String = row.get(0)?;
                if channel.is_some_and(|ch| ch.content.as_str() != key) {
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
                let candidate = CandidateRef {
                    source: *source,
                    feed,
                    incarnation: self.feed_incarnation(*source, feed)?.unwrap_or_default(),
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
                    items.push(ProgrammeInfo {
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
        tx.commit()?;
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
        let subject = match self.inspection_channel(key)? {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        };
        if query.scope != SourceScope::Source(key.source) || !query.valid() {
            return Ok(Err(InspectionError::InvalidRequest));
        }
        let tx = self.conn.unchecked_transaction()?;
        let context = context(query, &format!("candidates:{key:?}"));
        let channels = self.guide_channels(key.source)?;
        let mut rows = Vec::new();
        for channel in channels {
            let incarnation = self
                .feed_incarnation(key.source, channel.feed)?
                .unwrap_or_default();
            let mut evidence = Vec::new();
            if matches!(&subject.choice,Choice::Channel{feed,epg_channel}
                if *feed==channel.feed && *epg_channel==channel.epg_channel)
            {
                evidence.push(Evidence::Manual);
            }
            if let Some(id) = &subject.epg_id {
                if id == &channel.epg_channel {
                    evidence.push(Evidence::ExactId);
                } else if id.eq_ignore_ascii_case(&channel.epg_channel) {
                    evidence.push(Evidence::CaseInsensitiveId);
                }
            }
            if let Some(name) = &channel.name {
                if [
                    &subject.name,
                    subject.epg_name.as_ref().unwrap_or(&subject.name),
                ]
                .iter()
                .any(|subject| iptv_epg::normalize_name(subject) == iptv_epg::normalize_name(name))
                {
                    evidence.push(Evidence::NormalizedName);
                }
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
        tx.commit()?;
        Ok(Ok(page(rows, query.limit, context, snapshot, |c| {
            vec![c.key.feed.0.to_string(), c.key.epg_channel.clone()]
        })))
    }

    pub fn inspection_coverage(&self, query: &Query, window: TimeWindow) -> Result<Page<Coverage>> {
        if !window.valid() {
            return Ok(Err(InspectionError::InvalidRequest));
        }
        let channels = match self.inspection_channels(query)? {
            Ok(p) => p,
            Err(e) => return Ok(Err(e)),
        };
        let mut items = Vec::new();
        for channel in channels.items {
            let binding = self.epg_binding(
                channel.key.source,
                &ChannelKey::new(channel.key.content.as_str()),
            )?;
            let guide_error: Option<String> = self
                .conn
                .query_row(
                    "SELECT last_error_kind FROM epg_guides WHERE source_id=?1",
                    [channel.key.source.0],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            let mut covered = 0u64;
            let mut count = 0;
            let mut latest = None;
            let mut previous = window.from;
            if let Some((feed, ref epg, _)) = binding {
                let mut statement=self.conn.prepare_cached(
                    "SELECT p.start,p.stop FROM epg_programmes p JOIN epg_channels c ON c.id=p.channel_id
                     WHERE c.source_id=?1 AND c.feed_id=?2 AND c.epg_channel=?3 ORDER BY p.start")?;
                let mut rows = statement.query(params![channel.key.source.0, feed.0, epg])?;
                while let Some(row) = rows.next()? {
                    let (start, stop): (i64, i64) = (row.get(0)?, row.get(1)?);
                    latest = Some(latest.map_or(stop, |old: i64| old.max(stop)));
                    let from = start.max(window.from).max(previous);
                    let to = stop.min(window.to);
                    if to > from {
                        covered += (to - from) as u64;
                        previous = to;
                        count += 1;
                    }
                }
            }
            let diagnosis = if channel.choice == Choice::NoGuide {
                Diagnosis::Disabled
            } else if guide_error.is_some() {
                Diagnosis::InputFailure
            } else if binding.is_none() && matches!(channel.choice, Choice::Channel { .. }) {
                Diagnosis::UnresolvedOverride
            } else if binding.is_none() {
                Diagnosis::Unmatched
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
                            .feed_incarnation(channel.key.source, feed)?
                            .unwrap_or_default(),
                        epg_channel,
                    })
                })
                .transpose()?;
            items.push(Coverage {
                channel,
                diagnosis,
                window,
                covered_seconds: covered,
                programme_count: count,
                binding,
                identity_verified: false,
            });
        }
        Ok(Ok(Page {
            items,
            next: channels.next,
        }))
    }
}
