//! Atomic, affected-row-conditional guide configuration edits and durable inverses.
//! Network and staged evidence validation belong to the runtime accepting a batch.
mod verification;
pub use verification::{
    RepairBinding, RepairChannel, RepairInputs, RepairPublication, RepairVerification,
};

use std::collections::{HashMap, HashSet};

use iptv_epg::FeedId;
use iptv_providers::{ContentId, SecretUrl};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};

use crate::{EpgOverride, GuideSource, Result, SourceId, Store, StoreError};

pub const MAX_REPAIR_MAPPINGS: usize = 500;
pub const REPAIR_UNDO_SECONDS: i64 = 7 * 86_400;
const MAX_IDENTITY_HISTORY_ROWS: i64 = 4096;
const MAX_IDENTITY_HISTORY_BYTES: u64 = 32 << 20;
const MAX_IDENTITY_HISTORY_RECORD_BYTES: u64 = 8 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepairChoice {
    Automatic,
    Disabled,
    Existing {
        feed: FeedId,
        epg_channel: String,
    },
    /// Index into the batch's additions. The real feed ID is allocated at commit.
    Addition {
        index: usize,
        epg_channel: String,
    },
}
#[derive(Clone, Debug)]
pub struct RepairMapping {
    pub channel: ContentId,
    pub before: Option<EpgOverride>,
    pub after: RepairChoice,
    pub replace_explicit: bool,
}
#[derive(Clone, Debug)]
pub struct RepairFeed {
    pub before: Option<GuideSource>,
    pub after: Option<GuideSource>,
}
#[derive(Clone, Debug)]
pub struct RepairCandidate {
    pub feed: FeedId,
    pub incarnation: String,
    pub epg_channel: String,
    /// A runtime-held, integrity-checked stage supplies the candidate presence.
    pub staged: bool,
}
#[derive(Clone, Debug)]
pub struct RepairBatch {
    pub source: SourceId,
    pub source_incarnation: String,
    pub mappings: Vec<RepairMapping>,
    pub feeds: Vec<RepairFeed>,
    /// Configuration only; each supplied `feed` is ignored until allocation.
    pub additions: Vec<GuideSource>,
    pub candidates: Vec<RepairCandidate>,
    pub preview_expires_at: i64,
    /// Explicit reorder of configured feeds; additions retain lowest precedence.
    pub order: Option<(Vec<FeedId>, Vec<FeedId>)>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepairCommit {
    pub id: String,
    pub source: SourceId,
    pub created_at: i64,
    pub expires_at: i64,
    pub undone: bool,
    pub mapping_revision: u64,
    pub configuration_revision: u64,
    pub added_feeds: Vec<FeedId>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepairResult {
    Committed(RepairCommit),
    Conflict(Vec<String>),
    SourceDisabled(SourceId),
    PermissionDenied,
    Missing,
    Expired,
    Invalid,
}

fn invalid() -> StoreError {
    StoreError::InvalidData("guide repair journal".into())
}

fn overlay(conn: &Connection, source: SourceId, channel: &str) -> Result<Option<EpgOverride>> {
    Ok(conn
        .query_row(
            "SELECT epg_mode, epg_source, epg_channel FROM channel_overlays
         WHERE source_id = ?1 AND channel_id = ?2",
            params![source.0, channel],
            |row| {
                Ok(EpgOverride::from_columns(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                ))
            },
        )
        .optional()?
        .flatten())
}
fn write_overlay(
    conn: &Connection,
    source: SourceId,
    channel: &str,
    value: Option<&EpgOverride>,
    now: i64,
) -> Result<()> {
    let (mode, feed, epg): (Option<&str>, Option<&str>, Option<&str>) = match value {
        None => (None, None, None),
        Some(EpgOverride::Disabled) => (Some("none"), None, None),
        Some(EpgOverride::Xmltv {
            epg_source,
            epg_channel,
        }) => (Some("xmltv"), Some(epg_source), Some(epg_channel)),
    };
    conn.execute(
        "INSERT INTO channel_overlays (source_id, channel_id, updated_at,
            epg_mode, epg_source, epg_channel) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (source_id, channel_id) DO UPDATE SET
            updated_at = excluded.updated_at, epg_mode = excluded.epg_mode,
            epg_source = excluded.epg_source, epg_channel = excluded.epg_channel",
        params![source.0, channel, now, mode, feed, epg],
    )?;
    conn.execute(
        "DELETE FROM channel_overlays WHERE source_id = ?1 AND channel_id = ?2
        AND custom_name IS NULL AND custom_number IS NULL AND hidden = 0
        AND blocked = 0 AND epg_mode IS NULL",
        params![source.0, channel],
    )?;
    Ok(())
}
fn overlay_json(value: Option<&EpgOverride>) -> Value {
    match value {
        None => Value::Null,
        Some(EpgOverride::Disabled) => json!({"disabled":true}),
        Some(EpgOverride::Xmltv {
            epg_source,
            epg_channel,
        }) => json!({"feed":epg_source,"channel":epg_channel}),
    }
}
fn overlay_from(value: &Value) -> Result<Option<EpgOverride>> {
    if value.is_null() {
        return Ok(None);
    }
    if value.get("disabled").and_then(Value::as_bool) == Some(true) {
        return Ok(Some(EpgOverride::Disabled));
    }
    Ok(Some(EpgOverride::Xmltv {
        epg_source: value
            .get("feed")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
            .into(),
        epg_channel: value
            .get("channel")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
            .into(),
    }))
}
fn feed_json(value: Option<&GuideSource>) -> Value {
    match value {
        None => Value::Null,
        Some(v) => json!({"feed":v.feed.0,"provider":v.provider_index,
            "url":v.url.expose(),"name":v.name,"enabled":v.enabled,"shift":v.shift_minutes}),
    }
}
fn feed_from(v: &Value) -> Result<Option<GuideSource>> {
    if v.is_null() {
        return Ok(None);
    }
    Ok(Some(GuideSource {
        feed: FeedId(
            v["feed"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(invalid)?,
        ),
        provider_index: if v["provider"].is_null() {
            None
        } else {
            Some(
                v["provider"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(invalid)?,
            )
        },
        url: SecretUrl::new(v["url"].as_str().ok_or_else(invalid)?),
        name: if v["name"].is_null() {
            None
        } else {
            Some(v["name"].as_str().ok_or_else(invalid)?.into())
        },
        enabled: v["enabled"].as_bool().ok_or_else(invalid)?,
        shift_minutes: v["shift"]
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(invalid)?,
    }))
}
fn equal_feed(a: Option<&GuideSource>, b: Option<&GuideSource>) -> bool {
    feed_json(a) == feed_json(b)
}
fn feeds(conn: &Connection, source: SourceId) -> Result<Vec<GuideSource>> {
    let mut statement = conn.prepare_cached(
        "SELECT feed_id,provider_index,url,name,enabled,shift_minutes FROM guide_sources
         WHERE source_id = ?1 ORDER BY position,feed_id",
    )?;
    Ok(statement
        .query_map([source.0], |r| {
            Ok(GuideSource {
                feed: FeedId(r.get(0)?),
                provider_index: r.get(1)?,
                url: SecretUrl::new(r.get::<_, String>(2)?),
                name: r.get(3)?,
                enabled: r.get(4)?,
                shift_minutes: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}
fn allocation_key(source: SourceId, incarnation: &str) -> String {
    format!("guide.repair.feed_high_water.{}.{incarnation}", source.0)
}

/// Retained choices and inverses reserve their numeric feed identities even
/// when no current configuration row uses them. Seed older journals once; all
/// subsequent successful repairs persist the monotonic high-water atomically.
fn feed_high_water(
    conn: &Connection,
    source: SourceId,
    incarnation: &str,
    now: i64,
) -> Result<u32> {
    let saved: Option<String> = conn
        .query_row(
            "SELECT value FROM store_meta WHERE key=?1",
            [allocation_key(source, incarnation)],
            |row| row.get(0),
        )
        .optional()?;
    let mut highest = match &saved {
        Some(value) => value.parse::<u32>().map_err(|_| invalid())?,
        None => 0,
    };
    for table in [
        "guide_sources",
        "guide_feed_incarnations",
        "epg_feeds",
        "epg_feed_channels",
        "epg_channels",
        "epg_feed_results",
    ] {
        let value: Option<u32> = conn.query_row(
            &format!("SELECT MAX(feed_id) FROM {table} WHERE source_id=?1"),
            [source.0],
            |row| row.get(0),
        )?;
        highest = highest.max(value.unwrap_or(0));
    }
    let manual: Option<u32> = conn.query_row(
        "SELECT MAX(CAST(value AS INTEGER)) FROM (
            SELECT CASE WHEN substr(epg_source,1,1)='+' THEN substr(epg_source,2)
                ELSE epg_source END AS value FROM channel_overlays
            WHERE source_id=?1 AND epg_mode='xmltv' AND epg_source IS NOT NULL)
        WHERE value!='' AND value NOT GLOB '*[^0-9]*'
            AND CAST(value AS INTEGER) BETWEEN 0 AND 4294967295",
        [source.0],
        |row| row.get(0),
    )?;
    highest = highest.max(manual.unwrap_or(0));
    let prefix = format!("inspection.feed.{}.", source.0);
    let provenance: Option<u32> = conn.query_row(
        "SELECT MAX(CAST(value AS INTEGER)) FROM (
            SELECT substr(key,?3) AS value FROM store_meta WHERE key>=?1 AND key<?2)
        WHERE value!='' AND value NOT GLOB '*[^0-9]*'
            AND CAST(value AS INTEGER) BETWEEN 0 AND 4294967295",
        params![prefix, format!("{prefix}:"), prefix.len() as i64 + 1],
        |row| row.get(0),
    )?;
    highest = highest.max(provenance.unwrap_or(0));
    if saved.is_none() {
        let mut statement = conn.prepare_cached(
            "SELECT length(CAST(payload AS BLOB)),payload FROM guide_repair_commits
             WHERE source_id=?1 AND source_incarnation=?2 AND expires_at>?3
             ORDER BY created_at DESC,id DESC LIMIT ?4",
        )?;
        let mut rows = statement.query(params![
            source.0,
            incarnation,
            now,
            MAX_IDENTITY_HISTORY_ROWS + 1
        ])?;
        let mut count = 0;
        let mut bytes = 0_u64;
        while let Some(row) = rows.next()? {
            count += 1;
            let record = u64::try_from(row.get::<_, i64>(0)?).map_err(|_| invalid())?;
            bytes = bytes.saturating_add(record);
            if count > MAX_IDENTITY_HISTORY_ROWS
                || record > MAX_IDENTITY_HISTORY_RECORD_BYTES
                || bytes > MAX_IDENTITY_HISTORY_BYTES
            {
                return Err(invalid());
            }
            let payload: Value =
                serde_json::from_str(&row.get::<_, String>(1)?).map_err(|_| invalid())?;
            for patch in payload["feeds"].as_array().ok_or_else(invalid)? {
                for key in ["before", "after"] {
                    if !patch[key].is_null() {
                        let feed = patch[key]["feed"]
                            .as_u64()
                            .and_then(|value| u32::try_from(value).ok())
                            .ok_or_else(invalid)?;
                        highest = highest.max(feed);
                    }
                }
            }
            for patch in payload["rows"].as_array().ok_or_else(invalid)? {
                for key in ["before", "after"] {
                    if let Some(value) = patch[key]
                        .get("feed")
                        .and_then(Value::as_str)
                        .and_then(|value| value.parse::<u32>().ok())
                    {
                        highest = highest.max(value);
                    }
                }
            }
            for value in payload["added"].as_array().ok_or_else(invalid)? {
                highest = highest.max(
                    value
                        .as_u64()
                        .and_then(|value| u32::try_from(value).ok())
                        .ok_or_else(invalid)?,
                );
            }
            if !payload["order"].is_null() {
                for order in payload["order"].as_array().ok_or_else(invalid)? {
                    for value in order.as_array().ok_or_else(invalid)? {
                        highest = highest.max(
                            value
                                .as_u64()
                                .and_then(|value| u32::try_from(value).ok())
                                .ok_or_else(invalid)?,
                        );
                    }
                }
            }
        }
    }
    Ok(highest)
}

/// Shared reservation floor for repair, ordinary edits and catalog refreshes.
pub(crate) fn feed_high_water_in(conn: &Connection, source: SourceId, now: i64) -> Result<u32> {
    let incarnation: String = conn
        .query_row(
            "SELECT incarnation FROM guide_revisions WHERE source_id=?1",
            [source.0],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(StoreError::SourceNotFound(source))?;
    feed_high_water(conn, source, &incarnation, now)
}

/// Remember removed and newly configured identities in the caller's transaction.
pub(crate) fn remember_feed_high_water(
    conn: &Connection,
    source: SourceId,
    highest: u32,
) -> Result<()> {
    let incarnation: String = conn.query_row(
        "SELECT incarnation FROM guide_revisions WHERE source_id=?1",
        [source.0],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT INTO store_meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![allocation_key(source, &incarnation), highest.to_string()],
    )?;
    Ok(())
}

fn revisions(conn: &Connection, source: SourceId) -> Result<(u64, u64)> {
    Ok(conn.query_row(
        "SELECT mapping_revision,configuration_revision FROM guide_revisions
        WHERE source_id = ?1",
        [source.0],
        |r| {
            Ok((
                crate::guide_revisions::revision(r, 0)?,
                crate::guide_revisions::revision(r, 1)?,
            ))
        },
    )?)
}
fn commit_value(conn: &Connection, id: &str) -> Result<Option<(RepairCommit, Value)>> {
    let row = conn
        .query_row(
            "SELECT source_id,created_at,expires_at,undone_at,payload
        FROM guide_repair_commits WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    SourceId(r.get(0)?),
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((source, created_at, expires_at, undone, payload)) = row else {
        return Ok(None);
    };
    let payload: Value = serde_json::from_str(&payload)?;
    let added_feeds = payload["added"]
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .map(FeedId)
                .ok_or_else(invalid)
        })
        .collect::<Result<Vec<_>>>()?;
    let revision_field = if undone.is_some() {
        "undo_revisions"
    } else {
        "revisions"
    };
    let recorded = payload[revision_field].as_array().ok_or_else(invalid)?;
    let mapping_revision = recorded
        .first()
        .and_then(Value::as_u64)
        .ok_or_else(invalid)?;
    let configuration_revision = recorded
        .get(1)
        .and_then(Value::as_u64)
        .ok_or_else(invalid)?;
    Ok(Some((
        RepairCommit {
            id: id.into(),
            source,
            created_at,
            expires_at,
            undone: undone.is_some(),
            mapping_revision,
            configuration_revision,
            added_feeds,
        },
        payload,
    )))
}

fn same_choice(before: Option<&EpgOverride>, after: &RepairChoice) -> bool {
    match (before, after) {
        (None, RepairChoice::Automatic) | (Some(EpgOverride::Disabled), RepairChoice::Disabled) => {
            true
        }
        (
            Some(EpgOverride::Xmltv {
                epg_source,
                epg_channel,
            }),
            RepairChoice::Existing {
                feed,
                epg_channel: chosen,
            },
        ) => epg_source == &feed.0.to_string() && epg_channel == chosen,
        _ => false,
    }
}

// Change only the specified feeds' relative order; unrelated later feeds keep
// their slots. This also preserves unrelated feed additions during inverse.
fn reorder(feeds: &mut [GuideSource], order: &[FeedId]) -> bool {
    let set: HashSet<_> = order.iter().copied().collect();
    let slots: Vec<_> = feeds
        .iter()
        .enumerate()
        .filter(|(_, f)| set.contains(&f.feed))
        .map(|(i, _)| i)
        .collect();
    if slots.len() != order.len() {
        return false;
    }
    let values: HashMap<_, _> = feeds
        .iter()
        .filter(|f| set.contains(&f.feed))
        .map(|f| (f.feed, f.clone()))
        .collect();
    for (slot, feed) in slots.into_iter().zip(order) {
        let Some(value) = values.get(feed) else {
            return false;
        };
        feeds[slot] = value.clone();
    }
    true
}

impl Store {
    /// Shared identity reservation for previews, ordinary edits and import planning.
    /// Legacy seeding reads only this incarnation's unexpired inverses, at most
    /// 4096 receipts / 32 MiB total / 8 MiB each. Larger seeds fail closed;
    /// successful commits cache the monotonic reservation for subsequent reads.
    pub fn guide_repair_feed_high_water(&self, source: SourceId) -> Result<u32> {
        feed_high_water(
            &self.conn,
            source,
            &self.source_incarnation(source)?,
            self.now(),
        )
    }

    /// Fallible current mapping read, including missing-channel detection.
    pub fn repair_mapping(
        &self,
        source: SourceId,
        channel: &ContentId,
    ) -> Result<Option<Option<EpgOverride>>> {
        crate::sources::ensure_source(&self.conn, source)?;
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM channels
            WHERE source_id=?1 AND id=?2)",
            params![source.0, channel.as_str()],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        Ok(Some(overlay(&self.conn, source, channel.as_str())?))
    }

    /// Validate affected values and commit configuration, mappings and inverse
    /// together. Unrelated edits and physical no-op refreshes remain admissible.
    pub fn apply_guide_repair(
        &mut self,
        id: &str,
        digest: &str,
        batch: &RepairBatch,
    ) -> Result<RepairResult> {
        if id.is_empty()
            || id.len() > 128
            || digest.is_empty()
            || digest.len() > 128
            || (batch.mappings.is_empty()
                && batch.feeds.is_empty()
                && batch.additions.is_empty()
                && batch.order.is_none())
            || batch.mappings.len() > MAX_REPAIR_MAPPINGS
            || batch.feeds.len() + batch.additions.len() > 64
        {
            return Ok(RepairResult::Invalid);
        }
        let clock = self.clock;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = clock();
        if let Some((commit, payload)) = commit_value(&tx, id)? {
            return Ok(if payload["digest"].as_str() == Some(digest) {
                RepairResult::Committed(commit)
            } else {
                RepairResult::Invalid
            });
        }
        if now >= batch.preview_expires_at {
            return Ok(RepairResult::Expired);
        }
        let incarnation: Option<String> = tx
            .query_row(
                "SELECT incarnation FROM guide_revisions WHERE source_id=?1",
                [batch.source.0],
                |r| r.get(0),
            )
            .optional()?;
        if incarnation.as_deref() != Some(&batch.source_incarnation) {
            return Ok(RepairResult::Conflict(vec!["source".into()]));
        }
        let enabled: bool = tx.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM source_policy
            WHERE source_id=?1 AND enabled=0)",
            [batch.source.0],
            |r| r.get(0),
        )?;
        if !enabled {
            return Ok(RepairResult::SourceDisabled(batch.source));
        }
        let mut conflicts = Vec::new();
        let mut current = feeds(&tx, batch.source)?;
        let mut seen = HashSet::new();
        for patch in &batch.feeds {
            let Some(feed) = patch
                .before
                .as_ref()
                .or(patch.after.as_ref())
                .map(|f| f.feed)
            else {
                return Ok(RepairResult::Invalid);
            };
            if !seen.insert(feed)
                || patch.before.as_ref().is_some_and(|f| f.feed != feed)
                || patch.after.as_ref().is_some_and(|f| f.feed != feed)
            {
                return Ok(RepairResult::Invalid);
            }
            if !equal_feed(
                current.iter().find(|f| f.feed == feed),
                patch.before.as_ref(),
            ) {
                conflicts.push(format!("feed:{}", feed.0));
            }
        }
        let mut channels = HashSet::new();
        for patch in &batch.mappings {
            if !channels.insert(patch.channel.as_str()) || patch.channel.as_str().len() > 4096 {
                return Ok(RepairResult::Invalid);
            }
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM channels
                WHERE source_id=?1 AND id=?2)",
                params![batch.source.0, patch.channel.as_str()],
                |r| r.get(0),
            )?;
            let blocked: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM channel_overlays
                WHERE source_id=?1 AND channel_id=?2 AND blocked=1)",
                params![batch.source.0, patch.channel.as_str()],
                |r| r.get(0),
            )?;
            if blocked {
                return Ok(RepairResult::PermissionDenied);
            }
            if !exists
                || overlay(&tx, batch.source, patch.channel.as_str())? != patch.before
                || (patch.before.is_some()
                    && !patch.replace_explicit
                    && !same_choice(patch.before.as_ref(), &patch.after))
            {
                conflicts.push(patch.channel.as_str().into());
            }
        }
        for candidate in &batch.candidates {
            let identity: Option<String> = tx
                .query_row(
                    "SELECT incarnation FROM guide_feed_incarnations
                WHERE source_id=?1 AND feed_id=?2",
                    params![batch.source.0, candidate.feed.0],
                    |r| r.get(0),
                )
                .optional()?;
            let present: bool = candidate.staged
                || tx.query_row(
                    "SELECT EXISTS(
                SELECT 1 FROM epg_feed_channels c
                JOIN epg_feeds f ON f.source_id=c.source_id AND f.feed_id=c.feed_id
                JOIN guide_sources g ON g.source_id=f.source_id AND g.feed_id=f.feed_id
                JOIN guide_feed_incarnations i ON i.source_id=g.source_id AND i.feed_id=g.feed_id
                JOIN store_meta m ON m.key='inspection.feed.' || g.source_id || '.' || g.feed_id
                WHERE m.value=i.incarnation AND c.source_id=?1 AND c.feed_id=?2 AND c.epg_channel=?3
                AND g.enabled=1 AND g.url=f.url AND g.shift_minutes=f.shift_minutes)",
                    params![batch.source.0, candidate.feed.0, candidate.epg_channel],
                    |r| r.get(0),
                )?;
            if identity.as_deref() != Some(candidate.incarnation.as_str()) || !present {
                conflicts.push(format!("candidate:{}", candidate.feed.0));
            }
        }
        if !conflicts.is_empty() {
            return Ok(RepairResult::Conflict(conflicts));
        }
        if let Some((before, after)) = &batch.order {
            let actual: Vec<_> = current.iter().map(|f| f.feed).collect();
            let unique: HashSet<_> = before.iter().copied().collect();
            let desired: HashSet<_> = after.iter().copied().collect();
            if unique.len() != before.len()
                || desired.len() != after.len()
                || unique != desired
                || before != &actual
            {
                return Ok(RepairResult::Conflict(vec!["feed_order".into()]));
            }
        }
        let original_positions: HashMap<_, _> = current
            .iter()
            .enumerate()
            .map(|(at, config)| (config.feed, at))
            .collect();
        let mut feed_payload = Vec::new();
        for patch in &batch.feeds {
            let feed = patch
                .before
                .as_ref()
                .or(patch.after.as_ref())
                .ok_or_else(invalid)?
                .feed;
            let before_position = original_positions.get(&feed).copied();
            feed_payload.push(json!({"before":feed_json(patch.before.as_ref()),
                "after":feed_json(patch.after.as_ref()),"position_before":before_position}));
            if let Some(position) = current.iter().position(|f| f.feed == feed) {
                match &patch.after {
                    Some(after) => current[position] = after.clone(),
                    None => {
                        current.remove(position);
                    }
                }
            } else if let Some(after) = &patch.after {
                current.push(after.clone());
            }
        }
        let mut next = feed_high_water(&tx, batch.source, &batch.source_incarnation, now)?.max(
            current
                .iter()
                .map(|config| config.feed.0)
                .max()
                .unwrap_or(0),
        );
        if next.checked_add(batch.additions.len() as u32).is_none() {
            return Ok(RepairResult::Invalid);
        }
        let mut additions = Vec::new();
        for config in &batch.additions {
            let Some(id) = next.checked_add(1) else {
                return Ok(RepairResult::Invalid);
            };
            next = id;
            let mut config = config.clone();
            config.feed = FeedId(id);
            config.provider_index = None;
            feed_payload.push(json!({"before":Value::Null,"after":feed_json(Some(&config))}));
            additions.push(config.feed);
            current.push(config);
        }
        if let Some((_, after)) = &batch.order
            && !reorder(&mut current, after)
        {
            return Ok(RepairResult::Conflict(vec!["feed_order".into()]));
        }
        let mut mapping_payload = Vec::new();
        for patch in &batch.mappings {
            let after = match &patch.after {
                RepairChoice::Automatic => None,
                RepairChoice::Disabled => Some(EpgOverride::Disabled),
                RepairChoice::Existing { feed, epg_channel } => {
                    if !current.iter().any(|f| f.feed == *feed && f.enabled) {
                        return Ok(RepairResult::Conflict(vec![format!("feed:{}", feed.0)]));
                    }
                    if !batch
                        .candidates
                        .iter()
                        .any(|c| c.feed == *feed && c.epg_channel == *epg_channel)
                    {
                        return Ok(RepairResult::Invalid);
                    }
                    Some(EpgOverride::for_feed(*feed, epg_channel.clone()))
                }
                RepairChoice::Addition { index, epg_channel } => {
                    let Some(feed) = additions.get(*index) else {
                        return Ok(RepairResult::Invalid);
                    };
                    Some(EpgOverride::for_feed(*feed, epg_channel.clone()))
                }
            };
            mapping_payload.push(json!({"channel":patch.channel.as_str(),
                "before":overlay_json(patch.before.as_ref()),"after":overlay_json(after.as_ref())}));
        }
        // Validate the complete batch before the first live mutation.
        crate::guide_sources::set_guide_sources_in(&tx, batch.source, &current, now)?;
        for row in &mapping_payload {
            let after = overlay_from(&row["after"])?;
            write_overlay(
                &tx,
                batch.source,
                row["channel"].as_str().ok_or_else(invalid)?,
                after.as_ref(),
                now,
            )?;
        }
        // set_guide_sources_in persisted the complete current allocation floor.
        let (mapping_revision, configuration_revision) = revisions(&tx, batch.source)?;
        let payload = json!({"digest":digest,"rows":mapping_payload,"feeds":feed_payload,
            "added":additions.iter().map(|f|f.0).collect::<Vec<_>>(),
            "revisions":[mapping_revision, configuration_revision],
            "order":batch.order.as_ref().map(|(before,after)|json!([
                before.iter().map(|f|f.0).collect::<Vec<_>>(),
                after.iter().map(|f|f.0).collect::<Vec<_>>()]))});
        tx.execute(
            "INSERT INTO guide_repair_commits
            (id,source_id,source_incarnation,created_at,expires_at,payload)
            VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                id,
                batch.source.0,
                batch.source_incarnation,
                now,
                now.saturating_add(REPAIR_UNDO_SECONDS),
                payload.to_string()
            ],
        )?;
        let commit = commit_value(&tx, id)?.ok_or_else(invalid)?.0;
        tx.commit()?;
        Ok(RepairResult::Committed(commit))
    }

    pub fn guide_repair_commit(&self, id: &str) -> Result<Option<RepairCommit>> {
        Ok(commit_value(&self.conn, id)?.map(|(commit, _)| commit))
    }

    /// All-or-nothing inverse against affected after-values, preserving every
    /// unrelated overlay/config edit. Ordinary restart retains the journal.
    pub fn undo_guide_repair(&mut self, id: &str) -> Result<RepairResult> {
        let clock = self.clock;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = clock();
        let Some((commit, mut payload)) = commit_value(&tx, id)? else {
            return Ok(RepairResult::Missing);
        };
        if commit.expires_at <= now {
            return Ok(RepairResult::Expired);
        }
        if commit.undone {
            return Ok(RepairResult::Committed(commit));
        }
        let incarnation: String = tx.query_row(
            "SELECT source_incarnation FROM guide_repair_commits
            WHERE id=?1",
            [id],
            |r| r.get(0),
        )?;
        let current_incarnation: String = tx.query_row(
            "SELECT incarnation FROM guide_revisions
            WHERE source_id=?1",
            [commit.source.0],
            |r| r.get(0),
        )?;
        if incarnation != current_incarnation {
            return Ok(RepairResult::Conflict(vec!["source".into()]));
        }
        let rows = payload["rows"].as_array().ok_or_else(invalid)?;
        let patches = payload["feeds"].as_array().ok_or_else(invalid)?;
        let enabled: bool = tx.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM source_policy
            WHERE source_id=?1 AND enabled=0)",
            [commit.source.0],
            |r| r.get(0),
        )?;
        if !enabled {
            return Ok(RepairResult::SourceDisabled(commit.source));
        }
        let mut conflicts = Vec::new();
        let mut current = feeds(&tx, commit.source)?;
        let touched: HashSet<_> = rows.iter().filter_map(|r| r["channel"].as_str()).collect();
        for row in rows {
            let channel = row["channel"].as_str().ok_or_else(invalid)?;
            let allowed: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM channels c
                LEFT JOIN channel_overlays o ON o.source_id=c.source_id AND o.channel_id=c.id
                WHERE c.source_id=?1 AND c.id=?2 AND COALESCE(o.blocked,0)=0)",
                params![commit.source.0, channel],
                |r| r.get(0),
            )?;
            if !allowed || overlay(&tx, commit.source, channel)? != overlay_from(&row["after"])? {
                conflicts.push(channel.into());
            }
        }
        let original_order = if payload["order"].is_null() {
            None
        } else {
            let read = |index: usize| -> Result<Vec<FeedId>> {
                payload["order"][index]
                    .as_array()
                    .ok_or_else(invalid)?
                    .iter()
                    .map(|v| {
                        v.as_u64()
                            .and_then(|n| u32::try_from(n).ok())
                            .map(FeedId)
                            .ok_or_else(invalid)
                    })
                    .collect()
            };
            let before = read(0)?;
            let after = read(1)?;
            let set: HashSet<_> = after.iter().copied().collect();
            let actual: Vec<_> = current
                .iter()
                .map(|f| f.feed)
                .filter(|f| set.contains(f))
                .collect();
            if actual != after {
                conflicts.push("feed_order".into());
            }
            Some(before)
        };
        let mut before_feeds = Vec::new();
        for patch in patches {
            let before = feed_from(&patch["before"])?;
            let after = feed_from(&patch["after"])?;
            let feed = before.as_ref().or(after.as_ref()).ok_or_else(invalid)?.feed;
            if !equal_feed(current.iter().find(|f| f.feed == feed), after.as_ref()) {
                conflicts.push(format!("feed:{}", feed.0));
            }
            if before.is_none() {
                let mut stmt = tx.prepare_cached(
                    "SELECT channel_id FROM channel_overlays
                    WHERE source_id=?1 AND epg_mode='xmltv' AND epg_source=?2",
                )?;
                let explicit = stmt
                    .query_map(params![commit.source.0, feed.0.to_string()], |r| {
                        r.get::<_, String>(0)
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                if explicit.iter().any(|id| !touched.contains(id.as_str())) {
                    conflicts.push(format!("feed_in_use:{}", feed.0));
                }
            }
            let position = patch["position_before"]
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .unwrap_or(current.len());
            before_feeds.push((position, feed, before));
        }
        if !conflicts.is_empty() {
            return Ok(RepairResult::Conflict(conflicts));
        }
        before_feeds.sort_by_key(|(position, _, _)| *position);
        for (position, feed, before) in before_feeds {
            if let Some(at) = current.iter().position(|f| f.feed == feed) {
                match before {
                    Some(v) => current[at] = v,
                    None => {
                        current.remove(at);
                    }
                }
            } else if let Some(v) = before {
                current.insert(position.min(current.len()), v);
            }
        }
        if let Some(before) = original_order
            && !reorder(&mut current, &before)
        {
            return Ok(RepairResult::Conflict(vec!["feed_order".into()]));
        }
        crate::guide_sources::set_guide_sources_in(&tx, commit.source, &current, now)?;
        for row in rows {
            let before = overlay_from(&row["before"])?;
            write_overlay(
                &tx,
                commit.source,
                row["channel"].as_str().ok_or_else(invalid)?,
                before.as_ref(),
                now,
            )?;
        }
        let (mapping_revision, configuration_revision) = revisions(&tx, commit.source)?;
        payload["undo_revisions"] = json!([mapping_revision, configuration_revision]);
        tx.execute(
            "UPDATE guide_repair_commits SET undone_at=?2, payload=?3 WHERE id=?1",
            params![id, now, payload.to_string()],
        )?;
        let result = commit_value(&tx, id)?.ok_or_else(invalid)?.0;
        tx.commit()?;
        Ok(RepairResult::Committed(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_providers::model::{Channel, M3uSourceConfig, Source, SourceConfig, StreamLocator};

    fn source(store: &mut Store) -> SourceId {
        let id = store
            .add_source(&Source {
                name: "Synthetic repair".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://synthetic.invalid/list.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        catalog(store, id);
        id
    }
    fn catalog(store: &mut Store, id: SourceId) {
        let rows: Vec<_> = ["amber", "birch", "cedar"]
            .into_iter()
            .map(|key| Channel {
                id: ContentId::from_stored(key),
                provider_id: None,
                name: format!("Synthetic {key}"),
                number: None,
                logo: None,
                groups: vec![],
                stream: StreamLocator::Url {
                    url: "https://synthetic.invalid/stream.ts".into(),
                    headers: vec![],
                    properties: vec![],
                },
                epg_id: None,
                epg_name: None,
                epg_shift_minutes: None,
                catch_up: None,
                radio: false,
            })
            .collect();
        store.replace_catalog(id, &[], &rows, &[], &[]).unwrap();
    }
    fn batch(store: &Store, source: SourceId) -> RepairBatch {
        RepairBatch {
            source,
            source_incarnation: store.source_incarnation(source).unwrap(),
            mappings: ["amber", "birch"]
                .into_iter()
                .map(|key| RepairMapping {
                    channel: ContentId::from_stored(key),
                    before: None,
                    after: RepairChoice::Disabled,
                    replace_explicit: false,
                })
                .collect(),
            feeds: vec![],
            additions: vec![],
            candidates: vec![],
            preview_expires_at: i64::MAX,
            order: None,
        }
    }
    fn committed(result: RepairResult) -> RepairCommit {
        match result {
            RepairResult::Committed(c) => c,
            other => panic!("{other:?}"),
        }
    }
    fn disabled(store: &Store, source: SourceId, key: &str) -> bool {
        store
            .repair_mapping(source, &ContentId::from_stored(key))
            .unwrap()
            == Some(Some(EpgOverride::Disabled))
    }
    fn feed() -> GuideSource {
        GuideSource {
            feed: FeedId(0),
            provider_index: None,
            url: SecretUrl::new("https://synthetic.invalid/guide.xml"),
            name: None,
            enabled: true,
            shift_minutes: 0,
        }
    }
    #[test]
    fn undo_restores_removed_feed_positions_and_preserves_unrelated_new_feed() {
        let mut store = Store::open_in_memory().unwrap();
        let source = source(&mut store);
        let configs: Vec<_> = (1..=4)
            .map(|id| GuideSource {
                feed: FeedId(id),
                ..feed()
            })
            .collect();
        store.set_guide_sources(source, &configs).unwrap();
        let mut prepared = batch(&store, source);
        prepared.mappings.clear();
        prepared.feeds = [0, 2]
            .into_iter()
            .map(|index| RepairFeed {
                before: Some(configs[index].clone()),
                after: None,
            })
            .collect();
        committed(
            store
                .apply_guide_repair("remove", "digest", &prepared)
                .unwrap(),
        );
        let mut later = store.guide_sources(source).unwrap();
        later.push(GuideSource {
            feed: FeedId(5),
            ..feed()
        });
        store.set_guide_sources(source, &later).unwrap();
        committed(store.undo_guide_repair("remove").unwrap());
        assert_eq!(
            store
                .guide_sources(source)
                .unwrap()
                .iter()
                .map(|f| f.feed.0)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4, 5]
        );
    }
    #[test]
    fn affected_conflict_and_explicit_protection_commit_nothing() {
        let mut store = Store::open_in_memory().unwrap();
        let id = source(&mut store);
        let preview = batch(&store, id);
        store
            .set_epg_override(
                id,
                &ContentId::from_stored("birch"),
                Some(&EpgOverride::Disabled),
            )
            .unwrap();
        assert!(matches!(
            store.apply_guide_repair("p1", "digest", &preview).unwrap(),
            RepairResult::Conflict(_)
        ));
        assert!(!disabled(&store, id, "amber"));
        assert!(store.guide_repair_commit("p1").unwrap().is_none());
        let mut preview = batch(&store, id);
        preview.mappings[1].before = Some(EpgOverride::Disabled);
        preview.mappings[1].after = RepairChoice::Automatic;
        assert!(matches!(
            store.apply_guide_repair("p2", "digest", &preview).unwrap(),
            RepairResult::Conflict(_)
        ));
        preview.mappings[1].replace_explicit = true;
        committed(store.apply_guide_repair("p2", "digest", &preview).unwrap());
        assert!(disabled(&store, id, "amber"));
        assert!(!disabled(&store, id, "birch"));
    }
    #[test]
    fn no_op_import_and_unrelated_edit_do_not_invalidate_apply_or_undo() {
        let mut store = Store::open_in_memory().unwrap();
        let id = source(&mut store);
        let preview = batch(&store, id);
        catalog(&mut store, id);
        committed(store.apply_guide_repair("p1", "digest", &preview).unwrap());
        store
            .set_hidden(id, &ContentId::from_stored("amber"), true)
            .unwrap();
        store
            .set_epg_override(
                id,
                &ContentId::from_stored("cedar"),
                Some(&EpgOverride::Disabled),
            )
            .unwrap();
        assert!(committed(store.undo_guide_repair("p1").unwrap()).undone);
        assert!(!disabled(&store, id, "amber"));
        assert!(!disabled(&store, id, "birch"));
        assert!(disabled(&store, id, "cedar"));
        assert!(
            store
                .channel(id, &ContentId::from_stored("amber"))
                .unwrap()
                .unwrap()
                .hidden
        );
    }
    #[test]
    fn later_edits_cannot_change_the_revision_an_old_commit_certifies() {
        let mut store = Store::open_in_memory().unwrap();
        let source = source(&mut store);
        let preview = batch(&store, source);
        let original = committed(
            store
                .apply_guide_repair("original", "digest", &preview)
                .unwrap(),
        );
        store
            .set_epg_override(
                source,
                &ContentId::from_stored("cedar"),
                Some(&EpgOverride::Disabled),
            )
            .unwrap();
        assert!(store.guide_revisions(source).unwrap().mapping > original.mapping_revision);
        assert_eq!(
            store.guide_repair_commit("original").unwrap(),
            Some(original.clone())
        );
        assert_eq!(
            committed(
                store
                    .apply_guide_repair("original", "digest", &preview)
                    .unwrap()
            ),
            original
        );
    }

    #[test]
    fn conflicting_undo_never_reverts_a_subset() {
        let mut store = Store::open_in_memory().unwrap();
        let id = source(&mut store);
        let preview = batch(&store, id);
        committed(store.apply_guide_repair("p1", "digest", &preview).unwrap());
        store
            .set_epg_override(id, &ContentId::from_stored("birch"), None)
            .unwrap();
        assert!(matches!(
            store.undo_guide_repair("p1").unwrap(),
            RepairResult::Conflict(_)
        ));
        assert!(disabled(&store, id, "amber"));
        assert!(!disabled(&store, id, "birch"));
        assert!(!store.guide_repair_commit("p1").unwrap().unwrap().undone);
    }
    #[test]
    fn durable_inverse_survives_restart_but_restore_invalidates_authority() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.sqlite");
        let mut store = Store::open(&path).unwrap();
        let id = source(&mut store);
        let preview = batch(&store, id);
        committed(store.apply_guide_repair("p1", "digest", &preview).unwrap());
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(committed(store.undo_guide_repair("p1").unwrap()).undone);
        let preview = batch(&store, id);
        committed(store.apply_guide_repair("p2", "digest", &preview).unwrap());
        store.invalidate_guide_repair_authority().unwrap();
        assert_eq!(
            store.undo_guide_repair("p2").unwrap(),
            RepairResult::Missing
        );
        assert!(disabled(&store, id, "amber"));
    }
    #[test]
    fn expired_inverse_is_explicit_and_duplicate_commit_does_not_reapply() {
        let mut store = Store::open_in_memory().unwrap();
        store.set_clock(|| 100);
        let id = source(&mut store);
        let preview = batch(&store, id);
        let first = committed(store.apply_guide_repair("p1", "digest", &preview).unwrap());
        let rev = store.guide_revisions(id).unwrap();
        assert_eq!(
            committed(store.apply_guide_repair("p1", "digest", &preview).unwrap()),
            first
        );
        assert_eq!(store.guide_revisions(id).unwrap(), rev);
        assert_eq!(
            store.apply_guide_repair("p1", "other", &preview).unwrap(),
            RepairResult::Invalid
        );
        store.set_clock(|| 100 + REPAIR_UNDO_SECONDS);
        assert_eq!(
            store.undo_guide_repair("p1").unwrap(),
            RepairResult::Expired
        );
        assert!(disabled(&store, id, "amber"));
    }
    #[test]
    fn new_feed_ids_allocate_only_at_commit_and_external_usage_blocks_undo() {
        let mut store = Store::open_in_memory().unwrap();
        let id = source(&mut store);
        let mut preview = batch(&store, id);
        preview.additions.push(feed());
        for mapping in &mut preview.mappings {
            mapping.after = RepairChoice::Addition {
                index: 0,
                epg_channel: "synthetic.amber".into(),
            };
        }
        assert!(store.guide_sources(id).unwrap().is_empty());
        let applied = committed(store.apply_guide_repair("p1", "digest", &preview).unwrap());
        assert_eq!(applied.added_feeds.len(), 1);
        let chosen = EpgOverride::for_feed(applied.added_feeds[0], "synthetic.amber");
        assert_eq!(
            store
                .repair_mapping(id, &ContentId::from_stored("amber"))
                .unwrap(),
            Some(Some(chosen.clone()))
        );
        store
            .set_epg_override(id, &ContentId::from_stored("cedar"), Some(&chosen))
            .unwrap();
        assert!(matches!(
            store.undo_guide_repair("p1").unwrap(),
            RepairResult::Conflict(_)
        ));
        store
            .set_epg_override(id, &ContentId::from_stored("cedar"), None)
            .unwrap();
        committed(store.undo_guide_repair("p1").unwrap());
        assert!(store.guide_sources(id).unwrap().is_empty());
        assert_eq!(
            store
                .repair_mapping(id, &ContentId::from_stored("amber"))
                .unwrap(),
            Some(None)
        );
    }
    #[test]
    fn allocated_feeds_never_retarget_old_manual_choices_and_history_survives_counter_reseeding() {
        let mut store = Store::open_in_memory().unwrap();
        let id = source(&mut store);
        let configs: Vec<_> = [1, 2]
            .into_iter()
            .map(|id| GuideSource {
                feed: FeedId(id),
                ..feed()
            })
            .collect();
        store.set_guide_sources(id, &configs).unwrap();
        let old_choice = EpgOverride::for_feed(FeedId(2), "synthetic.shared");
        store
            .set_epg_override(id, &ContentId::from_stored("cedar"), Some(&old_choice))
            .unwrap();
        store.set_guide_sources(id, &configs[..1]).unwrap();
        let mut prepared = batch(&store, id);
        prepared.additions.push(feed());
        for mapping in &mut prepared.mappings {
            mapping.after = RepairChoice::Addition {
                index: 0,
                epg_channel: "synthetic.shared".into(),
            };
        }
        let applied = committed(
            store
                .apply_guide_repair("new", "digest", &prepared)
                .unwrap(),
        );
        assert_eq!(
            applied.added_feeds,
            [FeedId(3)],
            "feed 2 remains reserved by the untouched old choice"
        );
        assert_eq!(
            store
                .repair_mapping(id, &ContentId::from_stored("cedar"))
                .unwrap(),
            Some(Some(old_choice.clone()))
        );
        assert!(
            !store
                .guide_sources(id)
                .unwrap()
                .iter()
                .any(|feed| feed.feed == FeedId(2)),
            "the untouched old manual choice stays unresolved even if the new feed lists the same channel ID"
        );
        assert!(
            committed(store.undo_guide_repair("new").unwrap()).undone,
            "an old unresolved choice cannot manufacture a new feed-in-use conflict"
        );
        // An older version may have journals without this cache. Reconstruct
        // their reservations, including the added feed of an undone commit.
        store
            .conn
            .execute(
                "DELETE FROM store_meta WHERE key LIKE 'guide.repair.feed_high_water.%'",
                [],
            )
            .unwrap();
        assert_eq!(store.guide_repair_feed_high_water(id).unwrap(), 3);
        let applied = committed(
            store
                .apply_guide_repair("newer", "digest", &prepared)
                .unwrap(),
        );
        assert_eq!(applied.added_feeds, [FeedId(4)]);
        assert!(committed(store.undo_guide_repair("newer").unwrap()).undone);
        assert_eq!(
            store
                .repair_mapping(id, &ContentId::from_stored("cedar"))
                .unwrap(),
            Some(Some(old_choice))
        );
    }

    #[test]
    fn every_numeric_manual_feed_spelling_reserves_the_matching_identity() {
        for spelling in ["2", "0002", "+0002"] {
            let mut store = Store::open_in_memory().unwrap();
            let id = source(&mut store);
            store
                .set_guide_sources(
                    id,
                    &[GuideSource {
                        feed: FeedId(1),
                        ..feed()
                    }],
                )
                .unwrap();
            let choice = EpgOverride::Xmltv {
                epg_source: spelling.into(),
                epg_channel: "synthetic.shared".into(),
            };
            assert_eq!(choice.feed(), Some(FeedId(2)));
            store
                .set_epg_override(id, &ContentId::from_stored("cedar"), Some(&choice))
                .unwrap();
            assert_eq!(store.guide_repair_feed_high_water(id).unwrap(), 2);
            let mut prepared = batch(&store, id);
            prepared.additions.push(feed());
            assert_eq!(
                committed(
                    store
                        .apply_guide_repair("numeric", "digest", &prepared)
                        .unwrap()
                )
                .added_feeds,
                [FeedId(3)]
            );
        }
    }

    #[test]
    fn removing_a_maximum_feed_and_adding_another_in_one_batch_has_a_valid_inverse() {
        let mut store = Store::open_in_memory().unwrap();
        let id = source(&mut store);
        let original = GuideSource {
            feed: FeedId(1),
            ..feed()
        };
        store
            .set_guide_sources(id, std::slice::from_ref(&original))
            .unwrap();
        let mut prepared = batch(&store, id);
        prepared.feeds.push(RepairFeed {
            before: Some(original.clone()),
            after: None,
        });
        prepared.additions.push(feed());
        let applied = committed(
            store
                .apply_guide_repair("replace", "digest", &prepared)
                .unwrap(),
        );
        assert_eq!(applied.added_feeds, [FeedId(2)]);
        assert_eq!(store.guide_sources(id).unwrap()[0].feed, FeedId(2));
        assert!(committed(store.undo_guide_repair("replace").unwrap()).undone);
        assert!(equal_feed(
            store.guide_sources(id).unwrap().first(),
            Some(&original)
        ));
    }

    #[test]
    fn legacy_identity_history_seeding_is_bounded_and_ignores_expired_authority() {
        let mut store = Store::open_in_memory().unwrap();
        store.set_clock(|| 100);
        let id = source(&mut store);
        let incarnation = store.source_incarnation(id).unwrap();
        let payload = json!({"digest":"synthetic","rows":[],"feeds":[],"added":[],"revisions":[0,0],"order":null}).to_string();
        // Invalid expired/other-incarnation histories have no current authority.
        for (key, origin, expires) in [
            ("expired", incarnation.as_str(), 100),
            ("retired", "retired", 101),
        ] {
            store.conn.execute("INSERT INTO guide_repair_commits
                (id,source_id,source_incarnation,created_at,expires_at,payload) VALUES(?1,?2,?3,0,?4,'invalid legacy payload')",
                params![key,id.0,origin,expires]).unwrap();
        }
        assert_eq!(store.guide_repair_feed_high_water(id).unwrap(), 0);
        let tx = store.conn.unchecked_transaction().unwrap();
        for index in 0..=MAX_IDENTITY_HISTORY_ROWS {
            tx.execute("INSERT INTO guide_repair_commits
                (id,source_id,source_incarnation,created_at,expires_at,payload) VALUES(?1,?2,?3,0,101,?4)",
                params![format!("synthetic-{index}"),id.0,incarnation,payload]).unwrap();
        }
        tx.commit().unwrap();
        let mut prepared = batch(&store, id);
        prepared.additions.push(feed());
        assert!(store.guide_repair_feed_high_water(id).is_err());
        assert!(
            store
                .apply_guide_repair("refused-history", "digest", &prepared)
                .is_err()
        );
        assert!(store.guide_sources(id).unwrap().is_empty());
        assert_eq!(
            store
                .repair_mapping(id, &ContentId::from_stored("amber"))
                .unwrap(),
            Some(None)
        );
        store
            .conn
            .execute("DELETE FROM guide_repair_commits", [])
            .unwrap();
        let huge = json!({"digest":"synthetic","rows":[],"feeds":[],"added":[],"revisions":[0,0],"order":null,
            "legacy_padding":"x".repeat(MAX_IDENTITY_HISTORY_RECORD_BYTES as usize)}).to_string();
        store.conn.execute("INSERT INTO guide_repair_commits
            (id,source_id,source_incarnation,created_at,expires_at,payload) VALUES('huge',?1,?2,0,101,?3)",
            params![id.0,incarnation,huge]).unwrap();
        assert!(
            store.guide_repair_feed_high_water(id).is_err(),
            "payload cap is checked before deserializing a valid oversized receipt"
        );
    }

    #[test]
    fn exhausted_reserved_feed_identity_space_rejects_before_any_live_mutation() {
        let mut store = Store::open_in_memory().unwrap();
        let id = source(&mut store);
        store
            .set_epg_override(
                id,
                &ContentId::from_stored("cedar"),
                Some(&EpgOverride::for_feed(FeedId(u32::MAX), "synthetic.shared")),
            )
            .unwrap();
        let mut prepared = batch(&store, id);
        prepared.additions.push(feed());
        assert_eq!(
            store
                .apply_guide_repair("exhausted", "digest", &prepared)
                .unwrap(),
            RepairResult::Invalid
        );
        assert!(store.guide_sources(id).unwrap().is_empty());
        assert_eq!(
            store
                .repair_mapping(id, &ContentId::from_stored("amber"))
                .unwrap(),
            Some(None)
        );
        assert!(store.guide_repair_commit("exhausted").unwrap().is_none());
        assert!(
            store
                .conn
                .query_row(
                    "SELECT value FROM store_meta WHERE key=?1",
                    [allocation_key(id, &store.source_incarnation(id).unwrap())],
                    |row| row.get::<_, String>(0)
                )
                .optional()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn replaced_feed_identity_and_deleted_channels_are_rejected() {
        let mut store = Store::open_in_memory().unwrap();
        let id = source(&mut store);
        let mut configured = feed();
        configured.feed = FeedId(7);
        store.set_guide_sources(id, &[configured.clone()]).unwrap();
        let mut preview = batch(&store, id);
        preview.candidates.push(RepairCandidate {
            feed: configured.feed,
            incarnation: store
                .feed_incarnation(id, configured.feed)
                .unwrap()
                .unwrap(),
            epg_channel: "synthetic.amber".into(),
            staged: true,
        });
        configured.shift_minutes = 60;
        store.set_guide_sources(id, &[configured]).unwrap();
        assert!(matches!(
            store.apply_guide_repair("p1", "digest", &preview).unwrap(),
            RepairResult::Conflict(_)
        ));
        let preview = batch(&store, id);
        store
            .conn
            .execute(
                "DELETE FROM channels WHERE source_id=?1 AND id='amber'",
                [id.0],
            )
            .unwrap();
        assert!(matches!(
            store.apply_guide_repair("p2", "digest", &preview).unwrap(),
            RepairResult::Conflict(_)
        ));
        assert!(!disabled(&store, id, "birch"));
    }
    #[test]
    fn feed_patches_and_additions_share_the_same_reservation_floor() {
        let mut store = Store::open_in_memory().unwrap();
        let id = source(&mut store);
        let mut prepared = batch(&store, id);
        prepared.feeds.push(RepairFeed {
            before: None,
            after: Some(GuideSource {
                feed: FeedId(5000),
                ..feed()
            }),
        });
        prepared.additions.push(feed());
        let applied = committed(
            store
                .apply_guide_repair("generated-patch", "digest", &prepared)
                .unwrap(),
        );
        assert_eq!(applied.added_feeds, [FeedId(5001)]);
        assert_eq!(store.guide_repair_feed_high_water(id).unwrap(), 5001);
        store.set_guide_sources(id, &[]).unwrap();
        assert_eq!(store.guide_repair_feed_high_water(id).unwrap(), 5001);
        let planned = store
            .plan_provider_guides(id, &[SecretUrl::new("https://guide.example.test/new.xml")])
            .unwrap();
        assert_eq!(planned[0].feed, FeedId(5002));
    }
}
