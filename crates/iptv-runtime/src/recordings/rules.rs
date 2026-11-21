//! Materialize durable local-time or EPG rules into ordinary bounded jobs.
use super::*;
use chrono::{DateTime, Local, TimeZone, Utc};
use iptv_core::recording::{RecurrenceTiming, Repeat, Rule, RuleId};

fn identity(entropy: crate::ports::Entropy) -> Result<i64, Error> {
    let mut bytes = [0; 8];
    entropy(&mut bytes).map_err(|_| Error::unavailable("Recording identity unavailable"))?;
    Ok((i64::from_ne_bytes(bytes) & i64::MAX).max(1))
}
pub(super) fn local_timestamp(utc: i64) -> Option<i64> {
    Some(
        DateTime::from_timestamp(utc, 0)?
            .with_timezone(&Local)
            .naive_local()
            .and_utc()
            .timestamp(),
    )
}
pub(super) fn utc_timestamp(local: i64) -> Option<i64> {
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
        self.schedule_repeating_recording_with_timing(request, repeat, None)
            .await
    }

    pub async fn schedule_repeating_recording_with_timing(
        &self,
        request: Request,
        repeat: Repeat,
        timing: Option<RecurrenceTiming>,
    ) -> Result<JobId, Error> {
        if repeat == Repeat::Once {
            return self.schedule_recording(request).await;
        }
        let admission = self.recording_admission(&request).await?;
        request.validate(now()).map_err(Error::unavailable)?;
        if self.recordings.is_stopping() || !self.services().is_current() {
            return Err(Error::unavailable(
                "Restart Patina before scheduling a recording",
            ));
        }
        if timing.is_some_and(|timing| {
            timing.program_start < 0
                || timing.program_end <= timing.program_start
                || timing.lead_seconds > 3600
                || timing.trail_seconds > 3600
        }) {
            return Err(Error::unavailable("Recording time unavailable"));
        }
        let mut nominal = request.clone();
        if let Some(timing) = timing {
            nominal.start = timing
                .program_start
                .saturating_sub(i64::from(timing.lead_seconds))
                .max(0);
            nominal.end = timing
                .program_end
                .saturating_add(i64::from(timing.trail_seconds));
        }
        nominal.validate(now()).map_err(Error::unavailable)?;
        let mut rule = Rule {
            id: RuleId(identity(self.services().ports().entropy)?),
            local_start: local_timestamp(nominal.start)
                .ok_or_else(|| Error::unavailable("Recording time unavailable"))?,
            request: nominal,
            repeat,
            enabled: true,
            lead_seconds: timing.map_or(0, |timing| timing.lead_seconds),
            trail_seconds: timing.map_or(0, |timing| timing.trail_seconds),
        };
        let occurrence_start = if repeat == Repeat::Series {
            let first_start = if let Some(timing) = timing {
                timing.program_start
            } else {
                // Older API callers still get a stable identity for a currently
                // airing episode whose record-now capture begins at `now`.
                let query = rule.clone();
                self.services()
                    .read(move |store| store.recording_series_windows(&query, now()))
                    .await?
                    .into_iter()
                    .filter(|(start, end)| *start < request.end && *end > request.start)
                    .min_by_key(|(start, _)| start.abs_diff(request.start))
                    .map_or(request.start, |(start, _)| start)
            };
            rule.local_start = local_timestamp(first_start)
                .ok_or_else(|| Error::unavailable("Recording time unavailable"))?;
            first_start
        } else {
            rule.lead_seconds = 0;
            rule.trail_seconds = 0;
            rule.request.start
        };
        self.sync_recording_contexts();
        self.start_recordings();
        let job = JobId(identity(self.services().ports().entropy)?);
        // The first occurrence and the recurring intent are one acknowledgement.
        let first = self
            .services()
            .call(move |store| {
                store.schedule_recording_rule_with_first_checked(
                    &rule,
                    job,
                    &request,
                    occurrence_start,
                    now(),
                    |store| admission.authorize(store),
                )
            })
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
        let admission = if enabled {
            let rule = self
                .services()
                .read(move |store| {
                    Ok(store
                        .recording_rules()?
                        .into_iter()
                        .find(|rule| rule.id == id))
                })
                .await?
                .ok_or_else(|| Error::unavailable("This recording does not repeat"))?;
            Some(self.recording_admission(&rule.request).await?)
        } else {
            None
        };
        if enabled && self.recordings.is_stopping() {
            return Err(Error::unavailable(
                "Restart Patina before scheduling a recording",
            ));
        }
        self.services()
            .call(move |store| match admission {
                Some(admission) => {
                    store.set_recording_rule_enabled_checked(id, true, now(), |store| {
                        admission.authorize(store)
                    })
                }
                None => store.set_recording_rule_enabled(id, false, now()),
            })
            .await?;
        if enabled {
            self.start_recordings();
            let _ = materialize(&self.services()).await;
        }
        self.recording_rules_changed().await
    }

    async fn recording_rules_changed(&self) -> Result<(), Error> {
        let (rules, jobs) = self
            .services()
            .read(|store| Ok((store.recording_rules()?, store.recording_jobs()?)))
            .await?;
        {
            let mut data = self.recordings.lock();
            data.intent_revision += 1;
            data.recurring = rules.iter().any(|rule| rule.enabled);
            data.jobs = jobs;
        }
        self.refresh_recording_wake().await?;
        Ok(())
    }
    pub async fn stop_repeating_recording(&self, job: JobId) -> Result<(), Error> {
        if !self
            .services()
            .call(move |store| store.cancel_recording_rules_for_job(job))
            .await?
        {
            return Err(Error::unavailable("This recording does not repeat"));
        }
        self.recording_rules_changed().await
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
            request.start = if rule.repeat == Repeat::Series {
                start.saturating_sub(i64::from(rule.lead_seconds))
            } else {
                start
            };
            request.end = if rule.repeat == Repeat::Series {
                end.saturating_add(i64::from(rule.trail_seconds))
            } else {
                end
            };
            if request.validate(current).is_err() {
                continue;
            }
            let job = JobId(identity(services.ports().entropy)?);
            services
                .call(move |store| {
                    store
                        .materialize_recording_rule_at(id, job, &request, start, current)
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
