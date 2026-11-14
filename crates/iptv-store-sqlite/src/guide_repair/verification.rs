//! Verification reads actual bindings/programmes, never infers them from commit.
use super::*;

#[derive(Clone, Debug)]
pub struct RepairVerification {
    pub commit: RepairCommit,
    pub examined_mapping: u64,
    pub examined_configuration: u64,
    pub channels: usize,
    pub programmes_available: usize,
    pub covered_seconds: u64,
    pub configuration_effective: bool,
    pub authorized: bool,
}
impl Store {
    /// Consistent read snapshot for preview preparation on the analysis lane.
    pub fn guide_repair_snapshot<T>(&self, read: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        let transaction = self.conn.unchecked_transaction()?;
        let result = read(self)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn verify_guide_repair(
        &self,
        id: &str,
        from: i64,
        to: i64,
    ) -> Result<Option<RepairVerification>> {
        let tx = self.conn.unchecked_transaction()?;
        let Some((commit, payload)) = commit_value(&tx, id)? else {
            return Ok(None);
        };
        let (examined_mapping, examined_configuration) = revisions(&tx, commit.source)?;
        let mut effective = true;
        let mut authorized = true;
        let mut available = 0;
        let mut covered = 0_u64;
        let rows = payload["rows"].as_array().ok_or_else(invalid)?;
        // Every enabled configured feed must have the currently requested
        // address/shift in the stored guide before automatic results certify it.
        let inputs_current: bool = tx.query_row(
            "SELECT NOT EXISTS (
            SELECT 1 FROM guide_sources g LEFT JOIN epg_feeds f
            ON f.source_id=g.source_id AND f.feed_id=g.feed_id
            WHERE g.source_id=?1 AND g.enabled=1 AND
            (f.feed_id IS NULL OR g.url IS NOT f.url OR g.shift_minutes IS NOT f.shift_minutes))",
            [commit.source.0],
            |r| r.get(0),
        )?;
        let mut totals = tx.prepare_cached(
            "SELECT COUNT(*),COALESCE(SUM(
            MAX(0,MIN(stop,?3)-MAX(start,?2))),0) FROM epg_programmes
            WHERE channel_id=?1 AND start<?3 AND stop>?2",
        )?;
        let mut binding = tx.prepare_cached(
            "SELECT c.id,c.feed_id,c.epg_channel,b.match_kind,
            f.url IS g.url AND f.shift_minutes IS g.shift_minutes AND g.enabled=1
            FROM epg_bindings b JOIN epg_channels c ON c.id=b.epg_channel_id
            JOIN epg_feeds f ON f.source_id=c.source_id AND f.feed_id=c.feed_id
            LEFT JOIN guide_sources g ON g.source_id=c.source_id AND g.feed_id=c.feed_id
            WHERE b.source_id=?1 AND b.channel_key=?2",
        )?;
        for row in rows {
            let channel = row["channel"].as_str().ok_or_else(invalid)?;
            let blocked: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM channel_overlays
                WHERE source_id=?1 AND channel_id=?2 AND blocked=1)",
                params![commit.source.0, channel],
                |r| r.get(0),
            )?;
            if blocked {
                authorized = false;
                continue;
            }
            let present: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM channels
                WHERE source_id=?1 AND id=?2)",
                params![commit.source.0, channel],
                |r| r.get(0),
            )?;
            if !present {
                effective = false;
                continue;
            }
            let wanted = overlay_from(&row[if commit.undone { "before" } else { "after" }])?;
            if overlay(&tx, commit.source, channel)? != wanted {
                effective = false;
            }
            let actual = binding
                .query_row(params![commit.source.0, channel], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        FeedId(r.get(1)?),
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<bool>>(4)?.unwrap_or(false),
                    ))
                })
                .optional()?;
            let correct = match (&wanted, &actual) {
                (Some(EpgOverride::Disabled), None) => true,
                (
                    Some(EpgOverride::Xmltv {
                        epg_source,
                        epg_channel,
                    }),
                    Some((_, feed, epg, kind, config)),
                ) => {
                    epg_source == &feed.0.to_string()
                        && epg_channel == epg
                        && kind == "override"
                        && *config
                }
                (None, Some((_, _, _, kind, config))) => {
                    kind != "override" && *config && inputs_current
                }
                (None, None) => inputs_current,
                _ => false,
            };
            effective &= correct;
            if let Some((channel, _, _, _, _)) = actual {
                let (count, seconds): (i64, i64) =
                    totals.query_row(params![channel, from, to], |r| Ok((r.get(0)?, r.get(1)?)))?;
                if count > 0 {
                    available += 1;
                }
                covered = covered.saturating_add(u64::try_from(seconds).unwrap_or(0));
            }
        }
        // Feed-only repairs have indirect effects; certify inputs, not invented
        // programme availability for channels never selected in the batch.
        if rows.is_empty() {
            effective &= inputs_current;
        }
        let result = RepairVerification {
            commit,
            examined_mapping,
            examined_configuration,
            channels: rows.len(),
            programmes_available: available,
            covered_seconds: covered,
            configuration_effective: effective,
            authorized,
        };
        drop(binding);
        drop(totals);
        tx.commit()?;
        Ok(Some(result))
    }
}

impl Store {
    /// Current inverse direction's mapping choices, privately consumed by the
    /// runtime cache publisher. A configuration edit requires a fresh import.
    pub fn guide_repair_publication(
        &self,
        id: &str,
    ) -> Result<Option<(RepairCommit, Vec<(ContentId, Option<EpgOverride>)>, bool)>> {
        let Some((commit, payload)) = commit_value(&self.conn, id)? else {
            return Ok(None);
        };
        let key = if commit.undone { "before" } else { "after" };
        let mut choices = Vec::new();
        for row in payload["rows"].as_array().ok_or_else(invalid)? {
            choices.push((
                ContentId::from_stored(row["channel"].as_str().ok_or_else(invalid)?),
                overlay_from(&row[key])?,
            ));
        }
        let changed_config = !payload["feeds"].as_array().ok_or_else(invalid)?.is_empty()
            || !payload["order"].is_null();
        Ok(Some((commit, choices, changed_config)))
    }
}

impl Store {
    /// Recent durable commits remain discoverable after an ordinary owner restart.
    pub fn recent_guide_repairs(&self, source: SourceId, limit: u32) -> Result<Vec<RepairCommit>> {
        crate::sources::ensure_source(&self.conn, source)?;
        if limit == 0 || limit > 50 {
            return Err(StoreError::InvalidData("guide repair history limit".into()));
        }
        let mut statement = self.conn.prepare_cached(
            "SELECT id FROM guide_repair_commits
            WHERE source_id=?1 ORDER BY created_at DESC,id DESC LIMIT ?2",
        )?;
        let ids = statement
            .query_map(params![source.0, limit], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter()
            .map(|id| {
                commit_value(&self.conn, &id)?
                    .map(|(commit, _)| commit)
                    .ok_or_else(invalid)
            })
            .collect()
    }
}

impl Store {
    pub fn guide_repair_receipt(&self, id: &str, digest: &str) -> Result<RepairResult> {
        Ok(match commit_value(&self.conn, id)? {
            Some((commit, payload)) if payload["digest"].as_str() == Some(digest) => {
                RepairResult::Committed(commit)
            }
            Some(_) => RepairResult::Invalid,
            None => RepairResult::Missing,
        })
    }
}
