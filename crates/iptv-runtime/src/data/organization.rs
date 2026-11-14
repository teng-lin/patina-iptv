//! Apply user organization to the guide without replacing playback channels.
use crate::model::SourceId;
use iptv_guide::EpgSchedule;

pub fn revision() -> u64 {
    crate::current().organization_revision().get()
}

use crate::commands::load_organization as load;

/// The schedule of an imported catalog with `source`'s organisation, built
/// off the UI thread.
///
/// Measured with a generated 100,000-channel, 1,000-group playlist, both
/// steps then on the UI thread: grouping the catalog took 70-100 ms and
/// organising it 50-65 ms on a desktop (release build), and 136-171 ms and
/// 126-173 ms on the Android TV emulator, twice per cold start (the stored
/// guide, then the refreshed one). So the data is made on the shared
/// runtime's blocking threads (`EpgSchedule::prepare`) and only turned into
/// the schedule here, which moves it.
pub async fn build(
    source: SourceId,
    groups: Vec<iptv_providers::model::Group>,
    channels: Vec<iptv_providers::model::Channel>,
    guide: iptv_epg::GuideStore,
    now: iptv_epg::UnixTime,
) -> EpgSchedule {
    let organization = match load(source).await {
        Ok(snapshot) => Some((snapshot.organization, snapshot.lists)),
        Err(error) => {
            log::warn!("iptv organization: store unavailable: {error}");
            None
        }
    };
    let count = channels.len();
    let started = std::time::Instant::now();
    let prepared = crate::executor::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let prepared = EpgSchedule::prepare(&groups, &channels, guide, now, organization);
        (prepared, started.elapsed())
    })
    .await;
    match prepared {
        Ok((prepared, took)) => {
            let schedule = EpgSchedule::from_prepared(prepared);
            log::info!(
                "iptv data: schedule of {count} channels built in {} ms off the UI thread \
                 ({} ms until here)",
                took.as_millis(),
                started.elapsed().as_millis()
            );
            schedule
        }
        Err(_) => {
            log::error!("iptv data: building a schedule failed; showing it empty");
            EpgSchedule::new(Vec::new(), iptv_epg::GuideStore::default(), now)
        }
    }
}
pub async fn apply(schedule: &EpgSchedule, source: SourceId) -> Option<EpgSchedule> {
    let snapshot = match load(source).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            log::warn!("iptv organization: store unavailable: {error}");
            crate::current().emit(crate::RuntimeEvent::Notify(
                "The list could not be reloaded. Reopen it and try again.",
            ));
            return None;
        }
    };
    let input = schedule.organization_builder();
    let prepared = crate::executor::spawn_blocking(move || {
        input.prepare(snapshot.organization, snapshot.lists)
    })
    .await
    .ok()?;
    Some(schedule.with_prepared_organization(prepared))
}
pub fn changed(source: SourceId) {
    crate::current().organization_revision().set(
        crate::current()
            .organization_revision()
            .get()
            .wrapping_add(1),
    );
    let ticket = crate::current().begin_organization_edit(source);
    crate::current().emit(crate::RuntimeEvent::LibraryChanged);
    if super::combined::active() {
        super::combined::organization_changed(source, ticket);
        return;
    }
    if crate::current().shown.get() != Some(source) {
        return;
    }
    let schedule = crate::current().schedule();
    super::spawn_in_app(async move {
        if let Some(epg) = schedule
            .as_any()
            .and_then(|s| s.downcast_ref::<EpgSchedule>())
            && let Some(replacement) = apply(epg, source).await
        {
            crate::current().emit(crate::RuntimeEvent::OrganizationReady {
                source,
                ticket,
                schedule,
                replacement: std::rc::Rc::new(replacement),
            });
        }
    });
}
