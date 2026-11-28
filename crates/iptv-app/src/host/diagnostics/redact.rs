//! Final safety redaction for diagnostic log records, panic payloads and backtraces.
//!
//! Call sites across Patina already redact URLs and paths, yet third-party crates
//! or panic messages can still mention raw addresses, `user:pass@` authorities,
//! query tokens or local storage paths. Every diagnostic record passes through
//! [`redact_text`] before it enters memory or disk.

const REDACTED_URL: &str = "[redacted-url]";
const REDACTED_AUTH: &str = "[redacted-auth]";
const REDACTED_PATH: &str = "[redacted-path]";
const REDACTED_VALUE: &str = "[redacted]";

const EQ_SENSITIVE_KEYS: &[&str] = &[
    "access_token",
    "refresh_token",
    "api_key",
    "apikey",
    "x-api-key",
    "x_api_key",
    "password",
    "passwd",
    "pass",
    "secret",
    "credential",
    "token",
    "auth",
    "authorization",
    "cookie",
    "set-cookie",
    "set_cookie",
    "session",
    "session_id",
    "sessionid",
    "pin",
    "username",
    "user",
    "login",
    "mac",
    "sn",
    "device_id",
    "device_id2",
    "signature",
    "signature2",
    "prehash",
    "response",
    "nonce",
];

const COLON_UNQUOTED_KEYS: &[&str] = &[
    "access_token",
    "refresh_token",
    "api_key",
    "apikey",
    "x-api-key",
    "x_api_key",
    "password",
    "passwd",
    "secret",
    "credential",
    "token",
    "authorization",
    "cookie",
    "set-cookie",
    "set_cookie",
    "session_id",
    "sessionid",
    "pin",
    "username",
    "mac",
    "device_id",
    "device_id2",
    "signature",
    "signature2",
    "prehash",
    "response",
    "nonce",
];

const HEADER_REST_KEYS: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "set_cookie",
    "x-api-key",
    "x_api_key",
];

/// Normalizes a sensitive directory root into `/` and `\` forms, without trailing
/// separators, sorted longest-first so nested prefixes match before shorter parents.
pub(crate) fn normalize_roots(raw: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out = Vec::new();
    for entry in raw {
        let trimmed = entry.trim().trim_end_matches(['/', '\\']);
        if trimmed.len() < 2 {
            continue;
        }
        let forward = trimmed.replace('\\', "/");
        let backward = trimmed.replace('/', "\\");
        for candidate in [trimmed.to_owned(), forward, backward] {
            if candidate.len() >= 2 && !out.contains(&candidate) {
                out.push(candidate);
            }
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.len()));
    out
}

/// Strips URLs, `user:pass@` authorities, query/header tokens and sensitive paths.
pub(crate) fn redact_text(input: &str, roots: &[String]) -> String {
    let step1 = redact_urls(input);
    let step2 = redact_xtream_paths(&step1);
    let step3 = redact_registered_roots(&step2, roots);
    let step4 = redact_os_user_paths(&step3);
    let step5 = redact_user_pass_at(&step4);
    let step6 = redact_bearer_and_basic(&step5);
    redact_kv_secrets(&step6)
}

fn is_token_terminator(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
        )
}

/// Scans to the end of a URL or authority+path token, keeping RFC 3986 sub-delims
/// (`,`, `;`, and unquoted `'`) and balanced `(...)` / `[...]` inside the URL while
/// stopping at whitespace, closing quotes, angle/curly brackets, or unmatched `)` / `]`.
fn url_body_end(rest: &str, enclosing_quote: Option<char>) -> usize {
    let mut bracket_depth = 0usize;
    let mut paren_depth = 0usize;
    for (idx, ch) in rest.char_indices() {
        match ch {
            '[' => bracket_depth += 1,
            ']' if bracket_depth > 0 => bracket_depth -= 1,
            ']' => return idx,
            '(' => paren_depth += 1,
            ')' if paren_depth > 0 => paren_depth -= 1,
            ')' => return idx,
            other if Some(other) == enclosing_quote => return idx,
            '"' if enclosing_quote.is_none() => return idx,
            other if other.is_whitespace() || matches!(other, '<' | '>' | '{' | '}') => {
                return idx;
            }
            _ => {}
        }
    }
    rest.len()
}

fn trim_trailing_url_punct(slice: &str, strip_apostrophe: bool) -> usize {
    let mut len = slice.len();
    while len > 0 {
        let last = slice.as_bytes()[len - 1];
        if matches!(last, b'.' | b',' | b';' | b':' | b'!' | b'?')
            || (strip_apostrophe && last == b'\'')
        {
            len -= 1;
        } else {
            break;
        }
    }
    len
}

fn space_continues_path(after_space: &str) -> bool {
    let window = &after_space[..after_space.floor_char_boundary(32)];
    for ch in window.chars() {
        if matches!(ch, '/' | '\\') {
            return true;
        }
        if ch.is_control()
            || matches!(
                ch,
                '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | ':' | '=' | ',' | ';'
            )
        {
            return false;
        }
    }
    false
}

fn path_end(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    for (idx, ch) in rest.char_indices() {
        if ch == ' ' && space_continues_path(&rest[idx + 1..]) {
            continue;
        }
        if is_token_terminator(ch) {
            return idx;
        }
        // Preserve `: reason` prose suffixes while allowing Windows drive colons.
        if ch == ':' {
            let next = bytes.get(idx + 1).copied();
            if next != Some(b'/') && next != Some(b'\\') {
                return idx;
            }
        }
    }
    rest.len()
}

fn find_root_match(haystack: &str, root: &str) -> Option<usize> {
    let win_drive = root.len() >= 3
        && root.as_bytes()[0].is_ascii_alphabetic()
        && root.as_bytes()[1] == b':'
        && matches!(root.as_bytes()[2], b'/' | b'\\');
    if !win_drive {
        return haystack.find(root);
    }
    if !root.is_ascii() {
        return haystack.find(root);
    }
    haystack
        .char_indices()
        .find(|&(idx, _)| {
            haystack[idx..]
                .get(..root.len())
                .is_some_and(|sub| sub.eq_ignore_ascii_case(root))
        })
        .map(|(idx, _)| idx)
}

fn redact_registered_roots(input: &str, roots: &[String]) -> String {
    let mut current = input.to_owned();
    for root in roots {
        if root.len() < 2 || find_root_match(&current, root).is_none() {
            continue;
        }
        let mut next = String::with_capacity(current.len());
        let mut rest = current.as_str();
        while let Some(pos) = find_root_match(rest, root) {
            next.push_str(&rest[..pos]);
            let after_root = &rest[pos + root.len()..];
            let boundary = after_root.is_empty()
                || after_root.starts_with(['/', '\\'])
                || is_token_terminator(after_root.chars().next().unwrap_or(' '))
                || after_root.starts_with(':');
            if boundary {
                let tail_len = if after_root.starts_with(['/', '\\']) {
                    path_end(after_root)
                } else {
                    0
                };
                next.push_str(REDACTED_PATH);
                rest = &after_root[tail_len..];
            } else {
                next.push_str(&rest[pos..pos + root.len()]);
                rest = after_root;
            }
        }
        next.push_str(rest);
        current = next;
    }
    current
}

fn is_path_boundary_before(before: Option<char>) -> bool {
    match before {
        None => true,
        Some(ch) => {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '"' | '\'' | '(' | '[' | '{' | '<' | '=' | ':' | ',' | ';'
                )
        }
    }
}

fn redact_os_user_paths(input: &str) -> String {
    const UNIX_PREFIXES: &[&str] = &[
        "/home/",
        "/Users/",
        "/data/user/",
        "/data/data/",
        "/private/var/mobile/",
        "/var/mobile/Containers/",
        "/var/folders/",
        "/run/user/",
    ];
    let mut out = String::with_capacity(input.len());
    let mut idx = 0usize;
    while idx < input.len() {
        let slice = &input[idx..];
        let prev = input[..idx].chars().next_back();
        if is_path_boundary_before(prev) {
            if let Some(matched) = UNIX_PREFIXES.iter().find(|p| slice.starts_with(**p)) {
                let end = matched.len() + path_end(&slice[matched.len()..]);
                out.push_str(REDACTED_PATH);
                idx += end;
                continue;
            }
            // Windows user profile paths: `C:\Users\...` or `C:/Users/...`.
            let bytes = slice.as_bytes();
            if bytes.len() >= 9
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'\\' | b'/')
                && slice[3..].get(..6).is_some_and(|u| {
                    u.eq_ignore_ascii_case("users\\") || u.eq_ignore_ascii_case("users/")
                })
            {
                let end = 3 + path_end(&slice[3..]);
                out.push_str(REDACTED_PATH);
                idx += end;
                continue;
            }
        }
        let ch = slice.chars().next().unwrap();
        out.push(ch);
        idx += ch.len_utf8();
    }
    out
}

fn is_xtream_seg_bad(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '"' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | '?' | '#' | '=' | ':' | '\\'
        )
}

fn host_prefix_start(out: &str) -> Option<usize> {
    let boundary = |ch: char| {
        ch.is_whitespace() || matches!(ch, '"' | '\'' | '(' | '{' | '<' | '=' | ',' | ';')
    };
    let start = out
        .rfind(boundary)
        .map_or(0, |i| i + out[i..].chars().next().unwrap().len_utf8());
    let cand = &out[start..];
    if cand.is_empty() || cand.starts_with("[redacted") || cand.contains(['/', '\\']) {
        return None;
    }
    let looks_host = (cand.starts_with('[') && cand.contains(']'))
        || (cand.contains('.')
            && cand
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphanumeric()));
    looks_host.then_some(start)
}

fn match_prefixed_xtream(slice: &str) -> Option<(&'static str, usize, usize)> {
    const PREFIXES: &[&str] = &["/live/", "/movie/", "/series/", "/timeshift/"];
    let &prefix = PREFIXES.iter().find(|p| slice.starts_with(**p))?;
    let rest = &slice[prefix.len()..];
    let (seg1, after1) = rest.split_once('/')?;
    let (seg2, _) = after1.split_once('/')?;
    if seg1.is_empty()
        || seg2.is_empty()
        || seg1.starts_with("[redacted")
        || seg2.starts_with("[redacted")
        || seg1.chars().any(is_xtream_seg_bad)
        || seg2.chars().any(is_xtream_seg_bad)
    {
        return None;
    }
    Some((prefix, seg1.len(), seg2.len()))
}

fn match_short_xtream(slice: &str) -> Option<(usize, usize)> {
    const RESERVED_FIRST: &[&str] = &[
        "live",
        "movie",
        "series",
        "timeshift",
        "data",
        "proc",
        "sys",
        "dev",
        "run",
        "var",
        "tmp",
        "usr",
        "lib",
        "etc",
        "opt",
        "home",
        "users",
        "private",
        "crates",
        "vendor",
        "src",
    ];
    if !slice.starts_with('/') || slice.starts_with("//") {
        return None;
    }
    let (seg1, after1) = slice[1..].split_once('/')?;
    let (seg2, after2) = after1.split_once('/')?;
    if seg1.is_empty()
        || seg2.is_empty()
        || seg1.starts_with("[redacted")
        || seg2.starts_with("[redacted")
        || RESERVED_FIRST.iter().any(|r| seg1.eq_ignore_ascii_case(r))
        || seg1.chars().any(is_xtream_seg_bad)
        || seg2.chars().any(is_xtream_seg_bad)
    {
        return None;
    }
    let raw_end = url_body_end(after2, None);
    let trimmed_end = trim_trailing_url_punct(&after2[..raw_end], true);
    let file_part = after2[..trimmed_end].split(['?', '#']).next().unwrap_or("");
    if file_part.is_empty() || file_part.contains(['/', '\\']) {
        return None;
    }
    let (stem, ext) = match file_part.split_once('.') {
        Some((s, e)) => (s, Some(e)),
        None => (file_part, None),
    };
    if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    matches!(ext, None | Some("ts" | "m3u8" | "mp4" | "mkv" | "aac"))
        .then_some((seg1.len(), seg2.len()))
}

/// Redacts scheme-less Xtream credential paths (`/live/<user>/<pass>/...`,
/// `/movie/...`, `/series/...`, `/timeshift/...`, and short `/<user>/<pass>/<id>.ts`),
/// including optional scheme-less `host[:port]` prefixes.
fn redact_xtream_paths(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut idx = 0usize;
    while idx < input.len() {
        let slice = &input[idx..];
        if slice.starts_with('/') {
            let prev = input[..idx].chars().next_back();
            let boundary = is_path_boundary_before(prev);
            let host_start = (!boundary).then(|| host_prefix_start(&out)).flatten();
            if boundary || host_start.is_some() {
                let matched = match_prefixed_xtream(slice)
                    .or_else(|| match_short_xtream(slice).map(|(s1, s2)| ("/", s1, s2)));
                if let Some((prefix, seg1_len, seg2_len)) = matched {
                    if let Some(hstart) = host_start {
                        out.truncate(hstart);
                        out.push_str(REDACTED_URL);
                        let raw_end = url_body_end(slice, None);
                        idx += trim_trailing_url_punct(&slice[..raw_end], true);
                    } else {
                        out.push_str(prefix);
                        out.push_str(REDACTED_VALUE);
                        out.push('/');
                        out.push_str(REDACTED_VALUE);
                        out.push('/');
                        idx += prefix.len() + seg1_len + 1 + seg2_len + 1;
                    }
                    continue;
                }
            }
        }
        let ch = slice.chars().next().unwrap();
        out.push(ch);
        idx += ch.len_utf8();
    }
    out
}

fn is_scheme_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')
}

fn enclosing_quote_before(prefix: &str) -> Option<char> {
    prefix
        .chars()
        .next_back()
        .filter(|c| matches!(c, '"' | '\''))
}

fn redact_urls(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    let bytes = input.as_bytes();
    while let Some(rel) = input[cursor..].find("://") {
        let colon = cursor + rel;
        let mut scheme_start = colon;
        while scheme_start > cursor && is_scheme_char(bytes[scheme_start - 1]) {
            scheme_start -= 1;
        }
        let scheme = &bytes[scheme_start..colon];
        let valid_scheme = scheme.len() >= 2 && scheme[0].is_ascii_alphabetic();
        if !valid_scheme {
            out.push_str(&input[cursor..colon + 3]);
            cursor = colon + 3;
            continue;
        }
        out.push_str(&input[cursor..scheme_start]);
        let quote = enclosing_quote_before(&input[..scheme_start]);
        let after_scheme = &input[colon + 3..];
        let raw_len = url_body_end(after_scheme, quote);
        let url_len = trim_trailing_url_punct(&after_scheme[..raw_len], quote.is_none());
        out.push_str(REDACTED_URL);
        cursor = colon + 3 + url_len;
    }
    out.push_str(&input[cursor..]);
    out
}

fn is_auth_boundary(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | '(' | '[' | '{' | '=')
}

fn redact_user_pass_at(input: &str) -> String {
    if !input.contains('@') {
        return input.to_owned();
    }
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while let Some(rel) = input[cursor..].find('@') {
        let at = cursor + rel;
        let left_slice = &input[cursor..at];
        let token_start_rel = left_slice.rfind(is_auth_boundary).map_or(0, |i| {
            i + left_slice[i..].chars().next().unwrap().len_utf8()
        });
        let userinfo = &left_slice[token_start_rel..];
        let quote = enclosing_quote_before(&input[..cursor + token_start_rel]);
        let right_slice = &input[at + 1..];
        let right_len = url_body_end(right_slice, quote);
        let host_part = &right_slice[..right_len];
        let has_host = host_part
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '[');
        let has_single_colon = !userinfo.contains("::")
            && userinfo
                .split_once(':')
                .is_some_and(|(u, p)| !u.is_empty() && !p.is_empty() && !u.starts_with('['));
        if has_host && has_single_colon {
            out.push_str(&left_slice[..token_start_rel]);
            let trimmed_right = trim_trailing_url_punct(host_part, quote.is_none());
            out.push_str(REDACTED_AUTH);
            cursor = at + 1 + trimmed_right;
        } else {
            out.push_str(&input[cursor..=at]);
            cursor = at + 1;
        }
    }
    out.push_str(&input[cursor..]);
    out
}

fn redact_bearer_and_basic(input: &str) -> String {
    let mut current = input.to_owned();
    for scheme in ["bearer ", "basic "] {
        let mut next = String::with_capacity(current.len());
        let mut cursor = 0usize;
        while cursor < current.len() {
            let found = current[cursor..]
                .char_indices()
                .find(|&(rel, _)| {
                    current[cursor + rel..]
                        .get(..scheme.len())
                        .is_some_and(|s| s.eq_ignore_ascii_case(scheme))
                })
                .map(|(rel, _)| cursor + rel);
            let Some(start) = found else {
                break;
            };
            let before_ok = current[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
            let matched_prefix = &current[start..start + scheme.len()];
            if !before_ok {
                next.push_str(&current[cursor..start + scheme.len()]);
                cursor = start + scheme.len();
                continue;
            }
            next.push_str(&current[cursor..start]);
            next.push_str(matched_prefix);
            let val_rest = &current[start + scheme.len()..];
            let trimmed = val_rest.trim_start_matches(' ');
            let skip_ws = val_rest.len() - trimmed.len();
            let val_len = trimmed.find(is_token_terminator).unwrap_or(trimmed.len());
            if val_len > 0 && !trimmed[..val_len].starts_with("[redacted") {
                next.push_str(REDACTED_VALUE);
                cursor = start + scheme.len() + skip_ws + val_len;
            } else {
                cursor = start + scheme.len();
            }
        }
        next.push_str(&current[cursor..]);
        current = next;
    }
    current
}

fn is_key_boundary(ch: char) -> bool {
    !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-'
}

fn redact_kv_secrets(input: &str) -> String {
    let step1 = redact_colon_secrets(input);
    redact_eq_secrets(&step1)
}

fn consume_secret_value(after: &str, out: &mut String) -> usize {
    if after.starts_with("[redacted")
        && let Some(close) = after.find(']')
    {
        out.push_str(&after[..=close]);
        return close + 1;
    }
    let bytes = after.as_bytes();
    if let Some(&quote @ (b'"' | b'\'')) = bytes.first() {
        let q = char::from(quote);
        let mut idx = 1usize;
        let mut escaped = false;
        while idx < after.len() {
            let ch = after[idx..].chars().next().unwrap();
            idx += ch.len_utf8();
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                let inner = &after[1..idx - 1];
                out.push(q);
                if inner.starts_with("[redacted") {
                    out.push_str(inner);
                } else {
                    out.push_str(REDACTED_VALUE);
                }
                out.push(q);
                return idx;
            }
        }
        out.push(q);
        out.push_str(REDACTED_VALUE);
        return after.len();
    }
    let val_len = after
        .find(|c: char| is_token_terminator(c) || c == '&')
        .unwrap_or(after.len());
    if !after[..val_len].starts_with("[redacted") {
        out.push_str(REDACTED_VALUE);
    } else {
        out.push_str(&after[..val_len]);
    }
    val_len
}

fn consume_header_value(after: &str, out: &mut String) -> usize {
    let end = after.find(['\n', '\r']).unwrap_or(after.len());
    let segment = after[..end].trim_end();
    if segment.is_empty() {
        return 0;
    }
    out.push_str(REDACTED_VALUE);
    segment.len()
}

fn redact_eq_secrets(input: &str) -> String {
    if !input.contains('=') {
        return input.to_owned();
    }
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while let Some(rel) = input[cursor..].find('=') {
        let eq = cursor + rel;
        let before = &input[cursor..eq];
        let key_start = before
            .rfind(is_key_boundary)
            .map_or(0, |i| i + before[i..].chars().next().unwrap().len_utf8());
        let key_name = &before[key_start..];
        let sensitive = EQ_SENSITIVE_KEYS
            .iter()
            .any(|k| key_name.eq_ignore_ascii_case(k));
        out.push_str(&input[cursor..=eq]);
        if !sensitive {
            cursor = eq + 1;
            continue;
        }
        let after = &input[eq + 1..];
        let consumed = consume_secret_value(after, &mut out);
        cursor = eq + 1 + consumed;
    }
    out.push_str(&input[cursor..]);
    out
}

fn redact_colon_secrets(input: &str) -> String {
    if !input.contains(':') {
        return input.to_owned();
    }
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while let Some(rel) = input[cursor..].find(':') {
        let colon = cursor + rel;
        // Skip `::` Rust module separators.
        if input.as_bytes().get(colon + 1) == Some(&b':') {
            out.push_str(&input[cursor..colon + 2]);
            cursor = colon + 2;
            continue;
        }
        let before_trimmed = input[cursor..colon].trim_end_matches(' ');
        let (matched_key, quoted) = if let Some(unquoted_end) = before_trimmed.strip_suffix('"')
            && let Some((_, k)) = unquoted_end.rsplit_once('"')
        {
            (k, true)
        } else {
            let start = before_trimmed.rfind(is_key_boundary).map_or(0, |i| {
                i + before_trimmed[i..].chars().next().unwrap().len_utf8()
            });
            // Do not treat `lane.write:` or `foo.pin:` dotted identifiers as headers.
            let prev = before_trimmed[..start].chars().next_back();
            if matches!(prev, Some('.' | ':')) {
                ("", false)
            } else {
                (&before_trimmed[start..], false)
            }
        };
        let sensitive = if quoted {
            EQ_SENSITIVE_KEYS
                .iter()
                .any(|cand| matched_key.eq_ignore_ascii_case(cand))
        } else {
            COLON_UNQUOTED_KEYS
                .iter()
                .any(|cand| matched_key.eq_ignore_ascii_case(cand))
        };
        out.push_str(&input[cursor..=colon]);
        if !sensitive {
            cursor = colon + 1;
            continue;
        }
        let after = &input[colon + 1..];
        let trimmed = after.trim_start_matches(' ');
        let ws_len = after.len() - trimmed.len();
        out.push_str(&after[..ws_len]);
        let is_header = !quoted
            && HEADER_REST_KEYS
                .iter()
                .any(|cand| matched_key.eq_ignore_ascii_case(cand));
        let consumed = if is_header && !trimmed.starts_with(['"', '\'']) {
            consume_header_value(trimmed, &mut out)
        } else {
            consume_secret_value(trimmed, &mut out)
        };
        cursor = colon + 1 + ws_len + consumed;
    }
    out.push_str(&input[cursor..]);
    out
}

/// Converts a compiler or backtrace file path into a crate-relative location,
/// stripping build-checkout, Cargo-home and user-directory prefixes.
#[cfg(any(not(target_family = "wasm"), test))]
pub(crate) fn crate_relative_location(file: &str) -> String {
    let normalized = file.replace('\\', "/");
    for marker in ["/crates/", "/vendor/"] {
        if let Some(pos) = normalized.rfind(marker) {
            return normalized[pos + 1..].to_owned();
        }
    }
    if normalized.starts_with("crates/") || normalized.starts_with("vendor/") {
        return normalized;
    }
    if let Some(src_pos) = normalized.rfind("/src/") {
        let prefix = &normalized[..src_pos];
        if let Some(crate_dir) = prefix.rsplit('/').next().filter(|s| !s.is_empty()) {
            return format!("{crate_dir}{}", &normalized[src_pos..]);
        }
        return normalized[src_pos + 1..].to_owned();
    }
    normalized
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

/// Strips build and user paths from `std::backtrace::Backtrace` output.
#[cfg(any(not(target_family = "wasm"), test))]
pub(crate) fn strip_backtrace_paths(backtrace: &str, roots: &[String]) -> String {
    let mut out = String::with_capacity(backtrace.len());
    for line in backtrace.lines() {
        let trimmed = line.trim_start();
        if let Some(at_rest) = trimmed.strip_prefix("at ") {
            let indent = &line[..line.len() - trimmed.len()];
            let (path_part, suffix) = split_location_suffix(at_rest.trim());
            let rel = crate_relative_location(path_part);
            out.push_str(indent);
            out.push_str("at ");
            out.push_str(&redact_text(&rel, roots));
            out.push_str(suffix);
            out.push('\n');
        } else {
            out.push_str(&redact_text(line, roots));
            out.push('\n');
        }
    }
    out
}

#[cfg(any(not(target_family = "wasm"), test))]
fn split_location_suffix(location: &str) -> (&str, &str) {
    // Peel `:col` and `:line` numeric suffixes while preserving Windows `C:\...`.
    let mut base = location;
    for _ in 0..2 {
        if let Some((head, tail)) = base.rsplit_once(':')
            && !tail.is_empty()
            && tail.bytes().all(|b| b.is_ascii_digit())
            && head.len() > 1
        {
            base = head;
        } else {
            break;
        }
    }
    (&location[..base.len()], &location[base.len()..])
}
