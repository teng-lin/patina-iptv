//! Manual channel order per list and custom groups.
//!
//! Both keep ids of channels that are currently gone, in place, so a
//! returning channel gets its old position back. Positions are rewritten
//! densely on every move: cheap for groups and favourites, ~one write per
//! channel for a manual order of the whole source.

use std::collections::HashSet;

use iptv_providers::ContentId;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::error::{Result, StoreError};
use crate::order::move_to;
use crate::overlay::present_ids;
use crate::sources::ensure_source;
use crate::{SourceId, Store};

pub use iptv_library::{CustomGroupId, OrderScope};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomGroup {
    pub id: CustomGroupId,
    pub source_id: SourceId,
    pub name: String,
    /// Members present in the current catalog (hidden ones included).
    pub channel_count: u32,
}

fn ids(conn: &Connection, sql: &str, args: impl rusqlite::Params) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(args, |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Current channels of a scope in provider order.
fn scope_channels(conn: &Connection, source: SourceId, scope: &OrderScope) -> Result<Vec<String>> {
    match scope {
        OrderScope::All => ids(
            conn,
            "SELECT id FROM channels WHERE source_id = ?1 ORDER BY position",
            [source.0],
        ),
        OrderScope::Group(g) => ids(
            conn,
            "SELECT channel_id FROM channel_groups WHERE source_id = ?1 AND group_id = ?2
             ORDER BY position",
            params![source.0, g.as_str()],
        ),
    }
}

fn stored_order(conn: &Connection, source: SourceId, scope: &OrderScope) -> Result<Vec<String>> {
    ids(
        conn,
        "SELECT channel_id FROM manual_order WHERE source_id = ?1 AND scope = ?2 ORDER BY position",
        params![source.0, scope.key()],
    )
}

fn write_order(
    tx: &Transaction<'_>,
    source: SourceId,
    scope: &OrderScope,
    list: &[String],
) -> Result<()> {
    tx.execute(
        "DELETE FROM manual_order WHERE source_id = ?1 AND scope = ?2",
        params![source.0, scope.key()],
    )?;
    let mut stmt = tx.prepare_cached(
        "INSERT OR IGNORE INTO manual_order (source_id, scope, channel_id, position)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for (position, id) in list.iter().enumerate() {
        stmt.execute(params![source.0, scope.key(), id, position as i64])?;
    }
    Ok(())
}

fn group_source(conn: &Connection, group: CustomGroupId) -> Result<SourceId> {
    conn.query_row(
        "SELECT source_id FROM custom_groups WHERE id = ?1",
        [group.0],
        |r| r.get(0),
    )
    .optional()?
    .map(SourceId)
    .ok_or(StoreError::CustomGroupNotFound(group.0))
}

fn members(conn: &Connection, group: CustomGroupId) -> Result<Vec<String>> {
    ids(
        conn,
        "SELECT channel_id FROM custom_group_members WHERE group_id = ?1 ORDER BY position",
        [group.0],
    )
}

fn write_members(tx: &Transaction<'_>, group: CustomGroupId, list: &[String]) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "UPDATE custom_group_members SET position = ?3 WHERE group_id = ?1 AND channel_id = ?2",
    )?;
    for (position, id) in list.iter().enumerate() {
        stmt.execute(params![group.0, id, position as i64])?;
    }
    Ok(())
}

impl Store {
    /// Atomically copy selected present channels into `to`. A move removes
    /// membership from `from` only; it never removes channels or provider
    /// membership. Both custom groups must belong to `source`. Duplicate
    /// destination membership is retained in place; new members append in
    /// the supplied order. Any stale/foreign selection rejects the whole edit.
    pub fn transfer_custom_members(
        &mut self,
        source: SourceId,
        from: Option<CustomGroupId>,
        to: CustomGroupId,
        selection: &[ContentId],
        move_members: bool,
    ) -> Result<usize> {
        let tx = self.conn.transaction()?;
        ensure_source(&tx, source)?;
        if group_source(&tx, to)? != source
            || from
                .map(|g| group_source(&tx, g))
                .transpose()?
                .is_some_and(|s| s != source)
        {
            return Err(StoreError::InvalidData(
                "custom group source mismatch".into(),
            ));
        }
        if move_members && from.is_none() {
            return Err(StoreError::InvalidData(
                "move requires a custom source group".into(),
            ));
        }
        let present = present_ids(&tx, source)?;
        let origin: HashSet<_> = from
            .map(|g| members(&tx, g))
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .collect();
        let mut seen = HashSet::new();
        let selected: Vec<_> = selection
            .iter()
            .filter(|id| seen.insert(id.as_str()))
            .collect();
        if selected.iter().any(|id| {
            !present.contains(id.as_str()) || (from.is_some() && !origin.contains(id.as_str()))
        }) {
            return Err(StoreError::InvalidData(
                "selection is no longer in its source list".into(),
            ));
        }
        if from == Some(to) {
            return Ok(0);
        }
        for id in &selected {
            tx.execute("INSERT OR IGNORE INTO custom_group_members (group_id, channel_id, position)
                VALUES (?1, ?2, (SELECT COALESCE(MAX(position)+1,0) FROM custom_group_members WHERE group_id=?1))",
                params![to.0,id.as_str()])?;
            if move_members {
                tx.execute(
                    "DELETE FROM custom_group_members WHERE group_id=?1 AND channel_id=?2",
                    params![from.unwrap().0, id.as_str()],
                )?;
            }
        }
        tx.commit()?;
        Ok(selected.len())
    }

    /// Atomic removal of selected custom memberships, keeping provider data.
    pub fn remove_custom_members(
        &mut self,
        group: CustomGroupId,
        selection: &[ContentId],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        group_source(&tx, group)?;
        for id in selection {
            tx.execute(
                "DELETE FROM custom_group_members WHERE group_id=?1 AND channel_id=?2",
                params![group.0, id.as_str()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Moves a channel to `index` in a list as shown in effective order
    /// (hidden channels included). The first move materialises the list's
    /// current order. Returns `false` if the channel is not in the list.
    pub fn move_channel(
        &mut self,
        source: SourceId,
        scope: &OrderScope,
        id: &ContentId,
        index: usize,
    ) -> Result<bool> {
        let tx = self.conn.transaction()?;
        ensure_source(&tx, source)?;
        let current = scope_channels(&tx, source, scope)?;
        let present: HashSet<String> = current.iter().cloned().collect();
        let mut list = stored_order(&tx, source, scope)?;
        let listed: HashSet<String> = list.iter().cloned().collect();
        list.extend(current.into_iter().filter(|c| !listed.contains(c)));
        if !move_to(&mut list, &present, id.as_str(), index) {
            return Ok(false);
        }
        write_order(&tx, source, scope, &list)?;
        tx.commit()?;
        Ok(true)
    }

    /// Sets a list's whole order (edit-mode commit). Gone channels that had
    /// a place keep one after the given ids; channels not named sort after
    /// them in provider order.
    pub fn set_manual_order(
        &mut self,
        source: SourceId,
        scope: &OrderScope,
        order: &[ContentId],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        ensure_source(&tx, source)?;
        let present = present_ids(&tx, source)?;
        let mut seen = HashSet::new();
        let mut list: Vec<String> = order
            .iter()
            .map(|c| c.as_str().to_owned())
            .filter(|c| seen.insert(c.clone()))
            .collect();
        list.extend(
            stored_order(&tx, source, scope)?
                .into_iter()
                .filter(|c| !present.contains(c) && !seen.contains(c)),
        );
        write_order(&tx, source, scope, &list)?;
        tx.commit()?;
        Ok(())
    }

    /// Back to provider order for that list.
    pub fn reset_manual_order(&mut self, source: SourceId, scope: &OrderScope) -> Result<()> {
        self.conn.execute(
            "DELETE FROM manual_order WHERE source_id = ?1 AND scope = ?2",
            params![source.0, scope.key()],
        )?;
        Ok(())
    }

    /// Creates an empty custom group at the end of the source's list.
    pub fn create_custom_group(&mut self, source: SourceId, name: &str) -> Result<CustomGroupId> {
        ensure_source(&self.conn, source)?;
        let now = self.now();
        self.conn.execute(
            "INSERT INTO custom_groups (source_id, name, position, created_at)
             VALUES (?1, ?2, (SELECT COALESCE(MAX(position) + 1, 0) FROM custom_groups
                              WHERE source_id = ?1), ?3)",
            params![source.0, name, now],
        )?;
        Ok(CustomGroupId(self.conn.last_insert_rowid()))
    }

    pub fn rename_custom_group(&mut self, group: CustomGroupId, name: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE custom_groups SET name = ?2 WHERE id = ?1",
            params![group.0, name],
        )?;
        if changed == 0 {
            return Err(StoreError::CustomGroupNotFound(group.0));
        }
        Ok(())
    }

    /// Deletes the group and its membership (not the channels).
    pub fn delete_custom_group(&mut self, group: CustomGroupId) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM custom_groups WHERE id = ?1", [group.0])?
            > 0)
    }

    /// Moves a custom group to `index` in its source's list.
    pub fn move_custom_group(&mut self, group: CustomGroupId, index: usize) -> Result<()> {
        let tx = self.conn.transaction()?;
        let source = group_source(&tx, group)?;
        let mut list = ids(
            &tx,
            "SELECT CAST(id AS TEXT) FROM custom_groups WHERE source_id = ?1 ORDER BY position, id",
            [source.0],
        )?;
        let present: HashSet<String> = list.iter().cloned().collect();
        move_to(&mut list, &present, &group.0.to_string(), index);
        {
            let mut stmt =
                tx.prepare_cached("UPDATE custom_groups SET position = ?2 WHERE id = ?1")?;
            for (position, id) in list.iter().enumerate() {
                stmt.execute(params![id, position as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// A source's custom groups in list order.
    pub fn custom_groups(&self, source: SourceId) -> Result<Vec<CustomGroup>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT g.id, g.name, (SELECT COUNT(*) FROM custom_group_members m
                 JOIN channels c ON c.source_id = g.source_id AND c.id = m.channel_id
                 WHERE m.group_id = g.id)
             FROM custom_groups g WHERE g.source_id = ?1 ORDER BY g.position, g.id",
        )?;
        let rows = stmt.query_map([source.0], |r| {
            Ok(CustomGroup {
                id: CustomGroupId(r.get(0)?),
                source_id: source,
                name: r.get(1)?,
                channel_count: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Appends a channel to a custom group. Returns `false` if it was
    /// already a member.
    pub fn add_to_custom_group(&mut self, group: CustomGroupId, id: &ContentId) -> Result<bool> {
        group_source(&self.conn, group)?;
        Ok(self.conn.execute(
            "INSERT OR IGNORE INTO custom_group_members (group_id, channel_id, position)
             VALUES (?1, ?2, (SELECT COALESCE(MAX(position) + 1, 0) FROM custom_group_members
                              WHERE group_id = ?1))",
            params![group.0, id.as_str()],
        )? > 0)
    }

    pub fn remove_from_custom_group(
        &mut self,
        group: CustomGroupId,
        id: &ContentId,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM custom_group_members WHERE group_id = ?1 AND channel_id = ?2",
            params![group.0, id.as_str()],
        )? > 0)
    }

    /// Puts a custom group's present members in provider order, in one
    /// transaction. Members whose channel is gone keep their place relative
    /// to their neighbours, as a series of [`Store::move_in_custom_group`]
    /// calls in that order would leave them.
    pub fn reset_custom_group_order(&mut self, group: CustomGroupId) -> Result<()> {
        let tx = self.conn.transaction()?;
        let source = group_source(&tx, group)?;
        let list = members(&tx, group)?;
        if list.is_empty() {
            return Ok(());
        }
        let present = present_ids(&tx, source)?;
        let provider = ids(
            &tx,
            "SELECT m.channel_id FROM custom_group_members m
             JOIN channels c ON c.source_id = ?2 AND c.id = m.channel_id
             WHERE m.group_id = ?1 ORDER BY c.position",
            params![group.0, source.0],
        )?;

        struct Node {
            item: String,
            prev: Option<usize>,
            next: Option<usize>,
            is_present: bool,
            placed: bool,
        }

        let len = list.len();
        let mut map = std::collections::HashMap::with_capacity(len);
        let mut nodes = Vec::with_capacity(len);
        let mut head = 0;
        let mut tail = len.saturating_sub(1);
        for (i, item) in list.into_iter().enumerate() {
            let is_present = present.contains(&item);
            map.insert(item.clone(), i);
            nodes.push(Node {
                item,
                prev: if i > 0 { Some(i - 1) } else { None },
                next: if i + 1 < len { Some(i + 1) } else { None },
                is_present,
                placed: false,
            });
        }

        let mut cursor = None;
        let mut curr = Some(head);
        while let Some(idx) = curr {
            if nodes[idx].is_present {
                cursor = Some(idx);
                break;
            }
            curr = nodes[idx].next;
        }

        for id in &provider {
            let Some(&node_idx) = map.get(id) else {
                continue;
            };
            nodes[node_idx].placed = true;

            if cursor == Some(node_idx) {
                // Advance cursor to next unplaced present node
                let mut n = nodes[node_idx].next;
                while let Some(idx) = n {
                    if nodes[idx].is_present && !nodes[idx].placed {
                        cursor = Some(idx);
                        break;
                    }
                    n = nodes[idx].next;
                }
                if n.is_none() {
                    cursor = None;
                }
                if let Some(target) = cursor {
                    // Removing and reinserting also moves past any orphan
                    // between this member and the next present member.
                    let previous = nodes[node_idx].prev;
                    let next = nodes[node_idx].next;
                    if let Some(previous) = previous {
                        nodes[previous].next = next;
                    } else {
                        head = next.expect("target follows current member");
                    }
                    if let Some(next) = next {
                        nodes[next].prev = previous;
                    }
                    let previous = nodes[target].prev;
                    nodes[node_idx].prev = previous;
                    nodes[node_idx].next = Some(target);
                    if let Some(previous) = previous {
                        nodes[previous].next = Some(node_idx);
                    } else {
                        head = node_idx;
                    }
                    nodes[target].prev = Some(node_idx);
                } else if node_idx != tail {
                    // No more unplaced present items; move node_idx to tail.
                    let p = nodes[node_idx].prev;
                    let n = nodes[node_idx].next;
                    if let Some(prev) = p {
                        nodes[prev].next = n;
                    } else {
                        head = n.unwrap_or(0);
                    }
                    if let Some(next) = n {
                        nodes[next].prev = p;
                    }

                    nodes[tail].next = Some(node_idx);
                    nodes[node_idx].prev = Some(tail);
                    nodes[node_idx].next = None;
                    tail = node_idx;
                }
            } else {
                // Detach node_idx
                let p = nodes[node_idx].prev;
                let n = nodes[node_idx].next;
                if let Some(prev) = p {
                    nodes[prev].next = n;
                } else {
                    head = n.unwrap_or(0);
                }
                if let Some(next) = n {
                    nodes[next].prev = p;
                } else {
                    tail = p.unwrap_or(0);
                }

                if let Some(target) = cursor {
                    // Insert node_idx before target
                    let target_prev = nodes[target].prev;
                    nodes[node_idx].prev = target_prev;
                    nodes[node_idx].next = Some(target);
                    if let Some(tp) = target_prev {
                        nodes[tp].next = Some(node_idx);
                    } else {
                        head = node_idx;
                    }
                    nodes[target].prev = Some(node_idx);
                } else {
                    // Insert at tail
                    nodes[tail].next = Some(node_idx);
                    nodes[node_idx].prev = Some(tail);
                    nodes[node_idx].next = None;
                    tail = node_idx;
                }
            }
        }

        let mut result_list = Vec::with_capacity(len);
        let mut curr = Some(head);
        while let Some(idx) = curr {
            result_list.push(std::mem::take(&mut nodes[idx].item));
            curr = nodes[idx].next;
        }

        write_members(&tx, group, &result_list)?;
        tx.commit()?;
        Ok(())
    }

    /// Moves a member to `index` among the group's present channels.
    pub fn move_in_custom_group(
        &mut self,
        group: CustomGroupId,
        id: &ContentId,
        index: usize,
    ) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let source = group_source(&tx, group)?;
        let mut list = members(&tx, group)?;
        let present = present_ids(&tx, source)?;
        if !move_to(&mut list, &present, id.as_str(), index) {
            return Ok(false);
        }
        write_members(&tx, group, &list)?;
        tx.commit()?;
        Ok(true)
    }
}
