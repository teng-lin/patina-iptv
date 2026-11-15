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
        source_current: r.get(11)?,
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
fn insert_job(
    tx: &rusqlite::Transaction<'_>,
    id: JobId,
    request: &Request,
    now: i64,
) -> Result<JobId> {
    request
        .validate(now)
        .map_err(|m| StoreError::InvalidData(m.into()))?;
    // An identical accepted request is idempotent, including a lost reply.
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM recording_jobs WHERE source_id=?1 AND channel_id=?2 AND starts_at=?3
             AND ends_at=?4 AND status IN ('scheduled','recording')
             AND source_incarnation=(SELECT incarnation FROM guide_revisions WHERE source_id=?1)",
            params![request.source, request.channel, request.start, request.end],
            |r| r.get(0),
        )
        .optional()?
    {
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
    Ok(id)
}

fn insert_rule(
    tx: &rusqlite::Transaction<'_>,
    rule: &iptv_core::recording::Rule,
    now: i64,
) -> Result<iptv_core::recording::RuleId> {
    use iptv_core::recording::Repeat;
    rule.request
        .validate(now)
        .map_err(|m| StoreError::InvalidData(m.into()))?;
    let repeat = match rule.repeat {
        Repeat::Daily => "daily",
        Repeat::Weekly => "weekly",
        Repeat::Series => "series",
        Repeat::Once => {
            return Err(StoreError::InvalidData(
                "A repeating rule needs a recurrence".into(),
            ));
        }
    };
    let request = &rule.request;
    let count: i64 = tx.query_row("SELECT COUNT(*) FROM recording_rules", [], |r| r.get(0))?;
    if count >= 100 {
        return Err(StoreError::Unavailable(
            "Remove older recording rules before adding more",
        ));
    }
    let allowed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM channels c LEFT JOIN channel_overlays o
             ON o.source_id=c.source_id AND o.channel_id=c.id LEFT JOIN source_policy p ON p.source_id=c.source_id
             WHERE c.source_id=?1 AND c.id=?2 AND COALESCE(o.blocked,0)=0 AND COALESCE(p.enabled,1)=1)",
            params![request.source, request.channel], |r| r.get(0))?;
    if !allowed {
        return Err(StoreError::Unavailable("Channel unavailable or blocked"));
    }
    let duplicate: Option<i64> = tx.query_row(
            "SELECT id FROM recording_rules WHERE source_id=?1 AND channel_id=?2 AND repeat=?3
             AND local_start=?4 AND title=?5 AND enabled=1 AND source_incarnation=(SELECT incarnation FROM guide_revisions WHERE source_id=?1)",
            params![request.source, request.channel, repeat, rule.local_start, request.title], |r|r.get(0)).optional()?;
    if let Some(id) = duplicate {
        return Ok(iptv_core::recording::RuleId(id));
    }
    tx.execute("INSERT INTO recording_rules(id,source_id,source_incarnation,channel_id,channel_name,title,starts_at,ends_at,local_start,repeat,enabled)
            VALUES(?1,?2,(SELECT incarnation FROM guide_revisions WHERE source_id=?2),?3,?4,?5,?6,?7,?8,?9,1)",
            params![rule.id.0,request.source,request.channel,request.channel_name,request.title,request.start,request.end,rule.local_start,repeat])?;
    Ok(rule.id)
}

impl Store {
    pub fn recording_jobs(&self) -> Result<Vec<Job>> {
        Ok(self
            .conn
            .prepare("SELECT j.*, EXISTS(SELECT 1 FROM guide_revisions r WHERE r.source_id=j.source_id
                AND r.incarnation=j.source_incarnation) FROM recording_jobs j ORDER BY starts_at DESC, id DESC LIMIT 1000")?
            .query_map([], row)?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn recording_job(&self, id: JobId) -> Result<Option<Job>> {
        Ok(self
            .conn
            .query_row(
                "SELECT j.*, EXISTS(SELECT 1 FROM guide_revisions r WHERE r.source_id=j.source_id
                AND r.incarnation=j.source_incarnation) FROM recording_jobs j WHERE id=?1",
                [id.0],
                row,
            )
            .optional()?)
    }
    pub fn schedule_recording(&mut self, id: JobId, request: &Request, now: i64) -> Result<JobId> {
        let tx = self.conn.transaction()?;
        let id = insert_job(&tx, id, request, now)?;
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
            WHERE id=?3 AND source_id=?4 AND channel_id=?5 AND status='scheduled'
            AND source_incarnation=(SELECT incarnation FROM guide_revisions WHERE source_id=?4)",
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
        self.conn.execute("DELETE FROM recording_rules", [])?;
        self.conn.execute("DELETE FROM recording_jobs", [])?;
        Ok(())
    }
}

impl Store {
    pub fn recording_progress_points(
        &self,
    ) -> Result<Vec<(JobId, iptv_core::on_demand::Progress)>> {
        Ok(self.conn.prepare("SELECT recording_id,position_ms,duration_ms,completed FROM recording_playback_progress LIMIT 1000")?
            .query_map([], |row| Ok((JobId(row.get(0)?), iptv_core::on_demand::Progress {
                position_ms: row.get::<_, i64>(1)? as u64,
                duration_ms: row.get::<_, Option<i64>>(2)?.map(|ms| ms as u64),
                completed: row.get(3)?,
            })))?.collect::<rusqlite::Result<_>>()?)
    }
    pub fn recording_progress(&self, id: JobId) -> Result<iptv_core::on_demand::Progress> {
        Ok(self.conn.query_row(
            "SELECT position_ms,duration_ms,completed FROM recording_playback_progress WHERE recording_id=?1",
            [id.0], |row| Ok(iptv_core::on_demand::Progress {
                position_ms: row.get::<_, i64>(0)? as u64,
                duration_ms: row.get::<_, Option<i64>>(1)?.map(|n| n as u64),
                completed: row.get(2)?,
            }),
        ).optional()?.unwrap_or_default())
    }
    pub fn save_recording_progress(
        &mut self,
        id: JobId,
        point: iptv_core::on_demand::Progress,
        now: i64,
    ) -> Result<()> {
        let point = iptv_core::on_demand::Progress::observed(
            point.position_ms,
            point.duration_ms,
            point.completed,
        );
        self.conn.execute(
            "INSERT INTO recording_playback_progress(recording_id,position_ms,duration_ms,completed,updated_at)
             SELECT id,?2,?3,?4,?5 FROM recording_jobs WHERE id=?1 AND status IN ('completed','partial') AND bytes>0
             ON CONFLICT(recording_id) DO UPDATE SET position_ms=excluded.position_ms,
                 duration_ms=excluded.duration_ms,completed=excluded.completed,updated_at=excluded.updated_at
             WHERE position_ms<>excluded.position_ms OR duration_ms IS NOT excluded.duration_ms OR completed<>excluded.completed",
            params![id.0, point.position_ms.min(i64::MAX as u64) as i64,
                point.duration_ms.map(|ms| ms.min(i64::MAX as u64) as i64), point.completed, now.max(0)],
        )?;
        Ok(())
    }
    /// Persist recurrence only for a currently available, unblocked channel.
    pub fn schedule_recording_rule(
        &mut self,
        rule: &iptv_core::recording::Rule,
        now: i64,
    ) -> Result<iptv_core::recording::RuleId> {
        let tx = self.conn.transaction()?;
        let id = insert_rule(&tx, rule, now)?;
        tx.commit()?;
        Ok(id)
    }
    pub fn schedule_recording_rule_and_first(
        &mut self,
        rule: &iptv_core::recording::Rule,
        job: JobId,
        now: i64,
    ) -> Result<JobId> {
        let tx = self.conn.transaction()?;
        let id = insert_rule(&tx, rule, now)?;
        let job = insert_job(&tx, job, &rule.request, now)?;
        tx.execute(
            "INSERT OR IGNORE INTO recording_rule_jobs(job_id,rule_id,starts_at) VALUES(?1,?2,?3)",
            params![job.0, id.0, rule.request.start],
        )?;
        tx.commit()?;
        Ok(job)
    }

    pub fn recording_rules(&self) -> Result<Vec<iptv_core::recording::Rule>> {
        use iptv_core::recording::{Repeat, Rule, RuleId};
        Ok(self.conn.prepare("SELECT q.id,q.source_id,q.channel_id,q.channel_name,q.title,q.starts_at,q.ends_at,q.local_start,q.repeat,
             q.enabled AND EXISTS(SELECT 1 FROM guide_revisions r WHERE r.source_id=q.source_id AND r.incarnation=q.source_incarnation)
             FROM recording_rules q ORDER BY q.id LIMIT 100")?.query_map([], |r| Ok(Rule {
                id: RuleId(r.get(0)?), request: Request {source:r.get(1)?,channel:r.get(2)?,channel_name:r.get(3)?,title:r.get(4)?,start:r.get(5)?,end:r.get(6)?},
                local_start:r.get(7)?, repeat:match r.get::<_,String>(8)?.as_str(){"daily"=>Repeat::Daily,"weekly"=>Repeat::Weekly,_=>Repeat::Series},enabled:r.get(9)?,
             }))?.collect::<rusqlite::Result<_>>()?)
    }

    /// A cancelled occurrence remains remembered, so refresh cannot recreate it.
    pub fn recording_rule_has_occurrence(
        &self,
        rule: iptv_core::recording::RuleId,
        start: i64,
    ) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM recording_rule_jobs WHERE rule_id=?1 AND starts_at=?2)",
            params![rule.0, start],
            |r| r.get(0),
        )?)
    }

    /// Job insertion and occurrence acknowledgement commit together. Disabled or
    /// replaced sources cannot race a materializer into rearming intent.
    pub fn materialize_recording_rule(
        &mut self,
        rule: iptv_core::recording::RuleId,
        id: JobId,
        request: &Request,
        now: i64,
    ) -> Result<Option<JobId>> {
        let tx = self.conn.transaction()?;
        let allowed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM recording_rules q JOIN guide_revisions r
            ON r.source_id=q.source_id AND r.incarnation=q.source_incarnation
            WHERE q.id=?1 AND q.enabled=1 AND q.source_id=?2 AND q.channel_id=?3)",
            params![rule.0, request.source, request.channel],
            |r| r.get(0),
        )?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM recording_rule_jobs WHERE rule_id=?1 AND starts_at=?2)",
            params![rule.0, request.start],
            |r| r.get(0),
        )?;
        if !allowed || exists {
            return Ok(None);
        }
        let id = insert_job(&tx, id, request, now)?;
        tx.execute(
            "INSERT INTO recording_rule_jobs(job_id,rule_id,starts_at) VALUES(?1,?2,?3)",
            params![id.0, rule.0, request.start],
        )?;
        tx.commit()?;
        Ok(Some(id))
    }

    pub fn attach_recording_rule_job(
        &mut self,
        rule: iptv_core::recording::RuleId,
        job: JobId,
        start: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO recording_rule_jobs(job_id,rule_id,starts_at) VALUES(?1,?2,?3)",
            params![job.0, rule.0, start],
        )?;
        Ok(())
    }

    pub fn set_recording_rule_enabled(
        &mut self,
        rule: iptv_core::recording::RuleId,
        enabled: bool,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE recording_rules SET enabled=?1 WHERE id=?2",
            params![enabled, rule.0],
        )?;
        if !enabled {
            tx.execute("UPDATE recording_jobs SET status='cancelled',message='Cancelled' WHERE status='scheduled'
                AND id IN (SELECT job_id FROM recording_rule_jobs WHERE rule_id=?1)",[rule.0])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn recording_rule_for_job(
        &self,
        job: JobId,
    ) -> Result<Option<iptv_core::recording::RuleId>> {
        Ok(self
            .conn
            .query_row(
                "SELECT j.rule_id FROM recording_rule_jobs j JOIN recording_rules q ON q.id=j.rule_id WHERE j.job_id=?1 AND q.enabled=1",
                [job.0],
                |r| r.get(0),
            )
            .optional()?
            .map(iptv_core::recording::RuleId))
    }

    /// Title matching uses only the configured channel's effective guide binding.
    pub fn recording_series_windows(
        &self,
        rule: &iptv_core::recording::Rule,
        now: i64,
    ) -> Result<Vec<(i64, i64)>> {
        Ok(self.conn.prepare("SELECT p.start,p.stop FROM epg_programmes p JOIN epg_bindings b ON b.epg_channel_id=p.channel_id
            WHERE b.source_id=?1 AND b.channel_key=?2 AND p.title=?3 AND p.start>=?4 AND p.start<?5 ORDER BY p.start LIMIT 64")?
            .query_map(params![rule.request.source,rule.request.channel,rule.request.title,now,now.saturating_add(14*86400)],|r|Ok((r.get(0)?,r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }
}
