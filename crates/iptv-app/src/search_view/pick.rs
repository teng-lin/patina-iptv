//! Search and add: multiview's tile menu opens the search screen to pick
//! a channel for a new tile. The screen shows over the grid without
//! leaving full screen (leaving it would end multiview); choosing a
//! channel, or a program (its channel), hands the channel to multiview
//! (`multiview::picked`) and closes the screen, so the grid is back with
//! the new tile. Back (or Close) closes it without one
//! (`multiview::list_closed`, from [`super::close`]).

use super::*;

/// Opens a fresh search screen picking for multiview. A screen waiting
/// for playback to end gives way to it.
pub fn open_to_pick(ui: Ui) {
    if ui.search.with_untracked(Option::is_some) {
        close(ui);
    }
    let history = if options(&crate::settings::get()).show_history {
        load_history()
    } else {
        SearchHistory::default()
    };
    log::info!("iptv search: picking for multiview");
    ui.search.set(Some(Screen {
        state: SearchState::new(history),
        suspended: false,
        programs: ProgramSearch::Building,
        pick: true,
    }));
    prepare(ui);
}

/// Whether the screen picks for multiview.
pub fn picking(ui: Ui) -> bool {
    ui.search.with(|s| s.as_ref().is_some_and(|s| s.pick))
}

/// Chooses result `id` while picking for multiview: its channel becomes a
/// new tile and the screen closes. `false` when not picking.
pub(super) fn choose(ui: Ui, id: &ResultId) -> bool {
    if !untracked(|| picking(ui)) {
        return false;
    }
    let schedule = crate::runtime_owner::schedule();
    let Some(channel) =
        resolve_result_channel(&crate::runtime_owner::runtime(), id.channel(), &*schedule)
    else {
        return true;
    };
    if crate::multiview::picked(ui, channel) {
        close(ui);
    }
    true
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use iptv_guide::combined::{CombinedSchedule, SourceSchedule};
    use iptv_guide::playback::SourceAuthority;
    use iptv_providers::SecretUrl;
    use iptv_providers::model::{M3uSourceConfig, SourceConfig};
    use std::rc::Rc;

    fn part(source: i64, incarnation: &str, names: &[&str]) -> SourceSchedule {
        let mut playlist = String::from("#EXTM3U\n");
        for name in names {
            playlist.push_str(&format!(
                "#EXTINF:-1,Generated {name}\nhttps://media.example.test/{name}\n"
            ));
        }
        let catalog = iptv_providers::m3u::parse_m3u(&playlist);
        let authority = SourceAuthority::new(
            source,
            incarnation.into(),
            SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(format!("https://playlist.example.test/{source}")),
                user_agent: None,
                referrer: None,
            }),
        );
        SourceSchedule {
            id: source,
            name: format!("Generated source {source}"),
            schedule: Rc::new(
                EpgSchedule::from_catalog(
                    &catalog.groups,
                    &catalog.channels.items,
                    Default::default(),
                    1_800_000_000,
                )
                .with_source_authority(Some(std::sync::Arc::new(authority))),
            ),
        }
    }

    #[test]
    fn multiview_and_normal_selection_resolve_the_same_incarnation_qualified_identity() {
        let directory = tempfile::tempdir().unwrap();
        let owner = crate::runtime::Runtime::new(crate::runtime::Ports::new(
            directory.path().to_path_buf(),
            |_| panic!("synthetic search identity lookup must not open a database"),
        ));
        let before = CombinedSchedule::new(
            vec![
                part(1, "generated-original", &["A", "B"]),
                part(2, "generated-second", &["A"]),
            ],
            1_800_000_000,
        );
        owner.prepare_search_channels(1, &before);
        let hit = owner
            .search_channels("Generated A", &Default::default(), Default::default())
            .into_iter()
            .find(|hit| hit.identity.source == Some(1))
            .unwrap();
        let channel = ResultId::Channel(hit.identity.clone());
        let program = ResultId::Program {
            channel: hit.identity,
            start: 1_800_000_000,
        };
        let shifted = CombinedSchedule::new(
            vec![
                part(2, "generated-second", &["A"]),
                part(1, "generated-original", &["B", "A"]),
            ],
            1_800_000_000,
        );
        for id in [&channel, &program] {
            assert_eq!(
                resolve_result_channel(&owner, id.channel(), &shifted),
                Some(2)
            );
        }
        assert_eq!(
            outcome(
                &channel,
                None,
                resolve_result_channel(&owner, channel.channel(), &shifted)
            ),
            Outcome::Live(2)
        );
        let recreated = CombinedSchedule::new(
            vec![
                part(2, "generated-second", &["A"]),
                part(1, "generated-recreated", &["B", "A"]),
            ],
            1_800_000_000,
        );
        for id in [&channel, &program] {
            assert_eq!(
                resolve_result_channel(&owner, id.channel(), &recreated),
                None,
                "neither normal playback nor multiview may adopt a recreated source"
            );
        }
        assert_eq!(
            outcome(
                &channel,
                None,
                resolve_result_channel(&owner, channel.channel(), &recreated)
            ),
            Outcome::Nothing
        );
    }
}
