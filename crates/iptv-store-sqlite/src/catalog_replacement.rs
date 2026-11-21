//! Catalog replacement as a resumable job ([`CatalogReplacement`]): the
//! one-shot [`Store::replace_catalog`] and the stage publications
//! (`catalog_stage.rs`) run one to completion.

use std::borrow::{BorrowMut, Cow};

use iptv_providers::model::{Channel, Group, Series, Source, VodItem};
use iptv_providers::{ContentId, GroupId, SecretUrl};
use rusqlite::types::Value;

use crate::catalog::{self, DELETE_CATALOG};
use crate::catalog_stage::{CatalogStage, STAGE_TABLES};
use crate::replacement::{
    self, ChannelCursor, Meter, Progress, Step, channel_slice_end, delete_some,
};
use crate::sources::{self, CatalogCounts, OnDemandErrors};
use crate::{GuideSource, Result, SourceId, Store, StoreError, search};

/// The key columns `delete_some` picks rows of `DELETE_CATALOG`'s tables
/// by, in its order.
const DELETE_KEYS: [(&str, &str); 7] = [
    ("channel_groups", "source_id, group_id, channel_id"),
    ("channels", "rowid"),
    ("vod_groups", "source_id, group_id, vod_id"),
    ("vod", "source_id, id"),
    ("series_groups", "source_id, group_id, series_id"),
    ("series", "source_id, id"),
    ("groups", "source_id, id"),
];

/// What a finished catalog replacement wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CatalogPublished {
    pub counts: CatalogCounts,
    /// The guide sources published with the catalog, when the publication
    /// set them ([`StagePublication::guides`],
    /// [`StagePublication::provider_guides`]).
    pub guides: Option<Vec<GuideSource>>,
}

/// What a stage publication changes besides the catalog, in the same
/// commit: the source's edited details and its guide sources.
#[derive(Clone, Debug, Default)]
pub struct StagePublication {
    details: Option<Source>,
    guides: Option<Vec<GuideSource>>,
    provider_urls: Option<Vec<SecretUrl>>,
}

impl StagePublication {
    /// Only the catalog ([`Store::replace_catalog_stage`]).
    pub fn new() -> Self {
        Self::default()
    }

    /// Also the source's edited details.
    pub fn details(mut self, details: Source) -> Self {
        self.details = Some(details);
        self
    }

    /// Also the guide sources ([`Store::publish_catalog_stage`]).
    pub fn guides(mut self, guides: Vec<GuideSource>) -> Self {
        // Explicit guides replace a provider plan set before.
        self.provider_urls = None;
        self.guides = Some(guides);
        self
    }

    /// Also guide identities replanned for the provider's guide URLs
    /// ([`Store::publish_catalog_stage_with_provider_guides`]); `initial`
    /// seeds only uninitialized guide state.
    pub fn provider_guides(
        mut self,
        urls: Vec<SecretUrl>,
        initial: Option<Vec<GuideSource>>,
    ) -> Self {
        self.provider_urls = Some(urls);
        self.guides = initial;
        self
    }
}

enum Input<'a> {
    Rows {
        groups: Cow<'a, [Group]>,
        channels: Cow<'a, [Channel]>,
        vod: Cow<'a, [VodItem]>,
        series: Cow<'a, [Series]>,
    },
    Stage {
        stage: Box<CatalogStage>,
        publication: Box<StagePublication>,
        attached: bool,
    },
}

#[derive(Clone, Debug)]
enum Phase {
    Begin,
    /// Search index rows of the old channels, by channel rowid.
    Forget {
        after: Option<ChannelCursor>,
    },
    /// `DELETE_CATALOG[table]`.
    Delete {
        table: usize,
    },
    /// Rows: groups, channels, VOD, series from index `at`, item `at`'s
    /// group memberships from `member` when it was inserted without them.
    /// Stage: table `STAGE_TABLES[part]` after key `after`.
    Insert {
        part: usize,
        at: usize,
        member: Option<usize>,
        after: Option<Vec<Value>>,
    },
    Index {
        after: Option<ChannelCursor>,
    },
    Commit,
}

/// A catalog replacement that runs until a row budget is spent and is
/// continued later ([`Self::step`]), or to completion ([`Self::finish`]):
/// the resumable form of [`Store::replace_catalog`] and of the catalog
/// stage publications. See `replacement.rs`.
///
/// `S` is the store it writes, borrowed (`&mut Store`) or owned (a browser
/// publish copy, `Store::begin_publish`); [`Self::into_store`] gives an
/// owned one back.
pub struct CatalogReplacement<'a, S: BorrowMut<Store>> {
    store: Option<S>,
    source: SourceId,
    input: Input<'a>,
    phase: Phase,
    progress: Progress,
    now: i64,
    fts: bool,
    /// Rows inserted per kind (rows input).
    inserted: [usize; 4],
    guides: Option<Vec<GuideSource>>,
    /// This job committed slices on a browser publish copy
    /// (`replacement::pause`).
    copy_claim: bool,
    /// Tests: an error the cleanup after the commit reports.
    #[cfg(test)]
    pub(crate) fail_cleanup: Option<StoreError>,
}

impl<S: BorrowMut<Store>> std::fmt::Debug for CatalogReplacement<'_, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogReplacement")
            .field("source", &self.source)
            .field("phase", &self.phase)
            .field("progress", &self.progress)
            .finish_non_exhaustive()
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl<'a, S: BorrowMut<Store>> CatalogReplacement<'a, S> {
    /// Replaces `source`'s provider catalog with these rows, in provider
    /// order, as [`Store::replace_catalog`] does.
    pub fn new(
        store: S,
        source: SourceId,
        groups: impl Into<Cow<'a, [Group]>>,
        channels: impl Into<Cow<'a, [Channel]>>,
        vod: impl Into<Cow<'a, [VodItem]>>,
        series: impl Into<Cow<'a, [Series]>>,
    ) -> Self {
        Self::with_input(
            store,
            source,
            Input::Rows {
                groups: groups.into(),
                channels: channels.into(),
                vod: vod.into(),
                series: series.into(),
            },
        )
    }

    /// Publishes a completed stage, with whatever else `publication` sets,
    /// as the stage publications of [`Store`] do.
    pub fn from_stage(
        store: S,
        source: SourceId,
        stage: CatalogStage,
        publication: StagePublication,
    ) -> Result<Self> {
        if !stage.ready {
            return Err(StoreError::InvalidData(
                "cannot publish an incomplete catalog stage".into(),
            ));
        }
        Ok(Self::with_input(
            store,
            source,
            Input::Stage {
                stage: Box::new(stage),
                publication: Box::new(publication),
                attached: false,
            },
        ))
    }

    fn with_input(store: S, source: SourceId, input: Input<'a>) -> Self {
        Self {
            store: Some(store),
            source,
            input,
            phase: Phase::Begin,
            progress: Progress::Running,
            now: 0,
            fts: false,
            inserted: [0; 4],
            guides: None,
            copy_claim: false,
            #[cfg(test)]
            fail_cleanup: None,
        }
    }

    /// Writes until about `rows` rows are written (at least one), then
    /// stops between statements with the transaction still open. On a
    /// browser publish copy the step commits instead: its rows stay in the
    /// copy until the copy is discarded, and if the job then ends without
    /// its last commit the copy is never swapped in.
    pub fn step(&mut self, rows: u32) -> Result<Step<CatalogPublished>> {
        self.run(Meter::rows(rows))
    }

    /// Runs to completion: exactly the statements of the one-shot form.
    pub fn finish(mut self) -> Result<CatalogPublished> {
        match self.run(Meter::UNLIMITED)? {
            Step::Done(published) => Ok(published),
            Step::Pending => Err(StoreError::Unavailable("an unlimited step stopped early")),
        }
    }

    /// Tests: the phase the next step continues in.
    #[cfg(test)]
    pub(crate) fn phase(&self) -> String {
        format!("{:?}", self.phase)
    }

    /// Whether the replacement committed.
    pub fn is_done(&self) -> bool {
        self.progress == Progress::Committed
    }

    /// The store back: after the commit, or abandoning (rolling back) an
    /// unfinished replacement. A browser publish copy keeps the slices it
    /// committed, so an unfinished job leaves it unpublishable: discard it
    /// (`abandon_publish`) and begin a new publish.
    pub fn into_store(mut self) -> S {
        self.abort();
        self.store
            .take()
            .expect("the store stays until the job is dropped")
    }

    pub(crate) fn store(&mut self) -> &mut Store {
        self.store
            .as_mut()
            .expect("the store stays until the job is dropped")
            .borrow_mut()
    }

    fn run(&mut self, mut meter: Meter) -> Result<Step<CatalogPublished>> {
        if self.progress != Progress::Running {
            return Err(replacement::finished_error());
        }
        let changes = self.store().conn.total_changes();
        let stepped = self.resume().and_then(|()| self.advance(&mut meter));
        let stepped = match stepped {
            Ok(Step::Pending) => {
                let store = self
                    .store
                    .as_mut()
                    .expect("the store stays until the job is dropped")
                    .borrow_mut();
                let changed = store.conn.total_changes() != changes;
                replacement::pause(store, &mut self.copy_claim, changed).map(|()| Step::Pending)
            }
            other => other,
        };
        match stepped {
            Ok(Step::Done(published)) => {
                // The catalog is published. Natively a failed detach is still
                // reported, as it always was, but the job stays committed; in
                // the browser the publish copy must not be abandoned over it
                // (`replacement::after_commit`).
                let cleanup = self.detach();
                #[cfg(test)]
                let cleanup = cleanup.and(self.fail_cleanup.take().map_or(Ok(()), Err));
                replacement::after_commit(self.store(), cleanup)?;
                Ok(Step::Done(published))
            }
            Ok(Step::Pending) => Ok(Step::Pending),
            Err(e) => {
                self.abort();
                self.progress = Progress::Failed;
                Err(e)
            }
        }
    }

    /// Reopens the transaction a browser publish copy committed between
    /// steps (`replacement::pause`).
    fn resume(&mut self) -> Result<()> {
        if matches!(self.phase, Phase::Begin) {
            return Ok(());
        }
        replacement::resume(self.store())
    }

    /// Rolls back an open transaction and detaches the stage.
    fn abort(&mut self) {
        let _ = self.detach();
    }

    /// Rolls back an open transaction, then detaches the stage; the detach's
    /// error.
    fn detach(&mut self) -> Result<()> {
        let Some(store) = self.store.as_mut() else {
            return Ok(());
        };
        let store = store.borrow_mut();
        if self.progress == Progress::Running {
            replacement::roll_back(&store.conn);
        }
        if self.progress != Progress::Committed {
            replacement::abandoned(store, &mut self.copy_claim);
        }
        if let Input::Stage { attached, .. } = &mut self.input
            && std::mem::take(attached)
        {
            store.conn.execute_batch("DETACH DATABASE catalog_import")?;
        }
        Ok(())
    }

    fn advance(&mut self, meter: &mut Meter) -> Result<Step<CatalogPublished>> {
        loop {
            if meter.exhausted() {
                return Ok(Step::Pending);
            }
            match self.phase.clone() {
                Phase::Begin => self.begin()?,
                Phase::Forget { after } => self.forget(meter, after)?,
                Phase::Delete { table } => self.delete(meter, table)?,
                Phase::Insert {
                    part,
                    at,
                    member,
                    after,
                } => self.insert(meter, part, at, member, after)?,
                Phase::Index { after } => self.index(meter, after)?,
                Phase::Commit => return self.commit().map(Step::Done),
            }
        }
    }

    fn begin(&mut self) -> Result<()> {
        let source = self.source;
        let store = self
            .store
            .as_mut()
            .expect("the store stays until the job is dropped")
            .borrow_mut();
        self.now = store.now();
        self.fts = store.fts_index;
        if let Input::Stage {
            stage, attached, ..
        } = &mut self.input
        {
            store.attach_stage(&stage.path, stage.file.as_deref(), "catalog_import")?;
            *attached = true;
        }
        replacement::begin(&store.conn)?;
        sources::ensure_source(&store.conn, source)?;
        if let Input::Stage { publication, .. } = &self.input {
            let now = self.now;
            let replanned = publication
                .provider_urls
                .as_deref()
                .map(|urls| {
                    crate::guide_sources::plan_provider_guides_in(
                        &store.conn,
                        source,
                        urls,
                        publication.guides.as_deref(),
                        now,
                    )
                })
                .transpose()?;
            let guides = replanned.or_else(|| publication.guides.clone());
            if let Some(details) = &publication.details {
                sources::update_source_in(&store.conn, source, details, now)?;
            }
            if let Some(guides) = &guides {
                crate::guide_sources::set_guide_sources_in(&store.conn, source, guides, now)?;
            }
            self.guides = guides;
        }
        self.phase = if self.fts {
            Phase::Forget { after: None }
        } else {
            Phase::Delete { table: 0 }
        };
        Ok(())
    }

    fn forget(&mut self, meter: &mut Meter, after: Option<ChannelCursor>) -> Result<()> {
        let source = self.source;
        let conn = &self.store().conn;
        if meter.unlimited() && after.is_none() {
            search::forget_source(conn, true, source)?;
            self.phase = Phase::Delete { table: 0 };
            return Ok(());
        }
        match channel_slice_end(conn, source.0, after, meter.limit())? {
            Some(last) => {
                let n = search::forget_channels(conn, source, after, last)?;
                meter.spend(n.max(1));
                self.phase = Phase::Forget { after: Some(last) };
            }
            None => self.phase = Phase::Delete { table: 0 },
        }
        Ok(())
    }

    fn delete(&mut self, meter: &mut Meter, table: usize) -> Result<()> {
        let source = self.source.0;
        let conn = &self.store().conn;
        let next = |table: usize| {
            if table + 1 < DELETE_CATALOG.len() {
                Phase::Delete { table: table + 1 }
            } else {
                Phase::Insert {
                    part: 0,
                    at: 0,
                    member: None,
                    after: None,
                }
            }
        };
        if meter.unlimited() {
            conn.execute(DELETE_CATALOG[table], [source])?;
            self.phase = next(table);
            return Ok(());
        }
        let (name, key) = DELETE_KEYS[table];
        let n = delete_some(conn, name, key, "source_id = ?1", &[&source], meter.limit())?;
        meter.spend(n);
        if n == 0 {
            self.phase = next(table);
        }
        Ok(())
    }

    fn insert(
        &mut self,
        meter: &mut Meter,
        part: usize,
        at: usize,
        member: Option<usize>,
        after: Option<Vec<Value>>,
    ) -> Result<()> {
        let source = self.source;
        let store = self
            .store
            .as_mut()
            .expect("the store stays until the job is dropped")
            .borrow_mut();
        let conn = &store.conn;
        let after_insert = |part: usize, parts: usize| {
            if part + 1 < parts {
                Phase::Insert {
                    part: part + 1,
                    at: 0,
                    member: None,
                    after: None,
                }
            } else if store.fts_index {
                Phase::Index { after: None }
            } else {
                Phase::Commit
            }
        };
        match &self.input {
            Input::Rows {
                groups,
                channels,
                vod,
                series,
            } => {
                let items = match part {
                    0 => Items::Groups(groups),
                    1 => Items::Channels(channels),
                    2 => Items::Vod(vod),
                    _ => Items::Series(series),
                };
                let (at, member) =
                    items.insert(conn, source, meter, at, member, &mut self.inserted[part])?;
                self.phase = if at < items.len() {
                    Phase::Insert {
                        part,
                        at,
                        member,
                        after: None,
                    }
                } else {
                    after_insert(part, 4)
                };
            }
            Input::Stage { .. } => {
                let table = STAGE_TABLES[part];
                let columns = stage_columns(conn, table)?;
                let list = columns.names.join(",");
                // The whole table in one statement, unless earlier steps
                // copied part of it (`finish` after `step`).
                if meter.unlimited() && after.is_none() {
                    conn.execute(
                        &format!(
                            "INSERT INTO main.{table}(source_id,{list}) \
                             SELECT ?1,{list} FROM catalog_import.{table}"
                        ),
                        [source.0],
                    )?;
                    self.phase = after_insert(part, STAGE_TABLES.len());
                    return Ok(());
                }
                let key = columns.key.join(",");
                let lower = after
                    .as_ref()
                    .map(|_| format!("({key}) > ({})", marks(&columns.key, 0)));
                // The last key of the next slice.
                let end_sql = format!(
                    "SELECT {key} FROM catalog_import.{table} {} ORDER BY {key} \
                     LIMIT 1 OFFSET ?{}",
                    lower
                        .as_ref()
                        .map_or(String::new(), |l| format!("WHERE {l}")),
                    if after.is_some() {
                        columns.key.len() + 1
                    } else {
                        1
                    },
                );
                let offset = meter.limit().saturating_sub(1);
                let mut bound: Vec<&dyn rusqlite::ToSql> = Vec::new();
                if let Some(after) = &after {
                    bound.extend(after.iter().map(|v| v as &dyn rusqlite::ToSql));
                }
                let mut end_params = bound.clone();
                end_params.push(&offset);
                let end: Option<Vec<Value>> = {
                    let mut stmt = conn.prepare_cached(&end_sql)?;
                    let mut rows = stmt.query(end_params.as_slice())?;
                    match rows.next()? {
                        Some(row) => Some(
                            (0..columns.key.len())
                                .map(|i| row.get::<_, Value>(i))
                                .collect::<rusqlite::Result<_>>()?,
                        ),
                        None => None,
                    }
                };
                let mut filters: Vec<String> = Vec::new();
                let mut params: Vec<&dyn rusqlite::ToSql> = vec![&source.0];
                if let Some(after) = &after {
                    filters.push(format!("({key}) > ({})", marks(&columns.key, 1)));
                    params.extend(after.iter().map(|v| v as &dyn rusqlite::ToSql));
                }
                if let Some(end) = &end {
                    filters.push(format!(
                        "({key}) <= ({})",
                        marks(&columns.key, params.len())
                    ));
                    params.extend(end.iter().map(|v| v as &dyn rusqlite::ToSql));
                }
                let filter = if filters.is_empty() {
                    String::new()
                } else {
                    format!("WHERE {}", filters.join(" AND "))
                };
                let n = conn
                    .prepare_cached(&format!(
                        "INSERT INTO main.{table}(source_id,{list}) \
                     SELECT ?1,{list} FROM catalog_import.{table} {filter} ORDER BY {key}"
                    ))?
                    .execute(params.as_slice())?;
                meter.spend(n.max(1));
                self.phase = match end {
                    Some(end) => Phase::Insert {
                        part,
                        at: 0,
                        member: None,
                        after: Some(end),
                    },
                    None => after_insert(part, STAGE_TABLES.len()),
                };
            }
        }
        Ok(())
    }

    fn index(&mut self, meter: &mut Meter, after: Option<ChannelCursor>) -> Result<()> {
        let source = self.source;
        let conn = &self.store().conn;
        if meter.unlimited() && after.is_none() {
            search::index_source(conn, true, source)?;
            self.phase = Phase::Commit;
            return Ok(());
        }
        match channel_slice_end(conn, source.0, after, meter.limit())? {
            Some(last) => {
                let n = search::index_channels(conn, source, after, last)?;
                meter.spend(n.max(1));
                self.phase = Phase::Index { after: Some(last) };
            }
            None => self.phase = Phase::Commit,
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<CatalogPublished> {
        let source = self.source;
        let now = self.now;
        let (counts, on_demand) = match &self.input {
            Input::Rows {
                groups,
                channels,
                vod,
                series,
            } => {
                let [g, c, v, s] = self.inserted;
                let total = groups.len() + channels.len() + vod.len() + series.len();
                let counts = CatalogCounts {
                    groups: count(g),
                    channels: count(c),
                    vod: count(v),
                    series: count(s),
                    duplicates: count(total - g - c - v - s),
                };
                (counts, OnDemandErrors::default())
            }
            Input::Stage { stage, .. } => (stage.counts, stage.on_demand_errors),
        };
        #[cfg(test)]
        let rows = matches!(self.input, Input::Rows { .. });
        let conn = &self.store().conn;
        sources::record_import_success(conn, source, &counts, on_demand, now)?;
        #[cfg(test)]
        if rows {
            crate::catalog::crash_boundary("staged");
        }
        conn.execute_batch("COMMIT")?;
        self.progress = Progress::Committed;
        #[cfg(test)]
        if rows {
            crate::catalog::crash_boundary("committed");
        }
        let store = self
            .store
            .as_mut()
            .expect("the store stays until the job is dropped")
            .borrow_mut();
        replacement::completed(store, &mut self.copy_claim);
        Ok(CatalogPublished {
            counts,
            guides: self.guides.take(),
        })
    }
}

impl<S: BorrowMut<Store>> Drop for CatalogReplacement<'_, S> {
    fn drop(&mut self) {
        self.abort();
    }
}

/// `?first+1, ?first+2, ...` for `columns`.
fn marks(columns: &[String], first: usize) -> String {
    (1..=columns.len())
        .map(|i| format!("?{}", first + i))
        .collect::<Vec<_>>()
        .join(",")
}

/// A stage table's columns (without `source_id`) and the key its rows are
/// sliced by: the primary key, or `rowid`.
struct StageColumns {
    names: Vec<String>,
    key: Vec<String>,
}

fn stage_columns(conn: &rusqlite::Connection, table: &str) -> Result<StageColumns> {
    let mut stmt = conn.prepare_cached(&format!("PRAGMA catalog_import.table_info({table})"))?;
    let mut names = Vec::new();
    let mut key: Vec<(i64, String)> = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        let pk: i64 = row.get(5)?;
        if pk > 0 {
            key.push((pk, format!("\"{name}\"")));
        }
        if name != "source_id" {
            names.push(format!("\"{name}\""));
        }
    }
    key.sort();
    let key = if key.is_empty() {
        vec!["rowid".to_owned()]
    } else {
        key.into_iter().map(|(_, name)| name).collect()
    };
    Ok(StageColumns { names, key })
}

/// One kind of catalog row, for inserting in slices.
enum Items<'x> {
    Groups(&'x [Group]),
    Channels(&'x [Channel]),
    Vod(&'x [VodItem]),
    Series(&'x [Series]),
}

impl Items<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Groups(items) => items.len(),
            Self::Channels(items) => items.len(),
            Self::Vod(items) => items.len(),
            Self::Series(items) => items.len(),
        }
    }

    /// Item `i`'s id, groups and membership statement (none for groups).
    fn members(&self, i: usize) -> Option<(&ContentId, &[GroupId], &'static str)> {
        match self {
            Self::Groups(_) => None,
            Self::Channels(items) => {
                Some((&items[i].id, &items[i].groups, catalog::CHANNEL_MEMBERS))
            }
            Self::Vod(items) => Some((&items[i].id, &items[i].groups, catalog::VOD_MEMBERS)),
            Self::Series(items) => Some((&items[i].id, &items[i].groups, catalog::SERIES_MEMBERS)),
        }
    }

    /// Rows item `i` writes: its own and its memberships.
    fn cost(&self, i: usize) -> u64 {
        1 + self
            .members(i)
            .map_or(0, |(_, groups, _)| groups.len() as u64)
    }

    /// Inserts items `range` with their memberships, as the one-shot form
    /// does; how many were new.
    fn insert_all(
        &self,
        conn: &rusqlite::Connection,
        source: SourceId,
        range: std::ops::Range<usize>,
    ) -> Result<usize> {
        let offset = range.start as i64;
        match self {
            Self::Groups(items) => catalog::insert_groups(conn, source, &items[range], offset),
            Self::Channels(items) => catalog::insert_channels(conn, source, &items[range], offset),
            Self::Vod(items) => catalog::insert_vod(conn, source, &items[range], offset),
            Self::Series(items) => catalog::insert_series(conn, source, &items[range], offset),
        }
    }

    /// Inserts item `i`'s own row; whether it was new.
    fn insert_row(&self, conn: &rusqlite::Connection, source: SourceId, i: usize) -> Result<bool> {
        let position = i as i64;
        match self {
            Self::Groups(items) => {
                let mut stmt = conn.prepare_cached(catalog::GROUP_ROW)?;
                catalog::group_row(&mut stmt, source, &items[i], position)
            }
            Self::Channels(items) => {
                let mut stmt = conn.prepare_cached(catalog::CHANNEL_ROW)?;
                catalog::channel_row(&mut stmt, source, &items[i], position)
            }
            Self::Vod(items) => {
                let mut stmt = conn.prepare_cached(catalog::VOD_ROW)?;
                catalog::vod_row(&mut stmt, source, &items[i], position)
            }
            Self::Series(items) => {
                let mut stmt = conn.prepare_cached(catalog::SERIES_ROW)?;
                catalog::series_row(&mut stmt, source, &items[i], position)
            }
        }
    }

    /// Inserts from item `at` (and its memberships from `member`, when it
    /// was inserted without them) until `meter` is spent, counting every row
    /// a membership writes too; returns the cursor and adds new items to
    /// `inserted`. An item with more memberships than the budget is written
    /// over several steps.
    fn insert(
        &self,
        conn: &rusqlite::Connection,
        source: SourceId,
        meter: &mut Meter,
        mut at: usize,
        mut member: Option<usize>,
        inserted: &mut usize,
    ) -> Result<(usize, Option<usize>)> {
        while at < self.len() && !meter.exhausted() {
            if let Some(from) = member {
                let (id, groups, sql) = self.members(at).expect("only items with groups");
                let n = meter.take(groups.len() - from);
                catalog::insert_members(conn, sql, source, id, &groups[from..from + n], at as i64)?;
                meter.spend(n);
                if from + n < groups.len() {
                    member = Some(from + n);
                } else {
                    member = None;
                    at += 1;
                }
                continue;
            }
            if meter.unlimited() {
                // The rest in one call, as the one-shot form inserts them.
                *inserted += self.insert_all(conn, source, at..self.len())?;
                at = self.len();
                continue;
            }
            // As many whole items as the budget holds, in one call.
            let mut end = at;
            let mut cost = 0;
            while end < self.len() && meter.fits(cost + self.cost(end)) {
                cost += self.cost(end);
                end += 1;
            }
            if end > at {
                *inserted += self.insert_all(conn, source, at..end)?;
                meter.spend(cost as usize);
                at = end;
            } else if self.insert_row(conn, source, at)? {
                // The item alone, its memberships in the next slices.
                *inserted += 1;
                meter.spend(1);
                member = Some(0);
            } else {
                meter.spend(1);
                at += 1;
            }
        }
        Ok((at, member))
    }
}
