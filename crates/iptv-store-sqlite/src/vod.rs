//! Bounded on-demand catalogue reads and durable, source-scoped watch progress.
use crate::{Result, Store, StoreError};
use iptv_core::vod::{Card, Category, Key, Kind, Progress, Query, Source};
use iptv_providers::model::StreamLocator;
use rusqlite::{OptionalExtension, params};

impl Store {
    pub fn vod_sources(&self) -> Result<Vec<Source>> {
        let mut s=self.conn.prepare_cached("SELECT s.id,s.name FROM sources s LEFT JOIN source_policy p ON p.source_id=s.id WHERE COALESCE(p.enabled,1)=1 AND (EXISTS(SELECT 1 FROM vod v WHERE v.source_id=s.id) OR EXISTS(SELECT 1 FROM series v WHERE v.source_id=s.id)) ORDER BY s.name,s.id")?;
        s.query_map([], |r| {
            Ok(Source {
                id: r.get(0)?,
                name: r.get(1)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
    }
    pub fn vod_categories(&self, query: &Query) -> Result<Vec<Category>> {
        let kind = query.kind.name();
        let mut s=self.conn.prepare_cached("SELECT g.source_id,g.id,g.name,g.adult FROM groups g JOIN sources s ON s.id=g.source_id LEFT JOIN source_policy p ON p.source_id=s.id WHERE COALESCE(p.enabled,1)=1 AND (?1 IS NULL OR s.id=?1) AND (g.kind=?2 OR (?2='series' AND g.kind='movie' AND EXISTS(SELECT 1 FROM vod_groups x JOIN vod v ON v.source_id=x.source_id AND v.id=x.vod_id WHERE x.source_id=g.source_id AND x.group_id=g.id AND v.is_series=1))) ORDER BY g.name COLLATE NOCASE,g.source_id,g.id LIMIT 2000")?;
        s.query_map(params![query.source, kind], |r| {
            Ok(Category {
                source: r.get(0)?,
                id: r.get(1)?,
                name: r.get(2)?,
                adult: r.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
    }
    pub fn vod_page(&self, query: &Query, offset: u32, limit: u32) -> Result<(Vec<Card>, bool)> {
        let limit = limit.clamp(1, 200);
        let order = match query.sort {
            iptv_core::vod::Sort::Name => "v.name COLLATE NOCASE",
            iptv_core::vod::Sort::Newest => "v.added DESC",
            iptv_core::vod::Sort::Year => "v.year DESC,v.name COLLATE NOCASE",
            iptv_core::vod::Sort::Rating => "v.rating DESC,v.name COLLATE NOCASE",
        };
        let sql = format!(
            "WITH items AS (SELECT v.source_id,v.id,CASE WHEN v.is_series=1 THEN 'series' ELSE 'movie' END kind,v.name,v.group_ids,v.poster,v.plot,v.year,v.rating_x10 rating,v.duration_secs,v.added FROM vod v WHERE v.is_series=0 OR NOT EXISTS(SELECT 1 FROM series z WHERE z.source_id=v.source_id AND z.id=v.id) UNION ALL SELECT v.source_id,v.id,'series',v.name,v.group_ids,v.poster,v.plot,v.year,v.rating_x10,NULL,v.last_modified FROM series v) SELECT v.source_id,v.id,v.kind,v.name,s.name,v.group_ids,v.poster,v.plot,v.year,v.rating,v.duration_secs,COALESCE(w.position_ms,0),w.duration_ms,COALESCE(w.completed,0) FROM items v JOIN sources s ON s.id=v.source_id LEFT JOIN source_policy p ON p.source_id=s.id LEFT JOIN vod_progress w ON w.source_id=v.source_id AND w.kind=v.kind AND w.content_id=v.id WHERE COALESCE(p.enabled,1)=1 AND v.kind=?1 AND (?2 IS NULL OR v.source_id=?2) AND (?3 IS NULL OR (v.source_id=?3 AND EXISTS(SELECT 1 FROM json_each(v.group_ids) x WHERE x.value=?4))) AND instr(lower(v.name),lower(?5))>0 AND (?6=0 OR (w.position_ms>=5000 AND w.completed=0)) ORDER BY {order},v.source_id,v.id LIMIT ?7 OFFSET ?8"
        );
        let mut s = self.conn.prepare(&sql)?;
        let rows = s.query_map(
            params![
                query.kind.name(),
                query.source,
                query.category.as_ref().map(|c| c.0),
                query.category.as_ref().map(|c| c.1.as_str()),
                query.text.trim(),
                query.continue_watching,
                limit + 1,
                offset
            ],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<u16>>(8)?,
                    r.get::<_, Option<u16>>(9)?,
                    r.get::<_, Option<u32>>(10)?,
                    r.get::<_, i64>(11)?.max(0) as u64,
                    r.get::<_, Option<i64>>(12)?.map(|v| v.max(0) as u64),
                    r.get::<_, bool>(13)?,
                ))
            },
        )?;
        let mut cards = Vec::new();
        for row in rows {
            let (
                source,
                id,
                kind,
                name,
                source_name,
                groups,
                poster,
                plot,
                year,
                rating_x10,
                duration_secs,
                position_ms,
                duration_ms,
                completed,
            ) = row?;
            cards.push(Card {
                key: Key {
                    source,
                    kind: Kind::parse(&kind)
                        .ok_or(StoreError::Unavailable("Invalid library kind"))?,
                    id,
                },
                name,
                source_name,
                categories: crate::codec::group_ids_from_json(&groups)?
                    .into_iter()
                    .map(|g| g.as_str().to_owned())
                    .collect(),
                poster,
                plot,
                year,
                rating_x10,
                duration_secs,
                progress: Progress {
                    position_ms,
                    duration_ms,
                    completed,
                },
            });
        }
        let more = cards.len() > limit as usize;
        cards.truncate(limit as usize);
        Ok((cards, more))
    }
    /// Returns only current, enabled source data; locator never crosses the UI boundary.
    pub fn vod_locator(&self, key: &Key) -> Result<Option<(Option<String>, StreamLocator)>> {
        let mut s=self.conn.prepare_cached("SELECT v.provider_id,v.stream FROM vod v LEFT JOIN source_policy p ON p.source_id=v.source_id WHERE v.source_id=?1 AND v.id=?2 AND COALESCE(p.enabled,1)=1 AND ((?3='series' AND v.is_series=1) OR (?3='movie' AND v.is_series=0))")?;
        let row: Option<(Option<String>, String)> = s
            .query_row(params![key.source, key.id, key.kind.name()], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        row.map(|(id, stream)| crate::codec::stream_from_json(&stream).map(|stream| (id, stream)))
            .transpose()
    }
    pub fn vod_series_provider_id(&self, key: &Key) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT provider_id FROM series WHERE source_id=?1 AND id=?2",
                params![key.source, key.id],
                |r| r.get(0),
            )
            .optional()
            .map(|v| v.flatten())
            .map_err(Into::into)
    }
    pub fn vod_card(&self, key: &Key) -> Result<Option<Card>> {
        let mut query = Query {
            kind: key.kind,
            source: Some(key.source),
            ..Default::default()
        };
        // Exact identity is resolved in SQL below, not a text/name match.
        let sql = "SELECT name FROM vod WHERE source_id=?1 AND id=?2 UNION ALL SELECT name FROM series WHERE source_id=?1 AND id=?2 LIMIT 1";
        let name: Option<String> = self
            .conn
            .query_row(sql, params![key.source, key.id], |r| r.get(0))
            .optional()?;
        let Some(name) = name else { return Ok(None) };
        query.text = name;
        let mut offset = 0;
        loop {
            let (cards, more) = self.vod_page(&query, offset, 200)?;
            if let Some(card) = cards.into_iter().find(|c| c.key == *key) {
                return Ok(Some(card));
            }
            if !more {
                return Ok(None);
            }
            offset += 200;
        }
    }
    pub fn vod_progress(&self, key: &Key) -> Result<Progress> {
        self.conn.query_row("SELECT position_ms,duration_ms,completed FROM vod_progress WHERE source_id=?1 AND kind=?2 AND content_id=?3",params![key.source,key.kind.name(),key.id],|r|Ok(Progress{position_ms:r.get::<_,i64>(0)?.max(0) as u64,duration_ms:r.get::<_,Option<i64>>(1)?.map(|v|v.max(0) as u64),completed:r.get(2)?})).optional().map(|v|v.unwrap_or_default()).map_err(Into::into)
    }
    pub fn save_vod_progress(
        &mut self,
        key: &Key,
        parent: Option<&str>,
        progress: Progress,
    ) -> Result<()> {
        if key.id.is_empty() || key.id.len() > 2048 || parent.is_some_and(|p| p.len() > 2048) {
            return Err(StoreError::Unavailable("Invalid on-demand identity"));
        }
        let cap = |v: u64| v.min(i64::MAX as u64) as i64;
        self.conn.execute("INSERT INTO vod_progress(source_id,kind,content_id,parent_id,position_ms,duration_ms,completed,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,unixepoch()) ON CONFLICT(source_id,kind,content_id) DO UPDATE SET parent_id=excluded.parent_id,position_ms=excluded.position_ms,duration_ms=excluded.duration_ms,completed=excluded.completed,updated_at=excluded.updated_at",params![key.source,key.kind.name(),key.id,parent,cap(progress.position_ms),progress.duration_ms.map(cap),progress.completed])?;
        if let Some(parent) = parent {
            self.conn.execute("INSERT INTO vod_progress(source_id,kind,content_id,position_ms,duration_ms,completed,updated_at) VALUES(?1,'series',?2,?3,?4,0,unixepoch()) ON CONFLICT(source_id,kind,content_id) DO UPDATE SET position_ms=excluded.position_ms,duration_ms=excluded.duration_ms,completed=0,updated_at=excluded.updated_at",params![key.source,parent,cap(progress.position_ms),progress.duration_ms.map(cap)])?;
        }
        Ok(())
    }
    pub fn clear_vod_progress(&mut self, key: &Key) -> Result<()> {
        self.conn.execute(
            "DELETE FROM vod_progress WHERE source_id=?1 AND kind=?2 AND content_id=?3",
            params![key.source, key.kind.name(), key.id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_providers::{
        HttpResponse, SecretUrl,
        model::{ContentKind, M3uSourceConfig, Source, SourceConfig},
    };
    fn fixture() -> (Store, Key) {
        let mut store = Store::open_in_memory().unwrap();
        let source = store
            .add_source(&Source {
                name: "Generated catalogue".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://example.test/generated.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        let groups = iptv_providers::xtream::parse_categories(
            &HttpResponse::ok(r#"[{"category_id":"7","category_name":"Generated category"}]"#),
            ContentKind::Movie,
        )
        .unwrap();
        let movies=iptv_providers::xtream::parse_vod_streams(&HttpResponse::ok(r#"[{"stream_id":10,"name":"Generated Aurora","category_id":"7","container_extension":"mp4"},{"stream_id":11,"name":"Generated Brook","category_id":"7","container_extension":"mp4"}]"#)).unwrap();
        store
            .replace_catalog(source, &groups.items, &[], &movies.items, &[])
            .unwrap();
        (
            store,
            Key {
                source: source.0,
                kind: Kind::Movie,
                id: movies.items[0].id.to_string(),
            },
        )
    }
    #[test]
    fn pages_search_categories_and_progress_are_source_scoped() {
        let (mut store, key) = fixture();
        let query = Query::default();
        let (first, more) = store.vod_page(&query, 0, 1).unwrap();
        assert!(more);
        assert_eq!(first[0].key, key);
        assert_eq!(
            store.vod_page(&query, 1, 1).unwrap().0[0].name,
            "Generated Brook"
        );
        let categories = store.vod_categories(&query).unwrap();
        assert_eq!(categories.len(), 1);
        let query = Query {
            text: "aurora".into(),
            category: Some((key.source, categories[0].id.clone())),
            ..Default::default()
        };
        assert_eq!(store.vod_page(&query, 0, 50).unwrap().0.len(), 1);
        assert!(
            store
                .vod_page(
                    &Query {
                        continue_watching: true,
                        ..Default::default()
                    },
                    0,
                    20
                )
                .unwrap()
                .0
                .is_empty()
        );
        let progress = Progress::observed(31_000, Some(120_000), false);
        store.save_vod_progress(&key, None, progress).unwrap();
        assert_eq!(store.vod_progress(&key).unwrap(), progress);
        assert_eq!(
            store
                .vod_page(
                    &Query {
                        continue_watching: true,
                        ..Default::default()
                    },
                    0,
                    20
                )
                .unwrap()
                .0[0]
                .key,
            key
        );
        let other = Key {
            source: key.source + 1,
            ..key.clone()
        };
        assert_eq!(store.vod_progress(&other).unwrap(), Progress::default());
        store
            .set_source_policy(
                crate::SourceId(key.source),
                crate::SourcePolicy {
                    enabled: false,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            store
                .vod_page(&Query::default(), 0, 20)
                .unwrap()
                .0
                .is_empty()
        );
        assert!(store.vod_locator(&key).unwrap().is_none());
    }
    #[test]
    fn completed_titles_leave_continue_watching() {
        let (mut store, key) = fixture();
        store
            .save_vod_progress(&key, None, Progress::observed(120_000, Some(120_000), true))
            .unwrap();
        assert!(
            store
                .vod_page(
                    &Query {
                        continue_watching: true,
                        ..Default::default()
                    },
                    0,
                    20
                )
                .unwrap()
                .0
                .is_empty()
        );
        assert_eq!(
            store.vod_card(&key).unwrap().unwrap().progress.resume_ms(),
            0
        );
    }
}
