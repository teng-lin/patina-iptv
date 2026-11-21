//! Resumable replacement: a catalog or guide publish as a job that writes
//! until a row budget is spent and is continued later
//! ([`crate::CatalogReplacement`], [`crate::EpgReplacement`]).
//!
//! A publish is one transaction, held open between steps, so readers of
//! another connection see the old data or the new, never a mix. A browser
//! publish runs on a copy of the database (`Store::begin_publish`) whose
//! readers are none: there each step commits (`pause`), and the swap of the
//! copy is what readers of the database see at once.
//!
//! The browser drives a job in slices of a few hundred rows (about 50 ms
//! each on a 2020 TV) between other work on its one thread; native callers
//! run it to completion in one call (`finish`), which executes exactly the
//! statements the one-shot methods always executed: an unlimited budget
//! never splits a statement.
//!
//! Dropping an unfinished job rolls its transaction back: the stored data
//! is unchanged. A publish copy that committed some steps is marked partial
//! instead, and `finish_publish` refuses it.

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

    /// Whether `rows` more rows fit in the budget.
    pub(crate) fn fits(&self, rows: u64) -> bool {
        self.left.is_none_or(|left| rows <= left)
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

/// Before a step that continues a job: reopens the transaction a browser
/// publish copy committed at the end of the last step ([`pause`]).
pub(crate) fn resume(store: &crate::Store) -> Result<()> {
    if store.conn.is_autocommit() {
        begin(&store.conn)?;
    }
    Ok(())
}

/// After a step that did not finish. On a browser publish copy the step's
/// work is committed: rolling back the copy is dropping it, and its swap
/// is what makes the publish atomic, so one transaction across all slices
/// would only keep a third copy of every page it touched in the memory
/// journal. The first such commit of a job registers it on the copy
/// (`claim`): until it commits its last step the copy holds part of it, and
/// if it ends without that, the copy holds half a publish for good.
/// Elsewhere the transaction stays open.
pub(crate) fn pause(store: &mut crate::Store, claim: &mut bool) -> Result<()> {
    #[cfg(target_family = "wasm")]
    if store.is_publish_copy() && !store.conn.is_autocommit() {
        if !*claim {
            store.copy_job_started();
            *claim = true;
        }
        store.conn.execute_batch("COMMIT")?;
    }
    let _ = (store, claim);
    Ok(())
}

/// After a job's last commit: the copy holds the whole of it.
pub(crate) fn completed(store: &mut crate::Store, claim: &mut bool) {
    #[cfg(target_family = "wasm")]
    if std::mem::take(claim) {
        store.copy_job_ended(true);
    }
    let _ = (store, claim);
}

/// A job that committed slices on a publish copy ended without its last
/// commit (dropped, failed, handed back): the copy is broken for good, and
/// `finish_publish` refuses it, whatever later jobs on it do.
pub(crate) fn abandoned(store: &mut crate::Store, claim: &mut bool) {
    #[cfg(target_family = "wasm")]
    if std::mem::take(claim) {
        store.copy_job_ended(false);
    }
    let _ = (store, claim);
}

/// Rolls back the job's transaction if it is open.
pub(crate) fn roll_back(conn: &Connection) {
    if !conn.is_autocommit() {
        let _ = conn.execute_batch("ROLLBACK");
    }
}

/// What a cleanup after a committed publish (a detach, restoring a
/// connection setting) returns to the caller. Natively its error, as the
/// one-shot methods always reported it. In the browser the publish runs on
/// a copy that the caller swaps in on success and abandons on error, so a
/// cleanup failure on a publish copy is logged instead of discarding
/// committed work. On any other store (the live database) it is returned:
/// a stage left attached would keep its memory and refuse the next attach.
pub(crate) fn after_commit(store: &crate::Store, cleanup: Result<()>) -> Result<()> {
    #[cfg(target_family = "wasm")]
    if store.is_publish_copy() {
        if let Err(e) = cleanup {
            log::warn!("cleanup after a committed publish failed: {e}");
        }
        return Ok(());
    }
    let _ = store;
    cleanup
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
#[path = "replacement/reference.rs"]
mod reference;
#[cfg(test)]
#[path = "replacement/tests.rs"]
mod tests;
