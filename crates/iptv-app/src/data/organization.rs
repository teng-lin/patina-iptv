//! Apply user organization to the guide without replacing playback channels.
use crate::Ui;
use crate::runtime::model::SourceId;
use iptv_guide::EpgSchedule;

pub fn revision() -> u64 {
    crate::runtime().organization_revision().get()
}

use crate::runtime::commands::load_organization as load;

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
pub async fn apply(schedule: &EpgSchedule, source: SourceId) {
    match load(source).await {
        Ok(snapshot) => snapshot.install(schedule),
        Err(error) => log::warn!("iptv organization: store unavailable: {error}"),
    }
}
pub fn changed(ui: Ui, source: SourceId) {
    crate::runtime().organization_revision().set(
        crate::runtime()
            .organization_revision()
            .get()
            .wrapping_add(1),
    );
    crate::runtime().emit(crate::runtime::RuntimeEvent::LibraryChanged);
    if super::combined::active(ui) {
        super::combined::organization_changed(ui, source);
        return;
    }
    if ui.library.shown.get_untracked() != Some(source) {
        return;
    }
    let schedule = crate::schedule();
    super::spawn_in_app(async move {
        if schedule
            .as_any()
            .and_then(|s| s.downcast_ref::<EpgSchedule>())
            .is_some()
        {
            let Ok(snapshot) = load(source).await else {
                return;
            };
            crate::runtime()
                .emit(crate::runtime::RuntimeEvent::OrganizationReady { schedule, snapshot });
        }
    });
}
