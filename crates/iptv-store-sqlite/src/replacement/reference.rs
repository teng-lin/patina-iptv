//! Master's one-shot replacements, copied unchanged (apart from names and
//! one helper's signature) from before they became jobs: the reference the
//! jobs are compared with, so a test of the jobs does not compare new code
//! with itself.
#![allow(clippy::too_many_arguments)]

use std::collections::{HashMap, HashSet};

use iptv_import::EpgImport;
use iptv_providers::model::{Channel, Group, Series, VodItem};
use rusqlite::{TransactionBehavior, params};

use crate::catalog::DELETE_CATALOG;
use crate::catalog_stage::{CatalogStage, STAGE_TABLES as TABLES};
use crate::codec::{catch_up_to_json, group_ids_to_json, stream_to_json};
use crate::epg::{
    COPY_CACHE_KIB, EpgCounts, EpgFeedMeta, copy_programs, delete_guide_rows, insert_feed,
    insert_feed_channels, kind_slug, matching_projection_key,
};
use crate::epg_stage::EpgStage;
use crate::sources::{self, CatalogCounts, ensure_source, record_import_success};
use crate::{Result, SourceId, Store, StoreError, catalog, search};
use iptv_providers::{ContentId, GroupId};
use rusqlite::Connection;

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn invalid(s: &str) -> StoreError {
    StoreError::InvalidData(s.into())
}

// Master's insert loops, unchanged apart from their names.
fn master_insert_groups(
    conn: &Connection,
    source: SourceId,
    groups: &[Group],
    offset: i64,
) -> Result<usize> {
    let mut stmt = conn.prepare_cached(
        "INSERT OR IGNORE INTO groups (source_id, id, kind, position, name, provider_id, adult)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    let mut inserted = 0;
    for (position, g) in groups.iter().enumerate() {
        inserted += stmt.execute(params![
            source.0,
            g.id.as_str(),
            g.kind.slug(),
            position as i64 + offset,
            g.name,
            g.provider_id,
            g.adult
        ])?;
    }
    Ok(inserted)
}

/// Inserts membership rows for one item; `sql` is a fixed statement.
fn master_insert_members(
    conn: &Connection,
    sql: &str,
    source: SourceId,
    item: &ContentId,
    groups: &[GroupId],
    position: i64,
) -> Result<()> {
    let mut stmt = conn.prepare_cached(sql)?;
    for g in groups {
        stmt.execute(params![source.0, g.as_str(), item.as_str(), position])?;
    }
    Ok(())
}

fn master_insert_channels(
    conn: &Connection,
    source: SourceId,
    channels: &[Channel],
    offset: i64,
) -> Result<usize> {
    let mut stmt = conn.prepare_cached(
        "INSERT OR IGNORE INTO channels (source_id, id, position, provider_id, name, number, logo,
             group_ids, stream, epg_id, epg_name, epg_shift_minutes, catch_up, radio)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )?;
    let mut inserted = 0;
    for (position, c) in channels.iter().enumerate() {
        let position = position as i64 + offset;
        let added = stmt.execute(params![
            source.0,
            c.id.as_str(),
            position,
            c.provider_id,
            c.name,
            c.number,
            c.logo,
            group_ids_to_json(&c.groups),
            stream_to_json(&c.stream),
            c.epg_id,
            c.epg_name,
            c.epg_shift_minutes,
            c.catch_up.as_ref().map(catch_up_to_json),
            c.radio
        ])?;
        if added > 0 {
            inserted += 1;
            master_insert_members(
                conn,
                "INSERT OR IGNORE INTO channel_groups (source_id, group_id, channel_id, position)
                 VALUES (?1, ?2, ?3, ?4)",
                source,
                &c.id,
                &c.groups,
                position,
            )?;
        }
    }
    Ok(inserted)
}

fn master_insert_vod(
    conn: &Connection,
    source: SourceId,
    vod: &[VodItem],
    offset: i64,
) -> Result<usize> {
    let mut stmt = conn.prepare_cached(
        "INSERT OR IGNORE INTO vod (source_id, id, position, provider_id, name, group_ids, poster,
             plot, year, rating_x10, added, duration_secs, stream, is_series, episode_numbers)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
    )?;
    let mut inserted = 0;
    for (position, v) in vod.iter().enumerate() {
        let position = position as i64 + offset;
        let added = stmt.execute(params![
            source.0,
            v.id.as_str(),
            position,
            v.provider_id,
            v.name,
            group_ids_to_json(&v.groups),
            v.poster,
            v.plot,
            v.year,
            v.rating_x10,
            v.added,
            v.duration_secs,
            stream_to_json(&v.stream),
            v.is_series,
            serde_json::to_string(&v.episode_numbers)?
        ])?;
        if added > 0 {
            inserted += 1;
            master_insert_members(
                conn,
                "INSERT OR IGNORE INTO vod_groups (source_id, group_id, vod_id, position)
                 VALUES (?1, ?2, ?3, ?4)",
                source,
                &v.id,
                &v.groups,
                position,
            )?;
        }
    }
    Ok(inserted)
}

fn master_insert_series(
    conn: &Connection,
    source: SourceId,
    series: &[Series],
    offset: i64,
) -> Result<usize> {
    let mut stmt = conn.prepare_cached(
        "INSERT OR IGNORE INTO series (source_id, id, position, provider_id, name, group_ids,
             poster, plot, year, rating_x10, last_modified)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    let mut inserted = 0;
    for (position, s) in series.iter().enumerate() {
        let position = position as i64 + offset;
        let added = stmt.execute(params![
            source.0,
            s.id.as_str(),
            position,
            s.provider_id,
            s.name,
            group_ids_to_json(&s.groups),
            s.poster,
            s.plot,
            s.year,
            s.rating_x10,
            s.last_modified
        ])?;
        if added > 0 {
            inserted += 1;
            master_insert_members(
                conn,
                "INSERT OR IGNORE INTO series_groups (source_id, group_id, series_id, position)
                 VALUES (?1, ?2, ?3, ?4)",
                source,
                &s.id,
                &s.groups,
                position,
            )?;
        }
    }
    Ok(inserted)
}

impl Store {
    pub(crate) fn reference_replace_catalog(
        &mut self,
        source: SourceId,
        groups: &[Group],
        channels: &[Channel],
        vod: &[VodItem],
        series: &[Series],
    ) -> Result<CatalogCounts> {
        let now = self.now();
        let fts = self.fts_index;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_source(&tx, source)?;
        search::forget_source(&tx, fts, source)?;
        for sql in DELETE_CATALOG {
            tx.execute(sql, [source.0])?;
        }
        let groups_n = master_insert_groups(&tx, source, groups, 0)?;
        let channels_n = master_insert_channels(&tx, source, channels, 0)?;
        let vod_n = master_insert_vod(&tx, source, vod, 0)?;
        let series_n = master_insert_series(&tx, source, series, 0)?;
        search::index_source(&tx, fts, source)?;
        let total = groups.len() + channels.len() + vod.len() + series.len();
        let counts = CatalogCounts {
            groups: count(groups_n),
            channels: count(channels_n),
            vod: count(vod_n),
            series: count(series_n),
            duplicates: count(total - groups_n - channels_n - vod_n - series_n),
        };
        record_import_success(&tx, source, &counts, Default::default(), now)?;
        tx.commit()?;
        Ok(counts)
    }

    pub(crate) fn reference_publish_stage(
        &mut self,
        source: SourceId,
        stage: CatalogStage,
        details: Option<&iptv_providers::model::Source>,
        guides: Option<&[crate::GuideSource]>,
        provider_urls: Option<&[iptv_providers::SecretUrl]>,
    ) -> Result<(CatalogCounts, Option<Vec<crate::GuideSource>>)> {
        if !stage.ready {
            return Err(invalid("cannot publish an incomplete catalog stage"));
        }
        self.attach_staging_database(&stage.path, "catalog_import")?;
        let result: Result<(CatalogCounts, Option<Vec<crate::GuideSource>>)> = (|| {
            let now = self.now();
            let fts = self.fts_index;
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            sources::ensure_source(&tx, source)?;
            let replanned = provider_urls
                .map(|urls| {
                    crate::guide_sources::plan_provider_guides_in(&tx, source, urls, guides, now)
                })
                .transpose()?;
            let guides = replanned.as_deref().or(guides);
            if let Some(details) = details {
                sources::update_source_in(&tx, source, details, now)?;
            }
            if let Some(guides) = guides {
                crate::guide_sources::set_guide_sources_in(&tx, source, guides, now)?;
            }
            search::forget_source(&tx, fts, source)?;
            for sql in catalog::DELETE_CATALOG {
                tx.execute(sql, [source.0])?;
            }
            for table in TABLES {
                let mut stmt = tx.prepare(&format!("PRAGMA catalog_import.table_info({table})"))?;
                let columns = stmt
                    .query_map([], |r| r.get::<_, String>(1))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let columns = columns
                    .into_iter()
                    .filter(|c| c != "source_id")
                    .map(|c| format!("\"{c}\""))
                    .collect::<Vec<_>>()
                    .join(",");
                tx.execute(
                    &format!(
                        "INSERT INTO main.{table}(source_id,{columns}) \
                         SELECT ?1,{columns} FROM catalog_import.{table}"
                    ),
                    [source.0],
                )?;
            }
            search::index_source(&tx, fts, source)?;
            sources::record_import_success(
                &tx,
                source,
                &stage.counts,
                stage.on_demand_errors,
                now,
            )?;
            let published_guides = guides.map(<[crate::GuideSource]>::to_vec);
            tx.commit()?;
            Ok((stage.counts, published_guides))
        })();
        let detached = self
            .conn
            .execute_batch("DETACH DATABASE catalog_import")
            .map_err(StoreError::from);
        let counts = result?;
        detached?;
        Ok(counts)
    }

    pub(crate) fn reference_replace_epg(
        &mut self,
        source: SourceId,
        stage: &mut EpgStage,
        import: &EpgImport,
        feeds: &[EpgFeedMeta],
        match_fingerprint: u64,
    ) -> Result<EpgCounts> {
        stage.flush()?;
        self.attach_staging_database(stage.path(), "epg_stage")?;
        // Every row references a row written just before it in the same
        // transaction: consistent by construction. Off for the copy's sake
        // (`copy_programs`); it cannot change inside a transaction. A
        // larger page cache holds more of `epg_programs_by_stop`, which
        // every channel's programs sweep (a third faster).
        let replaced = self
            .conn
            .pragma_update(None, "foreign_keys", false)
            .and_then(|()| self.conn.pragma_update(None, "cache_size", -COPY_CACHE_KIB))
            .map_err(StoreError::from)
            .and_then(|()| {
                self.reference_replace_staged(source, stage, import, feeds, match_fingerprint)
            });
        let restored = self
            .conn
            .pragma_update(None, "foreign_keys", true)
            .and_then(|()| {
                self.conn
                    .pragma_update(None, "cache_size", -crate::CACHE_KIB)
            });
        let detached = self.conn.execute("DETACH DATABASE epg_stage", []);
        let counts = replaced?;
        restored?;
        detached?;
        Ok(counts)
    }

    fn reference_replace_staged(
        &mut self,
        source: SourceId,
        stage: &EpgStage,
        import: &EpgImport,
        feeds: &[EpgFeedMeta],
        match_fingerprint: u64,
    ) -> Result<EpgCounts> {
        let now = self.now();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_source(&tx, source)?;
        delete_guide_rows(&tx, source)?;

        tx.execute(
            "INSERT INTO epg_guides (source_id, replaced_at, attempted_at, last_error_kind,
                 match_fingerprint)
             VALUES (?1, ?2, ?2, NULL, ?3)
             ON CONFLICT (source_id) DO UPDATE SET replaced_at = excluded.replaced_at,
                 attempted_at = excluded.attempted_at, last_error_kind = NULL,
                 match_fingerprint = excluded.match_fingerprint",
            params![source.0, now, match_fingerprint as i64],
        )?;
        let mut counts = EpgCounts::default();
        for f in feeds {
            insert_feed(&tx, source, f)?;
            let origin_key = format!("inspection.feed.{}.{}", source.0, f.config.id.0);
            tx.execute("DELETE FROM store_meta WHERE key=?1", [&origin_key])?;
            tx.execute(
                "INSERT INTO store_meta(key,value)
                 SELECT ?1,i.incarnation FROM guide_feed_incarnations i JOIN guide_sources g
                 ON g.source_id=i.source_id AND g.feed_id=i.feed_id
                 WHERE g.source_id=?2 AND g.feed_id=?3 AND g.url=?4 AND g.shift_minutes=?5",
                params![
                    origin_key,
                    source.0,
                    f.config.id.0,
                    f.config.url.expose(),
                    f.config.time.shift_minutes
                ],
            )?;

            insert_feed_channels(&tx, source, f.config.id, import.listed(f.config.id))?;
            counts.feeds += 1;
        }
        let known: HashSet<u32> = feeds.iter().map(|f| f.config.id.0).collect();
        // EPG channel -> row id, and per-feed counts.
        let mut channels: HashMap<(u32, &str), i64> = HashMap::new();
        let mut per_feed: HashMap<u32, (u64, u64)> = HashMap::new();
        let mut ends_at: Option<i64> = None;
        // By key (`import.bindings` is sorted): deterministic row ids.
        for (key, epg) in &import.bindings {
            if !known.contains(&epg.source.0) {
                counts.skipped_bindings += 1;
                continue;
            }
            let id = match channels.get(&(epg.source.0, epg.epg_id.as_str())) {
                Some(&id) => id,
                None => {
                    tx.prepare_cached(
                        "INSERT INTO epg_channels (source_id, feed_id, epg_channel)
                         VALUES (?1, ?2, ?3)",
                    )?
                    .execute(params![source.0, epg.source.0, epg.epg_id])?;
                    let id = tx.last_insert_rowid();
                    let (n, d, last) = match stage.channel(epg.source, &epg.epg_id) {
                        Some(staged) => {
                            let (n, d) = copy_programs(&tx, id, staged.id)?;
                            (n, d, staged.last_stop)
                        }
                        None => (0, 0, None),
                    };
                    counts.channels += 1;
                    counts.programs += n;
                    counts.descriptions += d;
                    ends_at = ends_at.max(last);
                    let feed = per_feed.entry(epg.source.0).or_default();
                    feed.0 += 1;
                    feed.1 += n;
                    channels.insert((epg.source.0, epg.epg_id.as_str()), id);
                    id
                }
            };
            tx.prepare_cached(
                "INSERT INTO epg_bindings (source_id, channel_key, epg_channel_id, match_kind)
                 VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![source.0, key.as_str(), id, kind_slug(epg.kind)])?;
            counts.bindings += 1;
        }
        for (feed, (channels, programs)) in per_feed {
            tx.execute(
                "UPDATE epg_feeds SET channel_count = ?3, program_count = ?4
                 WHERE source_id = ?1 AND feed_id = ?2",
                params![source.0, feed, channels as i64, programs as i64],
            )?;
        }
        tx.execute(
            "UPDATE epg_guides SET bound_channels = ?2, ends_at = ?3 WHERE source_id = ?1",
            params![source.0, counts.bindings as i64, ends_at],
        )?;
        // Only a complete binding replacement proves the whole automatic
        // projection. A one-row cached rebind cannot renew this authority.
        tx.execute(
            "INSERT INTO store_meta(key,value)
             SELECT ?1,?2 || ':' || configuration_revision FROM guide_revisions WHERE source_id=?3
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![
                matching_projection_key(source),
                format!("{match_fingerprint:016x}"),
                source.0
            ],
        )?;
        tx.commit()?;
        Ok(counts)
    }
}
