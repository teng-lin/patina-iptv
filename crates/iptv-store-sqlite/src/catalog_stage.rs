//! Private, bounded catalog staging. Downloads never hold the live writer lock.
//! A stage is published in one transaction only after the complete import passes.
use crate::{CatalogCounts, Result, SourceId, Store, StoreError, catalog, codec, search, sources};
use iptv_import::{CatalogRow, CatalogSink, SinkError};
use iptv_providers::model::{Channel, Group, Series, VodItem};
use rusqlite::{Connection, TransactionBehavior};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

const ROWS: usize = 512;
const TEXT: usize = 1 << 20;
const SOURCE: SourceId = SourceId(1);
const TABLES: &[&str] = &[
    "groups",
    "channels",
    "channel_groups",
    "vod",
    "vod_groups",
    "series",
    "series_groups",
];
#[derive(Default)]
struct Batch {
    groups: Vec<Group>,
    channels: Vec<Channel>,
    vod: Vec<VodItem>,
    series: Vec<Series>,
    bytes: usize,
}
impl Batch {
    fn len(&self) -> usize {
        self.groups.len() + self.channels.len() + self.vod.len() + self.series.len()
    }
}

pub struct CatalogStage {
    path: PathBuf,
    conn: Option<Connection>,
    batch: Batch,
    positions: [i64; 4],
    checkpoint: Option<[i64; 4]>,
    ready: bool,
    counts: CatalogCounts,
}
fn invalid(s: &str) -> StoreError {
    StoreError::InvalidData(s.into())
}
fn sink_error(_: StoreError) -> SinkError {
    SinkError("could not write the catalog stage".into())
}
/// The staged channels in playlist order, in the column order
/// `channels::decode` reads: the stage has no overlay columns, so those
/// are constants.
const STAGED_CHANNELS: &str = "SELECT source_id,id,provider_id,name,number,logo,group_ids,stream,\
     epg_id,epg_name,epg_shift_minutes,catch_up,radio,NULL,NULL,0,0,0,NULL,NULL,NULL \
     FROM channels ORDER BY position";

impl CatalogStage {
    pub fn create_in(dir: impl AsRef<Path>) -> Result<Self> {
        Self::create_in_with(dir, |path| Ok(Connection::open(path)?))
    }
    /// The connection factory may apply the live store's encryption key.
    pub fn create_in_with(
        dir: impl AsRef<Path>,
        open: impl FnOnce(&Path) -> Result<Connection>,
    ) -> Result<Self> {
        iptv_sqlcipher_init::initialize()?;
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = loop {
            let path = dir.as_ref().join(format!(
                "catalog-stage-{}-{}.sqlite",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    drop(file);
                    break path;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(invalid("cannot create catalog stage")),
            }
        };
        let result = open(&path).and_then(|conn| Self::from_connection(path.clone(), conn));
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }
    /// Call once at startup, before import workers start. A killed process
    /// cannot run Drop; remove its stages and SQLite sidecars here.
    pub fn remove_stale(dir: impl AsRef<Path>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("catalog-stage-")
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
    pub fn from_connection(path: PathBuf, conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", false)?;
        conn.pragma_update(None, "journal_mode", "MEMORY")?;
        conn.pragma_update(None, "synchronous", "OFF")?;
        conn.pragma_update(None, "cache_size", -4096)?;
        // Catalog columns are the original stable schema; no user overlays or
        // search index are needed in the private stage.
        conn.execute_batch(include_str!("../migrations/0001_provider_data.sql"))?;
        Ok(Self {
            path,
            conn: Some(conn),
            batch: Batch::default(),
            positions: [0; 4],
            checkpoint: None,
            ready: false,
            counts: CatalogCounts::default(),
        })
    }
    fn db(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("stage connection is open until drop")
    }
    fn flush(&mut self) -> Result<()> {
        if self.batch.len() == 0 {
            return Ok(());
        }
        self.db().execute_batch("SAVEPOINT catalog_batch")?;
        let result = (|| {
            catalog::insert_groups(self.db(), SOURCE, &self.batch.groups, self.positions[0])?;
            catalog::insert_channels(self.db(), SOURCE, &self.batch.channels, self.positions[1])?;
            catalog::insert_vod(self.db(), SOURCE, &self.batch.vod, self.positions[2])?;
            catalog::insert_series(self.db(), SOURCE, &self.batch.series, self.positions[3])?;
            Ok(())
        })();
        if result.is_ok() {
            self.db().execute_batch("RELEASE catalog_batch")?;
        } else {
            self.db()
                .execute_batch("ROLLBACK TO catalog_batch; RELEASE catalog_batch")?;
            return result;
        }
        for (position, n) in self.positions.iter_mut().zip([
            self.batch.groups.len(),
            self.batch.channels.len(),
            self.batch.vod.len(),
            self.batch.series.len(),
        ]) {
            *position += n as i64;
        }
        self.batch = Batch::default();
        Ok(())
    }
    fn complete(&mut self) -> Result<()> {
        if self.checkpoint.is_some() {
            return Err(invalid("catalog step is unfinished"));
        }
        self.flush()?;
        let count = |table: &str| -> Result<u32> {
            Ok(self
                .db()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?)
        };
        let initial = [
            count("groups")?,
            count("channels")?,
            count("vod")?,
            count("series")?,
        ];
        for (members, kind) in [
            ("channel_groups", "live"),
            ("vod_groups", "movie"),
            ("series_groups", "series"),
        ] {
            self.db().execute_batch(&format!("INSERT OR IGNORE INTO groups(source_id,id,kind,position,name,provider_id,adult)
                SELECT 1,group_id,'{kind}',(SELECT COALESCE(MAX(position),-1) FROM groups)+ROW_NUMBER() OVER(ORDER BY MIN(position),group_id),
                    CASE WHEN instr(group_id,'-group:')>0 THEN 'Category '||substr(group_id,instr(group_id,'-group:')+7) ELSE 'Other' END,
                    CASE WHEN instr(group_id,'-group:')>0 THEN substr(group_id,instr(group_id,'-group:')+7) ELSE NULL END,0
                FROM {members} WHERE group_id NOT IN(SELECT id FROM groups) GROUP BY group_id"))?;
        }
        let attempted: u64 = self.positions.iter().map(|n| *n as u64).sum();
        let unique: u64 = initial.iter().map(|n| u64::from(*n)).sum();
        self.counts = CatalogCounts {
            groups: count("groups")?,
            channels: initial[1],
            vod: initial[2],
            series: initial[3],
            duplicates: attempted.saturating_sub(unique).min(u64::from(u32::MAX)) as u32,
        };
        self.ready = true;
        Ok(())
    }
    pub fn counts(&self) -> CatalogCounts {
        self.counts
    }
    pub fn load_channels(&self) -> Result<Vec<Channel>> {
        if !self.ready {
            return Err(invalid("catalog stage is not complete"));
        }
        let mut stmt = self.db().prepare(STAGED_CHANNELS)?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(crate::channels::decode(row)?.channel);
        }
        Ok(out)
    }
    pub fn load_groups(&self) -> Result<Vec<Group>> {
        let mut stmt = self
            .db()
            .prepare("SELECT id,kind,name,provider_id,adult FROM groups ORDER BY position")?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(Group {
                id: iptv_providers::GroupId::from_stored(r.get::<_, String>(0)?),
                kind: codec::content_kind_from(&r.get::<_, String>(1)?)?,
                name: r.get(2)?,
                provider_id: r.get(3)?,
                adult: r.get(4)?,
            });
        }
        Ok(out)
    }
}
impl CatalogSink for CatalogStage {
    fn begin_step(&mut self) -> std::result::Result<(), SinkError> {
        if self.checkpoint.is_some() || self.ready {
            return Err(SinkError("invalid catalog stage state".into()));
        }
        self.flush().map_err(sink_error)?;
        self.db()
            .execute_batch("SAVEPOINT catalog_step")
            .map_err(|_| SinkError("could not begin catalog step".into()))?;
        self.checkpoint = Some(self.positions);
        Ok(())
    }
    fn push(&mut self, row: CatalogRow) -> std::result::Result<(), SinkError> {
        if self.ready {
            return Err(SinkError("catalog stage is already complete".into()));
        }
        let bytes = match &row {
            CatalogRow::Group(v) => {
                v.id.as_str().len() + v.name.len() + v.provider_id.as_ref().map_or(0, String::len)
            }
            CatalogRow::Channel(v) => {
                v.id.as_str().len()
                    + v.name.len()
                    + codec::stream_to_json(&v.stream).len()
                    + v.groups.iter().map(|g| g.as_str().len()).sum::<usize>()
                    + v.logo.as_ref().map_or(0, String::len)
                    + v.epg_id.as_ref().map_or(0, String::len)
                    + v.epg_name.as_ref().map_or(0, String::len)
                    + v.catch_up
                        .as_ref()
                        .map_or(0, |c| codec::catch_up_to_json(c).len())
            }
            CatalogRow::Vod(v) => {
                v.name.len()
                    + codec::stream_to_json(&v.stream).len()
                    + v.groups.iter().map(|g| g.as_str().len()).sum::<usize>()
                    + v.plot.as_ref().map_or(0, String::len)
                    + v.poster.as_ref().map_or(0, String::len)
            }
            CatalogRow::Series(v) => {
                v.name.len()
                    + v.groups.iter().map(|g| g.as_str().len()).sum::<usize>()
                    + v.plot.as_ref().map_or(0, String::len)
                    + v.poster.as_ref().map_or(0, String::len)
            }
        };
        self.batch.bytes = self.batch.bytes.saturating_add(bytes);
        match row {
            CatalogRow::Group(v) => self.batch.groups.push(v),
            CatalogRow::Channel(v) => self.batch.channels.push(v),
            CatalogRow::Vod(v) => self.batch.vod.push(v),
            CatalogRow::Series(v) => self.batch.series.push(v),
        }
        if self.batch.len() >= ROWS || self.batch.bytes >= TEXT {
            self.flush().map_err(sink_error)?;
        }
        Ok(())
    }
    fn end_step(&mut self, success: bool) -> std::result::Result<(), SinkError> {
        let checkpoint = self
            .checkpoint
            .ok_or_else(|| SinkError("no active catalog step".into()))?;
        if success {
            self.flush().map_err(sink_error)?;
        } else {
            self.batch = Batch::default();
            self.positions = checkpoint;
            self.db()
                .execute_batch("ROLLBACK TO catalog_step")
                .map_err(|_| SinkError("could not roll back catalog step".into()))?;
        }
        self.db()
            .execute_batch("RELEASE catalog_step")
            .map_err(|_| SinkError("could not finish catalog step".into()))?;
        self.checkpoint = None;
        Ok(())
    }
    fn finish(&mut self) -> std::result::Result<(), SinkError> {
        self.complete().map_err(sink_error)
    }
}
impl Drop for CatalogStage {
    fn drop(&mut self) {
        drop(self.conn.take());
        let _ = std::fs::remove_file(&self.path);
        for suffix in ["-journal", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

impl Store {
    /// Atomically publish a completed private stage, preserving all overlays.
    pub fn replace_catalog_stage(
        &mut self,
        source: SourceId,
        stage: CatalogStage,
    ) -> Result<CatalogCounts> {
        self.publish_stage(source, stage, None, None)
    }

    /// Publish edited source details, guide sources and catalog in one commit.
    /// A failed publication preserves all three, including saved credentials.
    pub fn publish_catalog_stage(
        &mut self,
        source: SourceId,
        stage: CatalogStage,
        details: Option<&iptv_providers::model::Source>,
        guides: &[crate::GuideSource],
    ) -> Result<CatalogCounts> {
        self.publish_stage(source, stage, details, Some(guides))
    }

    fn publish_stage(
        &mut self,
        source: SourceId,
        stage: CatalogStage,
        details: Option<&iptv_providers::model::Source>,
        guides: Option<&[crate::GuideSource]>,
    ) -> Result<CatalogCounts> {
        if !stage.ready {
            return Err(invalid("cannot publish an incomplete catalog stage"));
        }
        self.attach_staging_database(&stage.path, "catalog_import")?;
        let result: Result<CatalogCounts> = (|| {
            let now = self.now();
            let fts = self.fts_index;
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            sources::ensure_source(&tx, source)?;
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
                tx.execute(&format!("INSERT INTO main.{table}(source_id,{columns}) SELECT ?1,{columns} FROM catalog_import.{table}"),[source.0])?;
            }
            search::index_source(&tx, fts, source)?;
            sources::record_import_success(&tx, source, &stage.counts, now)?;
            tx.commit()?;
            Ok(stage.counts)
        })();
        let detached = self
            .conn
            .execute_batch("DETACH DATABASE catalog_import")
            .map_err(StoreError::from);
        let counts = result?;
        detached?;
        Ok(counts)
    }
}
