//! A stored source opening (a cold start, or a switch to another stored
//! playlist): what the store has is on screen before any network, in the
//! order a viewer needs it.
//!
//! On the UI thread, as soon as the store is open ([`begin`], cold start
//! only): the guide's shell replaces the synthetic guide (no channels, so
//! nothing made up flashes and nothing is laid out twice), and the last
//! channel starts from its stored row (`player/resume.rs`).
//!
//! Through the store worker ([`send_stored`]): the channels with the first
//! screen's programmes ([`first_rows`] around the channel the guide opens
//! on, [`first_screen`]: the hour before now to [`FIRST_SCREEN_HOURS`] on),
//! shown with every other channel and time loading
//! (`EpgSchedule::held_only`, `held_channels`); then the whole window
//! (`stored::load_guide`), which replaces it. On a TV box the
//! window read takes over a second; the first screen's a fraction of it
//! (measured on a 32-bit Android TV box).

use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender as Sender;

use iptv_epg::{ChannelKey, GuideStore, UnixTime};
use iptv_providers::model::Channel;
use iptv_store_sqlite::{EpgWindow, SourceId};

use super::imports::{Imported, Job, Msg, Streams};
use super::stored;

/// Hours of programmes after the hour of now read first.
pub const FIRST_SCREEN_HOURS: i64 = 6;
/// Channels read first either side of the one the guide opens on: a
/// screen's rows and more.
pub const FIRST_SCREEN_ROWS: usize = 30;

/// What a first screen holds: the programmes of `keys` overlapping
/// `[from, to)`.
pub struct Held {
    pub from: UnixTime,
    pub to: UnixTime,
    pub keys: Vec<ChannelKey>,
}

/// The times the first screen's read holds: from the hour before now's to
/// [`FIRST_SCREEN_HOURS`] after now's hour.
pub fn first_screen(now: UnixTime) -> (UnixTime, UnixTime) {
    let hour = now - now.rem_euclid(3600);
    (hour - 3600, hour + FIRST_SCREEN_HOURS * 3600)
}

/// The channels around the one the guide opens on, `focus` (a channel key;
/// the first channel without one or when it is gone), in playlist order
/// (the guide's list at a cold start: all channels).
pub fn first_rows(channels: &[Channel], focus: Option<&str>) -> Vec<ChannelKey> {
    let at = focus
        .and_then(|key| channels.iter().position(|c| c.id.as_str() == key))
        .unwrap_or(0);
    let from = at.saturating_sub(FIRST_SCREEN_ROWS);
    let to = (at + FIRST_SCREEN_ROWS + 1).min(channels.len());
    channels[from..to]
        .iter()
        .map(|c| ChannelKey::new(c.id.as_str()))
        .collect()
}

/// Sends `id`'s stored channels with the first screen's programmes, then
/// with the whole guide window. Nothing when it has no channels.
pub(crate) async fn send_stored(
    services: &super::store_service::Services,
    job: &Job,
    id: SourceId,
    now: UnixTime,
    tx: &Sender<Msg>,
) -> bool {
    let settings = services.settings();
    let started = Instant::now();
    let ms = |t: Instant| t.elapsed().as_millis();
    let (groups, channels) = match services
        .read(move |store| stored::catalog_in(store, id))
        .await
    {
        Ok(catalog) => catalog,
        Err(e) => {
            log::warn!("iptv data: stored catalog: {e}");
            return false;
        }
    };
    if channels.is_empty() {
        return false;
    }
    let n = channels.len();
    log::info!("iptv data: showing {n} stored channels before the refresh");
    services.mark("channels read", format_args!("{n} in {} ms", ms(started)));
    let imported = |groups, channels, guide| {
        let context = super::playback::stored_context(&job.source.config);
        Box::new(Imported {
            groups,
            channels,
            resume: context.is_some(),
            playback: Streams::New(context, None),
            guide,
            now,
            source: id,
        })
    };
    let started = Instant::now();
    let (from, to) = first_screen(now);
    let resumes = super::settings::flag(&settings, iptv_settings::keys::RESUME_LAST_CHANNEL);
    let focus = if resumes {
        let files = services.clone();
        services
            .read(move |_| Ok(super::playback::load_last_channel(&files, Some(id))))
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let keys = first_rows(&channels, focus.as_deref());
    let query_keys = keys.clone();
    if let Ok(guide) = services
        .read(move |store| {
            store.load_epg(
                id,
                &EpgWindow {
                    from,
                    to,
                    keys: Some(&query_keys),
                    ..EpgWindow::from(now, 0)
                },
            )
        })
        .await
        && guide.bound_count() > 0
    {
        let kib = guide.heap_bytes() / 1024;
        services.mark(
            "first screen read",
            format_args!("{} channels, {kib} KiB in {} ms", keys.len(), ms(started)),
        );
        let first = imported(groups.clone(), channels.clone(), guide);
        let held = Held { from, to, keys };
        let _ = tx.send(Msg::FirstScreen(first, held));
    }
    let started = Instant::now();
    let window = super::paging::window(now, stored::keep_past_days(&settings));
    let guide = services
        .read(move |store| store.load_epg(id, &super::paging::store_window(window)))
        .await
        .unwrap_or_default();
    let (bound, kib) = (guide.bound_count(), guide.heap_bytes() / 1024);
    services.mark(
        "guide window read",
        format_args!("{bound} channels, {kib} KiB in {} ms", ms(started)),
    );
    // A cached-only opening must complete without claiming a network slot.
    let metadata = services
        .read(move |store| {
            Ok((
                store.source_policy(id)?,
                store.import_status(id)?,
                store.epg_guide(id)?,
                store.guide_sources(id)?,
                store.epg_overrides(id)?,
            ))
        })
        .await;
    if let Ok((policy, status, state, sources, overrides)) = metadata {
        let playlist_due = status.last_success_at.is_none()
            || policy.due(
                now,
                status.last_attempt_at,
                matches!(
                    job.purpose,
                    super::imports::Purpose::Startup | super::imports::Purpose::Combined
                ),
            );
        if !playlist_due {
            let configs = stored::configs_from_sources(
                &settings,
                &sources,
                state.as_ref(),
                Some(&job.source.config),
            );
            let overrides = overrides
                .into_iter()
                .map(|(id, epg)| (ChannelKey::new(id.as_str()), epg.to_override()))
                .collect();
            let matching = stored::Matching {
                channels: &channels,
                overrides: &overrides,
            };
            let decision = stored::plan_with(
                &settings,
                job.purpose.trigger(),
                &configs,
                matching,
                state.as_ref(),
                now,
            );
            if !matches!(decision, iptv_import::Decision::Refresh(_)) {
                let guide = if matches!(decision, iptv_import::Decision::Discard) {
                    if services
                        .call(move |store| store.delete_epg(id))
                        .await
                        .is_err()
                    {
                        let _ = tx.send(Msg::Guide(imported(groups, channels, guide)));
                        return false;
                    }
                    GuideStore::default()
                } else {
                    guide
                };
                let state = if matches!(decision, iptv_import::Decision::Discard) {
                    None
                } else {
                    state.as_ref()
                };
                if let Some(at) = stored::check_at(
                    stored::plan_with(
                        &settings,
                        iptv_import::Trigger::Timer,
                        &configs,
                        matching,
                        state,
                        now,
                    ),
                    now,
                ) {
                    let _ = tx.send(Msg::NextCheck { source: id, at });
                }
                let counts = iptv_core::wizard::ImportCounts {
                    channels: channels.len() as u32,
                    groups: groups.len() as u32,
                    movies: 0,
                    series: 0,
                };
                let _ = tx.send(Msg::Playlist(Ok((
                    counts,
                    imported(groups, channels, guide),
                ))));
                return true;
            }
        }
    }
    let _ = tx.send(Msg::Guide(imported(groups, channels, guide)));
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channels(n: usize) -> Vec<Channel> {
        let mut m3u = String::from("#EXTM3U\n");
        for i in 0..n {
            m3u.push_str(&format!(
                "#EXTINF:-1 tvg-id=\"c{i}\",C{i}\nhttp://s.example.test/{i}.m3u8\n"
            ));
        }
        iptv_providers::m3u::parse_m3u(&m3u).channels.items
    }

    #[test]
    fn the_first_rows_are_around_the_last_channel() {
        let all = channels(100);
        let key = |i: usize| ChannelKey::new(all[i].id.as_str());
        let rows = first_rows(&all, Some(all[50].id.as_str()));
        assert_eq!(rows.len(), 2 * FIRST_SCREEN_ROWS + 1);
        assert_eq!((rows[0].clone(), rows[60].clone()), (key(20), key(80)));
        // Near the start, without one, or with one gone: from the top.
        let top = first_rows(&all, Some(all[3].id.as_str()));
        assert_eq!((top[0].clone(), top.len()), (key(0), 34));
        assert_eq!(first_rows(&all, None)[0], key(0));
        assert_eq!(first_rows(&all, Some("gone")).len(), FIRST_SCREEN_ROWS + 1);
        assert_eq!(first_rows(&all[..5], None).len(), 5);
    }

    #[test]
    fn the_first_screen_is_the_hour_before_now_to_six_hours_on() {
        let hour = 1_800_000_000;
        assert_eq!(first_screen(hour + 1234), (hour - 3600, hour + 6 * 3600));
        assert_eq!(first_screen(hour), (hour - 3600, hour + 6 * 3600));
    }
}
