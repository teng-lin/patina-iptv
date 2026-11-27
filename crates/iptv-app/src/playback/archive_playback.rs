//! Explicit archive choices keep their list and occurrence through authorization.
use crate::{Playback, Ui};
use iptv_core::guide::{Program, Schedule};
use iptv_guide::playback::ResolutionAuthority;
use iptv_presentation::lists::{ChannelList, ListId};
use std::rc::Rc;

pub(crate) struct Choice {
    list: ListId,
    playing_context: bool,
    pending: crate::playback::player::PendingArchive,
    end: i64,
    authority: Option<ResolutionAuthority>,
}

impl Choice {
    fn capture(
        schedule: &dyn Schedule,
        list: ListId,
        channel: usize,
        program: Program,
        playing_context: bool,
    ) -> Option<Self> {
        let authority = iptv_guide::combined::epg_channel(schedule, channel)
            .and_then(|(epg, local)| epg.resolution_authority(local));
        Some(Self {
            list,
            playing_context,
            pending: crate::playback::player::PendingArchive::capture(
                schedule,
                channel,
                program.start,
                0,
            )?,
            end: crate::guide::unix_of(schedule, program.end),
            authority,
        })
    }

    fn remap(
        &self,
        schedule: &dyn Schedule,
        now: i32,
        playing: Option<usize>,
    ) -> Option<(usize, Program)> {
        let (channel, Playback::CatchUp { start, .. }) = self.pending.remap(schedule)? else {
            return None;
        };
        let authority = iptv_guide::combined::epg_channel(schedule, channel)
            .and_then(|(epg, local)| epg.resolution_authority(local));
        let program = schedule.program_at(channel, start);
        if (self.playing_context && playing != Some(channel))
            || authority != self.authority
            || program.start != start
            || crate::guide::unix_of(schedule, program.end) != self.end
            || !program.can_replay(now, schedule.catchup_days(channel))
        {
            return None;
        }
        Some((channel, program))
    }

    pub(crate) fn resolve(&self, ui: Ui) -> Option<(usize, Program)> {
        self.resolve_in(ui, false)
    }

    pub(crate) fn resolve_handoff(&self, ui: Ui) -> Option<(usize, Program)> {
        self.resolve_in(ui, true)
    }

    fn resolve_in(&self, ui: Ui, handoff: bool) -> Option<(usize, Program)> {
        let schedule = crate::wiring::runtime_owner::schedule();
        if !self.playing_context
            && ui
                .guide
                .list
                .with_untracked(|list| ListId::of(list.entry(), &*schedule))
                != self.list
        {
            return None;
        }
        let now = ui.guide.focus.with_untracked(|focus| focus.now);
        let mut playing = ui.playback.playing.get_untracked();
        if handoff
            && self.playing_context
            && playing.is_none()
            && let Some(authority) = &self.authority
            && crate::playback::external_player::pending_target(ui).as_ref()
                == Some(&iptv_core::parental::Target::live(
                    authority.source(),
                    authority.channel().id.as_str(),
                ))
        {
            // The authorized handoff deliberately retires native playback.
            // Only its current, same-target transfer may bridge that boundary.
            playing = self.pending.remap(&*schedule).map(|(channel, _)| channel);
        }
        let (channel, program) = self.remap(&*schedule, now, playing)?;
        if self.playing_context {
            return Some((channel, program));
        }
        let entry = self.list.resolve(&*schedule)?;
        // Rebuild membership from current data: the mounted list can be waiting
        // for the same catalog publication to reach its own effect.
        let members = ChannelList::build(
            entry,
            &*schedule,
            &crate::guide::favorites::favorite_channels(),
            &crate::guide::favorites::recent_channels(),
        );
        members.row_of(channel)?;
        Some((channel, program))
    }

    pub(crate) fn external_context(&self, ui: Ui) -> bool {
        let schedule = crate::wiring::runtime_owner::schedule();
        let Some(entry) = self.list.resolve(&*schedule) else {
            return false;
        };
        let Some((channel, _)) = self.resolve(ui) else {
            return false;
        };
        ui.guide
            .list
            .with_untracked(|list| ListId::of(list.entry(), &*schedule))
            == self.list
            && ChannelList::build(
                entry,
                &*schedule,
                &crate::guide::favorites::favorite_channels(),
                &crate::guide::favorites::recent_channels(),
            )
            .row_of(channel)
            .is_some()
    }
}

pub(crate) fn select(
    ui: Ui,
    list: ListId,
    channel: usize,
    program: Program,
    playing_context: bool,
) {
    let Some(choice) = Choice::capture(
        &*crate::wiring::runtime_owner::schedule(),
        list,
        channel,
        program,
        playing_context,
    )
    .map(Rc::new) else {
        return;
    };
    let Some((channel, _)) = choice.resolve(ui) else {
        return;
    };
    if !crate::playback::player::allow_guide_channel(channel) {
        return;
    }
    let missing_handler = match crate::playback::external_player::open_archive(ui, choice.clone()) {
        crate::playback::external_player::Outcome::Handled => return,
        crate::playback::external_player::Outcome::Internal => None,
        crate::playback::external_player::Outcome::MissingHandler(list) => Some(list),
    };
    // Internal fallback uses the same captured choice after PIN entry. Ui::play
    // remains the internal player operation and still applies its own gates.
    crate::playback::external_player::cancel_pending(ui);
    let ticket = ui.cast.external.request.get_untracked();
    crate::playback::browse_playback::hold(ui);
    let guarded = choice.clone();
    let current =
        move || ui.cast.external.request.get_untracked() == ticket && guarded.resolve(ui).is_some();
    let before_play = current.clone();
    let play = move || {
        if !before_play() {
            return;
        }
        let Some((channel, program)) = choice.resolve(ui) else {
            return;
        };
        let session = ui
            .playback
            .player
            .main()
            .health
            .with_untracked(|health| health.session());
        ui.play(
            channel,
            Playback::CatchUp {
                start: program.start,
                offset: 0,
            },
        );
        if ui
            .playback
            .player
            .main()
            .health
            .with_untracked(|health| health.session())
            != session
            && ui.playback.playing.get_untracked() == Some(channel)
            && ui.playback.playback.get_untracked()
                == (Playback::CatchUp {
                    start: program.start,
                    offset: 0,
                })
        {
            ui.chrome
                .mode
                .apply(crate::chrome::mode::Action::EnterFullscreen);
            ui.show_playing();
            if let Some(list) = missing_handler {
                crate::playback::external_player::missing_handler(ui, list);
            }
        }
    };
    if let Some(target) = crate::screens::parental_view::live_target(ui, channel) {
        crate::wiring::navigation::request_if(ui, target, current, play);
    } else {
        // The built-in generated guide has no provider or protected target.
        play();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_epg::{
        ChannelKey, EpgRef, FeedId, GuideStore, MatchKind, ProgramData, SourceBuilder, StoreOptions,
    };
    use iptv_guide::{EpgSchedule, playback::SourceAuthority};
    use iptv_providers::{
        SecretUrl,
        model::{M3uSourceConfig, SourceConfig},
    };

    const NOW: i64 = 1_800_000_000;

    fn guide(
        reversed: bool,
        epoch: i64,
        incarnation: &str,
        end: i64,
        changed_stream: bool,
    ) -> EpgSchedule {
        let parsed = iptv_providers::m3u::parse_m3u(concat!(
            "#EXTM3U\n",
            "#EXTINF:-1 tvg-id=\"one\" group-title=\"Generated\" catchup=\"shift\" catchup-days=\"3\",One\nhttps://example.test/one.ts\n",
            "#EXTINF:-1 tvg-id=\"two\" group-title=\"Generated\" catchup=\"shift\" catchup-days=\"3\",Two\nhttps://example.test/two.ts\n",
        ));
        let mut channels = parsed.channels.items;
        let mut builder = SourceBuilder::new(FeedId(1), 0, StoreOptions::default(), NOW);
        let mut matches = std::collections::HashMap::new();
        for (channel, epg_id) in channels.iter().zip(["one", "two"]) {
            builder.push_data(
                epg_id,
                &ProgramData {
                    start: NOW - 3600,
                    stop: Some(end),
                    title: "Generated archive",
                    ..Default::default()
                },
            );
            matches.insert(
                ChannelKey::new(channel.id.as_str()),
                vec![EpgRef {
                    source: FeedId(1),
                    epg_id: epg_id.into(),
                    kind: MatchKind::TvgId,
                }],
            );
        }
        let mut store = GuideStore::new();
        store.replace_source(builder.finish());
        store.set_matches(matches);
        if changed_stream {
            channels[0].stream = iptv_providers::model::StreamLocator::Url {
                url: "https://example.test/replacement.ts".into(),
                headers: Vec::new(),
                properties: Vec::new(),
            };
        }
        if reversed {
            channels.reverse();
        }
        EpgSchedule::from_catalog(&parsed.groups, &channels, store, epoch).with_source_authority(
            Some(std::sync::Arc::new(SourceAuthority::new(
                7,
                incarnation.into(),
                SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://example.test/generated.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            ))),
        )
    }

    #[test]
    fn archive_choice_keeps_channel_and_utc_occurrence_across_reorder_and_epoch() {
        let before = guide(false, NOW, "original", NOW - 1800, false);
        let program = before.program_at(0, before.minute(NOW - 3600));
        let choice = Choice::capture(&before, ListId::All, 0, program, false).unwrap();
        let after = guide(true, NOW + 86400, "original", NOW - 1800, false);
        let (channel, remapped) = choice.remap(&after, after.minute(NOW), None).unwrap();
        assert_eq!(channel, 1);
        assert_eq!(after.channel_key(channel), before.channel_key(0));
        assert_eq!(crate::guide::unix_of(&after, remapped.start), NOW - 3600);
        assert_eq!(crate::guide::unix_of(&after, remapped.end), NOW - 1800);
    }

    #[test]
    fn playing_context_keeps_its_target_without_borrowing_the_browse_list() {
        let schedule = guide(false, NOW, "original", NOW - 1800, false);
        let program = schedule.program_at(0, schedule.minute(NOW - 3600));
        let choice = Choice::capture(
            &schedule,
            ListId::Group("unrelated".into()),
            0,
            program,
            true,
        )
        .unwrap();
        assert_eq!(
            choice.remap(&schedule, schedule.minute(NOW), Some(0)),
            Some((0, program))
        );
        assert!(
            choice
                .remap(&schedule, schedule.minute(NOW), Some(1))
                .is_none()
        );
        assert!(
            choice
                .remap(&schedule, schedule.minute(NOW), None)
                .is_none()
        );
    }

    #[test]
    fn archive_choice_rejects_changed_occurrence_source_channel_and_guide_offset() {
        let before = guide(false, NOW, "original", NOW - 1800, false);
        let choice = Choice::capture(
            &before,
            ListId::All,
            0,
            before.program_at(0, before.minute(NOW - 3600)),
            false,
        )
        .unwrap();
        for after in [
            guide(false, NOW, "original", NOW - 1200, false),
            guide(false, NOW, "replacement", NOW - 1800, false),
            guide(false, NOW, "original", NOW - 1800, true),
        ] {
            assert!(choice.remap(&after, after.minute(NOW), None).is_none());
        }
        assert!(
            choice
                .remap(&before, before.minute(NOW + 4 * 86400), None)
                .is_none()
        );
        before.set_organization(iptv_guide::Organization {
            group_preferences: [(
                before.group_key(0),
                iptv_core::organization::GroupPreferences {
                    epg_offset_minutes: 15,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        });
        assert!(choice.remap(&before, before.minute(NOW), None).is_none());
    }
}
