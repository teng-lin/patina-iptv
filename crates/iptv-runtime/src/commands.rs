//! Named library operations. Views send values and await results; only this
//! module creates database work items.
use super::store_service;
use iptv_core::membership::Member;
use iptv_library::{CustomGroupId, SourceId};
use iptv_providers::ContentId;
use iptv_store_sqlite::{ChannelQuery, ChannelScope, CustomGroup, Store, StoreError};
use std::future::Future;

pub fn custom_groups(
    source: SourceId,
) -> impl Future<Output = Result<Vec<CustomGroup>, StoreError>> {
    store_service::read(move |store| store.custom_groups(source))
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

pub fn members(
    source: SourceId,
    scope: ChannelScope,
) -> impl Future<Output = Result<Vec<Member>, StoreError>> {
    store_service::read(move |store| member_rows(store, source, scope))
}

pub fn group_members(
    source: SourceId,
    scope: ChannelScope,
) -> impl Future<Output = Result<(Vec<CustomGroup>, Vec<Member>), StoreError>> {
    store_service::read(move |store| {
        Ok((
            store.custom_groups(source)?,
            member_rows(store, source, scope)?,
        ))
    })
}

pub fn save_custom_group(
    source: SourceId,
    group: Option<CustomGroupId>,
    name: String,
) -> impl Future<Output = Result<(), StoreError>> {
    store_service::call(move |store| match group {
        Some(group) => store.rename_custom_group(group, &name),
        None => store.create_custom_group(source, &name).map(|_| ()),
    })
}

pub fn remove_custom_members(
    group: CustomGroupId,
    members: Vec<ContentId>,
) -> impl Future<Output = Result<(), StoreError>> {
    store_service::call(move |store| store.remove_custom_members(group, &members))
}

pub fn transfer_custom_members(
    source: SourceId,
    from: Option<CustomGroupId>,
    to: CustomGroupId,
    members: Vec<ContentId>,
    moving: bool,
) -> impl Future<Output = Result<usize, StoreError>> {
    store_service::call(move |store| {
        store.transfer_custom_members(source, from, to, &members, moving)
    })
}

pub fn delete_custom_group(group: CustomGroupId) -> impl Future<Output = Result<(), StoreError>> {
    store_service::call(move |store| store.delete_custom_group(group).map(|_| ()))
}

pub fn hide_channel(
    source: SourceId,
    channel: String,
) -> impl Future<Output = Result<(), StoreError>> {
    store_service::call(move |store| {
        store.set_hidden(source, &ContentId::from_stored(channel), true)
    })
}

pub fn remember_combined_channel(key: String) -> impl Future<Output = Result<(), StoreError>> {
    store_service::call(move |store| store.set_combined_last_channel(&key))
}

pub fn channel(
    source: SourceId,
    channel: ContentId,
) -> impl Future<Output = Result<Option<iptv_store_sqlite::ChannelRow>, StoreError>> {
    store_service::read(move |store| store.channel(source, &channel))
}

pub fn save_search_history(history: Vec<String>) -> impl Future<Output = Result<(), StoreError>> {
    store_service::call(move |store| store.set_search_history(&history))
}

use iptv_core::organization::{Action as Edit, Editor, Item};
use iptv_guide::Organization;
use iptv_library::OrganizationScope as Kind;
use iptv_providers::GroupId;
use iptv_store_sqlite::{ChannelOrder, OrderScope};

pub async fn organization_items(
    source: SourceId,
    kind: Kind,
    favorites: Vec<String>,
) -> Result<Vec<Item>, StoreError> {
    // Edits reload their rows only after the edit's reply: the reader sees it.
    store_service::read(move |store| {
        let org = organization_in(store, source)?;
        let rows = match kind {
            Kind::Channels => org.channels,
            Kind::Groups => org.groups,
            Kind::Favorites => {
                let names: std::collections::HashMap<_, _> = org
                    .channels
                    .into_iter()
                    .map(|(id, name, hidden)| (id, (name, hidden)))
                    .collect();
                favorites
                    .into_iter()
                    .filter_map(|id| {
                        names
                            .get(&id)
                            .map(|(name, hidden)| (id.clone(), name.clone(), *hidden))
                    })
                    .collect()
            }
            Kind::GroupChannels(group) => store
                .load_channels(
                    source,
                    &ChannelQuery {
                        scope: ChannelScope::Group(group),
                        include_hidden: true,
                        ..Default::default()
                    },
                )?
                .into_iter()
                .map(|r| (r.channel.id.as_str().to_owned(), r.name, r.hidden))
                .collect(),
            Kind::CustomChannels(group) => store
                .load_channels(
                    source,
                    &ChannelQuery {
                        scope: ChannelScope::Custom(group),
                        include_hidden: true,
                        ..Default::default()
                    },
                )?
                .into_iter()
                .map(|r| (r.channel.id.as_str().to_owned(), r.name, r.hidden))
                .collect(),
        };
        Ok(rows
            .into_iter()
            .map(|(id, name, hidden)| Item { id, name, hidden })
            .collect())
    })
    .await
}

pub(crate) fn organization_in(store: &Store, source: SourceId) -> Result<Organization, StoreError> {
    let channels = store
        .load_channels(
            source,
            &ChannelQuery {
                include_hidden: true,
                ..Default::default()
            },
        )?
        .into_iter()
        .map(|r| (r.channel.id.as_str().to_owned(), r.name, r.hidden))
        .collect();
    let groups = store
        .library_groups(source)?
        .into_iter()
        .filter(|r| r.group.kind == iptv_providers::model::ContentKind::Live)
        .map(|r| (r.group.id.as_str().to_owned(), r.name, r.hidden))
        .collect();
    Ok(Organization { channels, groups })
}
pub fn edit_organization(
    source: SourceId,
    kind: Kind,
    editor: Editor,
    item: Item,
    edit: Edit,
) -> impl Future<Output = Result<(), StoreError>> {
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
            "favourites use their ordered writer",
        )),
    };
    store_service::call(move |store| {
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
                let ids: Vec<_> = editor
                    .items
                    .iter()
                    .filter(|i| all || editor.selected.contains(&i.id))
                    .map(|i| i.id.clone())
                    .collect();
                store.set_library_visibility(source, &ids, kind == StoredKind::Groups, hidden)?;
            }
            _ => {}
        }
        Ok(())
    })
}

pub fn favorite_provider_order(
    source: SourceId,
    keys: Vec<String>,
) -> impl Future<Output = Result<Vec<String>, StoreError>> {
    store_service::call(move |store| {
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

pub fn rename_organization_item(
    source: SourceId,
    kind: Kind,
    id: String,
    text: String,
) -> impl Future<Output = Result<(), StoreError>> {
    store_service::call(move |store| {
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

pub fn load_search_history(
    legacy: std::path::PathBuf,
) -> impl Future<Output = Result<Vec<String>, StoreError>> {
    store_service::call(move |store| {
        import_legacy_history(store, &legacy)?;
        store.search_history()
    })
}

use iptv_search::SearchHistory;

use super::state::OrganizationSnapshot;
pub async fn load_organization(source: SourceId) -> Result<OrganizationSnapshot, StoreError> {
    store_service::read(move |store| {
        Ok(OrganizationSnapshot {
            organization: organization_in(store, source)?,
            lists: group_lists_in(store, source)?,
        })
    })
    .await
}

/// Includes empty custom groups so their membership can be populated later.
fn group_lists_in(
    store: &Store,
    source: SourceId,
) -> Result<Vec<(String, String, Vec<String>)>, StoreError> {
    use iptv_store_sqlite::ChannelScope;
    let mut lists = Vec::new();
    for g in store
        .library_groups(source)?
        .into_iter()
        .filter(|g| !g.hidden && g.group.kind == iptv_providers::model::ContentKind::Live)
    {
        let rows = store.load_channels(
            source,
            &ChannelQuery {
                scope: ChannelScope::Group(g.group.id.clone()),
                include_hidden: true,
                ..Default::default()
            },
        )?;
        lists.push((
            g.group.id.as_str().to_owned(),
            g.name,
            rows.into_iter()
                .map(|r| r.channel.id.as_str().to_owned())
                .collect(),
        ));
    }
    for g in store.custom_groups(source)? {
        let rows = store.load_channels(
            source,
            &ChannelQuery {
                scope: ChannelScope::Custom(g.id),
                include_hidden: true,
                ..Default::default()
            },
        )?;
        lists.push((
            format!("custom:{}", g.id.0),
            g.name,
            rows.into_iter()
                .map(|r| r.channel.id.as_str().to_owned())
                .collect(),
        ));
    }
    Ok(lists)
}

pub fn startup_library() -> impl std::future::Future<
    Output = Result<
        (
            Option<iptv_store_sqlite::StoredSource>,
            bool,
            bool,
            super::library::Snapshot,
        ),
        StoreError,
    >,
> {
    super::store_service::call(|store| {
        iptv_store_sqlite::EpgStage::remove_stale(super::paths::data_dir());
        iptv_store_sqlite::CatalogStage::remove_stale(super::paths::data_dir());
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

pub fn source_policies() -> impl std::future::Future<
    Output = Result<
        Vec<(iptv_store_sqlite::SourceSummary, iptv_library::SourcePolicy)>,
        StoreError,
    >,
> {
    super::store_service::call(|store| {
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

pub fn guide_status_snapshot()
-> impl std::future::Future<Output = Result<super::state::StatusSnapshot, StoreError>> {
    super::store_service::call(|store| {
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

pub fn description(
    source: SourceId,
    key: iptv_epg::ChannelKey,
    start: iptv_epg::UnixTime,
) -> impl std::future::Future<Output = Result<Option<String>, StoreError>> {
    super::store_service::read(move |store| store.epg_description(source, &key, start))
}

pub fn guide_sources(
    source: SourceId,
) -> impl std::future::Future<
    Output = Result<
        (
            Option<iptv_store_sqlite::StoredGuide>,
            Vec<iptv_library::GuideSource>,
        ),
        StoreError,
    >,
> {
    super::store_service::call(move |store| {
        Ok((store.epg_guide(source)?, store.guide_sources(source)?))
    })
}

pub fn clear_guides(
    sources: Vec<SourceId>,
) -> impl std::future::Future<Output = Result<(bool, Vec<super::imports::Imported>), StoreError>> {
    super::store_service::call(move |store| {
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

pub fn first_programme(
    source: SourceId,
) -> impl std::future::Future<Output = Result<Option<iptv_epg::UnixTime>, StoreError>> {
    super::store_service::read(move |store| store.epg_first_start(source))
}

pub fn guide_page(
    source: SourceId,
    request: iptv_guide::pages::PageRequest,
) -> impl std::future::Future<Output = Result<iptv_epg::GuideStore, StoreError>> {
    super::store_service::read(move |store| {
        let window = iptv_store_sqlite::EpgWindow {
            from: request.from,
            to: request.to,
            ..iptv_store_sqlite::EpgWindow::from(0, 0)
        };
        store.load_epg(source, &window)
    })
}

pub fn combined_resume_key() -> impl std::future::Future<Output = Result<Option<String>, StoreError>>
{
    super::store_service::read(|store| {
        let s = store;
        Ok(super::playback::load_combined_channel()
            .or(s.combined_last_channel()?)
            .or_else(|| {
                // On first opening the combined browser, keep the previous
                // single-source startup preference, including legacy files.
                let source = s.active_source().ok().flatten()?;
                if !s.source_policy(source).ok()?.enabled {
                    return None;
                }
                let key = super::playback::load_last_channel(Some(source))?;
                Some(iptv_guide::combined::qualified_key(source.0, &key))
            }))
    })
}
