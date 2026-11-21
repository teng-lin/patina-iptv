//! User overlays: favorites, hidden, blocked, custom name and number, EPG
//! override, and the orphan report over every overlay table.
//!
//! Setters accept any channel id, present or not: overlays are keyed by
//! stable id and may be written before the channel is (re)imported, for
//! example when restoring a backup.

use std::collections::HashSet;

use iptv_providers::ContentId;
use rusqlite::types::Value;
use rusqlite::{Connection, params};

use crate::error::Result;
use crate::order::move_to;
use crate::sources::ensure_source;
use crate::{SourceId, Store, search};

fn favorite_ids(conn: &Connection, source: SourceId) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT channel_id FROM favorites WHERE source_id = ?1 ORDER BY position",
    )?;
    Ok(stmt
        .query_map([source.0], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

fn write_favorite_order(conn: &Connection, source: SourceId, keys: &[String]) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "UPDATE favorites SET position = ?3 WHERE source_id = ?1 AND channel_id = ?2",
    )?;
    for (position, key) in keys.iter().enumerate() {
        stmt.execute(params![source.0, key, position as i64])?;
    }
    Ok(())
}

/// A `channel_overlays` row with nothing customized left.
const EMPTY_OVERLAY: &str = "custom_name IS NULL AND custom_number IS NULL \
    AND hidden = 0 AND blocked = 0 AND epg_mode IS NULL";

/// Which guide a channel uses instead of automatic matching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EpgOverride {
    /// Show no guide for this channel.
    Disabled,
    /// Use channel `epg_channel` of the EPG source keyed `epg_source`
    /// (an opaque key chosen by the EPG layer, never a URL with secrets).
    Xmltv {
        epg_source: String,
        epg_channel: String,
    },
}

impl EpgOverride {
    /// Channel `epg_channel` of the playlist source's guide feed `feed`
    /// (the feed id is the opaque key).
    pub fn for_feed(feed: iptv_epg::FeedId, epg_channel: impl Into<String>) -> Self {
        Self::Xmltv {
            epg_source: feed.0.to_string(),
            epg_channel: epg_channel.into(),
        }
    }

    /// The feed of [`EpgOverride::for_feed`]; `None` for another key.
    pub fn feed(&self) -> Option<iptv_epg::FeedId> {
        match self {
            Self::Xmltv { epg_source, .. } => epg_source.parse().ok().map(iptv_epg::FeedId),
            Self::Disabled => None,
        }
    }

    /// What EPG matching applies: a key that names no feed matches the
    /// channel id in every feed.
    pub fn to_override(&self) -> iptv_epg::Override {
        match self {
            Self::Disabled => iptv_epg::Override::NoEpg,
            Self::Xmltv { epg_channel, .. } => iptv_epg::Override::Epg {
                source: self.feed(),
                epg_id: epg_channel.clone(),
            },
        }
    }

    pub(crate) fn from_columns(
        mode: Option<String>,
        source: Option<String>,
        channel: Option<String>,
    ) -> Option<Self> {
        match mode.as_deref() {
            Some("none") => Some(Self::Disabled),
            Some("xmltv") => Some(Self::Xmltv {
                epg_source: source.unwrap_or_default(),
                epg_channel: channel.unwrap_or_default(),
            }),
            _ => None,
        }
    }
}

/// Customizations whose channel is not in the current catalog. They are
/// kept and apply again when the id returns.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OrphanedOverlay {
    pub source_id: SourceId,
    pub channel_id: ContentId,
    pub favorite: bool,
    /// Name, number, hidden, blocked or EPG override.
    pub customized: bool,
    /// Has a place in a manual order.
    pub ordered: bool,
    /// Member of at least one custom group.
    pub in_custom_group: bool,
}

/// Every overlay table as `(source_id, channel_id, favorite, customized,
/// ordered, in_custom_group)` rows.
const OVERLAY_ROWS: &str = "
    SELECT source_id, channel_id, 1 AS favorite, 0 AS customized, 0 AS ordered, 0 AS grouped
        FROM favorites
    UNION ALL SELECT source_id, channel_id, 0, 1, 0, 0 FROM channel_overlays
    UNION ALL SELECT source_id, channel_id, 0, 0, 1, 0 FROM manual_order
    UNION ALL SELECT g.source_id, m.channel_id, 0, 0, 0, 1
        FROM custom_group_members m JOIN custom_groups g ON g.id = m.group_id";

/// Deletes a source's orphans, one statement per overlay table.
const PURGE_ORPHANS: &[&str] = &[
    "DELETE FROM favorites WHERE source_id = ?1 AND NOT EXISTS (
         SELECT 1 FROM channels c WHERE c.source_id = ?1 AND c.id = favorites.channel_id)",
    "DELETE FROM channel_overlays WHERE source_id = ?1 AND NOT EXISTS (
         SELECT 1 FROM channels c WHERE c.source_id = ?1 AND c.id = channel_overlays.channel_id)",
    "DELETE FROM manual_order WHERE source_id = ?1 AND NOT EXISTS (
         SELECT 1 FROM channels c WHERE c.source_id = ?1 AND c.id = manual_order.channel_id)",
    "DELETE FROM custom_group_members
     WHERE group_id IN (SELECT id FROM custom_groups WHERE source_id = ?1) AND NOT EXISTS (
         SELECT 1 FROM channels c WHERE c.source_id = ?1 AND c.id = custom_group_members.channel_id)",
];

/// Sets `channel_overlays` columns, then drops the row if it became empty.
/// Column names are fixed identifiers from this module.
fn set_overlay_columns(
    conn: &Connection,
    source: SourceId,
    id: &ContentId,
    columns: &[(&str, Value)],
    now: i64,
) -> Result<()> {
    ensure_source(conn, source)?;
    let names: Vec<&str> = columns.iter().map(|(c, _)| *c).collect();
    let placeholders: Vec<String> = (0..columns.len()).map(|i| format!("?{}", i + 4)).collect();
    let updates: Vec<String> = names
        .iter()
        .map(|c| format!("{c} = excluded.{c}"))
        .collect();
    let sql = format!(
        "INSERT INTO channel_overlays (source_id, channel_id, updated_at, {})
         VALUES (?1, ?2, ?3, {})
         ON CONFLICT (source_id, channel_id) DO UPDATE
         SET updated_at = excluded.updated_at, {}",
        names.join(", "),
        placeholders.join(", "),
        updates.join(", ")
    );
    let mut args = vec![
        Value::Integer(source.0),
        Value::Text(id.as_str().to_owned()),
        Value::Integer(now),
    ];
    args.extend(columns.iter().map(|(_, v)| v.clone()));
    conn.execute(&sql, rusqlite::params_from_iter(args))?;
    conn.execute(
        &format!(
            "DELETE FROM channel_overlays WHERE source_id = ?1 AND channel_id = ?2 AND {EMPTY_OVERLAY}"
        ),
        params![source.0, id.as_str()],
    )?;
    Ok(())
}

/// Ids of the source's current channels.
pub(crate) fn present_ids(conn: &Connection, source: SourceId) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare_cached("SELECT id FROM channels WHERE source_id = ?1")?;
    let ids = stmt
        .query_map([source.0], |r| r.get(0))?
        .collect::<rusqlite::Result<HashSet<String>>>()?;
    Ok(ids)
}

impl Store {
    fn set_column(
        &mut self,
        source: SourceId,
        id: &ContentId,
        column: &str,
        value: Value,
    ) -> Result<()> {
        let now = self.now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        set_overlay_columns(&tx, source, id, &[(column, value)], now)?;
        tx.commit()?;
        Ok(())
    }

    /// All saved favorites in order, including channels absent from the latest import.
    pub fn favorite_keys(&self, source: SourceId) -> Result<Vec<String>> {
        favorite_ids(&self.conn, source)
    }

    /// Adds to (at the end) or removes from favorites. Idempotent.
    pub fn set_favorite(&mut self, source: SourceId, id: &ContentId, on: bool) -> Result<()> {
        let now = self.now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure_source(&tx, source)?;
        if on {
            tx.execute(
                "INSERT OR IGNORE INTO favorites (source_id, channel_id, position, added_at)
                 VALUES (?1, ?2, (SELECT COALESCE(MAX(position) + 1, 0) FROM favorites
                                  WHERE source_id = ?1), ?3)",
                params![source.0, id.as_str(), now],
            )?;
        } else {
            tx.execute(
                "DELETE FROM favorites WHERE source_id = ?1 AND channel_id = ?2",
                params![source.0, id.as_str()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Flips the favorite flag; returns the new state.
    pub fn toggle_favorite(&mut self, source: SourceId, id: &ContentId) -> Result<bool> {
        let on = !self.is_favorite(source, id)?;
        self.set_favorite(source, id, on)?;
        Ok(on)
    }

    pub fn is_favorite(&self, source: SourceId, id: &ContentId) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM favorites WHERE source_id = ?1 AND channel_id = ?2)",
            params![source.0, id.as_str()],
            |r| r.get(0),
        )?)
    }

    /// Moves a favorite to `index` in the favorites list as shown (present
    /// channels, hidden ones included). Returns `false` if it is not a
    /// favorite.
    pub fn move_favorite(
        &mut self,
        source: SourceId,
        id: &ContentId,
        index: usize,
    ) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut list = favorite_ids(&tx, source)?;
        let present = present_ids(&tx, source)?;
        if !move_to(&mut list, &present, id.as_str(), index) {
            return Ok(false);
        }
        write_favorite_order(&tx, source, &list)?;
        tx.commit()?;
        Ok(true)
    }

    /// Commits an order for the favorites in the current catalog.
    pub fn save_favorite_order(
        &mut self,
        source: SourceId,
        original: &[String],
        order: &[String],
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure_source(&tx, source)?;
        let list = favorite_ids(&tx, source)?;
        let present = present_ids(&tx, source)?;
        let expected = list.iter().filter(|id| present.contains(*id));
        if !expected.eq(original.iter())
            || original.len() != order.len()
            || original.iter().collect::<HashSet<_>>() != order.iter().collect::<HashSet<_>>()
        {
            return Err(crate::StoreError::Unavailable(
                "the favorites list changed during editing",
            ));
        }
        let mut favorites = iptv_core::favorites::Favorites::from_keys(list);
        favorites.apply_order(order);
        write_favorite_order(&tx, source, favorites.keys())?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_hidden(&mut self, source: SourceId, id: &ContentId, hidden: bool) -> Result<()> {
        self.set_column(source, id, "hidden", Value::from(hidden))
    }

    /// Parental block: blocked channels stay listed (flagged) by default.
    pub fn set_blocked(&mut self, source: SourceId, id: &ContentId, blocked: bool) -> Result<()> {
        self.set_column(source, id, "blocked", Value::from(blocked))
    }

    /// Sets a custom name, or restores the provider name with `None`.
    /// Blank names restore too.
    pub fn rename_channel(
        &mut self,
        source: SourceId,
        id: &ContentId,
        name: Option<&str>,
    ) -> Result<()> {
        let now = self.now();
        let fts = self.fts_index;
        let value = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map_or(Value::Null, |n| Value::Text(n.to_owned()));
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        set_overlay_columns(&tx, source, id, &[("custom_name", value)], now)?;
        search::reindex_channel(&tx, fts, source, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Sets a custom number, or restores the provider number with `None`.
    pub fn renumber_channel(
        &mut self,
        source: SourceId,
        id: &ContentId,
        number: Option<u32>,
    ) -> Result<()> {
        let value = number.map_or(Value::Null, |n| Value::Integer(i64::from(n)));
        self.set_column(source, id, "custom_number", value)
    }

    /// Overrides which guide the channel shows; `None` restores automatic
    /// matching.
    pub fn set_epg_override(
        &mut self,
        source: SourceId,
        id: &ContentId,
        epg: Option<&EpgOverride>,
    ) -> Result<()> {
        let (mode, epg_source, epg_channel) = match epg {
            None => (Value::Null, Value::Null, Value::Null),
            Some(EpgOverride::Disabled) => (Value::Text("none".into()), Value::Null, Value::Null),
            Some(EpgOverride::Xmltv {
                epg_source,
                epg_channel,
            }) => (
                Value::Text("xmltv".into()),
                epg_source.clone().into(),
                epg_channel.clone().into(),
            ),
        };
        let now = self.now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        set_overlay_columns(
            &tx,
            source,
            id,
            &[
                ("epg_mode", mode),
                ("epg_source", epg_source),
                ("epg_channel", epg_channel),
            ],
            now,
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Overlay rows whose channel is not in the current catalog, for one
    /// source or all.
    pub fn orphaned_overlays(&self, source: Option<SourceId>) -> Result<Vec<OrphanedOverlay>> {
        let sql = format!(
            "SELECT x.source_id, x.channel_id, MAX(x.favorite), MAX(x.customized),
                 MAX(x.ordered), MAX(x.grouped)
             FROM ({OVERLAY_ROWS}) x
             WHERE (?1 IS NULL OR x.source_id = ?1)
               AND NOT EXISTS (SELECT 1 FROM channels c
                               WHERE c.source_id = x.source_id AND c.id = x.channel_id)
             GROUP BY x.source_id, x.channel_id
             ORDER BY x.source_id, x.channel_id"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([source.map(|s| s.0)], |r| {
            Ok(OrphanedOverlay {
                source_id: SourceId(r.get(0)?),
                channel_id: ContentId::from_stored(r.get::<_, String>(1)?),
                favorite: r.get(2)?,
                customized: r.get(3)?,
                ordered: r.get(4)?,
                in_custom_group: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Deletes a source's orphaned overlays. Returns the rows removed.
    pub fn purge_orphaned_overlays(&mut self, source: SourceId) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut removed = 0;
        for sql in PURGE_ORPHANS {
            removed += tx.execute(sql, [source.0])?;
        }
        tx.commit()?;
        Ok(removed)
    }
}
