//! Channel reads with user overlays merged in.

use std::fmt;

use iptv_providers::ContentId;
use iptv_providers::model::Channel;
use rusqlite::params_from_iter;
use rusqlite::types::Value;

use crate::codec::{catch_up_from_json, group_ids_from_json, narrow, stream_from_json};
use crate::error::Result;
use crate::overlay::EpgOverride;
use crate::search::{fts_query, like_pattern, words};
use crate::{SourceId, Store};

/// A channel as the UI shows it: provider data plus the user's overlay.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChannelRow {
    pub source_id: SourceId,
    /// Provider data as imported (`channel.name` is the provider's name).
    pub channel: Channel,
    /// Displayed name: the custom name, else the provider name.
    pub name: String,
    /// Displayed number: the custom number, else the provider number.
    pub number: Option<u32>,
    pub custom_name: Option<String>,
    pub custom_number: Option<u32>,
    pub favourite: bool,
    pub hidden: bool,
    /// Parental block; the UI asks for the PIN before playing.
    pub blocked: bool,
    /// `None`: automatic EPG matching on `channel.epg_id`/`epg_name`.
    pub epg_override: Option<EpgOverride>,
}

/// Leaves out the stream locator: its URL and headers may carry
/// credentials.
impl fmt::Debug for ChannelRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChannelRow")
            .field("source_id", &self.source_id)
            .field("id", &self.channel.id)
            .field("name", &self.name)
            .field("number", &self.number)
            .field("groups", &self.channel.groups)
            .field("favourite", &self.favourite)
            .field("hidden", &self.hidden)
            .field("blocked", &self.blocked)
            .finish_non_exhaustive()
    }
}

pub use iptv_library::ChannelScope;

/// Sort order of a channel list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChannelOrder {
    /// The user's order where they set one, else provider order: the
    /// manual order of the list (all channels or a group), the favourites
    /// order for an all-channels favourites list, member order for a custom
    /// group. Channels without a manual place follow in provider order.
    #[default]
    Effective,
    /// The provider's order, ignoring manual orders.
    Provider,
    /// Displayed name, case-insensitive; ties in provider order.
    Name,
    /// Displayed number; unnumbered channels last, in provider order.
    Number,
}

/// A channel list request. The default lists every visible channel of the
/// source in effective order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelQuery {
    pub scope: ChannelScope,
    pub favourites_only: bool,
    /// Include hidden channels (edit mode).
    pub include_hidden: bool,
    /// Drop blocked channels (they are listed, flagged, by default).
    pub exclude_blocked: bool,
    pub order: ChannelOrder,
    pub offset: u32,
    /// `None` returns every row from `offset`.
    pub limit: Option<u32>,
}

pub(crate) const ROW_COLUMNS: &str = "c.source_id, c.id, c.provider_id, c.name, c.number, \
    c.logo, c.group_ids, c.stream, c.epg_id, c.epg_name, c.epg_shift_minutes, c.catch_up, \
    c.radio, o.custom_name, o.custom_number, COALESCE(o.hidden, 0), COALESCE(o.blocked, 0), \
    f.channel_id IS NOT NULL, o.epg_mode, o.epg_source, o.epg_channel";

pub(crate) const OVERLAY_JOINS: &str = "
    LEFT JOIN channel_overlays o ON o.source_id = c.source_id AND o.channel_id = c.id
    LEFT JOIN favourites f ON f.source_id = c.source_id AND f.channel_id = c.id";

pub(crate) fn decode(row: &rusqlite::Row<'_>) -> Result<ChannelRow> {
    let catch_up: Option<String> = row.get(11)?;
    let channel = Channel {
        id: ContentId::from_stored(row.get::<_, String>(1)?),
        provider_id: row.get(2)?,
        name: row.get(3)?,
        number: narrow(row.get(4)?, "channels.number")?,
        logo: row.get(5)?,
        groups: group_ids_from_json(&row.get::<_, String>(6)?)?,
        stream: stream_from_json(&row.get::<_, String>(7)?)?,
        epg_id: row.get(8)?,
        epg_name: row.get(9)?,
        epg_shift_minutes: narrow(row.get(10)?, "channels.epg_shift_minutes")?,
        catch_up: catch_up.as_deref().map(catch_up_from_json).transpose()?,
        radio: row.get(12)?,
    };
    let custom_name: Option<String> = row.get(13)?;
    let custom_number: Option<u32> = narrow(row.get(14)?, "channel_overlays.custom_number")?;
    Ok(ChannelRow {
        source_id: SourceId(row.get(0)?),
        name: custom_name.clone().unwrap_or_else(|| channel.name.clone()),
        number: custom_number.or(channel.number),
        custom_name,
        custom_number,
        hidden: row.get(15)?,
        blocked: row.get(16)?,
        favourite: row.get(17)?,
        epg_override: EpgOverride::from_columns(row.get(18)?, row.get(19)?, row.get(20)?),
        channel,
    })
}

fn collect(stmt: &mut rusqlite::Statement<'_>, args: Vec<Value>) -> Result<Vec<ChannelRow>> {
    let mut rows = stmt.query(params_from_iter(args))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(decode(row)?);
    }
    Ok(out)
}

/// What a statement needs besides the query itself.
#[derive(Clone, Copy)]
enum Purpose {
    /// Rows; `manual` says the list has a manual order to sort by.
    Rows {
        manual: bool,
    },
    Count,
}

/// `FROM … WHERE …` and the `ORDER BY` terms for a query; parameters are
/// appended to `args`. Joins are added only when the purpose needs them, so
/// common lists stay index-ordered scans (no sort over the whole source).
fn build(
    source: SourceId,
    q: &ChannelQuery,
    purpose: Purpose,
    args: &mut Vec<Value>,
) -> (String, String) {
    args.push(Value::Integer(source.0));
    let rows = matches!(purpose, Purpose::Rows { .. });
    let manual =
        matches!(purpose, Purpose::Rows { manual: true }) && q.order == ChannelOrder::Effective;
    let overlay = rows || !q.include_hidden || q.exclude_blocked;
    let favourites = if q.favourites_only {
        " JOIN favourites f ON f.source_id = c.source_id AND f.channel_id = c.id"
    } else if rows {
        " LEFT JOIN favourites f ON f.source_id = c.source_id AND f.channel_id = c.id"
    } else {
        ""
    };
    let mut joins = String::new();
    if overlay {
        joins.push_str(
            " LEFT JOIN channel_overlays o ON o.source_id = c.source_id AND o.channel_id = c.id",
        );
    }
    let manual_join = |scope: &str| {
        if manual {
            format!(
                " LEFT JOIN manual_order mo ON mo.source_id = c.source_id
                     AND mo.scope = {scope} AND mo.channel_id = c.id"
            )
        } else {
            String::new()
        }
    };
    let by_manual = if manual {
        "mo.position IS NULL, mo.position"
    } else {
        "c.position"
    };
    let (mut from, effective) = match &q.scope {
        // Favourites are few: drive the scan from them, in their order.
        ChannelScope::All if q.favourites_only => (
            format!(
                "FROM favourites f
                 JOIN channels c ON c.source_id = f.source_id AND c.id = f.channel_id{joins}
                 WHERE f.source_id = ?1"
            ),
            "f.position",
        ),
        ChannelScope::All => (
            format!(
                "FROM channels c{joins}{favourites}{} WHERE c.source_id = ?1",
                manual_join("''")
            ),
            by_manual,
        ),
        ChannelScope::Group(g) => {
            args.push(Value::Text(g.as_str().to_owned()));
            (
                format!(
                    "FROM channel_groups m
                     JOIN channels c ON c.source_id = m.source_id AND c.id = m.channel_id
                     {joins}{favourites}{}
                     WHERE m.source_id = ?1 AND m.group_id = ?2",
                    manual_join("?2")
                ),
                by_manual,
            )
        }
        ChannelScope::Custom(g) => {
            args.push(Value::Integer(g.0));
            (
                format!(
                    "FROM custom_group_members cm
                     JOIN custom_groups cg ON cg.id = cm.group_id
                     JOIN channels c ON c.source_id = cg.source_id AND c.id = cm.channel_id
                     {joins}{favourites}
                     WHERE cg.source_id = ?1 AND cm.group_id = ?2"
                ),
                "cm.position",
            )
        }
    };
    if !q.include_hidden {
        from.push_str(" AND COALESCE(o.hidden, 0) = 0");
    }
    if q.exclude_blocked {
        from.push_str(" AND COALESCE(o.blocked, 0) = 0");
    }
    let order = match q.order {
        ChannelOrder::Effective => format!("{effective}, c.position"),
        ChannelOrder::Provider => "c.position".into(),
        ChannelOrder::Name => "COALESCE(o.custom_name, c.name) COLLATE NOCASE, c.position".into(),
        ChannelOrder::Number => "COALESCE(o.custom_number, c.number) IS NULL, \
             COALESCE(o.custom_number, c.number), c.position"
            .into(),
    };
    (from, order)
}

impl Store {
    /// Whether the list of `scope` has a stored manual order.
    fn has_manual_order(&self, source: SourceId, scope: &ChannelScope) -> Result<bool> {
        let key = match scope {
            ChannelScope::All => "",
            ChannelScope::Group(g) => g.as_str(),
            ChannelScope::Custom(_) => return Ok(false),
        };
        Ok(self
            .conn
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM manual_order WHERE source_id = ?1 AND scope = ?2)",
            )?
            .query_row(rusqlite::params![source.0, key], |r| r.get(0))?)
    }

    /// Channels of one source with overlays applied, filtered, in the
    /// requested order, paged. Overlays of channels that are not in the
    /// catalog are never returned (see [`Store::orphaned_overlays`]).
    pub fn load_channels(&self, source: SourceId, query: &ChannelQuery) -> Result<Vec<ChannelRow>> {
        let manual = query.order == ChannelOrder::Effective
            && !(query.favourites_only && query.scope == ChannelScope::All)
            && self.has_manual_order(source, &query.scope)?;
        let mut args = Vec::new();
        let (from, order) = build(source, query, Purpose::Rows { manual }, &mut args);
        let sql = format!(
            "SELECT {ROW_COLUMNS} {from} ORDER BY {order} LIMIT {} OFFSET {}",
            query.limit.map_or(-1, i64::from),
            query.offset
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        collect(&mut stmt, args)
    }

    /// Rows `load_channels` would return without paging.
    pub fn channel_count(&self, source: SourceId, query: &ChannelQuery) -> Result<u64> {
        let mut args = Vec::new();
        let (from, _) = build(source, query, Purpose::Count, &mut args);
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT COUNT(*) {from}"))?;
        let n: i64 = stmt.query_row(params_from_iter(args), |r| r.get(0))?;
        Ok(n.max(0) as u64)
    }

    /// One channel by stable id, whatever its flags.
    pub fn channel(&self, source: SourceId, id: &ContentId) -> Result<Option<ChannelRow>> {
        let sql = format!(
            "SELECT {ROW_COLUMNS} FROM channels c {OVERLAY_JOINS}
             WHERE c.source_id = ?1 AND c.id = ?2"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let mut rows = collect(
            &mut stmt,
            vec![
                Value::Integer(source.0),
                Value::Text(id.as_str().to_owned()),
            ],
        )?;
        Ok(rows.pop())
    }

    /// Visible channels whose displayed or provider name contains every
    /// word of `text`, best match first. `source = None` searches every
    /// source. Hidden channels are skipped; blocked ones are returned
    /// flagged.
    pub fn search_channels(
        &self,
        source: Option<SourceId>,
        text: &str,
        limit: u32,
    ) -> Result<Vec<ChannelRow>> {
        let words = words(text);
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let mut args = vec![Value::Integer(i64::from(limit))];
        let source_filter = match source {
            Some(s) => {
                args.push(Value::Integer(s.0));
                "AND c.source_id = ?2"
            }
            None => "",
        };
        let visible = "AND COALESCE(o.hidden, 0) = 0";
        let sql = if self.fts {
            args.push(Value::Text(fts_query(&words)));
            let at = args.len();
            format!(
                "SELECT {ROW_COLUMNS} FROM channel_fts s
                 JOIN channels c ON c.rowid = s.rowid {OVERLAY_JOINS}
                 WHERE channel_fts MATCH ?{at} {source_filter} {visible}
                 ORDER BY s.rank, c.source_id, c.position LIMIT ?1"
            )
        } else {
            let mut filters = String::new();
            for w in &words {
                args.push(Value::Text(like_pattern(w)));
                let at = args.len();
                filters.push_str(&format!(
                    " AND (c.name LIKE ?{at} ESCAPE '\\' OR o.custom_name LIKE ?{at} ESCAPE '\\')"
                ));
            }
            format!(
                "SELECT {ROW_COLUMNS} FROM channels c {OVERLAY_JOINS}
                 WHERE 1 = 1 {source_filter} {visible} {filters}
                 ORDER BY c.source_id, c.position LIMIT ?1"
            )
        };
        let mut stmt = self.conn.prepare(&sql)?;
        collect(&mut stmt, args)
    }
}
