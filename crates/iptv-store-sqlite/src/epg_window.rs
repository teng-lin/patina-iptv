//! A guide window's programmes, read into settled builders
//! (`Store::load_epg`).
//!
//! One query per bound EPG channel (one statement over all of them was no
//! faster). Each seeks to the last programme starting at or before the
//! window: stored programmes do not overlap, so no older one reaches into
//! it, and the days the store keeps before the window (`epg.keep_past`)
//! are not walked.

use std::collections::{HashMap, HashSet};

use iptv_epg::{ProgramData, SettledBuilder};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, Row, params};

use crate::epg::{Binding, EpgWindow};
use crate::error::{Result, StoreError};

/// The columns of a programme row, from `p` (and `d` for descriptions).
const COLUMNS: &str = "p.start, p.stop, p.title, p.sub_title, p.category, p.icon, p.rating,
     p.catchup_id, p.season, p.episode";

/// The seek: the last programme of channel `{ch}` starting at or before
/// `?2` on (`?2` itself when none does).
pub(crate) fn seek(ch: &str) -> String {
    format!(
        "p.start >= COALESCE((SELECT MAX(q.start) FROM epg_programs q
                              WHERE q.channel_id = {ch} AND q.start <= ?2), ?2)"
    )
}

/// One channel's (`?1`) programmes overlapping `[?2, ?3)`.
fn channel_sql(descriptions: bool) -> String {
    let (desc, join) = desc_parts(descriptions);
    format!(
        "SELECT {COLUMNS}, {desc} FROM epg_programs p {join}
         WHERE p.channel_id = ?1 AND p.start < ?3 AND p.stop > ?2 AND {}
         ORDER BY p.start",
        seek("?1")
    )
}

fn desc_parts(descriptions: bool) -> (&'static str, &'static str) {
    if descriptions {
        (
            "d.description",
            "LEFT JOIN epg_descriptions d ON d.channel_id = p.channel_id AND d.start = p.start",
        )
    } else {
        ("NULL", "")
    }
}

/// Reads `window`'s programmes of the channels `bindings` bind into
/// `builders` (by feed; a feed without one is skipped), each EPG channel
/// once.
pub(crate) fn read_programs(
    conn: &Connection,
    bindings: &[Binding],
    builders: &mut HashMap<u32, SettledBuilder>,
    window: &EpgWindow<'_>,
) -> Result<()> {
    let mut seen = HashSet::new();
    let channels: Vec<&Binding> = bindings
        .iter()
        .filter(|b| builders.contains_key(&b.feed) && seen.insert(b.channel))
        .collect();
    let mut stmt = conn.prepare_cached(&channel_sql(window.descriptions))?;
    for b in channels {
        let Some(builder) = builders.get_mut(&b.feed) else {
            continue;
        };
        builder.channel(&b.epg_channel);
        let mut rows = stmt.query(params![b.channel, window.from, window.to])?;
        while let Some(r) = rows.next()? {
            builder.push(&program(r)?);
        }
    }
    Ok(())
}

/// A row's programme (columns as [`COLUMNS`], then the description).
fn program<'r>(r: &'r Row<'_>) -> Result<ProgramData<'r>> {
    Ok(ProgramData {
        start: r.get(0)?,
        stop: Some(r.get(1)?),
        title: text(r, 2)?.unwrap_or_default(),
        sub_title: text(r, 3)?,
        category: text(r, 4)?,
        icon: text(r, 5)?,
        rating: text(r, 6)?,
        catchup_id: text(r, 7)?,
        season: small(r, 8)?,
        episode: small(r, 9)?,
        desc: text(r, 10)?,
    })
}

/// A text column borrowed from the row.
fn text<'r>(row: &'r Row<'_>, i: usize) -> Result<Option<&'r str>> {
    match row.get_ref(i)? {
        ValueRef::Null => Ok(None),
        ValueRef::Text(t) => std::str::from_utf8(t)
            .map(Some)
            .map_err(|_| StoreError::InvalidData("epg_programs text".into())),
        _ => Err(StoreError::InvalidData("epg_programs text".into())),
    }
}

fn small(row: &Row<'_>, i: usize) -> Result<Option<u16>> {
    Ok(row
        .get::<_, Option<i64>>(i)?
        .and_then(|n| u16::try_from(n).ok()))
}
