//! Durable recording intent. Claims and terminal transitions are conditional writes.
#[cfg(test)]
mod tests;
use crate::{Result, Store, StoreError};
use iptv_core::recording::{Job, JobId, MAX_JOBS, Request, Status};
use rusqlite::{OptionalExtension, params};

fn status(value: &str) -> Status {
    match value {
        "scheduled" => Status::Scheduled,
        "recording" => Status::Recording,
        "completed" => Status::Completed,
        "partial" => Status::Partial,
        "cancelled" => Status::Cancelled,
        "missed" => Status::Missed,
        _ => Status::Failed,
    }
}
fn stored(value: Status) -> &'static str {
    match value {
        Status::Scheduled => "scheduled",
        Status::Recording => "recording",
        Status::Completed => "completed",
        Status::Partial => "partial",
        Status::Failed => "failed",
        Status::Cancelled => "cancelled",
        Status::Missed => "missed",
    }
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: JobId(r.get(0)?),
        request: Request {
            source: r.get(1)?,
            channel: r.get(2)?,
            channel_name: r.get(3)?,
            title: r.get(4)?,
            start: r.get(5)?,
            end: r.get(6)?,
        },
        status: status(&r.get::<_, String>(7)?),
        bytes: r.get::<_, i64>(8)? as u64,
        message: r.get(9)?,
    })
}
impl Store {
    pub fn recording_jobs(&self) -> Result<Vec<Job>> {
        Ok(self
            .conn
            .prepare("SELECT * FROM recording_jobs ORDER BY starts_at DESC, id DESC LIMIT 1000")?
            .query_map([], row)?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn recording_job(&self, id: JobId) -> Result<Option<Job>> {
        Ok(self
            .conn
            .query_row("SELECT * FROM recording_jobs WHERE id=?1", [id.0], row)
            .optional()?)
    }
    pub fn schedule_recording(&mut self, id: JobId, request: &Request, now: i64) -> Result<JobId> {
        request
            .validate(now)
            .map_err(|m| StoreError::InvalidData(m.into()))?;
        let tx = self.conn.transaction()?;
        // An identical accepted request is idempotent, including a lost reply.
        if let Some(id) = tx.query_row(
            "SELECT id FROM recording_jobs WHERE source_id=?1 AND channel_id=?2 AND starts_at=?3
             AND ends_at=?4 AND status IN ('scheduled','recording')",
            params![request.source, request.channel, request.start, request.end], |r| r.get(0)).optional()? {
            return Ok(JobId(id));
        }
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM recording_jobs", [], |r| r.get(0))?;
        if count >= MAX_JOBS as i64 {
            return Err(StoreError::Unavailable(
                "Remove older recordings before adding more",
            ));
        }
        // Recheck current channel and access policy in the writer transaction.
        let allowed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM channels c JOIN sources s ON s.id=c.source_id
             LEFT JOIN channel_overlays o ON o.source_id=c.source_id AND o.channel_id=c.id
             LEFT JOIN source_policy p ON p.source_id=c.source_id
             WHERE c.source_id=?1 AND c.id=?2 AND COALESCE(o.blocked,0)=0 AND COALESCE(p.enabled,1)=1)",
            params![request.source, request.channel],
            |r| r.get(0),
        )?;
        if !allowed {
            return Err(StoreError::Unavailable("Channel unavailable or blocked"));
        }
        tx.execute("INSERT INTO recording_jobs(source_id,channel_id,channel_name,title,starts_at,ends_at,status,id,source_incarnation)
            VALUES(?1,?2,?3,?4,?5,?6,'scheduled',?7,(SELECT incarnation FROM guide_revisions WHERE source_id=?1))",
            params![request.source, request.channel, request.channel_name, request.title, request.start, request.end, id.0])?;
        tx.commit()?;
        Ok(id)
    }
    pub fn transition_recording(
        &mut self,
        id: JobId,
        from: Status,
        to: Status,
        bytes: u64,
        message: &str,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE recording_jobs SET status=?1,bytes=?2,message=?3 WHERE id=?4 AND status=?5",
            params![
                stored(to),
                bytes.min(i64::MAX as u64) as i64,
                message,
                id.0,
                stored(from)
            ],
        )? == 1)
    }
    /// Numeric source IDs may be reused after deletion; old recording intent
    /// has authority only for the original playlist incarnation.
    pub fn recording_source_is_current(&self, id: JobId) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM recording_jobs j JOIN guide_revisions r
             ON r.source_id=j.source_id AND r.incarnation=j.source_incarnation WHERE j.id=?1)",
            [id.0],
            |row| row.get(0),
        )?)
    }
    /// Compare the full accepted window: an edit/cancel racing the scheduler
    /// invalidates its old read instead of starting the previous schedule.
    pub fn claim_recording(&mut self, job: &Job) -> Result<bool> {
        let request = &job.request;
        Ok(self.conn.execute(
            "UPDATE recording_jobs SET status='recording',message='Starting recording'
            WHERE id=?1 AND status='scheduled' AND starts_at=?2 AND ends_at=?3",
            params![job.id.0, request.start, request.end],
        )? == 1)
    }
    pub fn update_recording(&mut self, id: JobId, request: &Request, now: i64) -> Result<bool> {
        request
            .validate(now)
            .map_err(|m| StoreError::InvalidData(m.into()))?;
        Ok(self.conn.execute(
            "UPDATE recording_jobs SET starts_at=?1,ends_at=?2
            WHERE id=?3 AND source_id=?4 AND channel_id=?5 AND status='scheduled'",
            params![
                request.start,
                request.end,
                id.0,
                request.source,
                request.channel
            ],
        )? == 1)
    }
    pub fn remove_recording(&mut self, id: JobId) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM recording_jobs WHERE id=?1 AND status NOT IN ('scheduled','recording')",
            [id.0],
        )? == 1)
    }
    /// Restored schedules must never rearm on a different host; media is not in catalogue backups.
    pub fn discard_recording_jobs(&mut self) -> Result<()> {
        self.conn.execute("DELETE FROM recording_jobs", [])?;
        Ok(())
    }
}
