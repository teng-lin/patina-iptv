//! Materialize durable local-time or EPG rules into ordinary bounded jobs.
use super::*;
use chrono::{DateTime, Local, TimeZone, Utc};
use iptv_core::recording::{Repeat, Rule, RuleId};

fn identity() -> Result<i64, Error> {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes)
        .map_err(|_| Error::unavailable("Recording identity unavailable"))?;
    Ok((i64::from_ne_bytes(bytes) & i64::MAX).max(1))
}
fn local_timestamp(utc: i64) -> Option<i64> {
    Some(
        DateTime::from_timestamp(utc, 0)?
            .with_timezone(&Local)
            .naive_local()
            .and_utc()
            .timestamp(),
    )
}
fn utc_timestamp(local: i64) -> Option<i64> {
    let local = DateTime::<Utc>::from_timestamp(local, 0)?.naive_utc();
    // A repeated fall-back hour is recorded once, using its earlier instant.
    // A spring-forward gap is skipped rather than silently changing its hour.
    Local
        .from_local_datetime(&local)
        .earliest()
        .map(|v| v.timestamp())
}
impl Runtime {
    pub async fn schedule_repeating_recording(
        &self,
        request: Request,
        repeat: Repeat,
    ) -> Result<JobId, Error> {
        if repeat == Repeat::Once {
            return self.schedule_recording(request).await;
        }
        self.parental_check(&iptv_core::parental::Target::live(
            request.source,
            request.channel.clone(),
        ))
        .await
        .map_err(|_| Error::unavailable("Parental access is locked"))?;
        request.validate(now()).map_err(Error::unavailable)?;
        let rule = Rule {
            id: RuleId(identity()?),
            local_start: local_timestamp(request.start)
                .ok_or_else(|| Error::unavailable("Recording time unavailable"))?,
            request: request.clone(),
            repeat,
            enabled: true,
        };
        self.sync_recording_contexts();
        self.start_recordings();
        let job = JobId(identity()?);
        // The first occurrence and the recurring intent are one acknowledgement.
        let first = self
            .services()
            .call(move |store| store.schedule_recording_rule_and_first(&rule, job, now()))
            .await?;
        self.acknowledge_recording(first).await?;
        // Later occurrences may be delayed by storage quotas; accepting the
        // first job must not report failure after its atomic commit.
        let _ = materialize(&self.services()).await;
        Ok(first)
    }
    pub async fn recording_rules(&self) -> Result<Vec<Rule>, Error> {
        self.services()
            .read(|store| store.recording_rules())
            .await
            .map_err(Into::into)
    }
    pub async fn set_recording_rule_enabled(&self, id: RuleId, enabled: bool) -> Result<(), Error> {
        self.services()
            .call(move |store| store.set_recording_rule_enabled(id, enabled))
            .await?;
        Ok(())
    }
    pub async fn stop_repeating_recording(&self, job: JobId) -> Result<(), Error> {
        let id = self
            .services()
            .read(move |store| store.recording_rule_for_job(job))
            .await?
            .ok_or_else(|| Error::unavailable("This recording does not repeat"))?;
        self.set_recording_rule_enabled(id, false).await
    }
    pub async fn repeating_recording_jobs(&self) -> Result<Vec<JobId>, Error> {
        self.services()
            .read(|store| {
                store
                    .recording_jobs()?
                    .into_iter()
                    .filter_map(|job| match store.recording_rule_for_job(job.id) {
                        Ok(Some(_)) => Some(Ok(job.id)),
                        Ok(None) => None,
                        Err(error) => Some(Err(error)),
                    })
                    .collect()
            })
            .await
            .map_err(Into::into)
    }
}

pub(super) async fn materialize(services: &Services) -> Result<(), Error> {
    let current = now();
    let rules = services.read(|store| store.recording_rules()).await?;
    for rule in rules.into_iter().filter(|rule| rule.enabled) {
        let windows = if rule.repeat == Repeat::Series {
            let query = rule.clone();
            services
                .read(move |store| store.recording_series_windows(&query, current))
                .await?
        } else {
            let mut windows = Vec::new();
            let mut cursor = local_timestamp(current).unwrap_or(current);
            for _ in 0..9 {
                let Some((start, end)) = rule.local_window(cursor) else {
                    break;
                };
                if let (Some(start), Some(end)) = (utc_timestamp(start), utc_timestamp(end)) {
                    if start > current + 8 * 86400 {
                        break;
                    }
                    if end > current {
                        windows.push((start, end));
                    }
                }
                cursor = end;
            }
            windows
        };
        for (start, end) in windows {
            let id = rule.id;
            if services
                .read(move |store| store.recording_rule_has_occurrence(id, start))
                .await?
            {
                continue;
            }
            let mut request = rule.request.clone();
            request.start = start;
            request.end = end;
            if request.validate(current).is_err() {
                continue;
            }
            let job = JobId(identity()?);
            services
                .call(move |store| {
                    store
                        .materialize_recording_rule(id, job, &request, current)
                        .map(|_| ())
                })
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_time_conversion_roundtrips_an_existing_instant() {
        let timestamp = 1_799_000_000;
        assert_eq!(
            utc_timestamp(local_timestamp(timestamp).unwrap()),
            Some(timestamp)
        );
    }
}
