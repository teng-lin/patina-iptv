//! Named library operations. Views send values and await results; only this
//! module creates database work items.
use super::store_service::Services;
use crate::error::AdapterError as StoreError;
use crate::model::{ChannelScope, CustomGroup};
use iptv_core::membership::Member;
use iptv_library::{CustomGroupId, SourceId};
use iptv_providers::ContentId;
use std::{collections::BTreeSet, future::Future};

fn custom_groups_native(
    services: &Services,
    source: SourceId,
) -> impl Future<Output = Result<Vec<CustomGroup>, StoreError>> + '_ {
    services.catalog().custom_groups(source)
}

fn members_native(
    services: &Services,
    source: SourceId,
    scope: ChannelScope,
) -> impl Future<Output = Result<Vec<Member>, StoreError>> + '_ {
    services.catalog().members(source, scope)
}

fn group_members_native(
    services: &Services,
    source: SourceId,
    scope: ChannelScope,
) -> impl Future<Output = Result<(Vec<CustomGroup>, Vec<Member>), StoreError>> + '_ {
    services.catalog().group_members(source, scope)
}

fn save_custom_group_native(
    services: &Services,
    source: SourceId,
    group: Option<CustomGroupId>,
    name: String,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services.catalog().save_custom_group(source, group, name)
}

fn remove_custom_members_native(
    services: &Services,
    group: CustomGroupId,
    members: Vec<ContentId>,
    access: crate::parental::Snapshot,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services
        .catalog()
        .remove_custom_members(group, members, access)
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
    services
        .catalog()
        .transfer_custom_members(source, from, to, members, moving, access)
}

fn delete_custom_group_native(
    services: &Services,
    group: CustomGroupId,
    access: crate::parental::Snapshot,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services.catalog().delete_custom_group(group, access)
}

fn hide_channel_native(
    services: &Services,
    source: SourceId,
    channel: String,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services.catalog().hide_channel(source, channel)
}

fn remember_combined_channel_native(
    services: &Services,
    key: String,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services.catalog().remember_combined_channel(key)
}

fn channel_native(
    services: &Services,
    source: SourceId,
    channel: ContentId,
    access: crate::parental::Snapshot,
) -> impl Future<Output = Result<Option<crate::model::PlaybackChannel>, StoreError>> + use<> {
    services.catalog().channel(source, channel, access)
}

fn save_search_history_native(
    services: &Services,
    history: Vec<String>,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services.system().set_search_history(history)
}

use iptv_core::organization::{BlockKind, Blocks, Edit, Item, Sort};
use iptv_library::OrganizationScope as Kind;

fn edit_organization_native(
    services: &Services,
    source: SourceId,
    kind: Kind,
    items: Vec<Item>,
    item: Item,
    edit: Edit,
    selected: BTreeSet<String>,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services
        .catalog()
        .edit_organization(source, kind, items, item, edit, selected)
}

fn favorite_provider_order_native(
    services: &Services,
    source: SourceId,
    keys: Vec<String>,
) -> impl Future<Output = Result<Vec<String>, StoreError>> + use<> {
    services.catalog().favorite_provider_order(source, keys)
}

fn rename_organization_item_native(
    services: &Services,
    source: SourceId,
    kind: Kind,
    id: String,
    text: String,
) -> impl Future<Output = Result<(), StoreError>> + use<> {
    services
        .catalog()
        .rename_organization_item(source, kind, id, text)
}

fn load_search_history_native(
    services: &Services,
    legacy: std::path::PathBuf,
) -> impl Future<Output = Result<Vec<String>, StoreError>> + use<> {
    services.system().load_search_history(legacy)
}

use super::state::OrganizationSnapshot;
async fn load_organization_native(
    services: &Services,
    source: SourceId,
) -> Result<OrganizationSnapshot, StoreError> {
    let (organization, lists) = services
        .catalog()
        .organization_with_group_lists(source)
        .await?;
    Ok(OrganizationSnapshot {
        organization,
        lists,
    })
}

fn startup_library_native(
    services: &Services,
) -> impl std::future::Future<
    Output = Result<
        (
            Option<crate::model::StoredSource>,
            bool,
            bool,
            super::library::Snapshot,
        ),
        StoreError,
    >,
> + use<> {
    services.catalog().startup_library()
}

fn source_policies_native(
    services: &Services,
) -> impl std::future::Future<
    Output = Result<Vec<(crate::model::SourceSummary, iptv_library::SourcePolicy)>, StoreError>,
> + use<> {
    services.catalog().source_policies()
}

fn guide_status_snapshot_native(
    services: &Services,
) -> impl std::future::Future<Output = Result<super::state::StatusSnapshot, StoreError>> + use<> {
    services.catalog().status_snapshot()
}

fn description_native(
    services: &Services,
    source: SourceId,
    key: iptv_epg::ChannelKey,
    start: iptv_epg::UnixTime,
) -> impl std::future::Future<Output = Result<Option<String>, StoreError>> + use<> {
    services.epg().description(source, key, start)
}

fn guide_sources_native(
    services: &Services,
    source: SourceId,
) -> impl std::future::Future<
    Output = Result<
        (
            Option<crate::model::StoredGuide>,
            Vec<iptv_library::GuideSource>,
        ),
        StoreError,
    >,
> + use<> {
    services.epg().guide_sources_snapshot(source)
}

fn clear_guides_native(
    services: &Services,
    sources: Vec<SourceId>,
) -> impl std::future::Future<Output = Result<(bool, Vec<super::imports::Imported>), StoreError>> + use<>
{
    services.epg().clear_guides(sources)
}

fn guide_page_native(
    services: &Services,
    source: SourceId,
    incarnation: Option<String>,
    request: iptv_guide::pages::PageRequest,
) -> impl std::future::Future<Output = Result<iptv_epg::GuideStore, StoreError>> + use<> {
    let services = services.clone();
    async move {
        services
            .epg()
            .authorized_page(source, incarnation, request)
            .await
    }
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
    let cleared = owner.services().system().clear_search_history(legacy);
    async move { cleared.await.map_err(crate::Error::from) }
}

pub fn organization_items(
    owner: &crate::Runtime,
    source: SourceId,
    kind: Kind,
    favorites: Vec<String>,
) -> impl Future<Output = Result<Vec<Item>, crate::Error>> + use<> {
    let result = owner
        .services()
        .catalog()
        .organization_rows(source, kind, favorites, None);
    async move { Ok(result.await?) }
}

/// Returns the saved guide blocks for the editor.
pub async fn guide_blocks(
    owner: &crate::Runtime,
    source: SourceId,
) -> Result<Blocks, crate::Error> {
    Ok(owner.services().catalog().guide_blocks(source).await?)
}

/// Checks the current store before accessing the channel's stream.
pub async fn guide_channel_blocked(
    owner: &crate::Runtime,
    source: SourceId,
    id: String,
) -> Result<bool, crate::Error> {
    Ok(owner
        .services()
        .catalog()
        .guide_channel_blocked(source, id)
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
    let saved = owner
        .services()
        .catalog()
        .set_guide_block(source, kind, id, blocked);
    async move { saved.await.map_err(crate::Error::from) }
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
    let (rows, sort, blocks) = owner
        .services()
        .catalog()
        .organization_editor(source, scope, favorites, draft_sort)
        .await?;
    Ok(OrganizationItems { rows, sort, blocks })
}

pub fn set_group_preference(
    owner: &crate::Runtime,
    source: SourceId,
    group: String,
    preference: iptv_core::organization::GroupPreference,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let saved = owner
        .services()
        .catalog()
        .set_group_preference(source, group, preference);
    async move { saved.await.map_err(crate::Error::from) }
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
        .catalog()
        .set_guide_sort(source, scope, sort);
    async move { Ok(result.await?) }
}

/// Hides a group without changing its channels’ own visibility.
pub fn hide_group(
    owner: &crate::Runtime,
    source: SourceId,
    id: String,
) -> impl Future<Output = Result<(), crate::Error>> + use<> {
    let result = owner.services().catalog().hide_group(source, id);
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
    let saved = owner.services().catalog().save_guide_order_from(
        source,
        scope,
        original,
        order,
        expected_sort,
        from_custom,
    );
    async move { saved.await.map_err(crate::Error::from) }
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
            Option<crate::model::StoredSource>,
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
    Output = Result<Vec<(crate::model::SourceSummary, iptv_library::SourcePolicy)>, crate::Error>,
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
            Option<crate::model::StoredGuide>,
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
    let starts = owner.services().epg().listing_starts(source, cutoff);
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
        services
            .catalog()
            .combined_resume_key()
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
    let window = super::paging::window(now, super::stored::stored_past_days());
    let read = owner.services().epg().page_after_writes(source, window);
    async move { Ok(read.await?) }
}
