//! Verification reads actual bindings/programmes, never infers them from commit.
use super::*;

#[derive(Clone, Debug)]
pub struct RepairVerification {
    pub commit: RepairCommit,
    pub examined_mapping: u64,
    pub examined_configuration: u64,
    pub channels: usize,
    pub programs_available: usize,
    pub covered_seconds: u64,
    pub configuration_effective: bool,
    pub authorized: bool,
    pub source_enabled: bool,
    pub evidence_revision: String,
}
impl Store {
    /// Consistent read snapshot for preview preparation on the analysis lane.
    pub fn guide_repair_snapshot<T>(&self, read: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        self.read_snapshot(read)
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
        let source_enabled: bool = tx.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM source_policy WHERE source_id=?1 AND enabled=0)",
            [commit.source.0],
            |row| row.get(0),
        )?;
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
            (f.feed_id IS NULL OR g.url IS NOT f.url OR g.shift_minutes IS NOT f.shift_minutes
             OR NOT EXISTS(SELECT 1 FROM store_meta m JOIN guide_feed_incarnations i
             ON i.source_id=g.source_id AND i.feed_id=g.feed_id
             WHERE m.key='inspection.feed.' || g.source_id || '.' || g.feed_id AND m.value=i.incarnation)))",
            [commit.source.0],
            |r| r.get(0),
        )?;
        let bindings_current:bool=tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM epg_bindings b
            JOIN epg_channels c ON c.id=b.epg_channel_id
            LEFT JOIN guide_sources g ON g.source_id=c.source_id AND g.feed_id=c.feed_id
            LEFT JOIN epg_feeds f ON f.source_id=c.source_id AND f.feed_id=c.feed_id
            LEFT JOIN guide_feed_incarnations i ON i.source_id=c.source_id AND i.feed_id=c.feed_id
            LEFT JOIN store_meta m ON m.key='inspection.feed.' || c.source_id || '.' || c.feed_id
            WHERE b.source_id=?1 AND (g.feed_id IS NULL OR g.enabled=0
            OR f.url IS NOT g.url OR f.shift_minutes IS NOT g.shift_minutes OR m.value IS NOT i.incarnation))",
            [commit.source.0],|r|r.get(0))?;
        let automatic_current = self
            .guide_repair_automatic_projection_current(commit.source, examined_configuration)?
            .0;
        let mut totals = tx.prepare_cached(
            "SELECT COUNT(*),COALESCE(SUM(
            MAX(0,MIN(stop,?3)-MAX(start,?2))),0) FROM epg_programs
            WHERE channel_id=?1 AND start<?3 AND stop>?2",
        )?;
        let mut binding = tx.prepare_cached(
            "SELECT c.id,c.feed_id,c.epg_channel,b.match_kind,
            f.url IS g.url AND f.shift_minutes IS g.shift_minutes AND g.enabled=1
            AND EXISTS(SELECT 1 FROM store_meta m JOIN guide_feed_incarnations i
             ON i.source_id=g.source_id AND i.feed_id=g.feed_id
             WHERE m.key='inspection.feed.' || g.source_id || '.' || g.feed_id AND m.value=i.incarnation)
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
                    kind != "override" && *config && inputs_current && automatic_current
                }
                (None, None) => inputs_current && automatic_current,
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
            effective &= inputs_current && bindings_current && automatic_current;
        }
        let evidence_revision = self.guide_repair_coverage_revision(commit.source, from, to)?;
        let result = RepairVerification {
            commit,
            examined_mapping,
            examined_configuration,
            channels: rows.len(),
            programs_available: available,
            covered_seconds: covered,
            configuration_effective: effective,
            authorized,
            source_enabled,
            evidence_revision,
        };
        drop(binding);
        drop(totals);
        tx.commit()?;
        Ok(Some(result))
    }
}

pub struct RepairPublication {
    pub commit: RepairCommit,
    pub choices: Vec<(ContentId, Option<EpgOverride>)>,
    pub config_changed: bool,
}
impl Store {
    /// Current inverse direction's mapping choices, privately consumed by the
    /// runtime cache publisher. A configuration edit requires a fresh import.
    pub fn guide_repair_publication(&self, id: &str) -> Result<Option<RepairPublication>> {
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
        Ok(Some(RepairPublication {
            commit,
            choices,
            config_changed: changed_config,
        }))
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

#[derive(Clone, Debug)]
pub struct RepairChannel {
    pub key: ContentId,
    pub name: String,
    pub provider_name: String,
    pub epg_id: Option<String>,
    pub epg_name: Option<String>,
    pub blocked: bool,
    pub epg_override: Option<EpgOverride>,
}
#[derive(Clone, Debug)]
pub struct RepairBinding {
    pub key: ContentId,
    pub candidate: iptv_epg::EpgRef,
    pub incarnation: Option<String>,
}
pub struct RepairInputs {
    pub channels: Vec<RepairChannel>,
    pub bindings: Vec<RepairBinding>,
    pub available: Vec<(iptv_epg::FeedId, String)>,
}
impl Store {
    /// Bounded channel metadata; do not materialize a full legacy feed index
    /// before checking its size. None means the explicit analysis limit.
    pub fn guide_repair_listed(
        &self,
        source: SourceId,
    ) -> Result<Option<Vec<crate::GuideChannel>>> {
        let mut statement = self.conn.prepare_cached(
            "SELECT feed_id,epg_channel,MAX(name) FROM (
            SELECT feed_id,epg_channel,name FROM epg_feed_channels WHERE source_id=?1
            UNION ALL SELECT feed_id,epg_channel,NULL FROM epg_channels WHERE source_id=?1)
            GROUP BY feed_id,epg_channel ORDER BY feed_id,epg_channel LIMIT 100001",
        )?;
        let mut query = statement.query([source.0])?;
        let mut listed = Vec::new();
        let mut bytes = 0usize;
        while let Some(row) = query.next()? {
            for column in [1, 2] {
                if let rusqlite::types::ValueRef::Text(value) = row.get_ref(column)? {
                    bytes = bytes.saturating_add(value.len());
                }
            }
            if listed.len() >= 100_000 || bytes > 64 * 1024 * 1024 {
                return Ok(None);
            }
            listed.push(crate::GuideChannel {
                feed: iptv_epg::FeedId(row.get(0)?),
                epg_channel: row.get(1)?,
                name: row.get(2)?,
                assigned: false,
            });
        }
        Ok(Some(listed))
    }

    /// Only matching metadata: stream URLs, headers, logos and large membership
    /// arrays never enter the repair analysis snapshot.
    pub fn guide_repair_inputs(&self, source: SourceId) -> Result<Option<RepairInputs>> {
        let bytes:i64=self.conn.query_row("SELECT COALESCE(SUM(bytes),0) FROM (
            SELECT length(CAST(c.id AS BLOB))+length(CAST(c.name AS BLOB))+
            COALESCE(length(CAST(o.custom_name AS BLOB)),0)+COALESCE(length(CAST(c.epg_id AS BLOB)),0)+
            COALESCE(length(CAST(c.epg_name AS BLOB)),0)+COALESCE(length(CAST(o.epg_channel AS BLOB)),0) AS bytes
            FROM channels c LEFT JOIN channel_overlays o ON o.source_id=c.source_id AND o.channel_id=c.id
            WHERE c.source_id=?1 LIMIT 100001)",[source.0],|r|r.get(0))?;
        if bytes > 64 * 1024 * 1024 {
            return Ok(None);
        }
        let mut statement = self.conn.prepare_cached(
            "SELECT c.id,COALESCE(o.custom_name,c.name),
            c.name,c.epg_id,c.epg_name,COALESCE(o.blocked,0),o.epg_mode,o.epg_source,o.epg_channel
            FROM channels c LEFT JOIN channel_overlays o
            ON o.source_id=c.source_id AND o.channel_id=c.id
            WHERE c.source_id=?1 ORDER BY c.id LIMIT 100001",
        )?;
        let channels = statement
            .query_map([source.0], |r| {
                Ok(RepairChannel {
                    key: ContentId::from_stored(r.get::<_, String>(0)?),
                    name: r.get(1)?,
                    provider_name: r.get(2)?,
                    epg_id: r.get(3)?,
                    epg_name: r.get(4)?,
                    blocked: r.get(5)?,
                    epg_override: EpgOverride::from_columns(r.get(6)?, r.get(7)?, r.get(8)?),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut statement = self.conn.prepare_cached(
            "SELECT b.channel_key,c.feed_id,c.epg_channel,
            b.match_kind,m.value FROM epg_bindings b JOIN epg_channels c ON c.id=b.epg_channel_id
            LEFT JOIN store_meta m ON m.key='inspection.feed.' || c.source_id || '.' || c.feed_id
            WHERE b.source_id=?1 ORDER BY b.channel_key LIMIT 100001",
        )?;
        let bindings = statement
            .query_map([source.0], |r| {
                Ok(RepairBinding {
                    key: ContentId::from_stored(r.get::<_, String>(0)?),
                    candidate: iptv_epg::EpgRef {
                        source: iptv_epg::FeedId(r.get(1)?),
                        epg_id: r.get(2)?,
                        kind: crate::epg::kind_from(&r.get::<_, String>(3)?),
                    },
                    incarnation: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut statement = self.conn.prepare_cached(
            "SELECT c.feed_id,c.epg_channel FROM epg_channels c
            JOIN guide_sources g ON g.source_id=c.source_id AND g.feed_id=c.feed_id
            JOIN epg_feeds f ON f.source_id=c.source_id AND f.feed_id=c.feed_id
            JOIN guide_feed_incarnations i ON i.source_id=g.source_id AND i.feed_id=g.feed_id
            JOIN store_meta m ON m.key='inspection.feed.' || g.source_id || '.' || g.feed_id
            WHERE c.source_id=?1 AND m.value=i.incarnation AND f.url=g.url
            AND f.shift_minutes=g.shift_minutes AND g.enabled=1
            AND EXISTS(SELECT 1 FROM epg_programs p WHERE p.channel_id=c.id) LIMIT 100001",
        )?;
        let available = statement
            .query_map([source.0], |r| Ok((iptv_epg::FeedId(r.get(0)?), r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(RepairInputs {
            channels,
            bindings,
            available,
        }))
    }
}

impl Store {
    /// Semantic programme/binding evidence, stable across physical no-op imports.
    /// Hash rows incrementally on the bounded analysis connection; no row IDs.
    pub fn guide_repair_coverage_revision(
        &self,
        source: SourceId,
        from: i64,
        to: i64,
    ) -> Result<String> {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update(self.source_incarnation(source)?.as_bytes());
        // Effective automatic proof is part of the examined evidence, including
        // catalogue changes that leave mapping/configuration revisions intact.
        let (_, configuration) = revisions(&self.conn, source)?;
        let (current, fingerprint) =
            self.guide_repair_automatic_projection_current(source, configuration)?;
        digest.update([u8::from(current)]);
        if let Some(fingerprint) = fingerprint {
            digest.update(fingerprint.to_le_bytes());
        }
        let mut statement=self.conn.prepare_cached("SELECT b.channel_key,c.feed_id,c.epg_channel,
            m.value,p.start,p.stop,p.title,b.match_kind FROM epg_bindings b
            JOIN epg_channels c ON c.id=b.epg_channel_id
            LEFT JOIN store_meta m ON m.key='inspection.feed.' || c.source_id || '.' || c.feed_id
            LEFT JOIN epg_programs p ON p.channel_id=c.id AND p.start<?3 AND p.stop>?2
            WHERE b.source_id=?1 ORDER BY b.channel_key,c.feed_id,c.epg_channel,p.start,p.stop,p.title")?;
        let mut rows = statement.query(params![source.0, from, to])?;
        while let Some(row) = rows.next()? {
            for column in 0..8 {
                use rusqlite::types::ValueRef;
                match row.get_ref(column)? {
                    ValueRef::Null => digest.update([0]),
                    ValueRef::Integer(value) => {
                        digest.update([1]);
                        digest.update(value.to_le_bytes());
                    }
                    ValueRef::Text(value) => {
                        digest.update([2]);
                        digest.update((value.len() as u64).to_le_bytes());
                        digest.update(value);
                    }
                    _ => return Err(invalid()),
                }
            }
        }
        Ok(format!("{:x}", digest.finalize()))
    }
}

impl Store {
    // Recompute the existing portable matching hash from bounded metadata only.
    // Stream locators, headers and group membership never enter this read.
    fn guide_repair_automatic_projection_current(
        &self,
        source: SourceId,
        configuration: u64,
    ) -> Result<(bool, Option<u64>)> {
        let enabled: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM guide_sources WHERE source_id=?1 AND enabled=1)",
            [source.0],
            |r| r.get(0),
        )?;
        let empty: bool = self.conn.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM epg_bindings WHERE source_id=?1)",
            [source.0],
            |r| r.get(0),
        )?;
        let stamp: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM store_meta WHERE key=?1",
                [crate::epg::matching_projection_key(source)],
                |r| r.get(0),
            )
            .optional()?;
        let mut bytes = 0usize;
        let mut channels = Vec::new();
        let mut statement = self.conn.prepare_cached(
            "SELECT id,name,epg_id,epg_name FROM channels WHERE source_id=?1 ORDER BY id LIMIT 100001")?;
        let mut rows = statement.query([source.0])?;
        while let Some(row) = rows.next()? {
            for column in 0..4 {
                if let rusqlite::types::ValueRef::Text(value) = row.get_ref(column)? {
                    bytes = bytes.saturating_add(value.len());
                }
            }
            if channels.len() >= 100_000 || bytes > 64 * 1024 * 1024 {
                return Ok((false, None));
            }
            channels.push(iptv_providers::model::Channel {
                id: ContentId::from_stored(row.get::<_, String>(0)?),
                provider_id: None,
                name: row.get(1)?,
                number: None,
                logo: None,
                groups: vec![],
                stream: iptv_providers::model::StreamLocator::Url {
                    url: String::new(),
                    headers: vec![],
                    properties: vec![],
                },
                epg_id: row.get(2)?,
                epg_name: row.get(3)?,
                epg_shift_minutes: None,
                catch_up: None,
                radio: false,
            });
        }
        let mut overrides = iptv_epg::Overrides::default();
        let mut statement = self.conn.prepare_cached(
            "SELECT channel_id,epg_mode,epg_source,epg_channel FROM channel_overlays
             WHERE source_id=?1 AND epg_mode IS NOT NULL ORDER BY channel_id LIMIT 100001",
        )?;
        let mut rows = statement.query([source.0])?;
        let mut count = 0usize;
        while let Some(row) = rows.next()? {
            for column in 0..4 {
                if let rusqlite::types::ValueRef::Text(value) = row.get_ref(column)? {
                    bytes = bytes.saturating_add(value.len());
                }
            }
            count += 1;
            if count > 100_000 || bytes > 64 * 1024 * 1024 {
                return Ok((false, None));
            }
            if let Some(value) = EpgOverride::from_columns(row.get(1)?, row.get(2)?, row.get(3)?) {
                overrides.insert(
                    iptv_epg::ChannelKey::new(row.get::<_, String>(0)?),
                    value.to_override(),
                );
            }
        }
        let fingerprint = iptv_import::matching_fingerprint(&channels, &overrides);
        Ok((
            (!enabled && empty)
                || (enabled
                    && stamp.as_deref()
                        == Some(format!("{fingerprint:016x}:{configuration}").as_str())),
            Some(fingerprint),
        ))
    }
}
