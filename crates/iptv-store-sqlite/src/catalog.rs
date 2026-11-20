//! Provider catalog: wholesale replace per source, plus group/VOD/series
//! reads. Channel reads with overlays applied are in `channels.rs`.

use iptv_providers::model::{Channel, ContentKind, Group, Series, VodItem};
use iptv_providers::{ContentId, GroupId};
use rusqlite::{Connection, TransactionBehavior, params};

use crate::codec::{
    catch_up_to_json, content_kind_from, group_ids_from_json, group_ids_to_json, narrow,
    stream_from_json, stream_to_json,
};
use crate::error::Result;
use crate::sources::{CatalogCounts, ensure_source, record_import_success};
use crate::{SourceId, Store, search};

/// Deletes in child-first order; overlay tables are never touched here.
pub(crate) const DELETE_CATALOG: &[&str] = &[
    "DELETE FROM channel_groups WHERE source_id = ?1",
    "DELETE FROM channels WHERE source_id = ?1",
    "DELETE FROM vod_groups WHERE source_id = ?1",
    "DELETE FROM vod WHERE source_id = ?1",
    "DELETE FROM series_groups WHERE source_id = ?1",
    "DELETE FROM series WHERE source_id = ?1",
    "DELETE FROM groups WHERE source_id = ?1",
];

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

pub(crate) fn insert_groups(
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
fn insert_members(
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

pub(crate) fn insert_channels(
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
            insert_members(
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

pub(crate) fn insert_vod(
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
            insert_members(
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

pub(crate) fn insert_series(
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
            insert_members(
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
    /// Replaces a source's whole provider catalog in one transaction and
    /// records a successful import. Input order is the provider order.
    /// Rows whose id repeats an earlier row are dropped and counted.
    /// User overlays are untouched: they are keyed by stable id and apply
    /// again to every returning id.
    pub fn replace_catalog(
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
        let groups_n = insert_groups(&tx, source, groups, 0)?;
        let channels_n = insert_channels(&tx, source, channels, 0)?;
        let vod_n = insert_vod(&tx, source, vod, 0)?;
        let series_n = insert_series(&tx, source, series, 0)?;
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
        #[cfg(test)]
        crash_tests::boundary("staged");
        tx.commit()?;
        #[cfg(test)]
        crash_tests::boundary("committed");
        Ok(counts)
    }

    /// Provider groups in provider order, optionally of one kind.
    pub fn groups(&self, source: SourceId, kind: Option<ContentKind>) -> Result<Vec<Group>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, kind, name, provider_id, adult FROM groups
             WHERE source_id = ?1 AND (?2 IS NULL OR kind = ?2)
             ORDER BY position",
        )?;
        let rows = stmt.query_map(params![source.0, kind.map(ContentKind::slug)], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, bool>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (id, kind, name, provider_id, adult) = row?;
            Ok(Group {
                id: GroupId::from_stored(id),
                name,
                kind: content_kind_from(&kind)?,
                provider_id,
                adult,
            })
        })
        .collect()
    }

    /// VOD items in provider order, optionally one group, paged.
    pub fn load_vod(
        &self,
        source: SourceId,
        group: Option<&GroupId>,
        offset: u32,
        limit: Option<u32>,
    ) -> Result<Vec<VodItem>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT v.id, v.provider_id, v.name, v.group_ids, v.poster, v.plot, v.year,
                 v.rating_x10, v.added, v.duration_secs, v.stream, v.is_series,v.episode_numbers
             FROM vod v
             WHERE v.source_id = ?1 AND (?2 IS NULL OR EXISTS (SELECT 1 FROM vod_groups m
                 WHERE m.source_id = v.source_id AND m.group_id = ?2 AND m.vod_id = v.id))
             ORDER BY v.position LIMIT ?3 OFFSET ?4",
        )?;
        let limit = limit.map_or(-1, i64::from);
        let rows = stmt.query_map(
            params![source.0, group.map(GroupId::as_str), limit, offset],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                    r.get::<_, Option<i64>>(7)?,
                    r.get::<_, Option<i64>>(8)?,
                    r.get::<_, Option<i64>>(9)?,
                    r.get::<_, String>(10)?,
                    r.get::<_, bool>(11)?,
                    r.get::<_, String>(12)?,
                ))
            },
        )?;
        rows.map(|row| {
            let (
                id,
                provider_id,
                name,
                groups,
                poster,
                plot,
                year,
                rating,
                added,
                dur,
                stream,
                is_series,
                episode_numbers,
            ) = row?;
            Ok(VodItem {
                id: ContentId::from_stored(id),
                provider_id,
                name,
                groups: group_ids_from_json(&groups)?,
                poster,
                plot,
                year: narrow(year, "vod.year")?,
                rating_x10: narrow(rating, "vod.rating_x10")?,
                added,
                duration_secs: narrow(dur, "vod.duration_secs")?,
                stream: stream_from_json(&stream)?,
                is_series,
                episode_numbers: serde_json::from_str(&episode_numbers)?,
            })
        })
        .collect()
    }

    /// Series in provider order, optionally one group, paged.
    pub fn load_series(
        &self,
        source: SourceId,
        group: Option<&GroupId>,
        offset: u32,
        limit: Option<u32>,
    ) -> Result<Vec<Series>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT s.id, s.provider_id, s.name, s.group_ids, s.poster, s.plot, s.year,
                 s.rating_x10, s.last_modified
             FROM series s
             WHERE s.source_id = ?1 AND (?2 IS NULL OR EXISTS (SELECT 1 FROM series_groups m
                 WHERE m.source_id = s.source_id AND m.group_id = ?2 AND m.series_id = s.id))
             ORDER BY s.position LIMIT ?3 OFFSET ?4",
        )?;
        let limit = limit.map_or(-1, i64::from);
        let rows = stmt.query_map(
            params![source.0, group.map(GroupId::as_str), limit, offset],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                    r.get::<_, Option<i64>>(7)?,
                    r.get::<_, Option<i64>>(8)?,
                ))
            },
        )?;
        rows.map(|row| {
            let (id, provider_id, name, groups, poster, plot, year, rating, last_modified) = row?;
            Ok(Series {
                id: ContentId::from_stored(id),
                provider_id,
                name,
                groups: group_ids_from_json(&groups)?,
                poster,
                plot,
                year: narrow(year, "series.year")?,
                rating_x10: narrow(rating, "series.rating_x10")?,
                last_modified,
            })
        })
        .collect()
    }
}

#[cfg(test)]
#[path = "catalog/crash_tests.rs"]
mod crash_tests;
