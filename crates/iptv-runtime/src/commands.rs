//! Named library operations. Views send values and await results; only this
//! module creates database work items.
use super::store_service::Services;
use iptv_core::membership::Member;
use iptv_core::parental::Target;
use iptv_guide::playback::ResolutionAuthority;
use iptv_library::{CustomGroupId, SourceId};
use iptv_providers::ContentId;
use iptv_store_sqlite::{ChannelQuery, ChannelScope, CustomGroup, Store, StoreError};
use std::{collections::BTreeSet, future::Future};

fn custom_groups_native(
    services: &Services,
    source: SourceId,
) -> impl Future<Output = Result<Vec<CustomGroup>, StoreError>> + use<> {
    services.read(move |store| store.custom_groups(source))
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

fn members_native(
    services: &Services,
    source: SourceId,
    scope: ChannelScope,
) -> impl Future<Output = Result<Vec<Member>, StoreError>> + use<> {
    services.read(move |store| member_rows(store, source, scope))
}

fn group_members_native(
    services: &Services,
    source: SourceId,
    scope: ChannelScope,
) -> impl Future<Output = Result<(Vec<CustomGroup>, Vec<Member>), StoreError>> + use<> {
    services.read(move |store| {
        Ok((
            store.custom_groups(source)?,
            member_rows(store, source, scope)?,
        ))
    })
}

fn save_custom_group_native(
    services: &Services,
    source: SourceId,
    group: Option<CustomGroupId>,
    name: String,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services.call(move |store| match group {
        Some(group) => store.rename_custom_group(group, &name),
        None => store.create_custom_group(source, &name).map(|_| ()),
    })
}

fn remove_custom_members_native(
    services: &Services,
    group: CustomGroupId,
    members: Vec<ContentId>,
    access: crate::parental::Snapshot,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services.call(move |store| {
        crate::parental::require_group_edit(store, group, &access)?;
        store.remove_custom_members(group, &members)
    })
}

fn transfer_custom_members_native(
    services: &Services,
    source: SourceId,
    from: Option<CustomGroupId>,
    to: CustomGroupId,
    members: Vec<ContentId>,
    moving: bool,
    access: crate::parental::Snapshot,
) -> impl Future<Output = Result<usize, StoreError>> + use<> {
    services.call(move |store| {
        if moving && let Some(group) = from {
            crate::parental::require_group_edit(store, group, &access)?;
        }
        store.transfer_custom_members(source, from, to, &members, moving)
    })
}

fn delete_custom_group_native(
    services: &Services,
    group: CustomGroupId,
    access: crate::parental::Snapshot,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services.call(move |store| {
        crate::parental::require_group_edit(store, group, &access)?;
        store.delete_custom_group(group).map(|_| ())
    })
}

fn hide_channel_native(
    services: &Services,
    source: SourceId,
    channel: String,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services.call(move |store| store.set_hidden(source, &ContentId::from_stored(channel), true))
}

fn remember_combined_channel_native(
    services: &Services,
    key: String,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services.call(move |store| store.set_combined_last_channel(&key))
}

fn channel_native(
    services: &Services,
    source: SourceId,
    channel: ContentId,
    mut access: crate::parental::Snapshot,
) -> impl Future<Output = Result<Option<crate::model::PlaybackChannel>, StoreError>> + use<> {
    let current = services.clone();
    services.read(move |store| {
        access.access.policy = crate::parental::policy_settings(&current.settings());
        let target = Target::live(source.0, channel.as_str());
        if store.guide_channel_blocked(source, channel.as_str())?
            || !crate::parental::native_allowed(store, &target, &access)?
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
}

/// Parental policy verified against committed storage for external playback.
pub struct ExternalPlaybackAuthorization {
    access: crate::parental::Snapshot,
    grants: Vec<Target>,
}

impl ExternalPlaybackAuthorization {
    /// Check immediately before granting the relay admission, without yielding.
    pub fn is_current(&self) -> bool {
        self.access.inspection_current(&self.grants)
    }
}

pub(crate) fn external_playback_check(
    store: &Store,
    authority: &ResolutionAuthority,
    group: &str,
    access: crate::parental::Snapshot,
) -> Result<Option<ExternalPlaybackAuthorization>, StoreError> {
    let source = SourceId(authority.source());
    let channel = &authority.channel().id;
    let target = Target::live(source.0, channel.as_str());
    store.read_snapshot(|store| {
        let Some(grants) = crate::parental::native_grants(store, &target, &access)? else {
            return Ok(None);
        };
        if !store.source_policy(source)?.enabled
            || !store
                .guide_group_preferences(source, group)?
                .external_player
            || store.guide_channel_blocked(source, channel.as_str())?
        {
            return Ok(None);
        }
        let Some(stored) = store.source(source)? else {
            return Ok(None);
        };
        let scope = Kind::group_channels(group);
        if !store
            .ordered_guide_channels(source, &scope)?
            .iter()
            .any(|row| &row.channel.id == channel)
        {
            return Ok(None);
        }
        let Some(row) = store.channel(source, channel)? else {
            return Ok(None);
        };
        Ok((!row.blocked
            && stored.incarnation == authority.incarnation()
            && &stored.source.config == authority.config()
            && &row.channel == authority.channel()
            && access.inspection_current(&grants))
        .then_some(ExternalPlaybackAuthorization { access, grants }))
    })
}

fn save_search_history_native(
    services: &Services,
    history: Vec<String>,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services.call(move |store| store.set_search_history(&history))
}

use iptv_core::organization::{BlockKind, Blocks, Edit, Item, Sort};
use iptv_guide::Organization;
use iptv_library::OrganizationScope as Kind;
use iptv_providers::GroupId;
use iptv_store_sqlite::{ChannelOrder, OrderScope};

fn organization_rows_in(
    store: &Store,
    source: SourceId,
    kind: &Kind,
    favorites: Vec<String>,
    sort: Option<Sort>,
) -> Result<Vec<Item>, StoreError> {
    if *kind == Kind::Groups {
        return match sort {
            Some(sort) => store.guide_groups_with_sort(source, sort),
            None => store.guide_groups(source),
        };
    }
    let scope = if *kind == Kind::Favorites {
        &Kind::Channels
    } else {
        kind
    };
    let rows = match sort {
        Some(sort) => store.guide_channels_with_sort(source, scope, sort),
        None => store.ordered_guide_channels(source, scope),
    }?;
    if *kind == Kind::Favorites {
        // The source's favorite writer publishes membership and order optimistically.
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

pub(crate) fn organization_in(store: &Store, source: SourceId) -> Result<Organization, StoreError> {
    let channels = store
        .ordered_guide_channels(source, &Kind::Channels)?
        .into_iter()
        .map(|r| (r.channel.id.as_str().to_owned(), r.name, r.hidden))
        .collect();
    let mut group_preferences = std::collections::HashMap::new();
    let mut groups = Vec::new();
    for item in store.guide_groups(source)? {
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
    Ok(Organization {
        channels,
        groups,
        blocks: store.guide_blocks(source)?,
        group_preferences,
        channel_sort: store.guide_sort(source, &Kind::Channels)?,
    })
}
fn edit_organization_native(
    services: &Services,
    source: SourceId,
    kind: Kind,
    items: Vec<Item>,
    item: Item,
    edit: Edit,
    selected: BTreeSet<String>,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    #[derive(PartialEq)]
    enum StoredKind {
        Channels,
        Groups,
        GroupChannels(GroupId),
        CustomChannels(CustomGroupId),
    }
    let kind = match kind {
        Kind::Channels => Ok(StoredKind::Channels),
        Kind::Groups => Ok(StoredKind::Groups),
        Kind::GroupChannels(group) => Ok(StoredKind::GroupChannels(group)),
        Kind::CustomChannels(group) => Ok(StoredKind::CustomChannels(group)),
        Kind::Favorites
            if !matches!(
                edit,
                Edit::Earlier | Edit::Later | Edit::First | Edit::Last | Edit::ResetOrder
            ) =>
        {
            Ok(StoredKind::Channels)
        }
        Kind::Favorites => Err(StoreError::Unavailable(
            "favorites use their ordered writer",
        )),
    };
    services.call(move |store| {
        let kind = kind?;
        let id = ContentId::from_stored(&item.id);
        let group = GroupId::from_stored(&item.id);
        let to = match edit {
            Edit::Earlier => iptv_store_sqlite::MoveTarget::Step(-1),
            Edit::Later => iptv_store_sqlite::MoveTarget::Step(1),
            Edit::Last => iptv_store_sqlite::MoveTarget::Index(usize::MAX),
            _ => iptv_store_sqlite::MoveTarget::Index(0),
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
                StoredKind::GroupChannels(g) => {
                    store.move_channel_to(source, &OrderScope::Group(g.clone()), &id, to)?;
                }
                StoredKind::CustomChannels(g) => {
                    store.move_in_custom_group_to(*g, &id, to)?;
                }
                StoredKind::Groups => store.move_group_to(source, &group, to)?,
            },
            Edit::ResetOrder => match &kind {
                StoredKind::Channels => store.reset_manual_order(source, &OrderScope::All)?,
                StoredKind::GroupChannels(g) => {
                    store.reset_manual_order(source, &OrderScope::Group(g.clone()))?
                }
                StoredKind::CustomChannels(g) => store.reset_custom_group_order(*g)?,
                StoredKind::Groups => store.reset_group_order(source)?,
            },
            Edit::HideSelected | Edit::ShowSelected | Edit::HideAll | Edit::ShowAll => {
                let all = matches!(edit, Edit::HideAll | Edit::ShowAll);
                let hidden = matches!(edit, Edit::HideSelected | Edit::HideAll);
                let ids: Vec<_> = items
                    .into_iter()
                    .filter(|i| all || selected.contains(&i.id))
                    .map(|i| i.id)
                    .collect();
                store.set_library_visibility(source, &ids, kind == StoredKind::Groups, hidden)?;
            }
        }
        Ok(())
    })
}

fn favorite_provider_order_native(
    services: &Services,
    source: SourceId,
    keys: Vec<String>,
) -> impl Future<Output = Result<Vec<String>, StoreError>> + use<> {
    services.call(move |store| {
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
            .collect::<Vec<_>>())
    })
}

fn rename_organization_item_native(
    services: &Services,
    source: SourceId,
    kind: Kind,
    id: String,
    text: String,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services.call(move |store| {
        let text = (!text.trim().is_empty()).then_some(text.trim());
        match kind {
            Kind::Groups => store.rename_group(source, &GroupId::from_stored(&id), text),
            _ => store.rename_channel(source, &ContentId::from_stored(&id), text),
        }
    })
}

pub(crate) fn import_legacy_history(
    store: &mut iptv_store_sqlite::Store,
    path: &std::path::Path,
) -> Result<(), iptv_store_sqlite::StoreError> {
    let Ok(bytes) = std::fs::read(path) else {
        return Ok(());
    };
    let list: Vec<String> = serde_json::from_slice(&bytes).unwrap_or_default();
    if !list.is_empty() && store.search_history()?.is_empty() {
        let list = SearchHistory::from_list(list, SearchHistory::DEFAULT_CAPACITY).to_list();
        store.set_search_history(&list)?;
        log::info!(
            "iptv search: moved {} recent searches into the store",
            list.len()
        );
    }
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!(
            "iptv search: could not delete the old recent searches file ({})",
            error.kind()
        );
    }
    Ok(())
}

fn load_search_history_native(
    services: &Services,
    legacy: std::path::PathBuf,
) -> impl Future<Output = Result<Vec<String>, StoreError>> + use<> {
    services.call(move |store| {
        import_legacy_history(store, &legacy)?;
        store.search_history()
    })
}

use iptv_search::SearchHistory;

use super::state::OrganizationSnapshot;
async fn load_organization_native(
    services: &Services,
    source: SourceId,
) -> Result<OrganizationSnapshot, StoreError> {
    services
        .read(move |store| {
            store.read_snapshot(|store| {
                let organization = organization_in(store, source)?;
                let lists = group_lists_in(store, source, &organization.groups)?;
                Ok(OrganizationSnapshot {
                    organization,
                    lists,
                })
            })
        })
        .await
}

/// Includes empty custom groups so their membership can be populated later.
fn group_lists_in(
    store: &Store,
    source: SourceId,
    groups: &[(String, String, bool)],
) -> Result<Vec<(String, String, Vec<String>)>, StoreError> {
    groups
        .iter()
        .filter(|(_, _, hidden)| !hidden)
        .map(|(id, name, _)| {
            let scope = Kind::group_channels(id);
            let channels = store
                .ordered_guide_channels(source, &scope)?
                .into_iter()
                .map(|row| row.channel.id.as_str().to_owned())
                .collect();
            Ok((id.clone(), name.clone(), channels))
        })
        .collect()
}

fn startup_library_native(
    services: &Services,
) -> impl std::future::Future<
    Output = Result<
        (
            Option<iptv_store_sqlite::StoredSource>,
            bool,
            bool,
            super::library::Snapshot,
        ),
        StoreError,
    >,
> + use<> {
    let directory = services.directory().to_owned();
    services.call_at_startup(move |store| {
        iptv_store_sqlite::EpgStage::remove_stale(&directory);
        iptv_store_sqlite::CatalogStage::remove_stale(&directory);
        let sources = store.sources()?;
        let mut enabled = 0;
        for source in &sources {
            enabled += usize::from(store.source_policy(source.id)?.enabled);
        }
        Ok((
            super::library::startup_source(store)?,
            store.all_playlists()? || enabled > 1,
            !sources.is_empty(),
            super::library::snapshot_in(store)?,
        ))
    })
}

fn source_policies_native(
    services: &Services,
) -> impl std::future::Future<
    Output = Result<
        Vec<(iptv_store_sqlite::SourceSummary, iptv_library::SourcePolicy)>,
        StoreError,
    >,
> + use<> {
    services.call(|store| {
        store
            .sources()?
            .into_iter()
            .map(|source| {
                let policy = store.source_policy(source.id)?;
                Ok((source, policy))
            })
            .collect::<Result<Vec<_>, StoreError>>()
    })
}

fn guide_status_snapshot_native(
    services: &Services,
) -> impl std::future::Future<Output = Result<super::state::StatusSnapshot, StoreError>> + use<> {
    services.call(|store| {
        let mut snapshot = super::state::StatusSnapshot {
            library: Some(super::library::snapshot_in(store)?),
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
}

fn description_native(
    services: &Services,
    source: SourceId,
    key: iptv_epg::ChannelKey,
    start: iptv_epg::UnixTime,
) -> impl std::future::Future<Output = Result<Option<String>, StoreError>> + use<> {
    services.read(move |store| store.epg_description(source, &key, start))
}

fn guide_sources_native(
    services: &Services,
    source: SourceId,
) -> impl std::future::Future<
    Output = Result<
        (
            Option<iptv_store_sqlite::StoredGuide>,
            Vec<iptv_library::GuideSource>,
        ),
        StoreError,
    >,
> + use<> {
    services.call(move |store| Ok((store.epg_guide(source)?, store.guide_sources(source)?)))
}

fn clear_guides_native(
    services: &Services,
    sources: Vec<SourceId>,
) -> impl std::future::Future<Output = Result<(bool, Vec<super::imports::Imported>), StoreError>> + use<>
{
    services.call(move |store| {
        let mut deleted = true;
        let mut imports = Vec::new();
        for source in sources {
            if let Err(e) = store.delete_epg(source) {
                log::warn!("iptv data: could not delete stored guide: {e}");
                deleted = false;
                continue;
            }
            if let Ok((groups, channels)) = super::stored::catalog_in(store, source) {
                imports.push(super::imports::Imported {
                    groups,
                    channels,
                    playback: super::imports::Streams::Current,
                    guide: iptv_epg::GuideStore::default(),
                    now: super::imports::now_unix(),
                    source,
                    resume: false,
                });
            }
        }
        Ok((deleted, imports))
    })
}

fn guide_page_native(
    services: &Services,
    source: SourceId,
    incarnation: Option<String>,
    request: iptv_guide::pages::PageRequest,
) -> impl std::future::Future<Output = Result<iptv_epg::GuideStore, StoreError>> + use<> {
    services.read(move |store| {
        store.read_snapshot(|store| {
            let Some(incarnation) = incarnation else {
                return Err(StoreError::Unavailable(
                    "guide page has no source authority",
                ));
            };
            if store.source(source)?.is_none()
                || !store.source_policy(source)?.enabled
                || store.source_incarnation(source)? != incarnation
            {
                return Err(StoreError::Unavailable("guide page source changed"));
            }
            let window = iptv_store_sqlite::EpgWindow {
                from: request.from,
                to: request.to,
                ..iptv_store_sqlite::EpgWindow::from(0, 0)
            };
            store.load_epg(source, &window)
        })
    })
}

fn combined_resume_key_native(
    services: &Services,
) -> impl std::future::Future<Output = Result<Option<String>, StoreError>> + use<> {
    let files = services.clone();
    services.read(move |store| {
        let s = store;
        Ok(super::playback::load_combined_channel(&files)
            .or(s.combined_last_channel()?)
            .or_else(|| {
                // On first opening the combined browser, keep the previous
                // single-source startup preference, including legacy files.
                let source = s.active_source().ok().flatten()?;
                if !s.source_policy(source).ok()?.enabled {
                    return None;
                }
                let key = super::playback::load_last_channel(&files, Some(source))?;
                Some(iptv_guide::combined::qualified_key(source.0, &key))
            }))
    })
}

pub fn custom_groups(
    owner: &crate::Runtime,
    source: SourceId,
) -> impl Future<Output = Result<Vec<CustomGroup>, crate::Error>> + use<> {
    let services = owner.services();
    async move {
        custom_groups_native(&services, source)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn members(
    owner: &crate::Runtime,
    source: SourceId,
    scope: ChannelScope,
) -> impl Future<Output = Result<Vec<Member>, crate::Error>> + use<> {
    let services = owner.services();
    async move {
        members_native(&services, source, scope)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn group_members(
    owner: &crate::Runtime,
    source: SourceId,
    scope: ChannelScope,
) -> impl Future<Output = Result<(Vec<CustomGroup>, Vec<Member>), crate::Error>> + use<> {
    let services = owner.services();
    async move {
        group_members_native(&services, source, scope)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn save_custom_group(
    owner: &crate::Runtime,
    source: SourceId,
    group: Option<CustomGroupId>,
    name: String,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let services = owner.services();
    async move {
        save_custom_group_native(&services, source, group, name)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn remove_custom_members(
    owner: &crate::Runtime,
    group: CustomGroupId,
    members: Vec<ContentId>,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let services = owner.services();
    let access = owner.parental_snapshot();
    async move {
        remove_custom_members_native(&services, group, members, access)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn transfer_custom_members(
    owner: &crate::Runtime,
    source: SourceId,
    from: Option<CustomGroupId>,
    to: CustomGroupId,
    members: Vec<ContentId>,
    moving: bool,
) -> impl Future<Output = Result<usize, crate::Error>> + use<> {
    let services = owner.services();
    let access = owner.parental_snapshot();
    async move {
        transfer_custom_members_native(&services, source, from, to, members, moving, access)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn delete_custom_group(
    owner: &crate::Runtime,
    group: CustomGroupId,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let services = owner.services();
    let access = owner.parental_snapshot();
    async move {
        delete_custom_group_native(&services, group, access)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn hide_channel(
    owner: &crate::Runtime,
    source: SourceId,
    channel: String,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let services = owner.services();
    async move {
        hide_channel_native(&services, source, channel)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn remember_combined_channel(
    owner: &crate::Runtime,
    key: String,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let services = owner.services();
    async move {
        remember_combined_channel_native(&services, key)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn channel(
    owner: &crate::Runtime,
    source: SourceId,
    channel: ContentId,
) -> impl Future<Output = Result<Option<crate::model::PlaybackChannel>, crate::Error>> + use<> {
    let services = owner.services();
    let access = owner.parental_snapshot();
    async move {
        channel_native(&services, source, channel, access)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn save_search_history(
    owner: &crate::Runtime,
    history: Vec<String>,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    // Submit now: polling order must never let an older save follow a clear.
    let saved = save_search_history_native(&owner.services(), history);
    async move { saved.await.map_err(crate::Error::from) }
}

/// Clears durable history and any unconsumed legacy file in writer order.
pub fn clear_search_history(
    owner: &crate::Runtime,
    legacy: std::path::PathBuf,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let cleared = owner.services().call(move |store| {
        store.set_search_history(&[])?;
        match std::fs::remove_file(legacy) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(StoreError::Unavailable(
                "legacy search history could not be removed",
            )),
        }
    });
    async move { cleared.await.map_err(crate::Error::from) }
}

pub fn organization_items(
    owner: &crate::Runtime,
    source: SourceId,
    kind: Kind,
    favorites: Vec<String>,
) -> impl Future<Output = Result<Vec<Item>, crate::Error>> + use<> {
    let result = owner.services().read(move |store| {
        store.read_snapshot(|store| organization_rows_in(store, source, &kind, favorites, None))
    });
    async move { Ok(result.await?) }
}

/// Returns the saved guide blocks for the editor.
pub async fn guide_blocks(
    owner: &crate::Runtime,
    source: SourceId,
) -> Result<Blocks, crate::Error> {
    Ok(owner
        .services()
        .read(move |store| store.guide_blocks(source))
        .await?)
}

/// Checks the current store before accessing the channel's stream.
pub async fn guide_channel_blocked(
    owner: &crate::Runtime,
    source: SourceId,
    id: String,
) -> Result<bool, crate::Error> {
    Ok(owner
        .services()
        .read(move |store| store.guide_channel_blocked(source, &id))
        .await?)
}

/// Writes a guide preference without invoking the parental PIN flow.
pub fn set_guide_block(
    owner: &crate::Runtime,
    source: SourceId,
    kind: BlockKind,
    id: String,
    blocked: bool,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let result = owner
        .services()
        .call(move |store| store.set_guide_block(source, kind, &id, blocked));
    async move { Ok(result.await?) }
}

/// One consistent snapshot of a guide editor's rows and preferences.
#[derive(Debug)]
pub struct OrganizationItems {
    pub rows: Vec<Item>,
    pub sort: Sort,
    pub blocks: Blocks,
}

/// Reads the scope and all its visible and hidden items together.
pub async fn organization_editor(
    owner: &crate::Runtime,
    source: SourceId,
    scope: Kind,
    favorites: Vec<String>,
    draft_sort: Option<Sort>,
) -> Result<OrganizationItems, crate::Error> {
    Ok(owner
        .services()
        .read(move |store| {
            store.read_snapshot(|store| {
                Ok(OrganizationItems {
                    rows: organization_rows_in(store, source, &scope, favorites, draft_sort)?,
                    sort: store.guide_sort(source, &scope)?,
                    blocks: store.guide_blocks(source)?,
                })
            })
        })
        .await?)
}

pub fn set_group_preference(
    owner: &crate::Runtime,
    source: SourceId,
    group: String,
    preference: iptv_core::organization::GroupPreference,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let saved = owner
        .services()
        .call(move |store| store.set_guide_group_preference(source, &group, preference));
    async move { Ok(saved.await?) }
}

/// Publishes a sort preference on the native writer.
pub fn set_organization_sort(
    owner: &crate::Runtime,
    source: SourceId,
    scope: Kind,
    sort: Sort,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let result = owner
        .services()
        .call(move |store| store.set_guide_sort(source, &scope, sort));
    async move { Ok(result.await?) }
}

/// Hides a group without changing its channels’ own visibility.
pub fn hide_group(
    owner: &crate::Runtime,
    source: SourceId,
    id: String,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let result = owner
        .services()
        .call(move |store| store.hide_group(source, &GroupId::from_stored(&id), true));
    async move { Ok(result.await?) }
}

/// Commits a complete reorder against the list the editor opened.
pub fn save_organization_order(
    owner: &crate::Runtime,
    source: SourceId,
    scope: Kind,
    original: Vec<String>,
    order: Vec<String>,
    expected_sort: Sort,
    from_custom: bool,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let result = owner.services().call(move |store| {
        store.save_guide_order_from(
            source,
            &scope,
            &original,
            &order,
            Some(expected_sort),
            from_custom,
        )
    });
    async move { Ok(result.await?) }
}

/// Applies an edit using the items and selection from the caller's view.
pub fn edit_organization(
    owner: &crate::Runtime,
    source: SourceId,
    kind: Kind,
    items: Vec<Item>,
    item: Item,
    edit: Edit,
    selected: BTreeSet<String>,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let services = owner.services();
    async move {
        edit_organization_native(&services, source, kind, items, item, edit, selected)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn favorite_provider_order(
    owner: &crate::Runtime,
    source: SourceId,
    keys: Vec<String>,
) -> impl Future<Output = Result<Vec<String>, crate::Error>> + use<> {
    let services = owner.services();
    async move {
        favorite_provider_order_native(&services, source, keys)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn rename_organization_item(
    owner: &crate::Runtime,
    source: SourceId,
    kind: Kind,
    id: String,
    text: String,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let services = owner.services();
    async move {
        rename_organization_item_native(&services, source, kind, id, text)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn load_search_history(
    owner: &crate::Runtime,
    legacy: std::path::PathBuf,
) -> impl Future<Output = Result<Vec<String>, crate::Error>> + use<> {
    // Legacy migration is a write and follows invocation order, like saves.
    let loaded = load_search_history_native(&owner.services(), legacy);
    async move { loaded.await.map_err(crate::Error::from) }
}

pub(crate) fn load_organization(
    owner: &crate::Runtime,
    source: SourceId,
) -> impl Future<Output = Result<OrganizationSnapshot, crate::Error>> + use<> {
    let services = owner.services();
    async move {
        load_organization_native(&services, source)
            .await
            .map_err(crate::Error::from)
    }
}

pub(crate) fn startup_library(
    services: &Services,
) -> impl std::future::Future<
    Output = Result<
        (
            Option<iptv_store_sqlite::StoredSource>,
            bool,
            bool,
            super::library::Snapshot,
        ),
        crate::Error,
    >,
> + use<> {
    let startup = startup_library_native(services);
    async move { startup.await.map_err(crate::Error::from) }
}

pub(crate) fn source_policies(
    owner: &crate::Runtime,
) -> impl std::future::Future<
    Output = Result<
        Vec<(iptv_store_sqlite::SourceSummary, iptv_library::SourcePolicy)>,
        crate::Error,
    >,
> + use<> {
    let services = owner.services();
    async move {
        source_policies_native(&services)
            .await
            .map_err(crate::Error::from)
    }
}

pub(crate) fn guide_status_snapshot(
    owner: &crate::Runtime,
) -> impl std::future::Future<Output = Result<super::state::StatusSnapshot, crate::Error>> + use<> {
    let services = owner.services();
    async move {
        guide_status_snapshot_native(&services)
            .await
            .map_err(crate::Error::from)
    }
}

pub(crate) fn description(
    owner: &crate::Runtime,
    source: SourceId,
    key: iptv_epg::ChannelKey,
    start: iptv_epg::UnixTime,
) -> impl std::future::Future<Output = Result<Option<String>, crate::Error>> + use<> {
    let services = owner.services();
    async move {
        description_native(&services, source, key, start)
            .await
            .map_err(crate::Error::from)
    }
}

pub(crate) fn guide_sources(
    owner: &crate::Runtime,
    source: SourceId,
) -> impl std::future::Future<
    Output = Result<
        (
            Option<iptv_store_sqlite::StoredGuide>,
            Vec<iptv_library::GuideSource>,
        ),
        crate::Error,
    >,
> + use<> {
    let services = owner.services();
    async move {
        guide_sources_native(&services, source)
            .await
            .map_err(crate::Error::from)
    }
}

pub(crate) fn clear_guides(
    owner: &crate::Runtime,
    sources: Vec<SourceId>,
) -> impl std::future::Future<Output = Result<(bool, Vec<super::imports::Imported>), crate::Error>> + use<>
{
    let services = owner.services();
    async move {
        clear_guides_native(&services, sources)
            .await
            .map_err(crate::Error::from)
    }
}

pub(crate) fn guide_listing_starts(
    owner: &crate::Runtime,
    source: SourceId,
    cutoff: iptv_epg::UnixTime,
) -> impl std::future::Future<
    Output = Result<
        std::collections::HashMap<iptv_epg::ChannelKey, iptv_epg::UnixTime>,
        crate::Error,
    >,
> + use<> {
    let starts = owner
        .services()
        .read(move |store| store.epg_listing_starts(source, cutoff));
    async move { Ok(starts.await?) }
}

pub(crate) fn guide_page(
    owner: &crate::Runtime,
    source: SourceId,
    incarnation: Option<String>,
    request: iptv_guide::pages::PageRequest,
) -> impl std::future::Future<Output = Result<iptv_epg::GuideStore, crate::Error>> + use<> {
    let services = owner.services();
    async move {
        guide_page_native(&services, source, incarnation, request)
            .await
            .map_err(crate::Error::from)
    }
}

pub fn combined_resume_key(
    owner: &crate::Runtime,
) -> impl std::future::Future<Output = Result<Option<String>, crate::Error>> + use<> {
    let services = owner.services();
    async move {
        combined_resume_key_native(&services)
            .await
            .map_err(crate::Error::from)
    }
}

/// The guide after every write queued before this request (deletion or rematching).
pub fn stored_guide(
    owner: &super::Runtime,
    source: SourceId,
    now: iptv_epg::UnixTime,
) -> impl Future<Output = Result<iptv_epg::GuideStore, super::Error>> + use<> {
    let window = super::paging::window(now, super::stored::keep_past_days(&owner.settings()));
    let read = owner
        .services()
        .call(move |store| store.load_epg(source, &super::paging::store_window(window)));
    async move { Ok(read.await?) }
}
