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
        // Schema 25 rewrote the stored 'cancelled'; the CHECK admits no other.
        "canceled" => Status::Canceled,
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
        Status::Canceled => "canceled",
        Status::Missed => "missed",
    }
}

fn cancel_rule(tx: &rusqlite::Transaction<'_>, rule: iptv_core::recording::RuleId) -> Result<()> {
    tx.execute("UPDATE recording_rules SET enabled=0 WHERE id=?1", [rule.0])?;
    tx.execute(
        "UPDATE recording_jobs SET status='canceled',message='Recurring schedule disabled'
        WHERE status='scheduled' AND id IN (SELECT job_id FROM recording_rule_jobs WHERE rule_id=?1)
        AND NOT EXISTS(SELECT 1 FROM recording_rule_jobs j JOIN recording_rules q ON q.id=j.rule_id
            WHERE j.job_id=recording_jobs.id AND q.enabled=1)",
        [rule.0],
    )?;
    tx.execute("DELETE FROM recording_rules WHERE id=?1", [rule.0])?;
    Ok(())
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: JobId(r.get(0)?),
        source_current: r.get(11)?,
        series_start: r.get(12)?,
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
// Raw callers have no in-memory parental grant. Checked callers must establish
// that authority inside this same transaction; accepted recurrence keeps it.
fn require_channel(conn: &rusqlite::Connection, request: &Request, authorized: bool) -> Result<()> {
    let allowed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM channels c JOIN sources s ON s.id=c.source_id
         LEFT JOIN channel_overlays o ON o.source_id=c.source_id AND o.channel_id=c.id
         LEFT JOIN source_policy p ON p.source_id=c.source_id
         WHERE c.source_id=?1 AND c.id=?2 AND (?3 OR COALESCE(o.blocked,0)=0)
         AND COALESCE(p.enabled,1)=1)",
        params![request.source, request.channel, authorized],
        |r| r.get(0),
    )?;
    if !allowed {
        return Err(StoreError::Unavailable("Channel unavailable or blocked"));
    }
    Ok(())
}

fn insert_job(
    tx: &rusqlite::Transaction<'_>,
    id: JobId,
    request: &Request,
    now: i64,
    authorized: bool,
) -> Result<JobId> {
    require_channel(tx, request, authorized)?;
    request
        .validate(now)
        .map_err(|_| StoreError::InvalidData("recording request refused".into()))?;
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
    tx.execute("INSERT INTO recording_jobs(source_id,channel_id,channel_name,title,starts_at,ends_at,status,id,source_incarnation)
            VALUES(?1,?2,?3,?4,?5,?6,'scheduled',?7,(SELECT incarnation FROM guide_revisions WHERE source_id=?1))",
            params![request.source, request.channel, request.channel_name, request.title, request.start, request.end, id.0])?;
    Ok(id)
}

fn insert_rule(
    tx: &rusqlite::Transaction<'_>,
    rule: &iptv_core::recording::Rule,
    now: i64,
    authorized: bool,
) -> Result<iptv_core::recording::RuleId> {
    use iptv_core::recording::Repeat;
    rule.request
        .validate(now)
        .map_err(|_| StoreError::InvalidData("recording request refused".into()))?;
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
    if rule.lead_seconds > 3600 || rule.trail_seconds > 3600 {
        return Err(StoreError::InvalidData(
            "Recording padding is too long".into(),
        ));
    }
    require_channel(tx, request, authorized)?;
    let duplicate: Option<i64> = tx
        .query_row(
            "SELECT id FROM recording_rules WHERE source_id=?1 AND channel_id=?2 AND repeat=?3
             AND local_start=?4 AND title=?5 AND enabled=1 AND lead_seconds=?6 AND trail_seconds=?7
             AND ends_at-starts_at=?8
             AND source_incarnation=(SELECT incarnation FROM guide_revisions WHERE source_id=?1)",
            params![
                request.source,
                request.channel,
                repeat,
                rule.local_start,
                request.title,
                rule.lead_seconds,
                rule.trail_seconds,
                request.end - request.start
            ],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = duplicate {
        return Ok(iptv_core::recording::RuleId(id));
    }
    let count: i64 = tx.query_row("SELECT COUNT(*) FROM recording_rules", [], |r| r.get(0))?;
    if count >= 100 {
        return Err(StoreError::Unavailable(
            "Remove older recording rules before adding more",
        ));
    }
    tx.execute("INSERT INTO recording_rules(id,source_id,source_incarnation,channel_id,channel_name,title,starts_at,ends_at,local_start,repeat,enabled,lead_seconds,trail_seconds)
            VALUES(?1,?2,(SELECT incarnation FROM guide_revisions WHERE source_id=?2),?3,?4,?5,?6,?7,?8,?9,1,?10,?11)",
            params![rule.id.0,request.source,request.channel,request.channel_name,request.title,request.start,request.end,rule.local_start,repeat,rule.lead_seconds,rule.trail_seconds])?;
    Ok(rule.id)
}

impl Store {
    pub fn recording_jobs(&self) -> Result<Vec<Job>> {
        Ok(self
            .conn
            .prepare("SELECT j.*, EXISTS(SELECT 1 FROM guide_revisions r WHERE r.source_id=j.source_id
                AND r.incarnation=j.source_incarnation),
                (SELECT MIN(rj.starts_at) FROM recording_rule_jobs rj JOIN recording_rules q ON q.id=rj.rule_id
                 WHERE rj.job_id=j.id AND q.repeat='series')
                FROM recording_jobs j ORDER BY starts_at DESC, id DESC LIMIT 1000")?
            .query_map([], row)?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn recording_job(&self, id: JobId) -> Result<Option<Job>> {
        Ok(self
            .conn
            .query_row(
                "SELECT j.*, EXISTS(SELECT 1 FROM guide_revisions r WHERE r.source_id=j.source_id
                AND r.incarnation=j.source_incarnation),
                (SELECT MIN(rj.starts_at) FROM recording_rule_jobs rj JOIN recording_rules q ON q.id=rj.rule_id
                 WHERE rj.job_id=j.id AND q.repeat='series')
                FROM recording_jobs j WHERE id=?1",
                [id.0],
                row,
            )
            .optional()?)
    }
    pub fn schedule_recording(&mut self, id: JobId, request: &Request, now: i64) -> Result<JobId> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let id = insert_job(&tx, id, request, now, false)?;
        tx.commit()?;
        Ok(id)
    }
    /// Authorize against this writer snapshot before accepting even an idempotent retry.
    /// The callback must validate the current owner, original source, and parental grant.
    pub fn schedule_recording_checked(
        &mut self,
        id: JobId,
        request: &Request,
        now: i64,
        authorize: impl FnOnce(&Store) -> Result<()>,
    ) -> Result<JobId> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        authorize(self)?;
        let id = insert_job(&tx, id, request, now, true)?;
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
        self.update_recording_authorized(id, request, now, false, |_| Ok(()))
    }
    /// The callback runs after acquiring the writer transaction, before the edit.
    pub fn update_recording_checked(
        &mut self,
        id: JobId,
        request: &Request,
        now: i64,
        authorize: impl FnOnce(&Store) -> Result<()>,
    ) -> Result<bool> {
        self.update_recording_authorized(id, request, now, true, authorize)
    }
    fn update_recording_authorized(
        &mut self,
        id: JobId,
        request: &Request,
        now: i64,
        authorized: bool,
        authorize: impl FnOnce(&Store) -> Result<()>,
    ) -> Result<bool> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        authorize(self)?;
        require_channel(&tx, request, authorized)?;
        request
            .validate(now)
            .map_err(|_| StoreError::InvalidData("recording request refused".into()))?;
        let changed = tx.execute(
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
        )? == 1;
        tx.commit()?;
        Ok(changed)
    }
    pub fn remove_recording(&mut self, id: JobId) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM recording_jobs WHERE id=?1 AND status NOT IN ('scheduled','recording')",
            [id.0],
        )? == 1)
    }
    /// Restored schedules must never rearm on a different host; media is not in catalog backups.
    pub fn discard_recording_jobs(&mut self) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM recording_rules", [])?;
        tx.execute("DELETE FROM recording_jobs", [])?;
        tx.execute("DELETE FROM retired_recording_schedules", [])?;
        tx.execute(
            "DELETE FROM store_meta WHERE key='retired_recording_notice'",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Replaces this library's recording rows with those of `from`, the
    /// host's own library: its media lives beside the catalog directory
    /// under these job ids, so a restored catalog keeps reopening it.
    /// Repeating rules are not carried; scheduling is retired.
    pub fn adopt_host_recordings(&mut self, from: &std::path::Path) -> Result<()> {
        self.adopt_recordings(from, None)
    }
    /// [`Self::adopt_host_recordings`] for a host library encrypted with `key`.
    pub fn adopt_host_recordings_encrypted(
        &mut self,
        from: &std::path::Path,
        key: &crate::DatabaseKey,
    ) -> Result<()> {
        self.adopt_recordings(from, Some(key))
    }
    fn adopt_recordings(
        &mut self,
        from: &std::path::Path,
        key: Option<&crate::DatabaseKey>,
    ) -> Result<()> {
        // Opening migrates the host library first, so both files have this
        // build's tables and nothing scheduled survives into the copy.
        let host = match key {
            Some(key) => Store::open_encrypted(from, key)?,
            None => Store::open(from)?,
        };
        // Validate every row through typed decoders before copying raw SQLite
        // values so corrupt B-tree cells cannot poison a pending restore.
        let _ = host.recording_jobs()?;
        let _ = host.recording_progress_points()?;
        let _ = host.retired_recording_schedules()?;
        let completions: Vec<(i64, String, String, i64, String, String)> = host
            .conn
            .prepare(
                "SELECT job_id,source_incarnation,outcome,bytes,digest,message
                 FROM recording_completions LIMIT 1000",
            )?
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (_, _, outcome, bytes, _, _) in &completions {
            if !matches!(outcome.as_str(), "completed" | "partial" | "failed") || *bytes < 0 {
                return Err(StoreError::InvalidData(
                    "invalid recording completion row".into(),
                ));
            }
        }
        let _: Option<String> = host
            .conn
            .query_row(
                "SELECT value FROM store_meta WHERE key='retired_recording_notice'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        // A scheduled row with media is an interrupted capture resuming.
        const KEPT: &str = "status<>'scheduled' OR bytes>0";
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // Children first, so this does not depend on foreign-key cascades.
        for table in [
            "recording_playback_progress",
            "recording_completions",
            "recording_rule_jobs",
            "recording_rules",
            "recording_jobs",
            "retired_recording_schedules",
        ] {
            tx.execute(&format!("DELETE FROM {table}"), [])?;
        }
        tx.execute(
            "DELETE FROM store_meta WHERE key='retired_recording_notice'",
            [],
        )?;
        let kept_job = format!("job_id IN (SELECT id FROM recording_jobs WHERE {KEPT})");
        for (table, filter) in [
            ("recording_jobs", KEPT.to_owned()),
            (
                "recording_playback_progress",
                kept_job.replace("job_id", "recording_id"),
            ),
            ("recording_completions", kept_job),
            ("retired_recording_schedules", "1".to_owned()),
            ("store_meta", "key='retired_recording_notice'".to_owned()),
        ] {
            copy_rows(&host.conn, &tx, table, &filter)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The schedules the upgrade that retired scheduled recording canceled,
    /// until the viewer has seen them.
    pub fn retired_recording_schedules(
        &self,
    ) -> Result<Vec<iptv_core::recording::RetiredSchedule>> {
        use iptv_core::recording::{Repeat, RetiredSchedule};
        Ok(self
            .conn
            .prepare(
                "SELECT channel_name,title,starts_at,ends_at,repeat
                 FROM retired_recording_schedules ORDER BY id",
            )?
            .query_map([], |r| {
                Ok(RetiredSchedule {
                    channel_name: r.get(0)?,
                    title: r.get(1)?,
                    start: r.get(2)?,
                    end: r.get(3)?,
                    repeat: match r.get::<_, String>(4)?.as_str() {
                        "daily" => Repeat::Daily,
                        "weekly" => Repeat::Weekly,
                        "series" => Repeat::Series,
                        _ => Repeat::Once,
                    },
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    /// Whether the viewer has yet to be told that scheduled recordings were
    /// removed. Marks it told, so the notice appears once.
    pub fn announce_retired_recordings(&mut self) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE store_meta SET value='announced'
             WHERE key='retired_recording_notice' AND value='pending'",
            [],
        )? == 1)
    }
    /// The viewer has seen the list of canceled schedules: forget it. The
    /// canceled jobs themselves remain in the library with their media.
    pub fn dismiss_retired_recording_schedules(&mut self) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM retired_recording_schedules", [])?;
        tx.execute(
            "DELETE FROM store_meta WHERE key='retired_recording_notice'",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }
}

/// Copies `table`'s rows matching `filter` between two libraries of this
/// build's schema, by column name.
fn copy_rows(
    from: &rusqlite::Connection,
    to: &rusqlite::Transaction<'_>,
    table: &str,
    filter: &str,
) -> Result<()> {
    let mut query = from.prepare(&format!("SELECT * FROM {table} WHERE {filter}"))?;
    let columns = query.column_count();
    let names = query
        .column_names()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(",");
    let values = (1..=columns)
        .map(|at| format!("?{at}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut insert = to.prepare(&format!("INSERT INTO {table}({names}) VALUES({values})"))?;
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        let values = (0..columns)
            .map(|at| row.get::<_, rusqlite::types::Value>(at))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        insert.execute(rusqlite::params_from_iter(values))?;
    }
    Ok(())
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
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let id = insert_rule(&tx, rule, now, false)?;
        tx.commit()?;
        Ok(id)
    }
    pub fn schedule_recording_rule_and_first(
        &mut self,
        rule: &iptv_core::recording::Rule,
        job: JobId,
        now: i64,
    ) -> Result<JobId> {
        self.schedule_recording_rule_and_first_at(rule, job, rule.request.start, now)
    }

    pub fn schedule_recording_rule_and_first_at(
        &mut self,
        rule: &iptv_core::recording::Rule,
        job: JobId,
        occurrence_start: i64,
        now: i64,
    ) -> Result<JobId> {
        self.schedule_recording_rule_with_first(rule, job, &rule.request, occurrence_start, now)
    }

    pub fn schedule_recording_rule_with_first(
        &mut self,
        rule: &iptv_core::recording::Rule,
        job: JobId,
        first: &Request,
        occurrence_start: i64,
        now: i64,
    ) -> Result<JobId> {
        self.schedule_recording_rule_with_first_authorized(
            rule,
            job,
            first,
            occurrence_start,
            now,
            None::<fn(&Store) -> Result<()>>,
        )
    }
    /// The callback validates current authority in the transaction that accepts both intents.
    pub fn schedule_recording_rule_with_first_checked(
        &mut self,
        rule: &iptv_core::recording::Rule,
        job: JobId,
        first: &Request,
        occurrence_start: i64,
        now: i64,
        authorize: impl FnOnce(&Store) -> Result<()>,
    ) -> Result<JobId> {
        self.schedule_recording_rule_with_first_authorized(
            rule,
            job,
            first,
            occurrence_start,
            now,
            Some(authorize),
        )
    }
    fn schedule_recording_rule_with_first_authorized(
        &mut self,
        rule: &iptv_core::recording::Rule,
        job: JobId,
        first: &Request,
        occurrence_start: i64,
        now: i64,
        authorize: Option<impl FnOnce(&Store) -> Result<()>>,
    ) -> Result<JobId> {
        if occurrence_start < 0
            || first.source != rule.request.source
            || first.channel != rule.request.channel
        {
            return Err(StoreError::InvalidData(
                "Recording occurrence does not match its rule".into(),
            ));
        }
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let authorized = authorize.is_some();
        if let Some(authorize) = authorize {
            authorize(self)?;
        }
        let id = insert_rule(&tx, rule, now, authorized)?;
        if let Some(previous) = tx
            .query_row(
                "SELECT job_id FROM recording_rule_jobs WHERE rule_id=?1 AND starts_at=?2",
                params![id.0, occurrence_start],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            let retained: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM recording_jobs WHERE id=?1)",
                [previous],
                |row| row.get(0),
            )?;
            if !retained {
                return Err(StoreError::Unavailable(
                    "This recording occurrence was already removed",
                ));
            }
            tx.commit()?;
            return Ok(JobId(previous));
        }
        let job = insert_job(&tx, job, first, now, authorized)?;
        let previous: Option<i64> = tx
            .query_row(
                "SELECT rule_id FROM recording_rule_jobs WHERE job_id=?1",
                [job.0],
                |row| row.get(0),
            )
            .optional()?;
        if previous.is_some_and(|previous| previous != id.0) {
            return Err(StoreError::Unavailable(
                "This recording already belongs to another repeating schedule",
            ));
        }
        tx.execute(
            "INSERT OR IGNORE INTO recording_rule_jobs(job_id,rule_id,starts_at) VALUES(?1,?2,?3)",
            params![job.0, id.0, occurrence_start],
        )?;
        tx.commit()?;
        Ok(job)
    }

    pub fn recording_rules(&self) -> Result<Vec<iptv_core::recording::Rule>> {
        use iptv_core::recording::{Repeat, Rule, RuleId};
        Ok(self.conn.prepare("SELECT q.id,q.source_id,q.channel_id,q.channel_name,q.title,q.starts_at,q.ends_at,q.local_start,q.repeat,
             q.enabled AND EXISTS(SELECT 1 FROM guide_revisions r WHERE r.source_id=q.source_id AND r.incarnation=q.source_incarnation),q.lead_seconds,q.trail_seconds
             FROM recording_rules q ORDER BY q.id LIMIT 100")?.query_map([], |r| Ok(Rule {
                id: RuleId(r.get(0)?), request: Request {source:r.get(1)?,channel:r.get(2)?,channel_name:r.get(3)?,title:r.get(4)?,start:r.get(5)?,end:r.get(6)?},
                local_start:r.get(7)?, repeat:match r.get::<_,String>(8)?.as_str(){"daily"=>Repeat::Daily,"weekly"=>Repeat::Weekly,_=>Repeat::Series},enabled:r.get(9)?,
                lead_seconds:r.get(10)?,trail_seconds:r.get(11)?,
             }))?.collect::<rusqlite::Result<_>>()?)
    }

    /// A canceled occurrence remains remembered, so refresh cannot recreate it.
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
        self.materialize_recording_rule_at(rule, id, request, request.start, now)
    }

    pub fn materialize_recording_rule_at(
        &mut self,
        rule: iptv_core::recording::RuleId,
        id: JobId,
        request: &Request,
        occurrence_start: i64,
        now: i64,
    ) -> Result<Option<JobId>> {
        self.materialize_recording_rule_at_with_offset(
            rule,
            id,
            request,
            occurrence_start,
            now,
            None,
        )
    }

    /// Rejects materialization from an older group guide projection.
    pub fn materialize_recording_rule_at_with_offset(
        &mut self,
        rule: iptv_core::recording::RuleId,
        id: JobId,
        request: &Request,
        occurrence_start: i64,
        now: i64,
        expected_offset: Option<i64>,
    ) -> Result<Option<JobId>> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        if let Some(offset) = expected_offset
            && self
                .channel_guide_offset_seconds(crate::SourceId(request.source), &request.channel)?
                != offset
        {
            return Ok(None);
        }
        let allowed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM recording_rules q JOIN guide_revisions r
            ON r.source_id=q.source_id AND r.incarnation=q.source_incarnation
            WHERE q.id=?1 AND q.enabled=1 AND q.source_id=?2 AND q.channel_id=?3)",
            params![rule.0, request.source, request.channel],
            |r| r.get(0),
        )?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM recording_rule_jobs WHERE rule_id=?1 AND starts_at=?2)",
            params![rule.0, occurrence_start],
            |r| r.get(0),
        )?;
        if !allowed || exists {
            return Ok(None);
        }
        let id = insert_job(&tx, id, request, now, true)?;
        tx.execute(
            "INSERT INTO recording_rule_jobs(job_id,rule_id,starts_at) VALUES(?1,?2,?3)",
            params![id.0, rule.0, occurrence_start],
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
        now: i64,
    ) -> Result<()> {
        self.set_recording_rule_enabled_authorized(rule, enabled, now, false, |_| Ok(()))
    }
    /// Disabling remains unconditional; re-enabling requires the callback's current authority.
    pub fn set_recording_rule_enabled_checked(
        &mut self,
        rule: iptv_core::recording::RuleId,
        enabled: bool,
        now: i64,
        authorize: impl FnOnce(&Store) -> Result<()>,
    ) -> Result<()> {
        self.set_recording_rule_enabled_authorized(rule, enabled, now, true, authorize)
    }
    fn set_recording_rule_enabled_authorized(
        &mut self,
        rule: iptv_core::recording::RuleId,
        enabled: bool,
        now: i64,
        authorized: bool,
        authorize: impl FnOnce(&Store) -> Result<()>,
    ) -> Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        if enabled {
            authorize(self)?;
            let allowed: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM recording_rules q JOIN guide_revisions r
                 ON r.source_id=q.source_id AND r.incarnation=q.source_incarnation
                 JOIN channels c ON c.source_id=q.source_id AND c.id=q.channel_id
                 LEFT JOIN channel_overlays o ON o.source_id=c.source_id AND o.channel_id=c.id
                 LEFT JOIN source_policy p ON p.source_id=c.source_id
                 WHERE q.id=?1 AND (?2 OR COALESCE(o.blocked,0)=0) AND COALESCE(p.enabled,1)=1)",
                params![rule.0, authorized],
                |r| r.get(0),
            )?;
            if !allowed {
                return Err(StoreError::Unavailable(
                    "Recording rule unavailable or blocked",
                ));
            }
        }
        tx.execute(
            "UPDATE recording_rules SET enabled=?1 WHERE id=?2",
            params![enabled, rule.0],
        )?;
        if enabled {
            // Only a rule pause can be undone. Manual cancellation and deleted
            // occurrence tombstones keep their original authority.
            tx.execute("UPDATE recording_jobs SET status='scheduled',message=''
                WHERE status='canceled' AND message='Recurring schedule disabled' AND ends_at>?2
                AND id IN (SELECT job_id FROM recording_rule_jobs WHERE rule_id=?1)
                AND source_incarnation=(SELECT incarnation FROM guide_revisions WHERE source_id=recording_jobs.source_id)",[rule.0,now])?;
        } else {
            tx.execute("UPDATE recording_jobs SET status='canceled',message='Recurring schedule disabled'
                WHERE status='scheduled' AND id IN (SELECT job_id FROM recording_rule_jobs WHERE rule_id=?1)
                AND NOT EXISTS(SELECT 1 FROM recording_rule_jobs j JOIN recording_rules q ON q.id=j.rule_id
                    WHERE j.job_id=recording_jobs.id AND q.enabled=1)",[rule.0])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Permanent cancellation removes the rule's authority, while retaining all
    /// media/history. Pausing via set_recording_rule_enabled retains tombstones.
    pub fn cancel_recording_rule(&mut self, rule: iptv_core::recording::RuleId) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        cancel_rule(&tx, rule)?;
        tx.commit()?;
        Ok(())
    }

    /// The UI's cancellation applies to every recurring intent attached to the
    /// selected recording, rather than choosing an arbitrary rule after overlap.
    pub fn cancel_recording_rules_for_job(&mut self, job: JobId) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let rules = tx
            .prepare("SELECT rule_id FROM recording_rule_jobs WHERE job_id=?1")?
            .query_map([job.0], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for rule in &rules {
            cancel_rule(&tx, iptv_core::recording::RuleId(*rule))?;
        }
        tx.commit()?;
        Ok(!rules.is_empty())
    }

    pub fn recording_rule_for_job(
        &self,
        job: JobId,
    ) -> Result<Option<iptv_core::recording::RuleId>> {
        Ok(self
            .conn
            .query_row(
                "SELECT j.rule_id FROM recording_rule_jobs j JOIN recording_rules q ON q.id=j.rule_id WHERE j.job_id=?1 AND q.enabled=1 ORDER BY j.rule_id LIMIT 1",
                [job.0],
                |r| r.get(0),
            )
            .optional()?
            .map(iptv_core::recording::RuleId))
    }

    /// The same primary provider group used when projecting guide timestamps.
    pub fn channel_guide_offset_seconds(
        &self,
        source: crate::SourceId,
        channel: &str,
    ) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(o.epg_offset_minutes, 0) FROM channels c
             LEFT JOIN groups g ON g.source_id=c.source_id AND g.kind='live'
               AND g.id=json_extract(c.group_ids,'$[0]')
             LEFT JOIN group_overlays o ON o.source_id=c.source_id AND o.group_id=COALESCE(g.id,'__other__')
             WHERE c.source_id=?1 AND c.id=?2",
            params![source.0, channel],
            |row| row.get::<_, i64>(0),
        ).optional()?.unwrap_or(0) * 60)
    }

    /// Title matching uses only the configured channel's effective guide binding.
    pub fn recording_series_windows(
        &self,
        rule: &iptv_core::recording::Rule,
        now: i64,
    ) -> Result<Vec<(i64, i64)>> {
        Ok(self.conn.prepare("SELECT p.start,p.stop FROM epg_programs p JOIN epg_bindings b ON b.epg_channel_id=p.channel_id
            WHERE b.source_id=?1 AND b.channel_key=?2 AND p.title=?3 AND p.stop>?4 AND p.start<?5 ORDER BY p.start LIMIT 64")?
            .query_map(params![rule.request.source,rule.request.channel,rule.request.title,now,now.saturating_add(14*86400)],|r|Ok((r.get(0)?,r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }
}

/// A durable terminal intent for a synced recording spool. Contains no paths or URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingCompletion {
    pub job: JobId,
    pub status: Status,
    pub bytes: u64,
    pub digest: String,
    pub message: String,
}
impl Store {
    /// Record terminal intent before touching the final filename. False means the
    /// accepted job changed or another receipt already won; callers must not overwrite it.
    pub fn prepare_recording_completion(
        &mut self,
        job: &Job,
        completion: &RecordingCompletion,
    ) -> Result<bool> {
        if job.id != completion.job
            || job.status != Status::Recording
            || !matches!(
                completion.status,
                Status::Completed | Status::Partial | Status::Failed
            )
            || completion.bytes > iptv_core::recording::MAX_BYTES
            || completion.digest.len() != 64
            || !completion.digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(StoreError::InvalidData(
                "invalid recording completion".into(),
            ));
        }
        self.durable_recording_write(|| {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO recording_completions(job_id,source_incarnation,outcome,bytes,digest,message)
             SELECT id,source_incarnation,?1,?2,?3,?4 FROM recording_jobs
             WHERE id=?5 AND status='recording' AND source_id=?6 AND channel_id=?7
             AND starts_at=?8 AND ends_at=?9",
            params![stored(completion.status), completion.bytes as i64, completion.digest,
                completion.message, job.id.0, job.request.source, job.request.channel,
                job.request.start, job.request.end],
        )?;
        let matches = self.recording_completion(job.id)?.as_ref() == Some(completion);
        tx.commit()?;
        Ok(matches)
        })
    }

    /// A receipt is usable only for the same accepted job incarnation still Recording.
    pub fn recording_completion(&self, id: JobId) -> Result<Option<RecordingCompletion>> {
        Ok(self
            .conn
            .query_row(
                "SELECT c.outcome,c.bytes,c.digest,c.message FROM recording_completions c
             JOIN recording_jobs j ON j.id=c.job_id AND j.source_incarnation=c.source_incarnation
             WHERE j.id=?1 AND j.status='recording'",
                [id.0],
                |row| {
                    Ok(RecordingCompletion {
                        job: id,
                        status: status(&row.get::<_, String>(0)?),
                        bytes: row.get::<_, i64>(1)? as u64,
                        digest: row.get(2)?,
                        message: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// Reconcile verified retained content. A superseded/deleted job is never overwritten.
    /// Repeating an already committed receipt succeeds without modifying newer state.
    pub fn commit_recording_completion(
        &mut self,
        completion: &RecordingCompletion,
    ) -> Result<bool> {
        self.durable_recording_write(|| {
            let tx = self.conn.unchecked_transaction()?;
            let current = self.recording_completion(completion.job)?;
            let applied = if current.as_ref() == Some(completion) {
                tx.execute(
                    "UPDATE recording_jobs SET status=?1,bytes=?2,message=?3
                WHERE id=?4 AND status='recording'",
                    params![
                        stored(completion.status),
                        completion.bytes as i64,
                        completion.message,
                        completion.job.0
                    ],
                )? == 1
            } else {
                self.recording_job(completion.job)?.is_some_and(|job| {
                    job.status == completion.status
                        && job.bytes == completion.bytes
                        && job.message == completion.message
                })
            };
            tx.commit()?;
            Ok(applied)
        })
    }

    /// Preserve verified spool media as Partial when its final name cannot be
    /// published. This requires the original receipt; newer catalog state wins.
    pub fn retain_partial_recording_completion(
        &mut self,
        completion: &RecordingCompletion,
    ) -> Result<bool> {
        self.durable_recording_write(|| {
            let tx = self.conn.unchecked_transaction()?;
            if self.recording_completion(completion.job)?.as_ref() != Some(completion) {
                return Ok(false);
            }
            let changed = tx.execute(
                "UPDATE recording_jobs SET status='partial',bytes=?1,
                message='Could not finalize the recording; partial media was retained'
                WHERE id=?2 AND status='recording'",
                params![completion.bytes as i64, completion.job.0],
            )? == 1;
            tx.commit()?;
            Ok(changed)
        })
    }

    fn durable_recording_write<T>(&self, write: impl FnOnce() -> Result<T>) -> Result<T> {
        // WAL NORMAL does not guarantee survival of a power loss between rename
        // and the next checkpoint. Completion receipts and acknowledgements do.
        let previous: i64 = self
            .conn
            .pragma_query_value(None, "synchronous", |r| r.get(0))?;
        self.conn.pragma_update(None, "synchronous", "FULL")?;
        let result = write();
        self.conn.pragma_update(None, "synchronous", previous)?;
        result
    }
}
