//! `catchup-source` placeholder expansion.

use crate::error::CatchupError;
use crate::time::wall_clock;

/// Values a template can refer to, already corrected and buffered.
#[derive(Clone, Copy, Debug)]
pub(super) struct Vars<'a> {
    /// Where the replay starts (Unix seconds).
    pub from: i64,
    /// Where it ends (Unix seconds).
    pub to: i64,
    pub now: i64,
    /// The program has not ended yet.
    pub on_air: bool,
    /// Viewer's UTC offset for date-part placeholders.
    pub local_offset_secs: i32,
    /// The program's XMLTV `catchup-id`.
    pub catchup_id: Option<&'a str>,
}

impl Vars<'_> {
    /// Replay length in seconds, at least 1.
    pub(super) fn duration(&self) -> i64 {
        self.to.saturating_sub(self.from).max(1)
    }

    fn offset(&self) -> i64 {
        self.now.saturating_sub(self.from).max(0)
    }
}

/// Which placeholders a string may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scope {
    /// A `catchup-source`: all of them.
    Archive,
    /// A channel's own URL: only the current time (`lutc`/`now`/`timestamp`).
    Live,
}

/// Expands `{x}` and `${x}` placeholders; unknown ones are left untouched.
/// `{catchup-id}` without an id is [`CatchupError::MissingTemplate`].
pub(super) fn expand(template: &str, vars: &Vars, scope: Scope) -> Result<String, CatchupError> {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let dollar = open > 0 && rest.as_bytes()[open - 1] == b'$';
        let Some(close) = rest[open..].find('}').map(|c| open + c) else {
            break;
        };
        let token = &rest[open + 1..close];
        let value = match scope {
            Scope::Archive if token == "catchup-id" => Some(
                vars.catchup_id
                    .ok_or(CatchupError::MissingTemplate)?
                    .to_owned(),
            ),
            Scope::Archive => placeholder(token, vars),
            Scope::Live => live_placeholder(token, vars),
        };
        match value {
            Some(value) => {
                out.push_str(&rest[..if dollar { open - 1 } else { open }]);
                out.push_str(&value);
            }
            None => out.push_str(&rest[..=close]),
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn live_placeholder(token: &str, vars: &Vars) -> Option<String> {
    let name = token.split_once(':').map_or(token, |(n, _)| n);
    matches!(name, "lutc" | "now" | "timestamp")
        .then(|| placeholder(token, vars))
        .flatten()
}

fn placeholder(token: &str, vars: &Vars) -> Option<String> {
    let (name, arg) = match token.split_once(':') {
        Some((n, a)) => (n, Some(a)),
        None => (token, None),
    };
    let divided = |v: i64| match arg {
        None => Some(v.to_string()),
        Some(a) => a
            .parse::<i64>()
            .ok()
            .filter(|d| *d > 0)
            .map(|d| (v / d).to_string()),
    };
    let local = |t: i64| t.saturating_add(i64::from(vars.local_offset_secs));
    let formatted = |t: i64| match arg {
        None => Some(t.to_string()),
        Some(fmt) => Some(format_time(fmt, local(t))),
    };
    match name {
        "utc" | "start" => formatted(vars.from),
        "utcend" | "end" => formatted(vars.to),
        "lutc" | "now" | "timestamp" => formatted(vars.now),
        "duration" => divided(vars.duration()),
        "offset" => divided(vars.offset()),
        "Y" | "m" | "d" | "H" | "M" | "S" if arg.is_none() => {
            Some(format_time(name, local(vars.from)))
        }
        _ => None,
    }
}

/// Formats the wall clock `t` (Unix seconds already shifted to the wanted
/// zone) with `Y m d H M S` letters; other characters are copied.
fn format_time(fmt: &str, t: i64) -> String {
    let w = wall_clock(t, 0);
    let mut out = String::with_capacity(fmt.len() * 2);
    for c in fmt.chars() {
        match c {
            'Y' => out.push_str(&format!("{:04}", w.year)),
            'm' => out.push_str(&format!("{:02}", w.month)),
            'd' => out.push_str(&format!("{:02}", w.day)),
            'H' => out.push_str(&format!("{:02}", w.hour)),
            'M' => out.push_str(&format!("{:02}", w.minute)),
            'S' => out.push_str(&format!("{:02}", w.second)),
            other => out.push(other),
        }
    }
    out
}
