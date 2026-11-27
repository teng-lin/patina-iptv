//! Typed catalog and library repositories over an owner's accepted store lane.

use super::{Services, StoreError};
use iptv_core::membership::Member;
use iptv_core::organization::{BlockKind, Blocks, Edit, Item, Sort};
use iptv_core::parental::Target;
use iptv_guide::playback::ResolutionAuthority;
use iptv_library::OrganizationScope as Scope;
use iptv_library::SourceId;
use iptv_library::{Playlist, SourcePolicy};
use iptv_providers::model::SourceConfig;
use iptv_providers::{ContentId, GroupId};
use iptv_store_sqlite::{
    ChannelOrder, ChannelQuery, ChannelScope, CustomGroup, CustomGroupId, MoveTarget, OrderScope,
    SourceSummary, Store, StoredSource,
};
use std::collections::BTreeMap;
use std::future::Future;

fn snapshot_in(store: &Store) -> Result<crate::library::Snapshot, StoreError> {
    let playlists = playlists_in(store)?;
    let mut details = BTreeMap::new();
    let mut imports = BTreeMap::new();
    for playlist in &playlists {
        imports.insert(playlist.id, store.import_status(playlist.id)?);
        if let Some(source) = store.source(playlist.id)? {
            details.insert(playlist.id, source);
        }
    }
    Ok(crate::library::Snapshot {
        playlists,
        details,
        imports,
        active: store.active_source()?,
    })
}

fn startup_source_in(store: &Store) -> Result<Option<StoredSource>, StoreError> {
    if let Some(id) = store.active_source()?
        && store.source_policy(id)?.enabled
    {
        return store.source(id);
    }
    for source in store.sources()?.iter().rev() {
        if store.source_policy(source.id)?.enabled {
            return store.source(source.id);
        }
    }
    Ok(None)
}

fn playlists_in(store: &Store) -> Result<Vec<Playlist>, StoreError> {
    store
        .sources()?
        .into_iter()
        .map(|summary| {
            let guide = store.epg_guide(summary.id)?;
            let policy = store.source_policy(summary.id)?;
            let kinds = if policy.enabled {
                store.on_demand_kinds(summary.id)?
            } else {
                (false, false)
            };
            let guides = store.guide_sources(summary.id)?;
            let config = store.source(summary.id)?.map(|stored| stored.source.config);
            let (user_agent, from_file) = match &config {
                Some(SourceConfig::M3u(config)) => (
                    config.user_agent.clone(),
                    !config.location.expose().contains("://"),
                ),
                Some(SourceConfig::Xtream(config)) => (config.user_agent.clone(), false),
                _ => (None, false),
            };
            Ok(Playlist {
                policy,
                id: summary.id,
                name: summary.name,
                kind: summary.kind,
                channels: summary.import.counts.channels,
                has_movies: kinds.0,
                has_series: kinds.1,
                movies_failed: summary.import.movies_error,
                series_failed: summary.import.series_error,
                updated_at: summary.import.last_success_at,
                failed: summary
                    .import
                    .last_error
                    .map(|kind| (kind, summary.import.last_error_at)),
                guide_failed: guide
                    .as_ref()
                    .is_some_and(|guide| guide.last_error.is_some()),
                guides,
                user_agent,
                from_file,
            })
        })
        .collect()
}

fn member_rows(
    store: &Store,
    source: SourceId,
    scope: ChannelScope,
) -> Result<Vec<Member>, StoreError> {
    Ok(store
        .load_channels(
            source,
            &ChannelQuery {
                scope,
                include_hidden: true,
                ..Default::default()
            },
        )?
        .into_iter()
        .map(|row| Member {
            id: row.channel.id.as_str().into(),
            name: row.name,
        })
        .collect())
}

pub(crate) struct CatalogRepo {
    pub(super) services: Services,
}

impl CatalogRepo {
    pub(crate) async fn source_policies(
        self,
    ) -> Result<Vec<(SourceSummary, SourcePolicy)>, StoreError> {
        self.services
            .call(|store| {
                store
                    .sources()?
                    .into_iter()
                    .map(|source| {
                        let policy = store.source_policy(source.id)?;
                        Ok((source, policy))
                    })
                    .collect()
            })
            .await
    }

    pub(crate) async fn status_snapshot(self) -> Result<crate::state::StatusSnapshot, StoreError> {
        self.services
            .call(|store| {
                let mut snapshot = crate::state::StatusSnapshot {
                    library: Some(snapshot_in(store)?),
                    combined: store.all_playlists()?,
                    ..Default::default()
                };
                for source in store.sources()? {
                    if store.source_policy(source.id)?.enabled {
                        snapshot.enabled.push(source.id);
                    }
                    if let Some(guide) = store.epg_guide(source.id)? {
                        snapshot.guides.insert(source.id, guide);
                    }
                    let results = store.epg_feed_results(source.id)?;
                    let lines = store
                        .guide_sources(source.id)?
                        .into_iter()
                        .map(|source| iptv_library::SourceLine {
                            result: results
                                .iter()
                                .find(|result| result.feed == source.feed)
                                .cloned(),
                            source,
                        })
                        .collect();
                    snapshot.lines.insert(source.id, lines);
                }
                Ok(snapshot)
            })
            .await
    }

    pub(crate) fn hide_group(
        self,
        source: SourceId,
        id: String,
    ) -> impl Future<Output = Result<(), StoreError>> + use<> {
        self.services
            .call(move |store| store.hide_group(source, &GroupId::from_stored(&id), true))
    }

    pub(crate) fn set_guide_sort(
        self,
        source: SourceId,
        scope: Scope,
        sort: Sort,
    ) -> impl Future<Output = Result<(), StoreError>> + use<> {
        self.services
            .call(move |store| store.set_guide_sort(source, &scope, sort))
    }

    pub(crate) async fn combined_resume_key(self) -> Result<Option<String>, StoreError> {
        let files = self.services.clone();
        self.services
            .read(move |store| {
                Ok(crate::playback::load_combined_channel(&files)
                    .or(store.combined_last_channel()?)
                    .or_else(|| {
                        // The first combined browse may still use a legacy single-source file.
                        let source = store.active_source().ok().flatten()?;
                        if !store.source_policy(source).ok()?.enabled {
                            return None;
                        }
                        let key = crate::playback::load_last_channel(&files, Some(source))?;
                        Some(iptv_guide::combined::qualified_key(source.0, &key))
                    }))
            })
            .await
    }

    pub(crate) fn startup_library(
        &self,
    ) -> impl Future<
        Output = Result<(Option<StoredSource>, bool, bool, crate::library::Snapshot), StoreError>,
    > + use<> {
        let directory = self.services.directory().to_owned();
        self.services.call_at_startup(move |store| {
            iptv_store_sqlite::EpgStage::remove_stale(&directory);
            iptv_store_sqlite::CatalogStage::remove_stale(&directory);
            let sources = store.sources()?;
            let mut enabled = 0;
            for source in &sources {
                enabled += usize::from(store.source_policy(source.id)?.enabled);
            }
            Ok((
                startup_source_in(store)?,
                store.all_playlists()? || enabled > 1,
                !sources.is_empty(),
                snapshot_in(store)?,
            ))
        })
    }

    pub(crate) fn update_source_policy<F>(
        self,
        id: SourceId,
        access: crate::parental::Snapshot,
        edit: F,
    ) -> impl Future<Output = Result<(SourcePolicy, bool), StoreError>> + use<F>
    where
        F: FnOnce(&mut SourcePolicy) + Send + 'static,
    {
        let services = self.services.clone();
        self.services.call(move |store| {
            crate::store_service::ParentalRepo::require_settings_write(
                store,
                &access,
                &services.settings(),
            )?;
            let mut policy = store.source_policy(id)?;
            let was_enabled = policy.enabled;
            edit(&mut policy);
            store.set_source_policy(id, policy)?;
            Ok((policy, was_enabled != policy.enabled))
        })
    }

    pub(crate) async fn organization_with_group_lists(
        &self,
        source: SourceId,
    ) -> Result<(iptv_guide::Organization, Vec<(String, String, Vec<String>)>), StoreError> {
        self.services
            .read(move |store| {
                store.read_snapshot(|store| {
                    let organization = organization_in(store, source)?;
                    let lists = group_lists_in(store, source, &organization.groups)?;
                    Ok((organization, lists))
                })
            })
            .await
    }

    pub(crate) async fn guide_blocks(&self, source: SourceId) -> Result<Blocks, StoreError> {
        self.services
            .read(move |store| store.guide_blocks(source))
            .await
    }

    pub(crate) async fn guide_channel_blocked(
        &self,
        source: SourceId,
        id: String,
    ) -> Result<bool, StoreError> {
        self.services
            .read(move |store| store.guide_channel_blocked(source, &id))
            .await
    }

    pub(crate) fn set_guide_block(
        self,
        source: SourceId,
        kind: BlockKind,
        id: String,
        blocked: bool,
    ) -> impl Future<Output = Result<(), StoreError>> + use<> {
        self.services
            .call(move |store| store.set_guide_block(source, kind, &id, blocked))
    }

    pub(crate) async fn organization_editor(
        &self,
        source: SourceId,
        scope: Scope,
        favorites: Vec<String>,
        draft_sort: Option<Sort>,
    ) -> Result<(Vec<Item>, Sort, Blocks), StoreError> {
        self.services
            .read(move |store| {
                store.read_snapshot(|store| {
                    Ok((
                        organization_rows_in(store, source, &scope, favorites, draft_sort)?,
                        store.guide_sort(source, &scope)?,
                        store.guide_blocks(source)?,
                    ))
                })
            })
            .await
    }

    pub(crate) fn set_group_preference(
        self,
        source: SourceId,
        group: String,
        preference: iptv_core::organization::GroupPreference,
    ) -> impl Future<Output = Result<(), StoreError>> + use<> {
        self.services
            .call(move |store| store.set_guide_group_preference(source, &group, preference))
    }

    pub(crate) fn save_guide_order_from(
        self,
        source: SourceId,
        scope: Scope,
        original: Vec<String>,
        order: Vec<String>,
        expected_sort: Sort,
        from_custom: bool,
    ) -> impl Future<Output = Result<(), StoreError>> + use<> {
        self.services.call(move |store| {
            store.save_guide_order_from(
                source,
                &scope,
                &original,
                &order,
                Some(expected_sort),
                from_custom,
            )
        })
    }
    pub(crate) fn snapshot(
        &self,
    ) -> impl Future<Output = Result<crate::library::Snapshot, StoreError>> + use<> {
        self.services.read(snapshot_in)
    }

    pub(crate) fn set_source_order(
        self,
        order: Vec<SourceId>,
        access: crate::parental::Snapshot,
    ) -> impl Future<Output = Result<(), StoreError>> + use<> {
        let services = self.services.clone();
        self.services.call(move |store| {
            crate::store_service::ParentalRepo::require_settings_write(
                store,
                &access,
                &services.settings(),
            )?;
            store.set_source_order(&order)
        })
    }

    pub(crate) fn set_active_source(
        self,
        source: SourceId,
    ) -> impl Future<Output = Result<(), StoreError>> + use<> {
        self.services
            .call(move |store| store.set_active_source(source))
    }

    pub(crate) fn rename_source(
        self,
        id: SourceId,
        name: String,
    ) -> impl Future<Output = Result<(), StoreError>> + use<> {
        self.services
            .call(move |store| store.rename_source(id, &name))
    }

    pub(crate) fn delete_source(
        self,
        id: SourceId,
        access: crate::parental::Snapshot,
    ) -> impl Future<Output = Result<bool, StoreError>> + use<> {
        self.services.call(move |store| {
            crate::store_service::ParentalRepo::require_admin(store, &access)?;
            store.delete_source(id)
        })
    }

    pub(crate) fn edit_guide_sources(
        self,
        id: SourceId,
        edit: iptv_library::guides::Edit,
    ) -> impl Future<Output = Result<bool, StoreError>> + use<> {
        self.services
            .call(move |store| store.edit_guide_sources(id, &edit))
    }

    pub(crate) fn set_all_playlists(
        self,
        all: bool,
    ) -> impl Future<Output = Result<(), StoreError>> + use<> {
        self.services
            .call(move |store| store.set_all_playlists(all))
    }

    pub(crate) fn collapsed_playlists(
        &self,
    ) -> impl Future<Output = Result<Vec<i64>, StoreError>> + use<> {
        self.services.call(|store| store.collapsed_playlists())
    }

    pub(crate) fn set_collapsed_playlists(
        self,
        ids: Vec<i64>,
    ) -> impl Future<Output = Result<(), StoreError>> + use<> {
        self.services
            .call(move |store| store.set_collapsed_playlists(&ids))
    }

    pub(crate) async fn custom_groups(
        self,
        source: SourceId,
    ) -> Result<Vec<CustomGroup>, StoreError> {
        self.services
            .read(move |store| store.custom_groups(source))
            .await
    }

    pub(crate) async fn members(
        self,
        source: SourceId,
        scope: ChannelScope,
    ) -> Result<Vec<Member>, StoreError> {
        self.services
            .read(move |store| member_rows(store, source, scope))
            .await
    }

    pub(crate) async fn group_members(
        self,
        source: SourceId,
        scope: ChannelScope,
    ) -> Result<(Vec<CustomGroup>, Vec<Member>), StoreError> {
        self.services
            .read(move |store| {
                Ok((
                    store.custom_groups(source)?,
                    member_rows(store, source, scope)?,
                ))
            })
            .await
    }

    pub(crate) async fn save_custom_group(
        self,
        source: SourceId,
        group: Option<CustomGroupId>,
        name: String,
    ) -> Result<(), StoreError> {
        self.services
            .call(move |store| match group {
                Some(group) => store.rename_custom_group(group, &name),
                None => store.create_custom_group(source, &name).map(|_| ()),
            })
            .await
    }

    pub(crate) async fn remove_custom_members(
        self,
        group: CustomGroupId,
        members: Vec<ContentId>,
        access: crate::parental::Snapshot,
    ) -> Result<(), StoreError> {
        self.services
            .call(move |store| {
                crate::store_service::ParentalRepo::require_group_edit(store, group, &access)?;
                store.remove_custom_members(group, &members)
            })
            .await
    }

    pub(crate) async fn transfer_custom_members(
        self,
        source: SourceId,
        from: Option<CustomGroupId>,
        to: CustomGroupId,
        members: Vec<ContentId>,
        moving: bool,
        access: crate::parental::Snapshot,
    ) -> Result<usize, StoreError> {
        self.services
            .call(move |store| {
                if moving && let Some(group) = from {
                    crate::store_service::ParentalRepo::require_group_edit(store, group, &access)?;
                }
                store.transfer_custom_members(source, from, to, &members, moving)
            })
            .await
    }

    pub(crate) async fn delete_custom_group(
        self,
        group: CustomGroupId,
        access: crate::parental::Snapshot,
    ) -> Result<(), StoreError> {
        self.services
            .call(move |store| {
                crate::store_service::ParentalRepo::require_group_edit(store, group, &access)?;
                store.delete_custom_group(group).map(|_| ())
            })
            .await
    }

    pub(crate) async fn hide_channel(
        self,
        source: SourceId,
        channel: String,
    ) -> Result<(), StoreError> {
        self.services
            .call(move |store| store.set_hidden(source, &ContentId::from_stored(channel), true))
            .await
    }

    pub(crate) async fn remember_combined_channel(self, channel: String) -> Result<(), StoreError> {
        self.services
            .call(move |store| store.set_combined_last_channel(&channel))
            .await
    }

    pub(crate) async fn channel(
        self,
        source: SourceId,
        channel: ContentId,
        mut access: crate::parental::Snapshot,
    ) -> Result<Option<crate::model::PlaybackChannel>, StoreError> {
        let services = self.services.clone();
        self.services
            .read(move |store| {
                access.access.policy = crate::parental::policy_settings(&services.settings());
                let target = Target::live(source.0, channel.as_str());
                if store.guide_channel_blocked(source, channel.as_str())?
                    || !crate::store_service::ParentalRepo::native_allowed(store, &target, &access)?
                {
                    return Ok(None);
                }
                Ok(store
                    .channel(source, &channel)?
                    .map(|row| crate::model::PlaybackChannel {
                        channel: row.channel,
                        blocked: row.blocked,
                    }))
            })
            .await
    }

    pub(crate) async fn playback_handoff(
        self,
        authority: ResolutionAuthority,
        external_group: Option<String>,
        access: crate::parental::Snapshot,
    ) -> Result<Option<crate::operations::ExternalPlaybackAuthorization>, StoreError> {
        self.services
            .read(move |store| {
                playback_handoff_in(store, &authority, external_group.as_deref(), access)
            })
            .await
    }

    pub(crate) async fn playback_handoff_ordered(
        self,
        authority: ResolutionAuthority,
        external_group: Option<String>,
        access: crate::parental::Snapshot,
    ) -> Result<Option<crate::operations::ExternalPlaybackAuthorization>, StoreError> {
        self.services
            .call(move |store| {
                playback_handoff_in(store, &authority, external_group.as_deref(), access)
            })
            .await
    }

    pub(crate) fn organization_rows(
        self,
        source: SourceId,
        kind: Scope,
        favorites: Vec<String>,
        sort: Option<Sort>,
    ) -> impl Future<Output = Result<Vec<Item>, StoreError>> + use<> {
        self.services.read(move |store| {
            store.read_snapshot(|store| organization_rows_in(store, source, &kind, favorites, sort))
        })
    }

    pub(crate) async fn edit_organization(
        self,
        source: SourceId,
        kind: Scope,
        items: Vec<Item>,
        item: Item,
        edit: Edit,
        selected: std::collections::BTreeSet<String>,
    ) -> Result<(), StoreError> {
        #[derive(PartialEq)]
        enum StoredKind {
            Channels,
            Groups,
            GroupChannels(GroupId),
            CustomChannels(CustomGroupId),
        }
        let kind = match kind {
            Scope::Channels => Ok(StoredKind::Channels),
            Scope::Groups => Ok(StoredKind::Groups),
            Scope::GroupChannels(group) => Ok(StoredKind::GroupChannels(group)),
            Scope::CustomChannels(group) => Ok(StoredKind::CustomChannels(group)),
            Scope::Favorites
                if !matches!(
                    edit,
                    Edit::Earlier | Edit::Later | Edit::First | Edit::Last | Edit::ResetOrder
                ) =>
            {
                Ok(StoredKind::Channels)
            }
            Scope::Favorites => Err(StoreError::Unavailable(
                "favorites use their ordered writer",
            )),
        };
        self.services
            .call(move |store| {
                let kind = kind?;
                let id = ContentId::from_stored(&item.id);
                let group = GroupId::from_stored(&item.id);
                let to = match edit {
                    Edit::Earlier => MoveTarget::Step(-1),
                    Edit::Later => MoveTarget::Step(1),
                    Edit::Last => MoveTarget::Index(usize::MAX),
                    _ => MoveTarget::Index(0),
                };
                match edit {
                    Edit::Visibility => match &kind {
                        StoredKind::Groups => store.hide_group(source, &group, !item.hidden)?,
                        _ => store.set_hidden(source, &id, !item.hidden)?,
                    },
                    Edit::ResetName => match &kind {
                        StoredKind::Groups => store.rename_group(source, &group, None)?,
                        _ => store.rename_channel(source, &id, None)?,
                    },
                    Edit::Earlier | Edit::Later | Edit::First | Edit::Last => match &kind {
                        StoredKind::Channels => {
                            store.move_channel_to(source, &OrderScope::All, &id, to)?;
                        }
                        StoredKind::GroupChannels(group) => {
                            store.move_channel_to(
                                source,
                                &OrderScope::Group(group.clone()),
                                &id,
                                to,
                            )?;
                        }
                        StoredKind::CustomChannels(group) => {
                            store.move_in_custom_group_to(*group, &id, to)?;
                        }
                        StoredKind::Groups => store.move_group_to(source, &group, to)?,
                    },
                    Edit::ResetOrder => match &kind {
                        StoredKind::Channels => {
                            store.reset_manual_order(source, &OrderScope::All)?
                        }
                        StoredKind::GroupChannels(group) => {
                            store.reset_manual_order(source, &OrderScope::Group(group.clone()))?;
                        }
                        StoredKind::CustomChannels(group) => {
                            store.reset_custom_group_order(*group)?
                        }
                        StoredKind::Groups => store.reset_group_order(source)?,
                    },
                    Edit::HideSelected | Edit::ShowSelected | Edit::HideAll | Edit::ShowAll => {
                        let all = matches!(edit, Edit::HideAll | Edit::ShowAll);
                        let hidden = matches!(edit, Edit::HideSelected | Edit::HideAll);
                        let ids: Vec<_> = items
                            .into_iter()
                            .filter(|item| all || selected.contains(&item.id))
                            .map(|item| item.id)
                            .collect();
                        store.set_library_visibility(
                            source,
                            &ids,
                            kind == StoredKind::Groups,
                            hidden,
                        )?;
                    }
                }
                Ok(())
            })
            .await
    }

    pub(crate) async fn favorite_provider_order(
        self,
        source: SourceId,
        keys: Vec<String>,
    ) -> Result<Vec<String>, StoreError> {
        self.services
            .call(move |store| {
                Ok(store
                    .load_channels(
                        source,
                        &ChannelQuery {
                            include_hidden: true,
                            order: ChannelOrder::Provider,
                            ..Default::default()
                        },
                    )?
                    .into_iter()
                    .filter(|row| keys.iter().any(|key| key == row.channel.id.as_str()))
                    .map(|row| row.channel.id.as_str().to_owned())
                    .collect())
            })
            .await
    }

    pub(crate) async fn rename_organization_item(
        self,
        source: SourceId,
        kind: Scope,
        id: String,
        text: String,
    ) -> Result<(), StoreError> {
        self.services
            .call(move |store| {
                let text = (!text.trim().is_empty()).then_some(text.trim());
                match kind {
                    Scope::Groups => store.rename_group(source, &GroupId::from_stored(&id), text),
                    _ => store.rename_channel(source, &ContentId::from_stored(&id), text),
                }
            })
            .await
    }
}

fn organization_in(
    store: &Store,
    source: SourceId,
) -> Result<iptv_guide::Organization, StoreError> {
    let channels = store
        .ordered_guide_channels(source, &Scope::Channels)?
        .into_iter()
        .map(|row| (row.channel.id.as_str().to_owned(), row.name, row.hidden))
        .collect();
    let mut group_preferences = std::collections::HashMap::new();
    let mut groups = Vec::new();
    let group_sort = store.guide_sort(source, &Scope::Groups)?;
    for item in store.guide_groups_with_sort(source, group_sort)? {
        group_preferences.insert(
            item.id.clone(),
            store.guide_group_preferences(source, &item.id)?,
        );
        groups.push((item.id, item.name, item.hidden));
    }
    if !group_preferences.contains_key("__other__") {
        group_preferences.insert(
            "__other__".into(),
            store.guide_group_preferences(source, "__other__")?,
        );
    }
    Ok(iptv_guide::Organization {
        channels,
        groups,
        blocks: store.guide_blocks(source)?,
        group_preferences,
        renamed_groups: store.renamed_groups(source)?,
        group_sort,
        channel_sort: store.guide_sort(source, &Scope::Channels)?,
    })
}

fn group_lists_in(
    store: &Store,
    source: SourceId,
    groups: &[(String, String, bool)],
) -> Result<Vec<(String, String, Vec<String>)>, StoreError> {
    groups
        .iter()
        .filter(|(_, _, hidden)| !hidden)
        .map(|(id, name, _)| {
            let scope = Scope::group_channels(id);
            let channels = store
                .ordered_guide_channels(source, &scope)?
                .into_iter()
                .map(|row| row.channel.id.as_str().to_owned())
                .collect();
            Ok((id.clone(), name.clone(), channels))
        })
        .collect()
}

fn organization_rows_in(
    store: &Store,
    source: SourceId,
    kind: &Scope,
    favorites: Vec<String>,
    sort: Option<Sort>,
) -> Result<Vec<Item>, StoreError> {
    if *kind == Scope::Groups {
        return match sort {
            Some(sort) => store.guide_groups_with_sort(source, sort),
            None => store.guide_groups(source),
        };
    }
    let scope = if *kind == Scope::Favorites {
        &Scope::Channels
    } else {
        kind
    };
    let rows = match sort {
        Some(sort) => store.guide_channels_with_sort(source, scope, sort),
        None => store.ordered_guide_channels(source, scope),
    }?;
    if *kind == Scope::Favorites {
        let mut by_id: std::collections::HashMap<_, _> = rows
            .into_iter()
            .map(|row| (row.channel.id.as_str().to_owned(), row))
            .collect();
        return Ok(favorites
            .into_iter()
            .filter_map(|id| {
                let row = by_id.remove(&id)?;
                Some(Item {
                    id,
                    name: row.name,
                    hidden: row.hidden,
                })
            })
            .collect());
    }
    Ok(rows
        .into_iter()
        .map(|row| Item {
            id: row.channel.id.as_str().to_owned(),
            name: row.name,
            hidden: row.hidden,
        })
        .collect())
}

fn playback_handoff_in(
    store: &Store,
    authority: &ResolutionAuthority,
    external_group: Option<&str>,
    access: crate::parental::Snapshot,
) -> Result<Option<crate::operations::ExternalPlaybackAuthorization>, StoreError> {
    let source = SourceId(authority.source());
    let channel = &authority.channel().id;
    let target = Target::live(source.0, channel.as_str());
    store.read_snapshot(|store| {
        let Some(grants) =
            crate::store_service::ParentalRepo::native_grants(store, &target, &access)?
        else {
            return Ok(None);
        };
        if !store.source_policy(source)?.enabled
            || store.guide_channel_blocked(source, channel.as_str())?
        {
            return Ok(None);
        }
        let Some(stored) = store.source(source)? else {
            return Ok(None);
        };
        if let Some(group) = external_group {
            let scope = iptv_library::OrganizationScope::group_channels(group);
            if !store
                .guide_group_preferences(source, group)?
                .external_player
                || !store
                    .ordered_guide_channels(source, &scope)?
                    .iter()
                    .any(|row| &row.channel.id == channel)
            {
                return Ok(None);
            }
        }
        let Some(row) = store.channel(source, channel)? else {
            return Ok(None);
        };
        let allowed = !row.blocked
            && stored.incarnation == authority.incarnation()
            && stored.source.config == *authority.config()
            && row.channel == *authority.channel()
            && access.playback_current(&target, &grants);
        Ok(allowed
            .then(|| crate::operations::ExternalPlaybackAuthorization::new(access, target, grants)))
    })
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    use iptv_providers::{
        SecretUrl,
        model::{M3uSourceConfig, Source},
    };
    #[test]
    fn startup_ignores_disabled_active_source_and_recovers_when_reenabled() {
        let mut s = Store::open_in_memory().unwrap();
        let source = |name: &str| Source {
            name: name.into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("http://example.test/list.m3u"),
                user_agent: None,
                referrer: None,
            }),
        };
        let a = s.add_source(&source("A")).unwrap();
        let b = s.add_source(&source("B")).unwrap();
        s.set_active_source(a).unwrap();
        assert_eq!(startup_source_in(&s).unwrap().unwrap().id, a);
        let off = SourcePolicy {
            enabled: false,
            ..Default::default()
        };
        s.set_source_policy(a, off).unwrap();
        assert_eq!(startup_source_in(&s).unwrap().unwrap().id, b);
        s.set_source_policy(b, off).unwrap();
        assert!(startup_source_in(&s).unwrap().is_none());
        s.set_source_policy(a, Default::default()).unwrap();
        assert_eq!(startup_source_in(&s).unwrap().unwrap().id, a);
    }
}
