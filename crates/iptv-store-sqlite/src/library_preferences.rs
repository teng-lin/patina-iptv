//! Library policy and provider-group overlays, never provider-row mutations.
use crate::error::Result;
use crate::sources::ensure_source;
use crate::{SourceId, Store};
use iptv_providers::{GroupId, model::Group};
use rusqlite::{OptionalExtension, params};

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
    pub fn rename_group(
        &mut self,
        source: SourceId,
        id: &GroupId,
        name: Option<&str>,
    ) -> Result<()> {
        ensure_source(&self.conn, source)?;
        let name = name.map(str::trim).filter(|s| !s.is_empty());
        self.conn
            .execute(UPSERT_GROUP_NAME, params![source.0, id.as_str(), name])?;
        Ok(())
    }
    pub fn hide_group(&mut self, source: SourceId, id: &GroupId, hidden: bool) -> Result<()> {
        ensure_source(&self.conn, source)?;
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
        let tx = self.conn.transaction()?;
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
        let tx = self.conn.transaction()?;
        let sql = if groups {
            UPSERT_GROUP_HIDDEN
        } else {
            UPSERT_CHANNEL_HIDDEN
        };
        {
            let mut stmt = tx.prepare_cached(sql)?;
            for id in ids {
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
