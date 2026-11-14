//! Atomic, affected-row-conditional guide configuration edits and durable inverses.
//! Network and staged evidence validation belong to the runtime accepting a batch.
use std::collections::{HashMap, HashSet};

use iptv_epg::FeedId;
use iptv_providers::{ContentId, SecretUrl};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};

use crate::{EpgOverride, GuideSource, Result, SourceId, Store, StoreError};

pub const MAX_REPAIR_MAPPINGS: usize = 500;
pub const REPAIR_UNDO_SECONDS: i64 = 7 * 86_400;

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
        let now = self.now();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
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
            if !exists
                || blocked
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
                SELECT 1 FROM epg_feed_channels WHERE source_id=?1 AND feed_id=?2
                AND epg_channel=?3)",
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
        let mut feed_payload = Vec::new();
        for patch in &batch.feeds {
            let feed = patch
                .before
                .as_ref()
                .or(patch.after.as_ref())
                .ok_or_else(invalid)?
                .feed;
            feed_payload.push(json!({"before":feed_json(patch.before.as_ref()),
                "after":feed_json(patch.after.as_ref())}));
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
        let mut next = current.iter().map(|f| f.feed.0).max().unwrap_or(0);
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
        if let Some((_, after)) = &batch.order {
            if !reorder(&mut current, after) {
                return Ok(RepairResult::Conflict(vec!["feed_order".into()]));
            }
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
        let now = self.now();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
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
        let mut conflicts = Vec::new();
        let mut current = feeds(&tx, commit.source)?;
        let touched: HashSet<_> = rows.iter().filter_map(|r| r["channel"].as_str()).collect();
        for row in rows {
            let channel = row["channel"].as_str().ok_or_else(invalid)?;
            if overlay(&tx, commit.source, channel)? != overlay_from(&row["after"])? {
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
        let mut before_feeds = HashMap::new();
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
            before_feeds.insert(feed, before);
        }
        if !conflicts.is_empty() {
            return Ok(RepairResult::Conflict(conflicts));
        }
        for (feed, before) in before_feeds {
            if let Some(at) = current.iter().position(|f| f.feed == feed) {
                match before {
                    Some(v) => current[at] = v,
                    None => {
                        current.remove(at);
                    }
                }
            } else if let Some(v) = before {
                current.push(v);
            }
        }
        if let Some(before) = original_order {
            if !reorder(&mut current, &before) {
                return Ok(RepairResult::Conflict(vec!["feed_order".into()]));
            }
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
}
