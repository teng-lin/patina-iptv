//! Catch-up (archive) URLs for M3U channels.
//!
//! [`build_catchup_url`] turns a channel's [`CatchUp`](crate::model::CatchUp) and a
//! [`CatchupRequest`] into the replay URL, or a [`CatchupError`].
//!
//! ## Modes
//!
//! Rules per [`CatchUpKind`] follow Kodi pvr.iptvsimple's modes, plus
//! IPTVnator's `shift` fallback for archive days without a type:
//!
//! | `catchup=` | URL |
//! | --- | --- |
//! | `default` (also: a source without a type) | the expanded `catchup-source`; one without `scheme://` is appended to the live URL |
//! | `append` | live URL + expanded `catchup-source` |
//! | `shift`, `timeshift` (also: days without a type) | live URL with `utc`/`lutc` set; other query bytes kept exactly |
//! | `flussonic`, `flussonic-hls`, `flussonic-ts`, `fs` | `NAME-{start}-{duration}.m3u8` / `-now.m3u8`, or `timeshift_abs-{start}.ts` ([Flussonic DVR playback]) |
//! | `xc` | `[/live]/U/P/ID[.EXT]` → `/timeshift/U/P/{minutes}/{Y-m-d:H-M}/ID.EXT`, EXT `m3u8` for an `.m3u8` live URL, else `ts`; clock: panel's, else the viewer's |
//! | `vod` | the expanded `catchup-source`, default `{catchup-id}` |
//! | other values, Stalker | [`CatchupError::Unsupported`] |
//!
//! For `shift`, `flussonic` and `xc`, a declared absolute HTTP(S)
//! `catchup-source` wins over the generated URL (IPTVnator). `default` and
//! `append` without a source are [`CatchupError::MissingTemplate`].
//!
//! **A source without placeholders is used verbatim** (Kodi: the source is
//! the full URL). IPTVnator sets `utc`/`lutc` on such a source instead; we
//! deliberately follow Kodi so a fixed or signed archive URL is never
//! altered.
//!
//! The live URL's own `{lutc}`/`${now}`/`${timestamp}` placeholders are
//! expanded first ([`expand_live_url`]); generated URLs keep its query and
//! fragment byte for byte, and no URL is re-serialised by a URL library.
//!
//! ## Placeholders
//!
//! In `{x}` or `${x}` form; unknown ones are left untouched.
//!
//! | Placeholder | Value |
//! | --- | --- |
//! | `utc`, `start` | replay start, Unix seconds (UTC) |
//! | `utcend`, `end` | replay end, Unix seconds |
//! | `lutc`, `now`, `timestamp` | now, Unix seconds |
//! | `duration`, `duration:N` | replay length in seconds (buffers included, ≥ 1), divided by N |
//! | `offset`, `offset:N` | now − replay start in seconds, divided by N |
//! | `Y` `m` `d` `H` `M` `S` | replay start, viewer's clock |
//! | `utc:FMT`, `start:FMT`, `utcend:FMT`, `end:FMT`, `lutc:FMT`, `now:FMT`, `timestamp:FMT` | that time formatted with the letters above (`{utc:Y-m-d}`), viewer's clock |
//! | `catchup-id` | [`CatchupRequest::catchup_id`]; missing → [`CatchupError::MissingTemplate`] |
//!
//! The viewer's clock is [`CatchupRequest::local_offset_secs`] (Kodi formats
//! these in the current locale); Unix numbers stay UTC.
//!
//! ## Timing
//!
//! `catchup-correction` ([`CatchUp::correction_secs`](crate::model::CatchUp::correction_secs)) is added to the
//! programme's `start` and `end` first (Kodi: the times used to build the
//! URL are corrected); `now` is not shifted, and the checks below use the
//! corrected times. The replay then runs from
//! `start − buffer_before_secs + offset_secs` to `end + buffer_after_secs`
//! (the end buffer stops at `now`). A programme that starts after `now`, or
//! a replay start after `now`, is [`CatchupError::FutureProgramme`] (also
//! for `vod`, which Kodi lets reach future programmes); a replay start past
//! the replay end, or before the archive window (`catchup-days`, else
//! [`CatchupRequest::default_window_days`]), is
//! [`CatchupError::OutsideArchive`].
//!
//! ## References
//!
//! Behaviour is written from these public descriptions (no code copied):
//! [Kodi pvr.iptvsimple README (Omega)], sections "Catchup", "Catchup format
//! specifiers" and "Supported M3U and XMLTV elements";
//! [Flussonic DVR playback]; IPTVnator's `catchup.utils.ts` (MIT) for the
//! `shift` fallback.
//!
//! [Kodi pvr.iptvsimple README (Omega)]: https://github.com/kodi-pvr/pvr.iptvsimple/blob/Omega/README.md
//! [Flussonic DVR playback]: https://flussonic.com/doc/dvr/playback/

mod template;

use template::{Scope, Vars, expand};

pub use crate::error::CatchupError;
use crate::model::{CatchUpKind, Channel, StreamLocator};

/// Archive window assumed when the playlist declares catch-up without
/// `catchup-days` (our default; Kodi leaves it to a user setting).
pub const DEFAULT_CATCHUP_WINDOW_DAYS: u32 = 7;

/// What to replay. All times are Unix seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatchupRequest<'a> {
    /// Programme start and end from the guide.
    pub start: i64,
    pub end: i64,
    pub now: i64,
    /// Where to begin inside the replay, from its start (the programme start
    /// minus `buffer_before_secs`), for seeking. `0` plays from the start.
    pub offset_secs: u32,
    /// The viewer's UTC offset in seconds at `start`. Date-part placeholders
    /// (`{Y}`…`{S}`, `{utc:FMT}`) render in this clock, like Kodi's
    /// locale-based formatting; Unix-number placeholders stay UTC. The host
    /// resolves its zone (and DST) to this number; no tz database here.
    pub local_offset_secs: i32,
    /// Only for `catchup="xc"`: the panel's UTC offset
    /// ([`crate::xtream::ServerClock::offset_minutes`]). `None` (unknown)
    /// uses `local_offset_secs`, as IPTVnator falls back to the viewer's clock.
    pub server_offset_minutes: Option<i32>,
    /// Archive window used when the channel declares none.
    pub default_window_days: u32,
    /// Extra time before the programme start and after its end (Kodi's
    /// "buffer before/after"); included in `{duration}`.
    pub buffer_before_secs: u32,
    pub buffer_after_secs: u32,
    /// The programme's XMLTV `catchup-id`, for `{catchup-id}` and the
    /// default `catchup="vod"` source.
    pub catchup_id: Option<&'a str>,
}

impl CatchupRequest<'_> {
    /// A request for `[start, end)` at `now` with no seek, no buffers and
    /// the [`DEFAULT_CATCHUP_WINDOW_DAYS`] window.
    pub const fn new(start: i64, end: i64, now: i64) -> Self {
        Self {
            start,
            end,
            now,
            offset_secs: 0,
            local_offset_secs: 0,
            server_offset_minutes: None,
            default_window_days: DEFAULT_CATCHUP_WINDOW_DAYS,
            buffer_before_secs: 0,
            buffer_after_secs: 0,
            catchup_id: None,
        }
    }
}

/// Expands the placeholders Kodi allows in a channel's own (live) URL:
/// `{lutc}`, `${now}`, `${timestamp}` and their `:FMT` forms (FMT in the
/// viewer's clock, `local_offset_secs`). Other placeholders are left as they
/// are. Catch-up URLs built from the live URL apply this first.
pub fn expand_live_url(url: &str, now: i64, local_offset_secs: i32) -> String {
    let vars = Vars {
        from: now,
        to: now,
        now,
        on_air: true,
        local_offset_secs,
        catchup_id: None,
    };
    // Live scope never fails: `{catchup-id}` is left as it is there.
    expand(url, &vars, Scope::Live).unwrap_or_else(|_| url.to_owned())
}

/// The replay URL for a programme on `channel`. See the module docs for the
/// rules and the checks.
pub fn build_catchup_url(channel: &Channel, req: CatchupRequest) -> Result<String, CatchupError> {
    let catch_up = channel.catch_up.as_ref().ok_or(CatchupError::Unsupported)?;
    let StreamLocator::Url { url: live, .. } = &channel.stream else {
        return Err(CatchupError::Unsupported);
    };
    if matches!(
        catch_up.kind,
        CatchUpKind::Unsupported(_) | CatchUpKind::Stalker
    ) {
        return Err(CatchupError::Unsupported);
    }
    let days = catch_up.window_days(req.default_window_days);
    let vars = window(&req, days, catch_up.correction_secs)?;
    let live = &expand(live, &vars, Scope::Live)?;
    let source = catch_up
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let full = |src: &str| checked(expand(src, &vars, Scope::Archive)?);
    let appended = |src: &str| checked(format!("{live}{}", expand(src, &vars, Scope::Archive)?));
    match &catch_up.kind {
        CatchUpKind::Vod => full(source.unwrap_or("{catchup-id}")),
        CatchUpKind::Append => appended(source.ok_or(CatchupError::MissingTemplate)?),
        CatchUpKind::Default => match source.ok_or(CatchupError::MissingTemplate)? {
            src if has_scheme(src) => full(src),
            src => appended(src),
        },
        _ => match source.filter(|s| is_http(s)) {
            Some(src) => full(src),
            None => match &catch_up.kind {
                CatchUpKind::Flussonic => flussonic(live, &vars),
                CatchUpKind::Xtream => xtream(
                    live,
                    &vars,
                    req.server_offset_minutes
                        .map_or(req.local_offset_secs, |m| m.saturating_mul(60)),
                ),
                _ => set_utc_params(live, &vars),
            },
        },
    }
}

/// `scheme://…`: a complete URL rather than a suffix for the live URL.
fn has_scheme(s: &str) -> bool {
    s.split_once("://").is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    })
}

/// The finished URL must parse; it is returned as built, not re-serialised.
fn checked(url: String) -> Result<String, CatchupError> {
    match url::Url::parse(&url) {
        Ok(_) => Ok(url),
        Err(_) => Err(CatchupError::MalformedUrl),
    }
}

/// Applies the correction, buffers and seek offset, and checks the result
/// against now and the archive window.
fn window<'a>(
    req: &CatchupRequest<'a>,
    days: u32,
    correction_secs: i32,
) -> Result<Vars<'a>, CatchupError> {
    let shift = i64::from(correction_secs);
    // Saturating: guide times are untrusted input and must not panic.
    let (start, end, now) = (
        req.start.saturating_add(shift),
        req.end.saturating_add(shift),
        req.now,
    );
    if start > now {
        return Err(CatchupError::FutureProgramme);
    }
    let from = start
        .saturating_sub(i64::from(req.buffer_before_secs))
        .saturating_add(i64::from(req.offset_secs));
    let to = end
        .saturating_add(i64::from(req.buffer_after_secs))
        .min(end.max(now));
    if from > now {
        return Err(CatchupError::FutureProgramme);
    }
    if from > to || from.max(start) < now.saturating_sub(i64::from(days) * 86_400) {
        return Err(CatchupError::OutsideArchive);
    }
    Ok(Vars {
        from,
        to,
        now,
        on_air: end > now,
        local_offset_secs: req.local_offset_secs,
        catchup_id: req.catchup_id,
    })
}

fn is_http(s: &str) -> bool {
    url::Url::parse(s)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}

/// Sets `utc`/`lutc` on `raw` by editing the string: earlier `utc`/`lutc`
/// pairs are dropped, every other query byte is kept as written (signed
/// tokens break when a URL library re-encodes them).
fn set_utc_params(raw: &str, vars: &Vars) -> Result<String, CatchupError> {
    let raw = raw.trim();
    if !is_http(raw) {
        return Err(CatchupError::MalformedUrl);
    }
    let (head, fragment) = match raw.split_once('#') {
        Some((h, f)) => (h, Some(f)),
        None => (raw, None),
    };
    let (base, query) = head.split_once('?').unwrap_or((head, ""));
    let mut out = String::with_capacity(raw.len() + 32);
    out.push_str(base);
    out.push('?');
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let key = pair.split_once('=').map_or(pair, |(k, _)| k);
        if key != "utc" && key != "lutc" {
            out.push_str(pair);
            out.push('&');
        }
    }
    out.push_str(&format!("utc={}&lutc={}", vars.from, vars.now));
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    Ok(out)
}

/// Flussonic DVR forms: HLS `NAME-FROM-DURATION.m3u8`, or `NAME-FROM-now.m3u8`
/// while the programme is on air; MPEG-TS `timeshift_abs-FROM.ts` (open-ended
/// either way). Query and fragment are kept byte for byte.
fn flussonic(live: &str, vars: &Vars) -> Result<String, CatchupError> {
    let (origin, path, rest) = split_path(live).ok_or(CatchupError::MalformedUrl)?;
    let new_path = if let Some(base) = path.strip_suffix("/mpegts") {
        format!("{base}/timeshift_abs-{}.ts", vars.from)
    } else {
        let stem = path
            .strip_suffix(".m3u8")
            .ok_or(CatchupError::MalformedUrl)?;
        if vars.on_air {
            format!("{stem}-{}-now.m3u8", vars.from)
        } else {
            format!("{stem}-{}-{}.m3u8", vars.from, vars.duration())
        }
    };
    checked(format!("{origin}{new_path}{rest}"))
}

/// An HTTP(S) URL split into `scheme://authority`, path and `?query#fragment`.
fn split_path(raw: &str) -> Option<(&str, &str, &str)> {
    let raw = raw.trim();
    if !is_http(raw) {
        return None;
    }
    let authority = raw.find("://")? + 3;
    let path_start = raw[authority..]
        .find(['/', '?', '#'])
        .map_or(raw.len(), |i| authority + i);
    let path_end = raw[path_start..]
        .find(['?', '#'])
        .map_or(raw.len(), |i| path_start + i);
    Some((
        &raw[..path_start],
        &raw[path_start..path_end],
        &raw[path_end..],
    ))
}

fn xtream(live: &str, vars: &Vars, offset_secs: i32) -> Result<String, CatchupError> {
    let (origin, path, rest) = split_path(live).ok_or(CatchupError::MalformedUrl)?;
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (user, pass, last) = match segments.as_slice() {
        ["live", u, p, last] => (u, p, last),
        [u, p, last] => (u, p, last),
        _ => return Err(CatchupError::MalformedUrl),
    };
    let (id, ext) = last.split_once('.').unwrap_or((last, ""));
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CatchupError::MalformedUrl);
    }
    // Kodi builds xc archives for both containers; keep the live one.
    let ext = if ext.eq_ignore_ascii_case("m3u8") {
        "m3u8"
    } else {
        "ts"
    };
    let minutes = crate::time::catchup_duration_minutes(vars.from, vars.to);
    let start =
        crate::time::format_catchup_start(vars.from.saturating_add(i64::from(offset_secs)), 0);
    checked(format!(
        "{origin}/timeshift/{user}/{pass}/{minutes}/{start}/{id}.{ext}{rest}"
    ))
}

#[cfg(test)]
mod tests;
