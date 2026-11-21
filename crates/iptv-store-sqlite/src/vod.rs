//! Bounded on-demand catalogue reads and durable, source-scoped watch progress.
use crate::{Result, Store, StoreError};
use iptv_core::vod::{
    Card, Category, DiscoveryPage, DiscoveryQuery, Key, Kind, Progress, Query, Shelf, Source,
};
use iptv_providers::model::StreamLocator;
use rusqlite::{OptionalExtension, params};

/// Every movie and series of the catalogues, once each: a series that a
/// source lists both as a series and as an embedded movie-catalogue series
/// is the series row. `added` is the provider's added time for movies and
/// last-modified time for series; never the release year.
macro_rules! catalog_items {
    () => {
        "WITH items AS (SELECT v.source_id,v.id,CASE WHEN v.is_series=1 THEN 'series' ELSE 'movie' \
END kind,v.name,v.group_ids,v.poster,v.plot,v.year,v.rating_x10 rating,v.duration_secs,v.added FROM \
vod v WHERE v.is_series=0 OR NOT EXISTS(SELECT 1 FROM series z WHERE z.source_id=v.source_id AND \
z.id=v.id) UNION ALL SELECT \
v.source_id,v.id,'series',v.name,v.group_ids,v.poster,v.plot,v.year,v.rating_x10,NULL,v.last_modified \
FROM series v)"
    };
}
/// Whether source `id` has movies, and series, as the catalogue reads them.
pub(crate) fn on_demand_kinds(
    conn: &rusqlite::Connection,
    id: crate::SourceId,
) -> Result<(bool, bool)> {
    // The kind rule of `catalogue_items!` on the base tables, keyed by
    // source: a movie-list row is a movie unless it is an embedded series;
    // a series is a series-list row or an embedded one. (The shared CTE
    // would be materialised across every source.)
    let mut statement = conn.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM vod WHERE source_id=?1 AND is_series=0), \
EXISTS(SELECT 1 FROM series WHERE source_id=?1) OR \
EXISTS(SELECT 1 FROM vod WHERE source_id=?1 AND is_series=1)",
    )?;
    Ok(statement.query_row([id.0], |r| Ok((r.get(0)?, r.get(1)?)))?)
}
/// The columns `raw_card` reads, from `items v`, `sources s`,
/// `vod_progress w` and the `last_episode!` record `l`. A series whose
/// episode watched last was finished resumes its next episode from the
/// start, so its card carries no resume point.
macro_rules! card_columns {
    () => {
        "v.source_id,v.id,v.kind,v.name,s.name,v.group_ids,v.poster,v.plot,v.year,v.rating,\
v.duration_secs,CASE WHEN l.completed=1 THEN 0 ELSE COALESCE(w.position_ms,0) END,CASE WHEN \
l.completed=1 THEN NULL ELSE w.duration_ms END,COALESCE(w.completed,0)"
    };
}
/// Joins `l`, the progress of the episode a series record `w` watched last
/// (a primary-key lookup; absent for movies).
macro_rules! last_episode {
    () => {
        " LEFT JOIN vod_progress l ON l.source_id=w.source_id AND l.kind='episode' AND \
l.content_id=w.last_episode_id"
    };
}

/// Whether the progress record `p` (a movie or a series' parent record) is
/// unfinished enough for Continue watching, `$min` being the minimum
/// position. A movie must be that far in. A series also counts when another
/// of its episodes was finished or watched that far, so advancing to the
/// next episode and stopping at once keeps it listed, while a series only
/// glanced at stays out. Its own `completed` (the whole series) always
/// wins: older episode records cannot bring a completed series back. The
/// episode lookup uses the covering source/parent index, so each series
/// checks its own watched episodes rather than every series' history.
macro_rules! unfinished {
    ($p:literal, $min:literal) => {
        concat!(
            $p,
            ".completed=0 AND (",
            $p,
            ".position_ms>=",
            $min,
            " OR (",
            $p,
            ".kind='series' AND EXISTS(SELECT 1 FROM vod_progress e WHERE e.source_id=",
            $p,
            ".source_id AND e.kind='episode' AND e.parent_id=",
            $p,
            ".content_id AND (e.completed=1 OR e.position_ms>=",
            $min,
            "))))"
        )
    };
}

/// The rows of Continue watching as CTE `c`, unordered: unfinished movies
/// and series of enabled sources with their card columns and progress time
/// (`$min` is the minimum position). The progress records drive the query;
/// each looks up its own title by primary key, so the work follows what was
/// watched, not the catalogue's size. `CROSS JOIN` keeps that order. A
/// series appears once, through its parent record, which follows the
/// episode watched last; when that episode was finished, the row resumes
/// the next one from its start (position 0).
macro_rules! continue_rows {
    ($min:literal) => {
        concat!(
            "WITH w AS (SELECT p.source_id,p.kind,p.content_id,p.position_ms,p.duration_ms,\
p.completed,p.updated_at,p.last_episode_id FROM vod_progress p WHERE p.kind IN ('movie','series') \
AND ",
            unfinished!("p", $min),
            "), found AS (SELECT w.source_id,w.content_id id,w.kind,v.name,v.group_ids,v.poster,\
v.plot,v.year,v.rating_x10 rating,v.duration_secs,w.position_ms,w.duration_ms,w.completed,\
w.updated_at,w.last_episode_id FROM w CROSS JOIN vod v WHERE v.source_id=w.source_id AND \
v.id=w.content_id AND ((w.kind='movie' AND v.is_series=0) OR (w.kind='series' AND v.is_series=1 \
AND NOT EXISTS(SELECT 1 FROM series z WHERE z.source_id=v.source_id AND z.id=v.id))) UNION ALL \
SELECT w.source_id,w.content_id,w.kind,v.name,v.group_ids,v.poster,v.plot,v.year,v.rating_x10,NULL,\
w.position_ms,w.duration_ms,w.completed,w.updated_at,w.last_episode_id FROM w CROSS JOIN series v \
WHERE w.kind='series' AND v.source_id=w.source_id AND v.id=w.content_id), c AS (SELECT \
f.source_id,f.id,f.kind,f.name,s.name source_name,f.group_ids,f.poster,f.plot,f.year,f.rating,\
f.duration_secs,CASE WHEN l.completed=1 THEN 0 ELSE f.position_ms END position_ms,CASE WHEN \
l.completed=1 THEN NULL ELSE f.duration_ms END duration_ms,f.completed,f.updated_at FROM found f \
JOIN sources s ON s.id=f.source_id LEFT JOIN source_policy q ON q.source_id=f.source_id LEFT JOIN \
vod_progress l ON l.source_id=f.source_id AND l.kind='episode' AND l.content_id=f.last_episode_id \
WHERE COALESCE(q.enabled,1)=1)"
        )
    };
}

/// Continue watching, newest progress first, then source, kind and id
/// (`?1` limit, `?2` offset, `?3` minimum position).
const CONTINUE_WATCHING_SQL: &str = concat!(
    continue_rows!("?3"),
    " SELECT c.source_id,c.id,c.kind,c.name,c.source_name,c.group_ids,c.poster,c.plot,c.year,\
c.rating,c.duration_secs,c.position_ms,c.duration_ms,c.completed FROM c ORDER BY c.updated_at \
DESC,c.source_id,c.kind,c.id LIMIT ?1 OFFSET ?2"
);

/// How many Continue watching titles precede one (`?2` source, `?3` kind,
/// `?4` id; `?1` minimum position); no row when it is not listed.
const CONTINUE_WATCHING_RANK_SQL: &str = concat!(
    continue_rows!("?1"),
    " SELECT (SELECT COUNT(*) FROM c o WHERE o.updated_at>m.updated_at OR \
(o.updated_at=m.updated_at AND (o.source_id,o.kind,o.id)<(m.source_id,m.kind,m.id))) FROM c m \
WHERE m.source_id=?2 AND m.kind=?3 AND m.id=?4"
);

/// Movies and series of enabled sources, newest provider time first; a
/// title without one (absent or not positive) follows, by name and then
/// identity. Every key is unique, so the order is total and pages are
/// stable while the library is unchanged.
///
/// No index serves this order across both catalogues: each page sorts the
/// enabled movies and series (a temporary B-tree over the whole library),
/// so it is linear in the library's size. The runtime therefore reads it
/// on the store's analysis lane, never on the reader that serves guide
/// pages, and cancels it when the shelf closes or a newer page is asked
/// for ([`Store::vod_discovery_cancelable`]).
const RECENTLY_ADDED_SQL: &str = concat!(
    catalog_items!(),
    " SELECT ",
    card_columns!(),
    " FROM items v JOIN sources s ON s.id=v.source_id LEFT JOIN source_policy p ON \
p.source_id=s.id LEFT JOIN vod_progress w ON w.source_id=v.source_id AND w.kind=v.kind AND \
w.content_id=v.id",
    last_episode!(),
    " WHERE COALESCE(p.enabled,1)=1 ORDER BY CASE WHEN v.added>0 THEN 0 ELSE 1 END,\
CASE WHEN v.added>0 THEN v.added END DESC,v.name COLLATE NOCASE,v.source_id,v.kind,v.id LIMIT ?1 \
OFFSET ?2"
);

/// How many Recently added titles precede one (`?1` source, `?2` kind, `?3`
/// id) in [`RECENTLY_ADDED_SQL`]'s order; no row when it is not listed.
/// Linear in the library like the page itself, so it runs where the page
/// does.
const RECENTLY_ADDED_RANK_SQL: &str = concat!(
    catalog_items!(),
    ", r AS (SELECT v.source_id,v.kind,v.id,v.name,CASE WHEN v.added>0 THEN v.added END added FROM \
items v JOIN sources s ON s.id=v.source_id LEFT JOIN source_policy p ON p.source_id=s.id WHERE \
COALESCE(p.enabled,1)=1) SELECT (SELECT COUNT(*) FROM r o WHERE (o.added IS NOT NULL AND m.added IS \
NULL) OR (((o.added IS NOT NULL AND m.added IS NOT NULL AND o.added=m.added) OR (o.added IS NULL AND \
m.added IS NULL)) AND (o.name COLLATE NOCASE<m.name OR (o.name COLLATE NOCASE=m.name AND \
(o.source_id,o.kind,o.id)<(m.source_id,m.kind,m.id)))) OR (o.added IS NOT NULL AND m.added IS NOT \
NULL AND o.added>m.added)) FROM r m WHERE m.source_id=?1 AND m.kind=?2 AND m.id=?3"
);

/// A card as its row holds it, before the fallible conversions.
struct RawCard {
    source: i64,
    id: String,
    kind: String,
    name: String,
    source_name: String,
    groups: String,
    poster: Option<String>,
    plot: Option<String>,
    year: Option<u16>,
    rating_x10: Option<u16>,
    duration_secs: Option<u32>,
    position_ms: u64,
    duration_ms: Option<u64>,
    completed: bool,
}

/// Reads the `card_columns!` of a row.
fn raw_card(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawCard> {
    Ok(RawCard {
        source: r.get(0)?,
        id: r.get(1)?,
        kind: r.get(2)?,
        name: r.get(3)?,
        source_name: r.get(4)?,
        groups: r.get(5)?,
        poster: r.get(6)?,
        plot: r.get(7)?,
        year: r.get(8)?,
        rating_x10: r.get(9)?,
        duration_secs: r.get(10)?,
        position_ms: r.get::<_, i64>(11)?.max(0) as u64,
        duration_ms: r.get::<_, Option<i64>>(12)?.map(|v| v.max(0) as u64),
        completed: r.get(13)?,
    })
}

impl RawCard {
    fn card(self) -> Result<Card> {
        Ok(Card {
            key: Key {
                source: self.source,
                kind: Kind::parse(&self.kind)
                    .ok_or(StoreError::Unavailable("Invalid library kind"))?,
                id: self.id,
            },
            name: self.name,
            source_name: self.source_name,
            categories: crate::codec::group_ids_from_json(&self.groups)?
                .into_iter()
                .map(|g| g.as_str().to_owned())
                .collect(),
            poster: self.poster,
            plot: self.plot,
            year: self.year,
            rating_x10: self.rating_x10,
            duration_secs: self.duration_secs,
            progress: Progress {
                position_ms: self.position_ms,
                duration_ms: self.duration_ms,
                completed: self.completed,
            },
        })
    }
}

/// The first `limit` cards of rows read with one extra, and whether that
/// extra one existed.
fn cards_page(
    rows: impl Iterator<Item = rusqlite::Result<RawCard>>,
    limit: u32,
) -> Result<(Vec<Card>, bool)> {
    let mut cards = Vec::new();
    for row in rows {
        cards.push(row?.card()?);
    }
    let more = cards.len() > limit as usize;
    cards.truncate(limit as usize);
    Ok((cards, more))
}

impl Store {
    pub fn vod_sources(&self) -> Result<Vec<Source>> {
        let mut s = self.conn.prepare_cached(
            "SELECT s.id,s.name FROM sources s LEFT JOIN source_policy p ON p.source_id=s.id WHERE \
COALESCE(p.enabled,1)=1 AND (EXISTS(SELECT 1 FROM vod v WHERE v.source_id=s.id) OR EXISTS(SELECT 1 \
FROM series v WHERE v.source_id=s.id)) ORDER BY s.name,s.id",
        )?;
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
        let mut s=self.conn.prepare_cached("SELECT g.source_id,g.id,g.name,g.adult FROM groups g JOIN sources s ON s.id=g.source_id LEFT JOIN \
source_policy p ON p.source_id=s.id WHERE COALESCE(p.enabled,1)=1 AND (?1 IS NULL OR s.id=?1) AND \
(g.kind=?2 OR (?2='series' AND g.kind='movie' AND EXISTS(SELECT 1 FROM vod_groups x JOIN vod v ON \
v.source_id=x.source_id AND v.id=x.vod_id WHERE x.source_id=g.source_id AND x.group_id=g.id AND \
v.is_series=1))) ORDER BY g.name COLLATE NOCASE,g.source_id,g.id LIMIT 2000")?;
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
        self.vod_page_inner(query, offset, limit, None)
    }
    fn vod_page_inner(
        &self,
        query: &Query,
        offset: u32,
        limit: u32,
        exact: Option<&Key>,
    ) -> Result<(Vec<Card>, bool)> {
        let limit = limit.clamp(1, 200);
        // Explicit equality lets SQLite use each catalogue's source/content key.
        // An optional OR predicate would still scan every shared-name item.
        let identity = if exact.is_some() {
            "v.source_id=?2 AND v.id=?9"
        } else {
            "?9 IS NULL"
        };
        let order = match query.sort {
            iptv_core::vod::Sort::Name => "v.name COLLATE NOCASE",
            iptv_core::vod::Sort::Newest => "v.added DESC",
            iptv_core::vod::Sort::Year => "v.year DESC,v.name COLLATE NOCASE",
            iptv_core::vod::Sort::Rating => "v.rating DESC,v.name COLLATE NOCASE",
        };
        let sql = format!(
            concat!(
                catalog_items!(),
                " SELECT ",
                card_columns!(),
                " FROM items v JOIN sources s ON s.id=v.source_id LEFT JOIN source_policy p ON \
p.source_id=s.id LEFT JOIN vod_progress w ON w.source_id=v.source_id AND w.kind=v.kind AND \
w.content_id=v.id",
                last_episode!(),
                " WHERE COALESCE(p.enabled,1)=1 AND {identity} AND v.kind=?1 AND (?2 IS NULL OR \
v.source_id=?2) AND (?3 IS NULL OR (v.source_id=?3 AND EXISTS(SELECT 1 FROM json_each(v.group_ids) \
x WHERE x.value=?4))) AND instr(lower(v.name),lower(?5))>0 AND (?6=0 OR (",
                // The same rule as the Continue watching shelf.
                unfinished!("w", "5000"),
                ")) ORDER BY {order},v.source_id,v.id LIMIT ?7 OFFSET ?8"
            ),
            identity = identity,
            order = order
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
                offset,
                exact.map(|key| key.id.as_str())
            ],
            raw_card,
        )?;
        cards_page(rows, limit)
    }
    /// One bounded page of a cross-source discovery shelf, mixing movies
    /// and series of every enabled source in a total order (see
    /// [`iptv_core::vod`]). `limit` is clamped to
    /// [`iptv_core::vod::DISCOVERY_PAGE_MAX`].
    ///
    /// Continue watching is driven by the progress records (bounded by what
    /// was watched, not by the catalogue): each record looks up its own
    /// title by primary key. Recently added sorts the enabled catalogues by
    /// the provider's added/last-modified time; it never uses release year.
    pub fn vod_discovery(&self, query: DiscoveryQuery) -> Result<DiscoveryPage> {
        let limit = query.bounded_limit();
        let (cards, more) = match query.shelf {
            Shelf::ContinueWatching => {
                // The threshold is shared with `Progress::unfinished`.
                let min = i64::try_from(iptv_core::vod::CONTINUE_MIN_MS).unwrap_or(i64::MAX);
                let mut s = self.conn.prepare_cached(CONTINUE_WATCHING_SQL)?;
                let rows = s.query_map(params![limit + 1, query.offset, min], raw_card)?;
                cards_page(rows, limit)?
            }
            Shelf::RecentlyAdded => {
                let mut s = self.conn.prepare_cached(RECENTLY_ADDED_SQL)?;
                let rows = s.query_map(params![limit + 1, query.offset], raw_card)?;
                cards_page(rows, limit)?
            }
        };
        Ok(DiscoveryPage {
            cards,
            more,
            start: query.offset,
        })
    }
    /// The bounded page of `shelf` holding `key` at its current place
    /// ([`DiscoveryQuery::around`]), however deep it is, or the first page
    /// when the shelf no longer lists it. The place and the page are read
    /// in one read transaction, so a write between them cannot move the
    /// title out of the page that was computed for it. A reopened shelf
    /// focuses its title again by identity this way; the page's `start`
    /// tells where Load more and Load previous continue.
    pub fn vod_discovery_around(&self, shelf: Shelf, key: &Key) -> Result<DiscoveryPage> {
        self.vod_read_snapshot(|store| {
            let rank = store.vod_discovery_rank(shelf, key)?;
            store.vod_discovery(DiscoveryQuery::around(shelf, rank))
        })
    }
    /// Read source authority and catalogue data from the same committed
    /// snapshot. A configuration edit cannot pair replacement credentials
    /// with an older incarnation checked by the caller. Errors roll back
    /// through the transaction guard without retaining the read snapshot.
    pub fn vod_read_snapshot<T>(&self, read: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        self.read_snapshot(read)
    }
    /// [`Store::vod_discovery_around`] that `cancel` stops
    /// ([`Store::vod_cancelable`]).
    pub fn vod_discovery_around_cancelable(
        &mut self,
        shelf: Shelf,
        key: &Key,
        cancel: &iptv_import::CancelToken,
    ) -> Result<DiscoveryPage> {
        self.vod_cancelable(cancel, |store| store.vod_discovery_around(shelf, key))
    }
    /// How many titles precede `key` on `shelf` in its current order (see
    /// [`Store::vod_discovery`]); `None` when the shelf does not list it (a
    /// finished, deleted or disabled title).
    pub fn vod_discovery_rank(&self, shelf: Shelf, key: &Key) -> Result<Option<u32>> {
        let rank: Option<i64> = match shelf {
            Shelf::ContinueWatching => {
                let min = i64::try_from(iptv_core::vod::CONTINUE_MIN_MS).unwrap_or(i64::MAX);
                self.conn
                    .prepare_cached(CONTINUE_WATCHING_RANK_SQL)?
                    .query_row(params![min, key.source, key.kind.name(), key.id], |r| {
                        r.get(0)
                    })
                    .optional()?
            }
            Shelf::RecentlyAdded => self
                .conn
                .prepare_cached(RECENTLY_ADDED_RANK_SQL)?
                .query_row(params![key.source, key.kind.name(), key.id], |r| r.get(0))
                .optional()?,
        };
        Ok(rank.map(|rank| u32::try_from(rank.max(0)).unwrap_or(u32::MAX)))
    }
    /// Runs `read` so that cancelling `cancel` (the shelf closed, a newer
    /// page was asked for, the application closed) stops its SQLite work
    /// within a bounded number of steps instead of finishing a long sort.
    pub fn vod_cancelable<T>(
        &mut self,
        cancel: &iptv_import::CancelToken,
        read: impl FnOnce(&Self) -> Result<T>,
    ) -> Result<T> {
        let canceled = || StoreError::Unavailable("On-demand read canceled");
        if cancel.is_canceled() {
            return Err(canceled());
        }
        let flag = cancel.clone();
        self.conn
            .progress_handler(1000, Some(move || flag.is_canceled()))?;
        let result = read(self);
        self.conn.progress_handler(0, None::<fn() -> bool>)?;
        if cancel.is_canceled() {
            return Err(canceled());
        }
        result
    }
    /// [`Store::vod_discovery`] that `cancel` stops ([`Store::vod_cancelable`]).
    pub fn vod_discovery_cancelable(
        &mut self,
        query: DiscoveryQuery,
        cancel: &iptv_import::CancelToken,
    ) -> Result<DiscoveryPage> {
        self.vod_cancelable(cancel, |store| store.vod_discovery(query))
    }
    /// Returns only current, enabled source data; locator never crosses the UI boundary.
    pub fn vod_episode_numbers(&self, key: &Key) -> Result<Vec<u32>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT episode_numbers FROM vod WHERE source_id=?1 AND id=?2",
                params![key.source, key.id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default())
    }
    pub fn vod_locator(&self, key: &Key) -> Result<Option<(Option<String>, StreamLocator)>> {
        let mut s=self.conn.prepare_cached("SELECT v.provider_id,v.stream FROM vod v LEFT JOIN source_policy p ON p.source_id=v.source_id WHERE \
v.source_id=?1 AND v.id=?2 AND COALESCE(p.enabled,1)=1 AND ((?3='series' AND v.is_series=1) OR \
(?3='movie' AND v.is_series=0))")?;
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
        let query = Query {
            kind: key.kind,
            source: Some(key.source),
            ..Default::default()
        };
        let (cards, _) = self.vod_page_inner(&query, 0, 1, Some(key))?;
        Ok(cards.into_iter().next())
    }
    pub fn vod_progress(&self, key: &Key) -> Result<Progress> {
        self.conn.query_row("SELECT position_ms,duration_ms,completed FROM vod_progress WHERE source_id=?1 AND kind=?2 AND content_id=?3",
params![key.source,
key.kind.name(),
key.id],
|r|Ok(Progress{
position_ms:r.get::<_,
i64>(0)?.max(0) as u64,
duration_ms:r.get::<_,
Option<i64>>(1)?.map(|v|v.max(0) as u64),
completed:r.get(2)?}
)).optional().map(|v|v.unwrap_or_default()).map_err(Into::into)
    }
    pub fn vod_resume_episode(&self, key: &Key) -> Result<Option<Key>> {
        if key.kind != Kind::Series {
            return Ok(None);
        }
        let id: Option<Option<String>> = self.conn.query_row(
            "SELECT last_episode_id FROM vod_progress WHERE source_id=?1 AND kind='series' AND content_id=?2",
            params![key.source, key.id],
            |row| row.get(0),
        ).optional()?;
        Ok(id.flatten().map(|id| Key {
            source: key.source,
            kind: Kind::Episode,
            id,
        }))
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
        if (key.kind == Kind::Episode) != parent.is_some() {
            return Err(StoreError::Unavailable("Invalid on-demand parent"));
        }
        let cap = |v: u64| v.min(i64::MAX as u64) as i64;
        let tx = self.conn.transaction()?;
        tx.execute("INSERT INTO \
vod_progress(source_id,kind,content_id,parent_id,position_ms,duration_ms,completed,updated_at) \
VALUES(?1,?2,?3,?4,?5,?6,?7,unixepoch()) ON CONFLICT(source_id,kind,content_id) DO UPDATE SET \
parent_id=excluded.parent_id,position_ms=excluded.position_ms,duration_ms=excluded.duration_ms,completed=excluded.completed,updated_at=excluded.updated_at",
params![key.source,
key.kind.name(),
key.id,
parent,
cap(progress.position_ms),
progress.duration_ms.map(cap),
progress.completed])?;
        if let Some(parent) = parent {
            // Only the episode list can decide that a whole series is complete.
            // Later completed tail ticks preserve that decision; a replay or a
            // different episode starts watching the series again.
            tx.execute("INSERT INTO vod_progress(source_id,kind,content_id,position_ms,duration_ms,completed,updated_at,last_episode_id) \
VALUES(?1,'series',?2,?3,?4,0,unixepoch(),?5) ON CONFLICT(source_id,kind,content_id) DO UPDATE SET \
position_ms=excluded.position_ms,duration_ms=excluded.duration_ms,\
completed=CASE WHEN vod_progress.last_episode_id=excluded.last_episode_id AND ?6 \
THEN vod_progress.completed ELSE 0 END,updated_at=excluded.updated_at,last_episode_id=excluded.last_episode_id",
params![key.source,
parent,
cap(progress.position_ms),
progress.duration_ms.map(cap), key.id, progress.completed])?;
        }
        tx.commit()?;
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
        model::{ContentKind, M3uSourceConfig, Series, Source, SourceConfig, VodItem},
    };
    #[test]
    fn series_resume_identity_keeps_write_order_and_is_removed_with_its_source() {
        let (mut store, movie) = fixture();
        let series = Key {
            source: movie.source,
            kind: Kind::Series,
            id: "generated-series".into(),
        };
        let first = Key {
            source: movie.source,
            kind: Kind::Episode,
            id: "episode-one".into(),
        };
        let fifth = Key {
            source: movie.source,
            kind: Kind::Episode,
            id: "episode-five".into(),
        };
        let point = Progress::observed(37_000, Some(120_000), false);
        for key in [&fifth, &first, &fifth] {
            store
                .save_vod_progress(key, Some(&series.id), point)
                .unwrap();
            assert_eq!(
                store.vod_resume_episode(&series).unwrap().as_ref(),
                Some(key)
            );
        }
        store
            .save_vod_progress(
                &series,
                None,
                Progress {
                    completed: true,
                    ..point
                },
            )
            .unwrap();
        assert_eq!(store.vod_resume_episode(&series).unwrap(), Some(fifth));
        let source = store
            .source(crate::SourceId(movie.source))
            .unwrap()
            .unwrap()
            .source;
        store.delete_source(crate::SourceId(movie.source)).unwrap();
        let replacement = store.add_source(&source).unwrap();
        assert_eq!(replacement.0, movie.source);
        assert_eq!(store.vod_resume_episode(&series).unwrap(), None);
    }
    #[test]
    fn completed_series_stays_complete_through_tail_ticks_and_departure() {
        let (mut store, movie) = fixture();
        let series = Key {
            kind: Kind::Series,
            id: "generated-series".into(),
            ..movie.clone()
        };
        let tail = Key {
            kind: Kind::Episode,
            id: "generated-tail".into(),
            ..movie.clone()
        };
        let completed = Progress::observed(114_000, Some(120_000), false);
        assert!(completed.completed, "completion can precede native EOF");
        store
            .save_vod_progress(&tail, Some(&series.id), completed)
            .unwrap();
        store
            .save_vod_progress(
                &series,
                None,
                Progress {
                    completed: true,
                    ..Default::default()
                },
            )
            .unwrap();
        // A later five-second tick and the outgoing-source flush must preserve
        // the series decision made when its final episode first completed.
        for ms in [119_000, 119_000, 120_000] {
            let point = Progress::observed(ms, Some(120_000), false);
            store
                .save_vod_progress(&tail, Some(&series.id), point)
                .unwrap();
            assert!(store.vod_progress(&series).unwrap().completed);
            assert_eq!(
                store.vod_resume_episode(&series).unwrap(),
                Some(tail.clone())
            );
        }
        // Deliberately replaying part of that episode starts watching again.
        store
            .save_vod_progress(
                &tail,
                Some(&series.id),
                Progress::observed(37_000, Some(120_000), false),
            )
            .unwrap();
        assert!(!store.vod_progress(&series).unwrap().completed);
        store
            .save_vod_progress(
                &series,
                None,
                Progress {
                    completed: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let earlier = Key {
            id: "generated-earlier".into(),
            ..tail
        };
        store
            .save_vod_progress(&earlier, Some(&series.id), completed)
            .unwrap();
        assert!(
            !store.vod_progress(&series).unwrap().completed,
            "a different episode replaces the tail's completion"
        );
    }
    #[test]
    fn exact_lookup_has_a_fixed_work_budget_among_many_shared_names() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let (mut store, key) = fixture();
        let source = crate::SourceId(key.source);
        let template = store.load_vod(source, None, 0, Some(1)).unwrap().remove(0);
        let movies: Vec<_> = (0..5000)
            .map(|index| {
                let mut item = template.clone();
                item.id = iptv_providers::ContentId::from_stored(format!("generated-{index:05}"));
                item.name = "Generated shared title".into();
                item.groups.clear();
                item
            })
            .collect();
        store
            .replace_catalog(source, &[], &[], &movies, &[])
            .unwrap();
        let key = Key {
            source: source.0,
            kind: Kind::Movie,
            id: movies.last().unwrap().id.to_string(),
        };
        let budget = Arc::new(AtomicUsize::new(0));
        let steps = budget.clone();
        store
            .conn
            .progress_handler(
                1000,
                Some(move || steps.fetch_add(1, Ordering::Relaxed) > 20),
            )
            .unwrap();
        let result = store.vod_card(&key);
        store
            .conn
            .progress_handler(0, None::<fn() -> bool>)
            .unwrap();
        assert_eq!(result.unwrap().unwrap().key, key);
        assert!(
            budget.load(Ordering::Relaxed) <= 20,
            "identity lookup scanned the catalog"
        );
    }
    fn fixture() -> (Store, Key) {
        let mut store = Store::open_in_memory().unwrap();
        let source = store
            .add_source(&Source {
                name: "Generated catalog".into(),
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
        let movies = iptv_providers::xtream::parse_vod_streams(&HttpResponse::ok(
            r#"[
  {
    "stream_id": 10,
    "name": "Generated Aurora",
    "category_id": "7",
    "container_extension": "mp4"
  },
  {
    "stream_id": 11,
    "name": "Generated Brook",
    "category_id": "7",
    "container_extension": "mp4"
  }
]"#,
        ))
        .unwrap();
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
    fn generated_movie(id: &str, name: &str, added: Option<i64>, year: Option<u16>) -> VodItem {
        VodItem {
            id: iptv_providers::ContentId::from_stored(id),
            provider_id: Some(id.into()),
            name: name.into(),
            groups: vec![],
            poster: Some(format!("https://example.test/poster/{id}.png")),
            plot: Some("Generated plot".into()),
            year,
            rating_x10: None,
            added,
            duration_secs: Some(120),
            stream: StreamLocator::Url {
                url: format!("https://example.test/media/{id}.mp4"),
                headers: vec![],
                properties: vec![],
            },
            is_series: false,
            episode_numbers: vec![],
        }
    }
    fn generated_series(id: &str, name: &str, last_modified: Option<i64>) -> Series {
        Series {
            id: iptv_providers::ContentId::from_stored(id),
            provider_id: Some(id.into()),
            name: name.into(),
            groups: vec![],
            poster: None,
            plot: None,
            year: Some(2099),
            rating_x10: None,
            last_modified,
        }
    }
    /// Two playlists with identical provider ids and titles. A movie and a
    /// series share an id and a title too, and a release year in the future
    /// must not make an undated title "recent".
    fn generated_sources() -> (Store, [crate::SourceId; 2], Vec<VodItem>, Vec<Series>) {
        let mut store = Store::open_in_memory().unwrap();
        let movies = vec![
            generated_movie(
                "generated-aurora",
                "Generated Aurora",
                Some(1_700_000_100),
                None,
            ),
            generated_movie("generated-brook", "Generated Brook", None, Some(2099)),
            generated_movie("generated-brook-lower", "generated brook", None, None),
            generated_movie("generated-shared", "Generated Shared", Some(0), None),
        ];
        let series = vec![
            generated_series("generated-voyage", "Generated Voyage", Some(1_700_000_200)),
            generated_series("generated-shared", "Generated Shared", None),
        ];
        let sources = ["alpha", "beta"].map(|name| {
            let source = store
                .add_source(&Source {
                    name: format!("Generated {name}"),
                    config: SourceConfig::M3u(M3uSourceConfig {
                        location: SecretUrl::new(format!("https://example.test/{name}.m3u")),
                        user_agent: None,
                        referrer: None,
                    }),
                })
                .unwrap();
            store
                .replace_catalog(source, &[], &[], &movies, &series)
                .unwrap();
            source
        });
        (store, sources, movies, series)
    }
    fn shelf(store: &Store, shelf: Shelf, offset: u32, limit: u32) -> DiscoveryPage {
        store
            .vod_discovery(DiscoveryQuery {
                shelf,
                offset,
                limit,
            })
            .unwrap()
    }
    fn every_page(store: &Store, which: Shelf, limit: u32) -> Vec<Key> {
        let mut out = Vec::new();
        loop {
            let page = shelf(store, which, out.len() as u32, limit);
            assert!(page.cards.len() <= limit as usize);
            out.extend(page.cards.into_iter().map(|c| c.key));
            if !page.more {
                return out;
            }
        }
    }
    fn key(source: crate::SourceId, kind: Kind, id: &str) -> Key {
        Key {
            source: source.0,
            kind,
            id: id.into(),
        }
    }
    fn touched(store: &Store, key: &Key, at: i64) {
        store
            .conn
            .execute(
                "UPDATE vod_progress SET updated_at=?4 WHERE source_id=?1 AND kind=?2 AND content_id=?3",
                params![key.source, key.kind.name(), key.id, at],
            )
            .unwrap();
    }
    #[test]
    fn recently_added_is_newest_provider_time_first_then_a_stable_name_order() {
        let (store, [a, b], _, _) = generated_sources();
        let all = every_page(&store, Shelf::RecentlyAdded, 100);
        assert_eq!(
            all,
            vec![
                key(a, Kind::Series, "generated-voyage"),
                key(b, Kind::Series, "generated-voyage"),
                key(a, Kind::Movie, "generated-aurora"),
                key(b, Kind::Movie, "generated-aurora"),
                // Undated (absent or zero): by name, then identity. The
                // future release year of "Generated Brook" is not a date.
                key(a, Kind::Movie, "generated-brook"),
                key(a, Kind::Movie, "generated-brook-lower"),
                key(b, Kind::Movie, "generated-brook"),
                key(b, Kind::Movie, "generated-brook-lower"),
                key(a, Kind::Movie, "generated-shared"),
                key(a, Kind::Series, "generated-shared"),
                key(b, Kind::Movie, "generated-shared"),
                key(b, Kind::Series, "generated-shared"),
            ]
        );
        for limit in [1, 3, 5] {
            assert_eq!(
                every_page(&store, Shelf::RecentlyAdded, limit),
                all,
                "pages of {limit} neither repeat nor skip"
            );
        }
        let page = shelf(&store, Shelf::RecentlyAdded, 0, 1);
        assert!(page.more);
        assert_eq!(page.cards[0].source_name, "Generated alpha");
        assert_eq!(
            shelf(&store, Shelf::RecentlyAdded, 0, u32::MAX).cards.len(),
            all.len(),
            "an oversized page is clamped, not refused"
        );
    }
    #[test]
    fn a_series_in_both_catalogs_is_listed_once() {
        let (mut store, [a, _], mut movies, series) = generated_sources();
        let mut embedded = generated_movie("generated-voyage", "Generated Voyage", Some(1), None);
        embedded.is_series = true;
        movies.push(embedded);
        store
            .replace_catalog(a, &[], &[], &movies, &series)
            .unwrap();
        let all = every_page(&store, Shelf::RecentlyAdded, 100);
        let voyage = key(a, Kind::Series, "generated-voyage");
        assert_eq!(all.iter().filter(|k| **k == voyage).count(), 1);
        store
            .save_vod_progress(
                &key(a, Kind::Episode, "generated-voyage:1"),
                Some("generated-voyage"),
                Progress::observed(30_000, Some(120_000), false),
            )
            .unwrap();
        assert_eq!(
            every_page(&store, Shelf::ContinueWatching, 100),
            vec![voyage]
        );
    }
    #[test]
    fn on_demand_kinds_follow_the_catalog_and_errors_clear_on_success() {
        let (mut store, [a, b], _, _) = generated_sources();
        let mut embedded = generated_movie("generated-voyage", "Generated Voyage", Some(1), None);
        embedded.is_series = true;
        store
            .replace_catalog(a, &[], &[], &[embedded], &[])
            .unwrap();
        assert_eq!(
            store.on_demand_kinds(a).unwrap(),
            (false, true),
            "an embedded series is a series, not a movie"
        );
        assert!(store.on_demand_kinds(b).unwrap().0);
        use iptv_library::ImportErrorKind;
        store
            .record_on_demand_errors(a, Some(ImportErrorKind::HttpStatus), None)
            .unwrap();
        let status = store.import_status(a).unwrap();
        assert_eq!(status.movies_error, Some(ImportErrorKind::HttpStatus));
        assert_eq!(status.series_error, None);
        store.replace_catalog(a, &[], &[], &[], &[]).unwrap();
        assert_eq!(store.import_status(a).unwrap().movies_error, None);
    }
    #[test]
    fn continue_watching_orders_by_progress_and_keeps_sources_apart() {
        let (mut store, [a, b], _, _) = generated_sources();
        assert!(every_page(&store, Shelf::ContinueWatching, 10).is_empty());
        let started = Progress::observed(30_000, Some(120_000), false);
        let movie_a = key(a, Kind::Movie, "generated-aurora");
        let movie_b = key(b, Kind::Movie, "generated-aurora");
        let shared_movie = key(a, Kind::Movie, "generated-shared");
        let barely = key(b, Kind::Movie, "generated-brook");
        let finished = key(a, Kind::Movie, "generated-brook");
        for k in [&movie_a, &movie_b, &shared_movie] {
            store.save_vod_progress(k, None, started).unwrap();
        }
        store
            .save_vod_progress(
                &barely,
                None,
                Progress::observed(4_000, Some(120_000), false),
            )
            .unwrap();
        store
            .save_vod_progress(
                &finished,
                None,
                Progress::observed(119_000, Some(120_000), false),
            )
            .unwrap();
        // The same episode identity on two playlists: two series records.
        let series_a = key(a, Kind::Series, "generated-shared");
        let series_b = key(b, Kind::Series, "generated-shared");
        for parent in [&series_a, &series_b] {
            for (episode, ms) in [
                ("generated-shared:1", 50_000),
                ("generated-shared:2", 9_000),
            ] {
                store
                    .save_vod_progress(
                        &Key {
                            kind: Kind::Episode,
                            id: episode.into(),
                            ..parent.clone()
                        },
                        Some(&parent.id),
                        Progress::observed(ms, Some(120_000), false),
                    )
                    .unwrap();
            }
        }
        touched(&store, &movie_a, 100);
        touched(&store, &movie_b, 300);
        touched(&store, &shared_movie, 200);
        touched(&store, &series_a, 200);
        touched(&store, &series_b, 50);
        let all = every_page(&store, Shelf::ContinueWatching, 100);
        assert_eq!(
            all,
            vec![
                movie_b.clone(),
                shared_movie.clone(),
                series_a.clone(),
                movie_a.clone(),
                series_b.clone(),
            ],
            "newest progress first; ties by source, kind and id; one row per series"
        );
        assert_eq!(every_page(&store, Shelf::ContinueWatching, 2), all);
        let page = shelf(&store, Shelf::ContinueWatching, 0, 100);
        let card = page.cards.iter().find(|c| c.key == series_a).unwrap();
        assert_eq!(card.progress.resume_ms(), 9_000, "the episode watched last");
        assert_eq!(card.source_name, "Generated alpha");
        // A disabled playlist leaves; enabling it again restores its titles.
        store
            .set_source_policy(
                b,
                crate::SourcePolicy {
                    enabled: false,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            every_page(&store, Shelf::ContinueWatching, 100)
                .iter()
                .chain(&every_page(&store, Shelf::RecentlyAdded, 100))
                .all(|k| k.source == a.0)
        );
        store
            .set_source_policy(b, crate::SourcePolicy::default())
            .unwrap();
        assert_eq!(every_page(&store, Shelf::ContinueWatching, 100), all);
    }
    #[test]
    fn a_completed_series_stays_out_whatever_older_episodes_hold() {
        let (mut store, [a, _], _, _) = generated_sources();
        let series = key(a, Kind::Series, "generated-voyage");
        let episode = |id: &str| key(a, Kind::Episode, id);
        // Episode one is left half-watched; episode two is watched last and
        // finished, and it is the final episode, so the series completes.
        store
            .save_vod_progress(
                &episode("generated-voyage:1"),
                Some(&series.id),
                Progress::observed(60_000, Some(120_000), false),
            )
            .unwrap();
        assert_eq!(
            every_page(&store, Shelf::ContinueWatching, 10),
            vec![series.clone()]
        );
        let tail = episode("generated-voyage:2");
        store
            .save_vod_progress(
                &tail,
                Some(&series.id),
                Progress::observed(120_000, Some(120_000), true),
            )
            .unwrap();
        store
            .save_vod_progress(
                &series,
                None,
                Progress {
                    completed: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            every_page(&store, Shelf::ContinueWatching, 10).is_empty(),
            "a completed tail does not revive the unfinished first episode"
        );
        // A later tick of the same tail keeps the series complete.
        store
            .save_vod_progress(
                &tail,
                Some(&series.id),
                Progress::observed(120_000, Some(120_000), false),
            )
            .unwrap();
        assert!(every_page(&store, Shelf::ContinueWatching, 10).is_empty());
        // Watching episode one again is a new decision: the series returns.
        store
            .save_vod_progress(
                &episode("generated-voyage:1"),
                Some(&series.id),
                Progress::observed(61_000, Some(120_000), false),
            )
            .unwrap();
        assert_eq!(
            every_page(&store, Shelf::ContinueWatching, 10),
            vec![series]
        );
    }
    #[test]
    fn a_series_advanced_to_its_next_episode_stays_until_the_series_completes() {
        let (mut store, [a, b], _, _) = generated_sources();
        let voyage = key(a, Kind::Series, "generated-voyage");
        let episode = |id: &str| key(a, Kind::Episode, id);
        let continuing = |store: &Store| every_page(store, Shelf::ContinueWatching, 10);
        // Episode one finished: the series waits on its next episode, which
        // starts from zero, so its row offers no resume point.
        store
            .save_vod_progress(
                &episode("generated-voyage:1"),
                Some(&voyage.id),
                Progress::observed(120_000, Some(120_000), true),
            )
            .unwrap();
        assert_eq!(continuing(&store), vec![voyage.clone()]);
        let row = shelf(&store, Shelf::ContinueWatching, 0, 10)
            .cards
            .remove(0);
        assert_eq!(
            row.progress.resume_ms(),
            0,
            "the next episode starts at zero, not at the finished one's end"
        );
        // Autoplay starts episode two and the viewer stops after two seconds.
        store
            .save_vod_progress(
                &episode("generated-voyage:2"),
                Some(&voyage.id),
                Progress::observed(2_000, Some(120_000), false),
            )
            .unwrap();
        assert_eq!(
            continuing(&store),
            vec![voyage.clone()],
            "a series being watched stays after a short start of its next episode"
        );
        assert_eq!(
            store.vod_resume_episode(&voyage).unwrap(),
            Some(episode("generated-voyage:2")),
            "it resumes the episode watched last"
        );
        let row = shelf(&store, Shelf::ContinueWatching, 0, 10)
            .cards
            .remove(0);
        assert_eq!(row.progress.resume_ms(), 0);
        // The Shows catalogue's Continue watching filter agrees.
        let shows = Query {
            kind: Kind::Series,
            continue_watching: true,
            ..Default::default()
        };
        assert_eq!(
            store.vod_page(&shows, 0, 20).unwrap().0[0].key,
            voyage.clone()
        );
        // A series only glanced at (two seconds of its first episode, on
        // the other playlist) stays out, as a movie would.
        let glanced = key(b, Kind::Series, "generated-voyage");
        store
            .save_vod_progress(
                &key(b, Kind::Episode, "generated-voyage:1"),
                Some(&glanced.id),
                Progress::observed(2_000, Some(120_000), false),
            )
            .unwrap();
        assert_eq!(continuing(&store), vec![voyage.clone()]);
        assert_eq!(store.vod_page(&shows, 0, 20).unwrap().0.len(), 1);
        // Completing the whole series removes it, whatever its episodes hold.
        store
            .save_vod_progress(
                &voyage,
                None,
                Progress {
                    completed: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(continuing(&store).is_empty());
        assert!(store.vod_page(&shows, 0, 20).unwrap().0.is_empty());
    }
    #[test]
    fn a_title_rank_matches_its_place_in_every_page() {
        let (mut store, [a, b], _, _) = generated_sources();
        let started = Progress::observed(30_000, Some(120_000), false);
        for (k, at) in [
            (key(a, Kind::Movie, "generated-aurora"), 100),
            (key(b, Kind::Movie, "generated-aurora"), 100),
            (key(a, Kind::Movie, "generated-shared"), 300),
            (key(b, Kind::Movie, "generated-brook"), 200),
        ] {
            store.save_vod_progress(&k, None, started).unwrap();
            touched(&store, &k, at);
        }
        for which in [Shelf::ContinueWatching, Shelf::RecentlyAdded] {
            let all = every_page(&store, which, 100);
            assert!(!all.is_empty());
            for (at, k) in all.iter().enumerate() {
                assert_eq!(
                    store.vod_discovery_rank(which, k).unwrap(),
                    Some(at as u32),
                    "{which:?} {k:?}"
                );
            }
        }
        // Not listed: an unwatched title, a title of a disabled playlist.
        let unwatched = key(a, Kind::Series, "generated-voyage");
        assert_eq!(
            store
                .vod_discovery_rank(Shelf::ContinueWatching, &unwatched)
                .unwrap(),
            None
        );
        let missing = key(a, Kind::Movie, "generated-missing");
        assert_eq!(
            store
                .vod_discovery_rank(Shelf::RecentlyAdded, &missing)
                .unwrap(),
            None
        );
        store
            .set_source_policy(
                b,
                crate::SourcePolicy {
                    enabled: false,
                    ..Default::default()
                },
            )
            .unwrap();
        for which in [Shelf::ContinueWatching, Shelf::RecentlyAdded] {
            assert_eq!(
                store
                    .vod_discovery_rank(which, &key(b, Kind::Movie, "generated-aurora"))
                    .unwrap(),
                None,
                "{which:?}"
            );
        }
    }
    /// The window around a deep title is one bounded page holding it, with
    /// its start, wherever the title now is; the pages before and after
    /// continue from that start.
    #[test]
    fn a_deep_title_is_found_in_one_bounded_window_after_reorder_or_departure() {
        use iptv_core::vod::DISCOVERY_PAGE;
        let (mut store, [a, b]) = large_library(1500);
        // Recently added: an old dated movie on the second playlist lies
        // far below the first 2,000 titles: no depth limits its window.
        let deep = key(b, Kind::Movie, "generated-movie-000004");
        let rank = store
            .vod_discovery_rank(Shelf::RecentlyAdded, &deep)
            .unwrap()
            .unwrap();
        assert!(rank > 2_000, "rank {rank}");
        let window = store
            .vod_discovery_around(Shelf::RecentlyAdded, &deep)
            .unwrap();
        assert_eq!(window.start, rank - rank % DISCOVERY_PAGE);
        assert!(window.cards.len() <= DISCOVERY_PAGE as usize);
        assert_eq!(
            window.cards[(rank - window.start) as usize].key,
            deep,
            "the title is in its window, at its place"
        );
        let next = store
            .vod_discovery(DiscoveryQuery::after(
                Shelf::RecentlyAdded,
                (window.start + window.cards.len() as u32) as usize,
            ))
            .unwrap();
        let previous = store
            .vod_discovery(DiscoveryQuery::before(Shelf::RecentlyAdded, window.start).unwrap())
            .unwrap();
        assert_eq!(previous.start + previous.cards.len() as u32, window.start);
        for k in previous.cards.iter().chain(&next.cards).map(|c| &c.key) {
            assert!(
                window.cards.iter().all(|c| &c.key != k),
                "pages do not overlap"
            );
        }
        // Continue watching: 2,100 movies watched across both playlists;
        // the oldest progress is the target, 2,100th.
        let started = Progress::observed(30_000, Some(120_000), false);
        for i in 0..1_050_i64 {
            for (source, later) in [(a, 0), (b, 1)] {
                let k = key(source, Kind::Movie, &format!("generated-movie-{i:06}"));
                store.save_vod_progress(&k, None, started).unwrap();
                touched(&store, &k, 1_000 + 2 * i + later);
            }
        }
        let oldest = key(a, Kind::Movie, "generated-movie-000000");
        assert_eq!(
            store
                .vod_discovery_rank(Shelf::ContinueWatching, &oldest)
                .unwrap(),
            Some(2_099)
        );
        let window = store
            .vod_discovery_around(Shelf::ContinueWatching, &oldest)
            .unwrap();
        assert_eq!(window.start, 2_080);
        assert_eq!(window.cards[19].key, oldest);
        assert!(!window.more);
        // Watched again meanwhile, as recently as the first playlist's
        // 501st: 1,099 titles are newer now, and its window moves with it.
        touched(&store, &oldest, 2_000);
        let rank = store
            .vod_discovery_rank(Shelf::ContinueWatching, &oldest)
            .unwrap()
            .unwrap();
        assert_eq!(rank, 1_099);
        let window = store
            .vod_discovery_around(Shelf::ContinueWatching, &oldest)
            .unwrap();
        assert_eq!(window.start, rank - rank % DISCOVERY_PAGE);
        assert!(window.cards.iter().any(|c| c.key == oldest));
        // Finished meanwhile: the shelf no longer lists it, and the window
        // falls back to the first page.
        store
            .save_vod_progress(
                &oldest,
                None,
                Progress::observed(120_000, Some(120_000), true),
            )
            .unwrap();
        let window = store
            .vod_discovery_around(Shelf::ContinueWatching, &oldest)
            .unwrap();
        assert_eq!(window.start, 0);
        assert_eq!(window.cards.len(), DISCOVERY_PAGE as usize);
        assert!(window.cards.iter().all(|c| c.key != oldest));
        // A cancelled window read on either shelf stops without a page.
        let cancel = iptv_import::CancelToken::new();
        cancel.cancel();
        for which in [Shelf::ContinueWatching, Shelf::RecentlyAdded] {
            assert!(
                store
                    .vod_discovery_around_cancelable(which, &deep, &cancel)
                    .is_err(),
                "{which:?}"
            );
        }
        let live = iptv_import::CancelToken::new();
        assert_eq!(
            store
                .vod_discovery_around_cancelable(Shelf::RecentlyAdded, &deep, &live)
                .unwrap()
                .start,
            store
                .vod_discovery_rank(Shelf::RecentlyAdded, &deep)
                .unwrap()
                .map(|r| r - r % DISCOVERY_PAGE)
                .unwrap()
        );
    }
    #[test]
    fn departed_titles_and_recreated_sources_leave_no_stale_progress() {
        let (mut store, [a, b], movies, series) = generated_sources();
        let started = Progress::observed(30_000, Some(120_000), false);
        let aurora_a = key(a, Kind::Movie, "generated-aurora");
        let aurora_b = key(b, Kind::Movie, "generated-aurora");
        store.save_vod_progress(&aurora_a, None, started).unwrap();
        store.save_vod_progress(&aurora_b, None, started).unwrap();
        // A refresh without the title hides its progress; the title coming
        // back with the next refresh brings the progress back too.
        store
            .replace_catalog(a, &[], &[], &movies[1..], &series)
            .unwrap();
        assert_eq!(
            every_page(&store, Shelf::ContinueWatching, 10),
            vec![aurora_b.clone()]
        );
        store
            .replace_catalog(a, &[], &[], &movies, &series)
            .unwrap();
        assert_eq!(every_page(&store, Shelf::ContinueWatching, 10).len(), 2);
        // Deleting a playlist removes its progress; the same id recreated
        // with the same catalogue starts clean.
        let source = store.source(b).unwrap().unwrap().source;
        store.delete_source(b).unwrap();
        assert_eq!(
            every_page(&store, Shelf::ContinueWatching, 10),
            vec![aurora_a.clone()]
        );
        let recreated = store.add_source(&source).unwrap();
        assert_eq!(recreated, b);
        store
            .replace_catalog(recreated, &[], &[], &movies, &series)
            .unwrap();
        assert_eq!(
            every_page(&store, Shelf::ContinueWatching, 10),
            vec![aurora_a]
        );
        assert_eq!(store.vod_progress(&aurora_b).unwrap(), Progress::default());
        assert!(
            every_page(&store, Shelf::RecentlyAdded, 100).contains(&aurora_b),
            "the recreated playlist's titles are discoverable again"
        );
    }
    fn plan(store: &Store, sql: &str, args: impl rusqlite::Params) -> Vec<String> {
        let mut s = store
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        s.query_map(args, |r| r.get::<_, String>(3))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }
    /// The plan lines that read every row of one of `tables` (by name or
    /// alias), in either of SQLite's wordings.
    fn scans(plan: &[String], tables: &[&str]) -> Vec<String> {
        plan.iter()
            .filter(|detail| {
                let mut words = detail.split_whitespace();
                if words.next() != Some("SCAN") {
                    return false;
                }
                let name = match words.next() {
                    Some("TABLE") => words.next(),
                    other => other,
                };
                name.is_some_and(|name| tables.contains(&name))
            })
            .cloned()
            .collect()
    }
    /// A library large enough that reading it whole shows in the plan and in
    /// the work budget: 5000 movies and 5000 series on each of two sources.
    fn large_library(per_source: usize) -> (Store, [crate::SourceId; 2]) {
        let (mut store, sources, _, _) = generated_sources();
        let movies: Vec<_> = (0..per_source)
            .map(|i| {
                generated_movie(
                    &format!("generated-movie-{i:06}"),
                    "Generated shared title",
                    (i % 3 != 0).then_some(1_700_000_000 + i as i64),
                    Some(2000),
                )
            })
            .collect();
        let series: Vec<_> = (0..per_source)
            .map(|i| {
                generated_series(
                    &format!("generated-series-{i:06}"),
                    "Generated shared title",
                    (i % 2 == 0).then_some(1_700_000_000 + i as i64),
                )
            })
            .collect();
        for source in sources {
            store
                .replace_catalog(source, &[], &[], &movies, &series)
                .unwrap();
        }
        (store, sources)
    }
    #[test]
    fn continue_watching_reads_titles_by_key_not_by_scanning_catalogs() {
        let (store, _) = large_library(200);
        let lines = plan(&store, CONTINUE_WATCHING_SQL, params![41, 0, 5_000]);
        assert!(
            scans(&lines, &["v", "z", "vod", "series"]).is_empty(),
            "catalog scan in {lines:#?}"
        );
        assert!(
            lines.iter().any(|line| {
                // SQLite can flatten the CTE and report the progress table
                // by its SQL alias instead of its underlying table name.
                line.contains("vod_progress") || line == "SCAN p" || line.starts_with("SEARCH p ")
            }),
            "progress drives the query: {lines:#?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("SEARCH v USING PRIMARY KEY")),
            "each watched title is looked up by catalog identity: {lines:#?}"
        );
        let lines = plan(
            &store,
            CONTINUE_WATCHING_RANK_SQL,
            params![5_000, 1, "movie", "generated-movie-000001"],
        );
        assert!(
            scans(&lines, &["v", "z", "vod", "series"]).is_empty(),
            "a reopened shelf finds its place without a catalog scan: {lines:#?}"
        );
        let lines = plan(&store, RECENTLY_ADDED_SQL, params![41, 0]);
        assert!(
            scans(&lines, &["w", "z", "vod_progress"]).is_empty(),
            "progress and duplicate checks are keyed lookups: {lines:#?}"
        );
    }
    /// Recently added has no index for its order: every page sorts the
    /// whole enabled library. That is why the runtime reads it on the
    /// analysis lane, not the guide's reader, and why it must stop when
    /// cancelled rather than finish the sort.
    #[test]
    fn recently_added_sorts_the_library_and_a_cancel_stops_it() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let (mut store, [a, _]) = large_library(5000);
        let linear = |lines: &[String]| {
            !scans(lines, &["v", "vod", "series", "items", "r", "o", "m"]).is_empty()
        };
        let page = plan(&store, RECENTLY_ADDED_SQL, params![41, 0]);
        assert!(
            linear(&page),
            "the documented linear cost is visible in the plan: {page:#?}"
        );
        let rank = plan(
            &store,
            RECENTLY_ADDED_RANK_SQL,
            params![a.0, "movie", "generated-movie-000001"],
        );
        assert!(linear(&rank), "{rank:#?}");
        let first = DiscoveryQuery::first(Shelf::RecentlyAdded);
        let cancel = iptv_import::CancelToken::new();
        let page = store.vod_discovery_cancelable(first, &cancel).unwrap();
        assert_eq!(page.cards.len(), iptv_core::vod::DISCOVERY_PAGE as usize);
        assert!(page.more);
        // Cancelled before it starts: no SQLite work at all.
        cancel.cancel();
        assert!(store.vod_discovery_cancelable(first, &cancel).is_err());
        // Cancelled while it ran (the shelf closed meanwhile): its result is
        // not reported as current. While it runs, the installed progress
        // handler is what interrupts SQLite's sort.
        let cancel = iptv_import::CancelToken::new();
        let steps = Arc::new(AtomicUsize::new(0));
        let counted = steps.clone();
        let stopper = cancel.clone();
        let result = store.vod_cancelable(&cancel, move |store| {
            // Installing a second handler would replace the cancelling one,
            // so the read cancels its own token from its row callback.
            store
                .conn
                .prepare(RECENTLY_ADDED_RANK_SQL)?
                .query_row(params![a.0, "movie", "generated-movie-004999"], |r| {
                    r.get::<_, i64>(0)
                })
                .map_err(Into::into)
                .inspect(|_| {
                    counted.fetch_add(1, Ordering::Relaxed);
                    stopper.cancel();
                })
        });
        assert!(
            result.is_err(),
            "a read canceled while it ran is not reported as current"
        );
        assert_eq!(steps.load(Ordering::Relaxed), 1);
        // The handler is removed afterwards: later reads run normally.
        assert_eq!(
            store.vod_discovery(first).unwrap().cards.len(),
            iptv_core::vod::DISCOVERY_PAGE as usize
        );
        assert_eq!(
            store
                .vod_discovery_rank(
                    Shelf::RecentlyAdded,
                    &key(a, Kind::Movie, "generated-movie-004999")
                )
                .unwrap(),
            Some(0),
            "the newest dated title comes first"
        );
    }
    #[test]
    fn continue_watching_has_a_fixed_work_budget_in_a_large_library() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let (mut store, [a, b]) = large_library(5000);
        for (source, id) in [(a, "generated-movie-004999"), (b, "generated-movie-000000")] {
            store
                .save_vod_progress(
                    &key(source, Kind::Movie, id),
                    None,
                    Progress::observed(30_000, Some(120_000), false),
                )
                .unwrap();
        }
        store
            .save_vod_progress(
                &key(a, Kind::Episode, "generated-series-002500:1"),
                Some("generated-series-002500"),
                Progress::observed(30_000, Some(120_000), false),
            )
            .unwrap();
        let budget = Arc::new(AtomicUsize::new(0));
        let steps = budget.clone();
        store
            .conn
            .progress_handler(
                1000,
                Some(move || steps.fetch_add(1, Ordering::Relaxed) > 40),
            )
            .unwrap();
        let page = store.vod_discovery(DiscoveryQuery::first(Shelf::ContinueWatching));
        store
            .conn
            .progress_handler(0, None::<fn() -> bool>)
            .unwrap();
        assert_eq!(page.unwrap().cards.len(), 3);
        assert!(
            budget.load(Ordering::Relaxed) <= 40,
            "Continue watching scanned the catalog"
        );
    }
    #[test]
    fn continuing_series_check_only_their_own_episode_history() {
        use rusqlite::StatementStatus;

        let mut previous_work = None;
        for per_source in [100, 1000] {
            let (mut store, sources) = large_library(per_source);
            for source in sources {
                for i in 0..per_source {
                    let parent = format!("generated-series-{i:06}");
                    // The parent follows a just-started next episode. Its own
                    // position cannot qualify it: EXISTS must find the earlier
                    // completed episode, even for parents late in key order.
                    for (episode, progress) in [
                        (1, Progress::observed(120_000, Some(120_000), true)),
                        (2, Progress::observed(1000, Some(120_000), false)),
                    ] {
                        store
                            .save_vod_progress(
                                &key(source, Kind::Episode, &format!("{parent}:{episode}")),
                                Some(&parent),
                                progress,
                            )
                            .unwrap();
                    }
                }
            }
            store
                .conn
                .execute("UPDATE vod_progress SET updated_at=1800000000", [])
                .unwrap();
            let target = format!("generated-series-{:06}", per_source - 1);
            for lines in [
                plan(&store, CONTINUE_WATCHING_SQL, params![41, 0, 5000]),
                plan(
                    &store,
                    CONTINUE_WATCHING_RANK_SQL,
                    params![5000, sources[1].0, "series", target],
                ),
            ] {
                let episodes: Vec<_> = lines
                    .iter()
                    .filter(|line| line.starts_with("SEARCH e "))
                    .collect();
                assert!(!episodes.is_empty(), "{lines:#?}");
                assert!(
                    episodes.iter().all(|line| {
                        line.contains("COVERING INDEX vod_progress_by_parent")
                            && line.contains("source_id=? AND parent_id=?")
                    }),
                    "each correlated episode lookup uses source and parent: {lines:#?}"
                );
            }
            let mut page = store.conn.prepare(CONTINUE_WATCHING_SQL).unwrap();
            let cards: Vec<String> = page
                .query_map(params![41, 0, 5000], |row| row.get(1))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert_eq!(cards.len(), 41);
            assert_eq!(cards[0], "generated-series-000000");
            let page_work = page.get_status(StatementStatus::VmStep);
            let mut rank = store.conn.prepare(CONTINUE_WATCHING_RANK_SQL).unwrap();
            assert_eq!(
                rank.query_row(params![5000, sources[1].0, "series", target], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
                (per_source * 2 - 1) as i64,
                "every generated series remains eligible through its earlier episode"
            );
            let rank_work = rank.get_status(StatementStatus::VmStep);
            let work = [page_work, rank_work];
            println!("{per_source} parents/source: page/rank VM steps {work:?}");
            for measured in work {
                assert!(
                    measured < (per_source * 2 * 400 + 10_000) as i32,
                    "episode history exceeded the linear work budget: {work:?}"
                );
            }
            if let Some(previous) = previous_work {
                for (measured, smaller) in work.into_iter().zip(previous) {
                    assert!(
                        measured < smaller * 14 + 10_000,
                        "ten times the history must not perform quadratic work: {work:?}"
                    );
                }
            }
            previous_work = Some(work);
        }
    }
    /// Evidence for review, not a gate: plans and page/rank latency of both
    /// shelves over a large generated library (200,000 titles across two
    /// playlists). It sets no time budget: Continue watching grows with the
    /// progress records (and their episodes), Recently added with the
    /// library; the runtime keeps both, and the windows around a focused
    /// title, off the guide's reader lane. Run with
    /// `cargo test -p iptv-store-sqlite --lib -- --ignored --nocapture discovery_latency`.
    #[test]
    #[ignore = "evidence: prints large-library discovery plans and timings"]
    fn discovery_latency_over_a_large_generated_library() {
        let (mut store, [a, b]) = large_library(50_000);
        for i in 0..500 {
            let source = if i % 2 == 0 { a } else { b };
            store
                .save_vod_progress(
                    &key(
                        source,
                        Kind::Movie,
                        &format!("generated-movie-{:06}", i * 97),
                    ),
                    None,
                    Progress::observed(30_000, Some(120_000), false),
                )
                .unwrap();
        }
        for (name, sql) in [
            ("continue watching", CONTINUE_WATCHING_SQL),
            ("recently added", RECENTLY_ADDED_SQL),
        ] {
            let lines = if sql == CONTINUE_WATCHING_SQL {
                plan(&store, sql, params![41, 0, 5_000])
            } else {
                plan(&store, sql, params![41, 0])
            };
            println!("{name} plan:\n  {}", lines.join("\n  "));
        }
        for which in [Shelf::ContinueWatching, Shelf::RecentlyAdded] {
            for offset in [0, 400, 4000] {
                let started = std::time::Instant::now();
                let page = shelf(&store, which, offset, iptv_core::vod::DISCOVERY_PAGE);
                let elapsed = started.elapsed();
                println!(
                    "{which:?} offset {offset}: {} titles in {elapsed:?}",
                    page.cards.len()
                );
                if which == Shelf::ContinueWatching && offset == 0 {
                    assert_eq!(page.cards.len(), iptv_core::vod::DISCOVERY_PAGE as usize);
                }
            }
            let target = key(a, Kind::Movie, &format!("generated-movie-{:06}", 498 * 97));
            let started = std::time::Instant::now();
            let rank = store.vod_discovery_rank(which, &target).unwrap();
            println!("{which:?} rank {rank:?} in {:?}", started.elapsed());
            assert!(rank.is_some(), "{which:?}");
            let started = std::time::Instant::now();
            let window = store.vod_discovery_around(which, &target).unwrap();
            println!(
                "{which:?} window at {} ({} titles) in {:?}",
                window.start,
                window.cards.len(),
                started.elapsed()
            );
            assert!(window.cards.iter().any(|c| c.key == target), "{which:?}");
        }
    }
    #[test]
    fn embedded_series_coordinates_round_trip() {
        let (mut store, key) = fixture();
        let mut item = store
            .load_vod(crate::SourceId(key.source), None, 0, Some(1))
            .unwrap()
            .remove(0);
        item.is_series = true;
        item.episode_numbers = vec![2, 4, 7];
        store
            .replace_catalog(crate::SourceId(key.source), &[], &[], &[item], &[])
            .unwrap();
        assert_eq!(store.vod_episode_numbers(&key).unwrap(), vec![2, 4, 7]);
        assert_eq!(
            store
                .load_vod(crate::SourceId(key.source), None, 0, None)
                .unwrap()[0]
                .episode_numbers,
            vec![2, 4, 7]
        );
    }
}
