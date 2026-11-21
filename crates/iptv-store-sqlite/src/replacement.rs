//! Resumable replacement: a catalog or guide publish as a job that writes
//! until a row budget is spent and is continued later
//! ([`crate::CatalogReplacement`], [`crate::EpgReplacement`]).
//!
//! A publish is one transaction, held open between steps, so readers of
//! another connection, or of the database while a browser publish runs on
//! its copy (`Store::begin_publish`), see the old data or
//! the new, never a mix. The browser drives a job in slices of a few
//! hundred rows (about 50 ms each on a 2020 TV) between other work on its
//! one thread; native callers run it to completion in one call
//! (`finish`), which executes exactly the statements the one-shot methods
//! always executed: an unlimited budget never splits a statement.
//!
//! Dropping an unfinished job rolls its transaction back: the stored data
//! is unchanged.

use rusqlite::{Connection, ToSql};

use crate::Result;

/// A step's progress: more to do, or the job committed with this result.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub enum Step<T> {
    Pending,
    Done(T),
}

/// How many rows a step may still write; `None` is unlimited.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Meter {
    left: Option<u64>,
}

impl Meter {
    pub(crate) const UNLIMITED: Self = Self { left: None };

    /// At least one row, so every step makes progress.
    pub(crate) fn rows(rows: u32) -> Self {
        Self {
            left: Some(u64::from(rows.max(1))),
        }
    }

    pub(crate) fn unlimited(&self) -> bool {
        self.left.is_none()
    }

    pub(crate) fn exhausted(&self) -> bool {
        self.left == Some(0)
    }

    /// The rows the next statement may write, as a `LIMIT` value.
    pub(crate) fn limit(&self) -> i64 {
        self.left
            .map_or(i64::MAX, |n| i64::try_from(n).unwrap_or(i64::MAX))
    }

    /// How many of `len` items the next slice may take.
    pub(crate) fn take(&self, len: usize) -> usize {
        self.left
            .map_or(len, |n| len.min(usize::try_from(n).unwrap_or(usize::MAX)))
    }

    pub(crate) fn spend(&mut self, rows: usize) {
        if let Some(left) = &mut self.left {
            *left = left.saturating_sub(rows as u64);
        }
    }
}

/// Where a job's transaction stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    /// Not begun, or between steps of an open transaction.
    Running,
    Committed,
    /// Failed and rolled back; the job can do nothing more.
    Failed,
}

pub(crate) fn begin(conn: &Connection) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    Ok(())
}

/// Rolls back the job's transaction if it is open.
pub(crate) fn roll_back(conn: &Connection) {
    if !conn.is_autocommit() {
        let _ = conn.execute_batch("ROLLBACK");
    }
}

pub(crate) fn finished_error() -> crate::StoreError {
    crate::StoreError::Unavailable("the replacement has already finished")
}

/// Deletes at most `limit` rows of `table` that match `filter`, chosen by
/// their key columns `key` (`rowid` or the primary key); returns how many.
/// Deleted rows are gone, so repeating it walks the whole set once.
pub(crate) fn delete_some(
    conn: &Connection,
    table: &str,
    key: &str,
    filter: &str,
    params: &[&dyn ToSql],
    limit: i64,
) -> Result<usize> {
    let limit_param = params.len() + 1;
    let sql = format!(
        "DELETE FROM {table} WHERE ({key}) IN \
         (SELECT {key} FROM {table} WHERE {filter} LIMIT ?{limit_param})"
    );
    let mut all: Vec<&dyn ToSql> = params.to_vec();
    all.push(&limit);
    Ok(conn.prepare_cached(&sql)?.execute(all.as_slice())?)
}

/// The last rowid of the next slice of at most `limit` channels of
/// `source` after rowid `after`, in rowid order; `None` when none is left.
/// `+source_id` keeps SQLite on the table's rowid order, so the slices
/// walk the table once instead of rescanning an index.
pub(crate) fn channel_slice_end(
    conn: &Connection,
    source: i64,
    after: i64,
    limit: i64,
) -> Result<Option<i64>> {
    Ok(conn
        .prepare_cached(
            "SELECT max(rowid) FROM (SELECT rowid FROM channels
             WHERE +source_id = ?1 AND rowid > ?2 ORDER BY rowid LIMIT ?3)",
        )?
        .query_row(rusqlite::params![source, after, limit], |r| r.get(0))?)
}

#[cfg(test)]
#[path = "replacement/tests.rs"]
mod tests;
