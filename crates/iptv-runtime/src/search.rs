//! Owned search indexes and their native database reader.
use iptv_core::guide::Schedule;
use iptv_epg::{ChannelKey, UnixTime};
use iptv_library::SourceId;
use iptv_search::{ChannelIndex, ProgramIndex};
use std::cell::{Cell, RefCell};

/// Search lifetime state stays together; only this module can mutate it.
#[derive(Default)]
pub(crate) struct State {
    indexes: RefCell<Indexes>,
    history: RefCell<Option<Vec<String>>>,
    generation: Cell<u64>,
}

#[derive(Default)]
struct Indexes {
    /// The `data_version` they were built for.
    pub version: Option<u64>,
    pub channels: ChannelIndex,
    /// Every day the store keeps; dropped when the screen closes (it is
    /// about the size of a week's guide).
    pub programs: Option<ProgramIndex>,
    /// The `data_version` whose program index is being built.
    pub building: Option<u64>,
}

fn programs_native(
    services: &super::store_service::Services,
    sources: Vec<(SourceId, String, Vec<ChannelKey>, usize)>,
    rows: usize,
    from: UnixTime,
) -> impl std::future::Future<Output = Result<ProgramIndex, crate::store_service::StoreError>> + use<>
{
    services.epg().program_index(sources, rows, from)
}

impl crate::Runtime {
    pub fn search_history(&self) -> Vec<String> {
        self.search.history.borrow().clone().unwrap_or_default()
    }
    /// An empty saved history is distinct from one that could not be loaded.
    pub fn search_history_loaded(&self) -> bool {
        self.search.history.borrow().is_some()
    }
    pub fn clear_loaded_search_history(&self) {
        *self.search.history.borrow_mut() = None;
    }
    pub fn install_search_history(&self, history: Vec<String>) {
        *self.search.history.borrow_mut() = Some(history);
    }
    pub fn search_history_matches(&self, history: &[String]) -> bool {
        self.search.history.borrow().as_deref() == Some(history)
    }
    pub fn begin_search(&self) -> u64 {
        let next = self.search.generation.get() + 1;
        self.search.generation.set(next);
        next
    }
    pub fn search_is_current(&self, ticket: u64) -> bool {
        self.search.generation.get() == ticket
    }

    pub fn prepare_search_channels(&self, version: u64, schedule: &dyn iptv_core::guide::Schedule) {
        if self.search.indexes.borrow().version == Some(version) {
            return;
        }
        let mut channels = ChannelIndex::from_schedule(schedule);
        for channel in 0..schedule.channel_count() {
            if let Some(identity) = self.search_channel_identity(schedule, channel) {
                channels.set_identity(channel, identity);
            }
            if !schedule.channel_hidden(channel) {
                channels.add_key(channel, &schedule.channel_number(channel).to_string());
            }
        }
        *self.search.indexes.borrow_mut() = Indexes {
            version: Some(version),
            channels,
            programs: None,
            building: None,
        };
    }
    /// Bind a row to its accepting schedule's source incarnation, without store I/O.
    pub fn search_channel_identity(
        &self,
        schedule: &dyn iptv_core::guide::Schedule,
        channel: usize,
    ) -> Option<iptv_search::ChannelId> {
        use iptv_guide::{EpgSchedule, combined::CombinedSchedule};
        if let Some(all) = schedule
            .as_any()
            .and_then(|value| value.downcast_ref::<CombinedSchedule>())
        {
            let (part, local) = all.channel_source(channel)?;
            let authority = part.schedule.source_authority()?;
            if authority.source() != part.id {
                return None;
            }
            return Some(iptv_search::ChannelId::in_source(
                part.id,
                authority.incarnation(),
                part.schedule.channel_key(local)?,
            ));
        }
        let key = schedule.channel_key(channel)?;
        if let Some(epg) = schedule
            .as_any()
            .and_then(|value| value.downcast_ref::<EpgSchedule>())
            && let Some(authority) = epg.source_authority()
        {
            if self.catalog.shown_source().map(|source| source.0) != Some(authority.source()) {
                return None;
            }
            return Some(iptv_search::ChannelId::in_source(
                authority.source(),
                authority.incarnation(),
                key,
            ));
        }
        // A standalone synthetic schedule has no stored source authority.
        self.catalog
            .shown_source()
            .is_none()
            .then(|| iptv_search::ChannelId::new(None, key))
    }

    /// Resolve against the current semantic identity immediately before use.
    pub fn resolve_search_channel(
        &self,
        identity: &iptv_search::ChannelId,
        schedule: &dyn iptv_core::guide::Schedule,
    ) -> Option<usize> {
        if identity.is_anonymous() {
            return None;
        }
        let combined = schedule
            .as_any()
            .is_some_and(|s| s.is::<iptv_guide::combined::CombinedSchedule>());
        let channel = if combined {
            let source = identity.source?;
            schedule.find_channel(&iptv_guide::combined::qualified_key(
                source,
                &identity.content,
            ))?
        } else if identity.source == self.catalog.shown_source().map(|source| source.0) {
            schedule.find_channel(&identity.content)?
        } else {
            return None;
        };
        (self.search_channel_identity(schedule, channel).as_ref() == Some(identity))
            .then_some(channel)
    }

    pub fn has_program_index(&self) -> bool {
        self.search.indexes.borrow().programs.is_some()
    }
    pub fn begin_program_index(&self, version: u64) -> bool {
        let mut indexes = self.search.indexes.borrow_mut();
        let wanted = indexes.version == Some(version) && indexes.programs.is_none();
        wanted && indexes.building.replace(version) != Some(version)
    }
    pub fn finish_program_index(&self, version: u64) -> bool {
        let mut indexes = self.search.indexes.borrow_mut();
        if indexes.building == Some(version) {
            indexes.building = None;
        }
        indexes.version == Some(version)
    }
    pub fn install_program_index(&self, index: ProgramIndex) {
        self.search.indexes.borrow_mut().programs = Some(index);
    }
    pub fn release_program_index(&self) {
        self.search.indexes.borrow_mut().programs = None;
    }
    pub fn search_channels(
        &self,
        query: &str,
        favorites: &std::collections::BTreeSet<usize>,
        options: iptv_search::ChannelSearchOptions,
    ) -> Vec<iptv_search::ChannelHit> {
        self.search
            .indexes
            .borrow()
            .channels
            .search_channels(query, favorites, options)
    }
    pub fn search_programs(
        &self,
        query: &str,
        now: UnixTime,
        schedule: &dyn Schedule,
        options: iptv_search::ProgramSearchOptions<'_>,
    ) -> Vec<iptv_search::ProgramHit> {
        self.search
            .indexes
            .borrow()
            .programs
            .as_ref()
            .map_or_else(Vec::new, |index| {
                // Options refer to current schedule rows, while the index can
                // still contain rows from the previous accepting schedule.
                let resolved: Vec<_> = index
                    .channel_identities()
                    .iter()
                    .map(|identity| self.resolve_search_channel(identity, schedule))
                    .collect();
                let visible = |row: usize| {
                    resolved[row].is_some_and(|current| {
                        !schedule.channel_hidden(current)
                            && options.visible.is_none_or(|filter| filter(current))
                    })
                };
                let days = |row: usize| {
                    resolved[row].map_or(0, |current| {
                        options.catchup_days.map_or(0, |window| window(current))
                    })
                };
                let offsets: Vec<_> = resolved
                    .iter()
                    .map(|row| {
                        row.and_then(|channel| iptv_guide::combined::epg_channel(schedule, channel))
                            .map_or(0, |(epg, channel)| {
                                i64::from(epg.epg_offset_minutes(channel)) * 60
                            })
                    })
                    .collect();
                let time_offset = |row: usize| offsets[row];
                let favorites = resolved
                    .iter()
                    .enumerate()
                    .filter_map(|(row, current)| {
                        current
                            .filter(|current| {
                                options.favorites.is_some_and(|set| set.contains(current))
                            })
                            .map(|_| row)
                    })
                    .collect();
                let hits = index.search_programs(
                    query,
                    now,
                    iptv_search::ProgramSearchOptions {
                        visible: Some(&visible),
                        catchup_days: Some(&days),
                        time_offset: Some(&time_offset),
                        favorites: Some(&favorites),
                        ..options
                    },
                );
                hits.into_iter()
                    .filter_map(|mut hit| {
                        hit.channel = resolved[hit.channel]?;
                        Some(hit)
                    })
                    .collect()
            })
    }
}

impl crate::Runtime {
    /// Reads the stored program titles of `sources` from `from` on into
    /// an index of `rows` channels (the search screen's program search).
    /// Each source includes the incarnation captured from the accepting schedule;
    /// queued work cannot adopt another source that reused its numeric id.
    pub fn read_program_index(
        &self,
        sources: Vec<(SourceId, String, Vec<ChannelKey>, usize)>,
        rows: usize,
        from: UnixTime,
    ) -> impl std::future::Future<Output = Result<ProgramIndex, crate::Error>> + use<> {
        let services = self.services();
        async move {
            programs_native(&services, sources, rows, from)
                .await
                .map_err(crate::Error::from)
        }
    }
}

#[cfg(all(test, feature = "native"))]
mod identity_tests {
    use super::*;
    use iptv_core::guide::Schedule;
    use iptv_guide::playback::SourceAuthority;
    use iptv_providers::SecretUrl;
    use iptv_providers::model::{M3uSourceConfig, Source, SourceConfig};
    use iptv_search::ProgramIndexBuilder;
    use std::rc::Rc;

    fn config(source: i64) -> SourceConfig {
        SourceConfig::M3u(M3uSourceConfig {
            location: SecretUrl::new(format!("https://playlist.example.test/{source}.m3u")),
            user_agent: None,
            referrer: None,
        })
    }
    fn schedule(source: i64, incarnation: &str, order: &[&str]) -> Rc<iptv_guide::EpgSchedule> {
        let mut text = String::from("#EXTM3U\n");
        for name in order {
            text.push_str(&format!(
                "#EXTINF:-1,Generated {name}\nhttp://stream.example.test/{name}\n"
            ));
        }
        let catalog = iptv_providers::m3u::parse_m3u(&text);
        Rc::new(
            iptv_guide::EpgSchedule::from_catalog(
                &catalog.groups,
                &catalog.channels.items,
                Default::default(),
                1_800_000_000,
            )
            .with_source_authority(Some(std::sync::Arc::new(SourceAuthority::new(
                source,
                incarnation.into(),
                config(source),
            )))),
        )
    }

    #[test]
    fn results_resolve_reorders_sources_and_combined_offsets_by_content() {
        let owner = crate::Runtime::default();
        owner.catalog.set_shown_source(Some(SourceId(1)));
        let original = schedule(1, "generated-first", &["A", "B"]);
        owner.prepare_search_channels(1, &*original);
        let hit = owner
            .search_channels("Generated A", &Default::default(), Default::default())
            .remove(0);
        assert_eq!(hit.channel, 0);
        let replacement = schedule(1, "generated-first", &["B", "A"]);
        assert_eq!(
            owner.resolve_search_channel(&hit.identity, &*replacement),
            Some(1)
        );
        owner.prepare_search_channels(2, &*replacement);
        let refreshed = owner
            .search_channels("Generated A", &Default::default(), Default::default())
            .remove(0);
        assert_eq!(
            refreshed.identity, hit.identity,
            "ordinary refresh preserves semantic focus"
        );
        assert_eq!(refreshed.channel, 1);
        assert_eq!(
            owner.resolve_search_channel(&hit.identity, &*schedule(1, "generated-first", &["B"])),
            None
        );
        owner.catalog.set_shown_source(Some(SourceId(2)));
        assert_eq!(
            owner.resolve_search_channel(&hit.identity, &*replacement),
            None
        );
        use iptv_guide::combined::{CombinedSchedule, SourceSchedule};
        let combined = CombinedSchedule::new(
            vec![
                SourceSchedule {
                    id: 2,
                    name: "Synthetic second".into(),
                    schedule: schedule(2, "generated-second", &["B", "A"]),
                },
                SourceSchedule {
                    id: 1,
                    name: "Synthetic first".into(),
                    schedule: original.clone(),
                },
            ],
            1_800_000_000,
        );
        assert_eq!(
            owner.resolve_search_channel(&hit.identity, &combined),
            Some(2)
        );
        owner.prepare_search_channels(3, &combined);
        let duplicates =
            owner.search_channels("Generated A", &Default::default(), Default::default());
        assert_eq!(duplicates.len(), 2);
        assert_ne!(
            duplicates[0].identity, duplicates[1].identity,
            "equal provider keys in different sources differ"
        );
        let reordered = CombinedSchedule::new(
            vec![
                SourceSchedule {
                    id: 1,
                    name: "Synthetic first".into(),
                    schedule: replacement,
                },
                SourceSchedule {
                    id: 2,
                    name: "Synthetic second".into(),
                    schedule: schedule(2, "generated-second", &["A"]),
                },
            ],
            1_800_000_000,
        );
        for result in duplicates {
            let at = owner
                .resolve_search_channel(&result.identity, &reordered)
                .unwrap();
            assert_eq!(
                reordered.channel_key(at),
                Some(iptv_guide::combined::qualified_key(
                    result.identity.source.unwrap(),
                    &result.identity.content,
                ))
            );
        }
        let recreated = CombinedSchedule::new(
            vec![SourceSchedule {
                id: 1,
                name: "Synthetic recreated".into(),
                schedule: schedule(1, "generated-recreated", &["A", "B"]),
            }],
            1_800_000_000,
        );
        assert_eq!(
            owner.resolve_search_channel(&hit.identity, &recreated),
            None
        );
    }

    #[test]
    fn same_numeric_source_and_content_cannot_adopt_a_new_incarnation() {
        let owner = crate::Runtime::default();
        owner.catalog.set_shown_source(Some(SourceId(1)));
        let original = schedule(1, "generated-original", &["A"]);
        owner.prepare_search_channels(1, &*original);
        let hit = owner
            .search_channels("Generated A", &Default::default(), Default::default())
            .remove(0);
        for (version, incarnation) in ["generated-recreated", "generated-connection-edit"]
            .into_iter()
            .enumerate()
        {
            let changed = schedule(1, incarnation, &["A"]);
            assert_eq!(changed.channel_key(0), original.channel_key(0));
            assert_eq!(owner.resolve_search_channel(&hit.identity, &*changed), None);
            owner.prepare_search_channels(version as u64 + 2, &*changed);
            let fresh = owner.search_channel_identity(&*changed, 0).unwrap();
            assert_ne!(fresh, hit.identity);
            let fresh_hit = owner
                .search_channels("Generated A", &Default::default(), Default::default())
                .remove(0);
            assert_eq!(fresh_hit.identity, fresh);
            assert_eq!(owner.resolve_search_channel(&fresh, &*changed), Some(0));
        }
    }

    #[test]
    fn failed_program_build_completion_allows_retry_without_accepting_stale_versions() {
        let owner = crate::Runtime::default();
        let guide = schedule(1, "generated-original", &["A"]);
        owner.catalog.set_shown_source(Some(SourceId(1)));
        owner.prepare_search_channels(1, &*guide);
        assert!(owner.begin_program_index(1));
        assert!(!owner.begin_program_index(1));
        assert!(owner.finish_program_index(1));
        assert!(
            !owner.has_program_index(),
            "failed reads do not cache an empty success"
        );
        assert!(owner.begin_program_index(1), "a later reopen can retry");
        owner.prepare_search_channels(2, &*guide);
        assert!(owner.begin_program_index(2));
        assert!(!owner.finish_program_index(1));
        assert!(
            !owner.begin_program_index(2),
            "an old completion cannot clear the current build"
        );
    }

    #[test]
    fn program_visibility_archive_and_limit_use_resolved_rows() {
        let owner = crate::Runtime::default();
        owner.catalog.set_shown_source(Some(SourceId(1)));
        let before = schedule(1, "generated-original", &["A", "B"]);
        owner.prepare_search_channels(1, &*before);
        let mut builder = ProgramIndexBuilder::new(2);
        for channel in 0..2 {
            builder.set_identity(
                channel,
                owner.search_channel_identity(&*before, channel).unwrap(),
            );
            builder.push(channel, 1000, 4600, "Generated bulletin");
        }
        owner.install_program_index(builder.finish());
        let reordered = schedule(1, "generated-original", &["B", "A"]);
        let visible = |channel| channel == 1;
        let archive = |channel| if channel == 1 { 7 } else { 0 };
        let hits = owner.search_programs(
            "bulletin",
            5000,
            &*reordered,
            iptv_search::ProgramSearchOptions {
                visible: Some(&visible),
                catchup_days: Some(&archive),
                limit: 1,
                ..Default::default()
            },
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].channel, 1);
        assert!(hits[0].catchup);
        assert_eq!(
            hits[0].identity,
            owner.search_channel_identity(&*before, 0).unwrap()
        );
        let removed = schedule(1, "generated-original", &["B"]);
        let hits = owner.search_programs(
            "bulletin",
            2000,
            &*removed,
            iptv_search::ProgramSearchOptions {
                limit: 1,
                ..Default::default()
            },
        );
        assert_eq!(
            hits.len(),
            1,
            "removed content is excluded before the result limit"
        );
        assert_eq!(hits[0].channel, 0);
        assert_eq!(
            hits[0].identity,
            owner.search_channel_identity(&*before, 1).unwrap()
        );
    }

    #[tokio::test]
    async fn queued_program_reads_reject_recreated_and_reconfigured_sources() {
        let directory = tempfile::tempdir().unwrap();
        let owner = crate::Runtime::new(crate::Ports::new(directory.path().into(), |_| {
            Ok(crate::model::DatabaseKey::from_bytes([0x53; 32]))
        }));
        owner.start_services();
        let source = Source {
            name: "Generated search source".into(),
            config: config(1),
        };
        let original = source.clone();
        let (id, incarnation) = owner
            .services()
            .call(move |store| {
                let id = store.add_source(&original)?;
                Ok((id, store.source_incarnation(id)?))
            })
            .await
            .unwrap();
        let keys = vec![ChannelKey::new("generated-shared-provider-key")];
        let old_read =
            owner.read_program_index(vec![(id, incarnation.clone(), keys.clone(), 0)], 1, 0);
        let recreated = source.clone();
        let (replacement, new_incarnation) = owner
            .services()
            .call(move |store| {
                store.delete_source(id)?;
                let replacement = store.add_source(&recreated)?;
                Ok((replacement, store.source_incarnation(replacement)?))
            })
            .await
            .unwrap();
        assert_eq!(replacement, id, "SQLite reused the numeric source id");
        assert_ne!(incarnation, new_incarnation);
        assert!(old_read.await.is_err());
        assert!(
            owner
                .read_program_index(vec![(id, new_incarnation.clone(), keys.clone(), 0)], 1, 0)
                .await
                .is_ok()
        );
        let prior_configuration_read =
            owner.read_program_index(vec![(id, new_incarnation.clone(), keys.clone(), 0)], 1, 0);
        let mut edited = source;
        edited.config = config(2);
        let latest = owner
            .services()
            .call(move |store| {
                store.update_source(id, &edited)?;
                store.source_incarnation(id)
            })
            .await
            .unwrap();
        assert_ne!(new_incarnation, latest);
        assert!(prior_configuration_read.await.is_err());
        assert!(
            owner
                .read_program_index(vec![(id, latest, keys, 0)], 1, 0)
                .await
                .is_ok()
        );
        assert!(owner.drain_services());
    }
}
