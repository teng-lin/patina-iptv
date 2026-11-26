//! Library policy and provider-group overlays, never provider-row mutations.
use crate::error::Result;
use crate::sources::ensure_source;
use crate::{SourceId, Store};
use iptv_core::organization::{BlockKind, Blocks, GroupPreference, GroupPreferences, Item, Sort};
use iptv_library::{ChannelScope, OrganizationScope};
use iptv_providers::model::ContentKind;
use iptv_providers::{GroupId, model::Group};
use rusqlite::{OptionalExtension, params};
use std::borrow::Cow;

pub use iptv_library::SourcePolicy;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupRow {
    pub group: Group,
    pub name: String,
    pub hidden: bool,
}
// The statements of this module. Each upsert writes one column of an
// overlay row and leaves the row's other columns as they are.
const SELECT_POLICY: &str =
    "SELECT enabled, refresh_seconds, refresh_on_start FROM source_policy WHERE source_id=?1";
const UPSERT_POLICY: &str = "INSERT INTO source_policy(source_id,enabled,refresh_seconds,refresh_on_start) \
     VALUES(?1,?2,?3,?4) ON CONFLICT(source_id) DO UPDATE SET enabled=excluded.enabled,\
     refresh_seconds=excluded.refresh_seconds,refresh_on_start=excluded.refresh_on_start";
const SELECT_GROUP_OVERLAY: &str =
    "SELECT custom_name,hidden,position FROM group_overlays WHERE source_id=?1 AND group_id=?2";
const UPSERT_GROUP_NAME: &str = "INSERT INTO group_overlays(source_id,group_id,custom_name) VALUES(?1,?2,?3) \
     ON CONFLICT(source_id,group_id) DO UPDATE SET custom_name=excluded.custom_name";
const UPSERT_GROUP_HIDDEN: &str = "INSERT INTO group_overlays(source_id,group_id,hidden) VALUES(?1,?2,?3) \
     ON CONFLICT(source_id,group_id) DO UPDATE SET hidden=excluded.hidden";
const UPSERT_GROUP_POSITION: &str = "INSERT INTO group_overlays(source_id,group_id,position) VALUES(?1,?2,?3) \
     ON CONFLICT(source_id,group_id) DO UPDATE SET position=excluded.position";
const UPSERT_CHANNEL_HIDDEN: &str = "INSERT INTO channel_overlays(source_id,channel_id,hidden,updated_at) \
     VALUES(?1,?2,?3,unixepoch()) ON CONFLICT(source_id,channel_id) \
     DO UPDATE SET hidden=excluded.hidden,updated_at=excluded.updated_at";
/// A channel overlay that says nothing any more (shown again, with no
/// name, number, block or guide choice of its own) is removed.
const DELETE_EMPTY_CHANNEL_OVERLAYS: &str = "DELETE FROM channel_overlays WHERE source_id=?1 \
     AND hidden=0 AND blocked=0 AND custom_name IS NULL AND custom_number IS NULL \
     AND epg_mode IS NULL";

const BLOCKED_MEMBERS: &str = "SELECT m.channel_id FROM guide_blocks b
     JOIN channel_groups m ON m.source_id=b.source_id AND m.group_id=b.content_id
     WHERE b.source_id=?1 AND b.kind='group'
     UNION ALL
     SELECT m.channel_id FROM guide_blocks b
     JOIN custom_groups g ON g.source_id=b.source_id AND 'custom:'||g.id=b.content_id
     JOIN custom_group_members m ON m.group_id=g.id
     WHERE b.source_id=?1 AND b.kind='group'
     UNION ALL
     SELECT c.id FROM guide_blocks b
     JOIN channels c ON c.source_id=b.source_id
     WHERE b.source_id=?1 AND b.kind='group' AND b.content_id='__other__'
     AND NOT EXISTS (
         SELECT 1 FROM groups g
         WHERE g.source_id=c.source_id AND g.kind='live'
           AND g.id IN ('__other__', json_extract(c.group_ids, '$[0]'))
     )";

fn ensure_custom_scope(conn: &rusqlite::Connection, source: SourceId, id: i64) -> Result<()> {
    if crate::organize::group_source(conn, iptv_library::CustomGroupId(id))? != source {
        return Err(crate::StoreError::CustomGroupNotFound(id));
    }
    Ok(())
}

fn custom_id(key: &str) -> Option<i64> {
    key.strip_prefix("custom:")?.parse().ok()
}

fn group_preference_key(id: &str) -> Cow<'_, str> {
    custom_id(id).map_or_else(
        || Cow::Borrowed(id),
        |group| Cow::Owned(format!("custom:{group}")),
    )
}

fn sort_key(scope: &OrganizationScope) -> String {
    match scope {
        OrganizationScope::Groups => "groups".into(),
        OrganizationScope::Channels => "channels".into(),
        OrganizationScope::GroupChannels(id) => format!("group:{}", id.as_str()),
        OrganizationScope::CustomChannels(id) => format!("custom:{}", id.0),
        OrganizationScope::Favorites => "favorites".into(),
    }
}

impl Store {
    pub fn source_policy(&self, id: SourceId) -> Result<SourcePolicy> {
        ensure_source(&self.conn, id)?;
        let policy = self
            .conn
            .query_row(SELECT_POLICY, [id.0], |r| {
                Ok(SourcePolicy {
                    enabled: r.get(0)?,
                    refresh_seconds: r.get(1)?,
                    refresh_on_start: r.get(2)?,
                })
            })
            .optional()?;
        Ok(policy.unwrap_or_default())
    }
    pub fn set_source_policy(&mut self, id: SourceId, policy: SourcePolicy) -> Result<()> {
        ensure_source(&self.conn, id)?;
        self.conn.execute(
            UPSERT_POLICY,
            params![
                id.0,
                policy.enabled,
                policy.refresh_seconds,
                policy.refresh_on_start
            ],
        )?;
        Ok(())
    }
    pub fn library_groups(&self, source: SourceId) -> Result<Vec<GroupRow>> {
        let mut rows = Vec::new();
        for (index, group) in self.groups(source, None)?.into_iter().enumerate() {
            let pref: Option<(Option<String>, bool, Option<i64>)> = self
                .conn
                .query_row(
                    SELECT_GROUP_OVERLAY,
                    params![source.0, group.id.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let (name, hidden, position) = pref.unwrap_or((None, false, None));
            rows.push((
                position.unwrap_or(i64::MAX),
                index,
                GroupRow {
                    name: name.unwrap_or_else(|| group.name.clone()),
                    group,
                    hidden,
                },
            ));
        }
        rows.sort_by_key(|(position, index, _)| (*position, *index));
        Ok(rows.into_iter().map(|(_, _, row)| row).collect())
    }
    /// The guide's provider and custom groups, including hidden groups in edit mode.
    pub fn guide_groups(&self, source: SourceId) -> Result<Vec<Item>> {
        let sort = self.guide_sort(source, &OrganizationScope::Groups)?;
        self.guide_groups_with_sort(source, sort)
    }

    pub fn guide_groups_with_sort(&self, source: SourceId, sort: Sort) -> Result<Vec<Item>> {
        let provider = self
            .groups(source, Some(ContentKind::Live))?
            .into_iter()
            .map(|group| (group.id.as_str().to_owned(), group.name));
        let custom = self
            .custom_groups(source)?
            .into_iter()
            .map(|group| (format!("custom:{}", group.id.0), group.name));
        let mut stmt = self.conn.prepare_cached(SELECT_GROUP_OVERLAY)?;
        let mut rows = Vec::new();
        for (id, name) in provider.chain(custom) {
            let pref: Option<(Option<String>, bool, Option<i64>)> = stmt
                .query_row(params![source.0, id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()?;
            let (custom_name, hidden, position) = pref.unwrap_or_default();
            rows.push((
                Item {
                    id,
                    name: custom_name.unwrap_or(name),
                    hidden,
                },
                position,
            ));
        }
        match sort {
            Sort::Manual => rows.sort_by_key(|(_, position)| position.unwrap_or(i64::MAX)),
            Sort::Name => rows.sort_by_cached_key(|(item, _)| item.name.to_lowercase()),
            Sort::NameDescending => {
                rows.sort_by_cached_key(|(item, _)| std::cmp::Reverse(item.name.to_lowercase()))
            }
            _ => {}
        }
        Ok(rows.into_iter().map(|(item, _)| item).collect())
    }

    /// IDs of groups whose saved display name differs from the provider name.
    pub fn renamed_groups(&self, source: SourceId) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT o.group_id
             FROM group_overlays o LEFT JOIN groups g ON g.source_id=o.source_id AND g.id=o.group_id
             WHERE o.source_id=?1 AND o.custom_name IS NOT NULL
               AND (g.id IS NULL OR o.custom_name <> g.name)",
        )?;
        stmt.query_map([source.0], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()
            .map_err(Into::into)
    }

    /// Reads guide blocks apart from the parental settings and credential.
    pub fn guide_blocks(&self, source: SourceId) -> Result<Blocks> {
        let mut blocks = Blocks::default();
        let mut stmt = self
            .conn
            .prepare_cached("SELECT kind,content_id FROM guide_blocks WHERE source_id=?1")?;
        let rows = stmt.query_map([source.0], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (kind, id) = row?;
            if kind == "group" {
                blocks.groups.insert(id);
            } else {
                blocks.channels.insert(id);
            }
        }
        if !blocks.groups.is_empty() {
            let mut stmt = self.conn.prepare_cached(BLOCKED_MEMBERS)?;
            blocks.members = stmt
                .query_map([source.0], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(blocks)
    }

    /// Checks durable guide blocks before opening a live or catch-up stream.
    pub fn guide_channel_blocked(&self, source: SourceId, id: &str) -> Result<bool> {
        let sql = format!(
            "SELECT EXISTS(
                SELECT 1 FROM guide_blocks WHERE source_id=?1 AND kind='channel' AND content_id=?2
                UNION ALL SELECT 1 FROM ({BLOCKED_MEMBERS}) AS members WHERE members.channel_id=?2
             )"
        );
        Ok(self
            .conn
            .prepare_cached(&sql)?
            .query_row(params![source.0, id], |row| row.get(0))?)
    }

    /// Sets a direct guide block without changing parental protection.
    pub fn set_guide_block(
        &mut self,
        source: SourceId,
        kind: BlockKind,
        id: &str,
        blocked: bool,
    ) -> Result<()> {
        ensure_source(&self.conn, source)?;
        if kind == BlockKind::Group
            && let Some(group) = custom_id(id)
        {
            ensure_custom_scope(&self.conn, source, group)?;
        }
        let kind = match kind {
            BlockKind::Channel => "channel",
            BlockKind::Group => "group",
        };
        let sql = if blocked {
            "INSERT OR IGNORE INTO guide_blocks(source_id,kind,content_id) VALUES(?1,?2,?3)"
        } else {
            "DELETE FROM guide_blocks WHERE source_id=?1 AND kind=?2 AND content_id=?3"
        };
        self.conn.execute(sql, params![source.0, kind, id])?;
        Ok(())
    }

    pub fn guide_group_preferences(&self, source: SourceId, id: &str) -> Result<GroupPreferences> {
        let sort = self.guide_sort(source, &OrganizationScope::group_channels(id))?;
        let id = group_preference_key(id);
        let prefs = self
            .conn
            .query_row(
                "SELECT favorites_only,external_player,epg_offset_minutes FROM group_overlays
             WHERE source_id=?1 AND group_id=?2",
                params![source.0, id.as_ref()],
                |row| {
                    Ok(GroupPreferences {
                        sort,
                        favorites_only: row.get(0)?,
                        external_player: row.get(1)?,
                        epg_offset_minutes: row.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(prefs.unwrap_or(GroupPreferences {
            sort,
            ..GroupPreferences::default()
        }))
    }

    pub fn set_guide_group_preference(
        &mut self,
        source: SourceId,
        id: &str,
        preference: GroupPreference,
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(group) = custom_id(id) {
            ensure_custom_scope(&tx, source, group)?;
            if matches!(preference, GroupPreference::EpgOffsetMinutes(_)) {
                return Err(crate::StoreError::Unavailable(
                    "Guide time offsets apply to provider groups",
                ));
            }
        } else {
            ensure_source(&tx, source)?;
            let exists = id == "__other__" || tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM groups WHERE source_id=?1 AND id=?2 AND kind='live')",
                params![source.0, id],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(crate::StoreError::Unavailable("Group no longer exists"));
            }
        }
        let (column, value) = match preference {
            GroupPreference::FavoritesOnly(value) => ("favorites_only", i32::from(value)),
            GroupPreference::ExternalPlayer(value) => ("external_player", i32::from(value)),
            GroupPreference::EpgOffsetMinutes(value) => ("epg_offset_minutes", value),
        };
        let id = group_preference_key(id);
        tx.execute(
            &format!(
                "INSERT INTO group_overlays(source_id,group_id,{column}) VALUES(?1,?2,?3)
             ON CONFLICT(source_id,group_id) DO UPDATE SET {column}=excluded.{column}"
            ),
            params![source.0, id.as_ref(), value],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Sort preference for this scope. Databases with legacy manual positions use them.
    pub fn guide_sort(&self, source: SourceId, scope: &OrganizationScope) -> Result<Sort> {
        let value: Option<String> = self
            .conn
            .query_row(
                "SELECT sort FROM guide_sort WHERE source_id=?1 AND scope=?2",
                params![source.0, sort_key(scope)],
                |row| row.get(0),
            )
            .optional()?;
        Ok(match value.as_deref() {
            Some("provider") => Sort::Provider,
            Some("name") => Sort::Name,
            Some("name_desc") => Sort::NameDescending,
            Some("last_watched") => Sort::LastWatched,
            Some("oldest_watched") => Sort::OldestWatched,
            Some("most_watched") => Sort::MostWatched,
            Some("least_watched") => Sort::LeastWatched,
            _ => Sort::Manual,
        })
    }

    /// Whether any guide list for this playlist sorts by viewing history.
    pub fn has_watch_sort(&self, source: SourceId) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM guide_sort WHERE source_id=?1
                            AND sort IN ('last_watched','oldest_watched','most_watched','least_watched'))",
            [source.0],
            |row| row.get(0),
        )?)
    }

    /// Changes the sort policy while retaining saved manual positions.
    pub fn set_guide_sort(
        &self,
        source: SourceId,
        scope: &OrganizationScope,
        sort: Sort,
    ) -> Result<()> {
        ensure_source(&self.conn, source)?;
        if *scope == OrganizationScope::Favorites && sort != Sort::Manual {
            return Err(crate::StoreError::Unavailable(
                "favorites support manual ordering",
            ));
        }
        if sort.uses_watch_history() && *scope == OrganizationScope::Groups {
            return Err(crate::StoreError::Unavailable(
                "expected a channel list with watch history",
            ));
        }
        if let OrganizationScope::CustomChannels(group) = scope {
            ensure_custom_scope(&self.conn, source, group.0)?;
        }
        let key = sort_key(scope);
        let value = match sort {
            Sort::Provider => "provider",
            Sort::Name => "name",
            Sort::NameDescending => "name_desc",
            Sort::LastWatched => "last_watched",
            Sort::OldestWatched => "oldest_watched",
            Sort::MostWatched => "most_watched",
            Sort::LeastWatched => "least_watched",
            Sort::Manual => {
                self.conn.execute(
                    "DELETE FROM guide_sort WHERE source_id=?1 AND scope=?2",
                    params![source.0, key],
                )?;
                return Ok(());
            }
        };
        self.conn.execute(
            "INSERT INTO guide_sort(source_id,scope,sort) VALUES(?1,?2,?3)
             ON CONFLICT(source_id,scope) DO UPDATE SET sort=excluded.sort",
            params![source.0, key, value],
        )?;
        Ok(())
    }

    /// Loads the same channel order for the guide and its organization editor.
    pub fn ordered_guide_channels(
        &self,
        source: SourceId,
        scope: &OrganizationScope,
    ) -> Result<Vec<crate::ChannelRow>> {
        self.guide_channels_with_sort(source, scope, self.guide_sort(source, scope)?)
    }

    pub fn guide_channels_with_sort(
        &self,
        source: SourceId,
        scope: &OrganizationScope,
        sort: Sort,
    ) -> Result<Vec<crate::ChannelRow>> {
        let channel_scope = match scope {
            OrganizationScope::Channels | OrganizationScope::Favorites => ChannelScope::All,
            OrganizationScope::GroupChannels(id) => ChannelScope::Group(id.clone()),
            OrganizationScope::CustomChannels(id) => ChannelScope::Custom(*id),
            OrganizationScope::Groups => {
                return Err(crate::StoreError::Unavailable("expected channel list"));
            }
        };
        let order = if sort == Sort::Manual {
            crate::ChannelOrder::Effective
        } else {
            crate::ChannelOrder::Provider
        };
        let mut rows = self.load_channels(
            source,
            &crate::ChannelQuery {
                scope: channel_scope,
                include_hidden: true,
                favorites_only: *scope == OrganizationScope::Favorites,
                order,
                ..Default::default()
            },
        )?;
        use std::cmp::Reverse;
        match sort {
            Sort::Name => rows.sort_by_cached_key(|row| row.name.to_lowercase()),
            Sort::NameDescending => rows.sort_by_cached_key(|row| Reverse(row.name.to_lowercase())),
            Sort::LastWatched | Sort::OldestWatched | Sort::MostWatched | Sort::LeastWatched => {
                let sql = match sort {
                    Sort::LastWatched | Sort::OldestWatched => {
                        "SELECT item_id, watched_at, id FROM (
                             SELECT item_id, watched_at, id,
                                    ROW_NUMBER() OVER (
                                        PARTITION BY item_id ORDER BY watched_at DESC, id DESC
                                    ) AS rank
                             FROM watch_history WHERE source_id=?1 AND kind='live'
                         ) WHERE rank=1"
                    }
                    _ => {
                        "SELECT channel_id, watched_ms, 0 FROM channel_watch_time WHERE source_id=?1"
                    }
                };
                let mut stmt = self.conn.prepare_cached(sql)?;
                let values = stmt
                    .query_map([source.0], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
                        ))
                    })?
                    .collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()?;
                let value = |row: &crate::ChannelRow| values.get(row.channel.id.as_str()).copied();
                match sort {
                    Sort::LastWatched | Sort::MostWatched => {
                        rows.sort_by_key(|row| Reverse(value(row)))
                    }
                    _ => rows.sort_by_key(|row| (value(row).is_none(), value(row))),
                }
            }
            _ => {}
        }
        Ok(rows)
    }

    pub(crate) fn write_guide_group_order(&self, source: SourceId, order: &[String]) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(UPSERT_GROUP_POSITION)?;
        for (position, id) in order.iter().enumerate() {
            stmt.execute(params![source.0, id, position as i64])?;
        }
        Ok(())
    }

    pub fn rename_group(
        &mut self,
        source: SourceId,
        id: &GroupId,
        name: Option<&str>,
    ) -> Result<()> {
        ensure_source(&self.conn, source)?;
        let name = name.map(str::trim).filter(|s| !s.is_empty());
        if let Some(group) = custom_id(id.as_str()) {
            ensure_custom_scope(&self.conn, source, group)?;
            return match name {
                Some(name) => self.rename_custom_group(iptv_library::CustomGroupId(group), name),
                None => Ok(()),
            };
        }
        self.conn
            .execute(UPSERT_GROUP_NAME, params![source.0, id.as_str(), name])?;
        Ok(())
    }
    pub fn hide_group(&mut self, source: SourceId, id: &GroupId, hidden: bool) -> Result<()> {
        ensure_source(&self.conn, source)?;
        if let Some(group) = custom_id(id.as_str()) {
            ensure_custom_scope(&self.conn, source, group)?;
        }
        self.conn
            .execute(UPSERT_GROUP_HIDDEN, params![source.0, id.as_str(), hidden])?;
        Ok(())
    }
    pub fn move_group(&mut self, source: SourceId, id: &GroupId, index: usize) -> Result<()> {
        self.move_group_to(source, id, crate::MoveTarget::Index(index))
    }
    pub fn move_group_to(
        &mut self,
        source: SourceId,
        id: &GroupId,
        target: crate::MoveTarget,
    ) -> Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let groups = self.library_groups(source)?;
        let Some(kind) = groups
            .iter()
            .find(|g| &g.group.id == id)
            .map(|g| g.group.kind)
        else {
            return Ok(());
        };
        let mut ids: Vec<_> = groups
            .iter()
            .filter(|g| g.group.kind == kind)
            .map(|g| g.group.id.clone())
            .collect();
        if let Some(at) = ids.iter().position(|g| g == id) {
            let index = target.resolve(at);
            let id = ids.remove(at);
            ids.insert(index.min(ids.len()), id);
        }
        let mut reordered = ids.into_iter();
        let ids: Vec<_> = groups
            .into_iter()
            .map(|g| {
                if g.group.kind == kind {
                    reordered.next().expect("same-kind positions retained")
                } else {
                    g.group.id
                }
            })
            .collect();
        for (position, id) in ids.iter().enumerate() {
            tx.execute(
                UPSERT_GROUP_POSITION,
                params![source.0, id.as_str(), position as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Atomic bulk visibility edits retain names, numbering and membership.
    pub fn set_library_visibility(
        &mut self,
        source: SourceId,
        ids: &[String],
        groups: bool,
        hidden: bool,
    ) -> Result<()> {
        ensure_source(&self.conn, source)?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let sql = if groups {
            UPSERT_GROUP_HIDDEN
        } else {
            UPSERT_CHANNEL_HIDDEN
        };
        {
            let mut stmt = tx.prepare_cached(sql)?;
            for id in ids {
                if groups && let Some(group) = custom_id(id) {
                    match ensure_custom_scope(&tx, source, group) {
                        Ok(()) => {}
                        Err(crate::StoreError::CustomGroupNotFound(_)) => continue,
                        Err(error) => return Err(error),
                    }
                }
                stmt.execute(params![source.0, id, hidden])?;
            }
        }
        if !groups && !hidden {
            tx.execute(DELETE_EMPTY_CHANNEL_OVERLAYS, [source.0])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn reset_group_order(&mut self, source: SourceId) -> Result<()> {
        self.conn.execute(
            "UPDATE group_overlays SET position=NULL WHERE source_id=?1",
            [source.0],
        )?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scheduling_respects_disablement_failures_clock_and_startup() {
        let p = SourcePolicy {
            enabled: true,
            refresh_seconds: 3600,
            refresh_on_start: false,
        };
        assert!(!p.due(100, Some(1000), false));
        assert!(!p.due(4599, Some(1000), false));
        assert!(p.due(4600, Some(1000), false));
        assert!(p.due(100, None, false));
        assert!(
            !SourcePolicy {
                enabled: false,
                ..p
            }
            .due(4600, None, true)
        );
        assert!(
            SourcePolicy {
                refresh_on_start: true,
                ..p
            }
            .due(100, Some(100), true)
        );
        assert!(SourcePolicy::default().due(100, None, true));
    }
}
