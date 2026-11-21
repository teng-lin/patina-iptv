//! Tokenizer for `#EXTINF:` and `#EXTM3U` attribute lines.
//!
//! `#EXTINF:<duration> key="value" key=value key='value',<title>`
//!
//! - Keys are lowercased; values keep their case. Quoted values may contain
//!   spaces and commas.
//! - The title is everything after the first comma outside quotes, so titles
//!   that themselves contain commas survive ("News, Weather").
//! - An unterminated quote runs to the next comma.
//! - A missing duration (`#EXTINF:tvg-id="x",T`) is tolerated.
//!
//! One pass over the line; no backtracking.

pub(crate) struct ExtInf<'a> {
    pub attrs: Vec<(String, String)>,
    /// `None` when the line has no comma at all.
    pub title: Option<&'a str>,
}

/// Parses the part after `#EXTINF:`.
pub(crate) fn parse_extinf(rest: &str) -> ExtInf<'_> {
    let rest = rest.trim_start();
    let token_end = rest
        .find(|c: char| c.is_whitespace() || c == ',')
        .unwrap_or(rest.len());
    let token = &rest[..token_end];
    // The duration (`-1`, `0`, `3.5`) is not used; a first token with '='
    // means the duration is missing.
    let start = if token.contains('=') { 0 } else { token_end };
    let (attrs, title) = parse_attrs(&rest[start..], true);
    ExtInf { attrs, title }
}

/// Parses `key=value` pairs. With `stop_at_comma`, a comma outside quotes
/// ends the attributes and the remainder is returned as the title.
pub(crate) fn parse_attrs(s: &str, stop_at_comma: bool) -> (Vec<(String, String)>, Option<&str>) {
    let bytes = s.as_bytes();
    let mut attrs = Vec::new();
    let mut i = 0;
    let n = bytes.len();
    loop {
        while i < n && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= n {
            return (attrs, None);
        }
        if bytes[i] == b',' {
            if stop_at_comma {
                return (attrs, Some(s[i + 1..].trim()));
            }
            i += 1;
            continue;
        }
        let key_start = i;
        while i < n && !matches!(bytes[i], b'=' | b',') && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = &s[key_start..i];
        if i >= n || bytes[i] != b'=' {
            // A bare word: not an attribute; ignore it.
            continue;
        }
        i += 1; // '='
        let value = if i < n && (bytes[i] == b'"' || bytes[i] == b'\'') {
            let quote = bytes[i];
            let v_start = i + 1;
            match bytes[v_start..].iter().position(|&b| b == quote) {
                Some(len) => {
                    i = v_start + len + 1;
                    &s[v_start..v_start + len]
                }
                None => {
                    let len = bytes[v_start..]
                        .iter()
                        .position(|&b| b == b',')
                        .unwrap_or(n - v_start);
                    i = v_start + len;
                    &s[v_start..v_start + len]
                }
            }
        } else {
            let v_start = i;
            while i < n && bytes[i] != b',' && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            &s[v_start..i]
        };
        if !key.is_empty() {
            attrs.push((key.to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
        attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn quoted_unquoted_and_title() {
        let e = parse_extinf(
            r#"-1 tvg-id="news.example" tvg-name='News One' tvg-chno=12 group-title="A, B",News, Weather"#,
        );
        assert_eq!(get(&e.attrs, "tvg-id"), Some("news.example"));
        assert_eq!(get(&e.attrs, "tvg-name"), Some("News One"));
        assert_eq!(get(&e.attrs, "tvg-chno"), Some("12"));
        assert_eq!(get(&e.attrs, "group-title"), Some("A, B"));
        assert_eq!(e.title, Some("News, Weather"));
    }

    #[test]
    fn tolerates_odd_shapes() {
        let e = parse_extinf("-1,Plain");
        assert_eq!(e.title, Some("Plain"));
        let e = parse_extinf(r#"tvg-id="x",No duration"#);
        assert_eq!(
            (get(&e.attrs, "tvg-id"), e.title),
            (Some("x"), Some("No duration"))
        );
        let e = parse_extinf("0.000 TVG-ID=abc ,  Spaced  ");
        assert_eq!(
            (get(&e.attrs, "tvg-id"), e.title),
            (Some("abc"), Some("Spaced"))
        );
        let e = parse_extinf(r#"-1 tvg-name="Unterminated,Title"#);
        assert_eq!(
            (get(&e.attrs, "tvg-name"), e.title),
            (Some("Unterminated"), Some("Title"))
        );
        let e = parse_extinf("-1 tvg-id=");
        assert_eq!((get(&e.attrs, "tvg-id"), e.title), (Some(""), None));
        let e = parse_extinf("");
        assert_eq!((e.attrs.len(), e.title), (0, None));
        let e = parse_extinf(r#"-1 bare tvg-logo="http://img.example.com/a,b.png",T"#);
        assert_eq!(
            (get(&e.attrs, "tvg-logo"), e.title),
            (Some("http://img.example.com/a,b.png"), Some("T"))
        );
    }

    #[test]
    fn header_attrs_without_title() {
        let (attrs, title) = parse_attrs(
            r#" url-tvg="http://epg.example.com/a.xml,http://epg.example.com/b.xml" catchup=shift"#,
            false,
        );
        assert_eq!(title, None);
        assert_eq!(
            get(&attrs, "url-tvg"),
            Some("http://epg.example.com/a.xml,http://epg.example.com/b.xml")
        );
        assert_eq!(get(&attrs, "catchup"), Some("shift"));
    }
}
