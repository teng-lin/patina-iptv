//! Rewrite every address an HLS playlist names so that the player fetches
//! it through the relay.
//!
//! A playlist is read by players that disagree about its syntax, and the
//! relay only protects what every one of them fetches. So a tag line that
//! names a URI is not searched for a pattern: its attribute list is parsed
//! and written again in one plain form that all of them read alike, and a
//! line that cannot be parsed refuses the playlist. The readers kept in
//! mind (the fuzz target `manifests` models each):
//!
//! - **the specification** (RFC 8216, 4.2): `NAME=value` separated by
//!   commas, a value quoted or not, a quoted one ending at the next `"`;
//! - **FFmpeg** (mpv; `ff_parse_key_value`): takes an unquoted value up to
//!   a comma or white space, reads `\"` inside quotes as a quote that does
//!   not end the value, and ends a line at a NUL byte as at a line feed;
//! - **Media3** (Android): finds `URI="…"` anywhere in the line with a
//!   regular expression, so only quoted values, and also inside what the
//!   others read as another attribute's value.

use reqwest::Url;

use super::{MAX_MANIFEST, invalid, resolve};
use crate::FetchError;

pub(super) fn rewrite_hls(body: &[u8], base: &Url, prefix: &str) -> Result<Vec<u8>, FetchError> {
    let text = std::str::from_utf8(body).map_err(|_| invalid("Invalid manifest text"))?;
    // FFmpeg ends a line at a NUL byte: what follows one in a tag line
    // would be a URI line to it, and part of the tag here.
    if text.contains('\0') {
        return Err(invalid("Invalid manifest text"));
    }
    let mut out = String::new();
    // A lone carriage return ends a line too. Some players read lines that
    // way (Java's `readLine`, behind the Android player; FFmpeg): a URI
    // hidden after a CR inside a tag line would pass here as part of the
    // tag and be fetched by the player directly, outside this relay and
    // its checks.
    for line in text.lines().flat_map(|line| line.split('\r')) {
        let line = line.trim();
        if line.is_empty() {
        } else if !line.starts_with('#') {
            out.push_str(&resolve(prefix, base, line)?);
        } else if names_a_uri(line) {
            rewrite_tag(line, base, prefix, &mut out)?;
        } else {
            out.push_str(line);
        }
        out.push('\n');
        if out.len() > MAX_MANIFEST * 8 {
            return Err(invalid("Rewritten manifest exceeds limit"));
        }
    }
    Ok(out.into_bytes())
}

/// Whether some reader could find a URI attribute in this tag line: all of
/// them need the text `URI=` for it. Any letter case, which none of them
/// accepts: the cost of a line too many is a stricter parse.
fn names_a_uri(line: &str) -> bool {
    line.as_bytes()
        .windows(4)
        .any(|window| window.eq_ignore_ascii_case(b"uri="))
}

fn malformed() -> FetchError {
    invalid("Invalid manifest attribute")
}

/// One `NAME=value` of a tag's attribute list.
struct Attribute<'a> {
    name: &'a str,
    /// Without its quotes.
    value: &'a str,
    quoted: bool,
}

impl Attribute<'_> {
    /// `URI`, and every name that ends like it (`SERVER-URI`,
    /// `X-ASSET-URI`): Media3's search finds those as well.
    fn is_uri(&self) -> bool {
        let name = self.name.as_bytes();
        name.len() >= 3 && name[name.len() - 3..].eq_ignore_ascii_case(b"uri")
    }
}

/// The attributes of `list`, in order. Spaces and tabs around a comma and
/// empty items (a trailing comma) are accepted, as players accept them.
/// Everything else the readers could take two ways is an error: an item
/// without `=`, a name with other characters than the specification's, an
/// unquoted value with a quote or white space in it, a quote left open,
/// text after a closing quote.
fn attributes(list: &str) -> Result<Vec<Attribute<'_>>, FetchError> {
    const BLANK: [char; 2] = [' ', '\t'];
    let mut attributes = Vec::new();
    let mut rest = list;
    loop {
        rest = rest.trim_start_matches([' ', '\t', ',']);
        if rest.is_empty() {
            return Ok(attributes);
        }
        let (name, tail) = rest.split_once('=').ok_or_else(malformed)?;
        let named = !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
        if !named {
            return Err(malformed());
        }
        let (value, quoted, tail) = if let Some(tail) = tail.strip_prefix('"') {
            let (value, tail) = tail.split_once('"').ok_or_else(malformed)?;
            (value, true, tail)
        } else {
            let end = tail.find(',').unwrap_or(tail.len());
            let value = tail[..end].trim_end_matches(BLANK);
            if value.chars().any(|c| c == '"' || c.is_whitespace()) {
                return Err(malformed());
            }
            (value, false, &tail[end..])
        };
        let tail = tail.trim_start_matches(BLANK);
        if !tail.is_empty() && !tail.starts_with(',') {
            return Err(malformed());
        }
        attributes.push(Attribute {
            name,
            value,
            quoted,
        });
        rest = tail;
    }
}

/// Whether FFmpeg would read past the quote that closes `value`: it takes a
/// backslash and the character after it as one, so a value ending in an odd
/// number of backslashes takes its closing quote along.
fn swallows_its_quote(value: &str) -> bool {
    let backslashes = value
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'\\')
        .count();
    backslashes % 2 == 1
}

/// Writes the tag `line` with every URI attribute on the relay: quoted,
/// whether it came quoted or not, and all of them when one is repeated
/// (players differ in which they take).
fn rewrite_tag(line: &str, base: &Url, prefix: &str, out: &mut String) -> Result<(), FetchError> {
    let (tag, list) = line.split_once(':').ok_or_else(malformed)?;
    let name = &tag[1..];
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(malformed());
    }
    // HLS key URIs are opaque bytes: the relay serves them as they come.
    let key_prefix;
    let resource_prefix = if matches!(name, "EXT-X-KEY" | "EXT-X-SESSION-KEY") {
        key_prefix = format!("{prefix}key/");
        key_prefix.as_str()
    } else {
        prefix
    };
    out.push_str(tag);
    out.push(':');
    for (index, attribute) in attributes(list)?.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(attribute.name);
        out.push('=');
        if attribute.is_uri() {
            // A URL holds a backslash only in its query, where `%5C` means
            // the same; in the quotes it would be FFmpeg's escape.
            let target = resolve(resource_prefix, base, attribute.value)?.replace('\\', "%5C");
            // Media3 searches for uppercase `URI="` even inside values.
            // If it ignores this mixed-case name, a trailing `URI=` plus
            // our closing quote must not become its first URI match.
            // Standard uppercase names consume their whole value normally.
            if target.contains('"')
                || (!attribute.name.ends_with("URI") && target.ends_with("URI="))
            {
                return Err(malformed());
            }
            out.push('"');
            out.push_str(&target);
            out.push('"');
        } else if attribute.quoted {
            // `…URI="` at the end of another attribute's value is where
            // Media3's search would start reading a URI.
            let value = attribute.value.as_bytes();
            let ends_like_a_uri =
                value.len() >= 4 && value[value.len() - 4..].eq_ignore_ascii_case(b"uri=");
            if swallows_its_quote(attribute.value) || ends_like_a_uri {
                return Err(malformed());
            }
            out.push('"');
            out.push_str(attribute.value);
            out.push('"');
        } else {
            out.push_str(attribute.value);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: &str = "http://127.0.0.1:1/cap/";

    fn base() -> Url {
        Url::parse("http://synthetic.example.test/live/main.m3u8").unwrap()
    }

    fn rewritten(body: &str) -> Result<Vec<String>, FetchError> {
        let out = String::from_utf8(rewrite_hls(body.as_bytes(), &base(), PREFIX)?).unwrap();
        Ok(out.lines().map(str::to_owned).collect())
    }

    fn line(tag: &str) -> String {
        let lines = rewritten(&format!("#EXTM3U\n{tag}\n")).unwrap();
        assert_eq!(lines.len(), 2, "{lines:?}");
        lines[1].clone()
    }

    fn refused(tag: &str) {
        assert!(
            rewritten(&format!("#EXTM3U\n{tag}\n")).is_err(),
            "accepted: {tag:?}"
        );
    }

    const ON_RELAY: &str = "http://127.0.0.1:1/cap/http://synthetic.example.test/live/";

    #[test]
    fn a_quoted_uri_is_rewritten_and_the_other_attributes_kept() {
        assert_eq!(
            line(
                "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"English, stereo\",\
                 DEFAULT=YES,URI=\"audio/en.m3u8\",CHANNELS=\"2\""
            ),
            format!(
                "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"English, stereo\",\
                 DEFAULT=YES,URI=\"{ON_RELAY}audio/en.m3u8\",CHANNELS=\"2\""
            )
        );
        assert_eq!(
            line("#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"720@0\""),
            format!("#EXT-X-MAP:URI=\"{ON_RELAY}init.mp4\",BYTERANGE=\"720@0\"")
        );
        // A key goes through the relay's opaque route, a session key too.
        for tag in ["EXT-X-KEY", "EXT-X-SESSION-KEY"] {
            assert_eq!(
                line(&format!("#{tag}:METHOD=AES-128,URI=\"key.bin\",IV=0x01")),
                format!(
                    "#{tag}:METHOD=AES-128,\
                     URI=\"{PREFIX}key/http://synthetic.example.test/live/key.bin\",IV=0x01"
                )
            );
        }
    }

    /// The reproduced bypass: FFmpeg reads a URI without quotes, so the
    /// rewriter must, and writes it in the one form every player reads.
    #[test]
    fn an_unquoted_uri_is_rewritten_and_comes_out_quoted() {
        assert_eq!(
            line("#EXT-X-KEY:METHOD=AES-128,URI=http://192.0.2.7/key.bin"),
            format!("#EXT-X-KEY:METHOD=AES-128,URI=\"{PREFIX}key/http://192.0.2.7/key.bin\"")
        );
        assert_eq!(
            line("#EXT-X-MAP:URI=init.mp4?a=1&b=2"),
            format!("#EXT-X-MAP:URI=\"{ON_RELAY}init.mp4?a=1&b=2\"")
        );
        assert_eq!(
            line("#EXT-X-MEDIA:URI=http://192.0.2.7/audio.m3u8,TYPE=AUDIO"),
            format!("#EXT-X-MEDIA:URI=\"{PREFIX}http://192.0.2.7/audio.m3u8\",TYPE=AUDIO")
        );
        // What FFmpeg skips between attributes is not kept.
        assert_eq!(
            line("#EXT-X-MAP: URI=init.mp4 ,\tBYTERANGE=\"720@0\" ,,"),
            format!("#EXT-X-MAP:URI=\"{ON_RELAY}init.mp4\",BYTERANGE=\"720@0\"")
        );
    }

    /// Players differ in which of two they take: FFmpeg the last, a search
    /// the first.
    #[test]
    fn every_repeated_uri_is_rewritten() {
        assert_eq!(
            line(
                "#EXT-X-KEY:URI=\"one.bin\",METHOD=AES-128,URI=http://192.0.2.7/two.bin,\
                 URI=\"three.bin\""
            ),
            format!(
                "#EXT-X-KEY:URI=\"{PREFIX}key/http://synthetic.example.test/live/one.bin\",\
                 METHOD=AES-128,URI=\"{PREFIX}key/http://192.0.2.7/two.bin\",\
                 URI=\"{PREFIX}key/http://synthetic.example.test/live/three.bin\""
            )
        );
    }

    #[test]
    fn names_that_end_in_uri_and_any_letter_case_count() {
        assert_eq!(
            line("#EXT-X-CONTENT-STEERING:SERVER-URI=\"steer.json\",PATHWAY-ID=\"a\""),
            format!("#EXT-X-CONTENT-STEERING:SERVER-URI=\"{ON_RELAY}steer.json\",PATHWAY-ID=\"a\"")
        );
        assert_eq!(
            line("#EXT-X-DATERANGE:ID=\"ad\",X-ASSET-URI=http://192.0.2.7/ad.m3u8"),
            format!("#EXT-X-DATERANGE:ID=\"ad\",X-ASSET-URI=\"{PREFIX}http://192.0.2.7/ad.m3u8\"")
        );
        assert_eq!(
            line("#EXT-X-MAP:uri=init.mp4"),
            format!("#EXT-X-MAP:uri=\"{ON_RELAY}init.mp4\"")
        );
    }

    #[test]
    fn a_line_whose_uri_cannot_be_parsed_refuses_the_playlist() {
        for tag in [
            // A quote left open, and text after a closing one.
            "#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin",
            "#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"x",
            "#EXT-X-KEY:METHOD=AES-128,URI=\"\"http://192.0.2.7/key.bin\"",
            // An unquoted value with a quote or white space in it.
            "#EXT-X-KEY:METHOD=AES-128,URI= \"http://192.0.2.7/key.bin\"",
            "#EXT-X-KEY:METHOD=AES-128,URI=key\".bin",
            "#EXT-X-KEY:METHOD=AES-128,URI=key .bin",
            // An item without `=`: FFmpeg reads `METHOD,URI=` as one name.
            "#EXT-X-KEY:METHOD,URI=http://192.0.2.7/key.bin",
            "#EXT-X-MAP:URI=http://192.0.2.7/init.mp4?a=1,2",
            // Names no specification attribute has.
            "#EXT-X-KEY:METHOD=AES-128,X URI=\"key.bin\"",
            "#EXT-X-KEY:=x,URI=\"key.bin\"",
            // Not an attribute list at all.
            "#EXT-X-KEY URI=\"key.bin\"",
            "#URI=\"http://192.0.2.7/key.bin\":A=1",
            "#EXTINF:2,URI=\"http://192.0.2.7/key.bin\"",
            "# a comment with URI=http://192.0.2.7/key.bin",
        ] {
            refused(tag);
        }
    }

    /// FFmpeg reads `\"` inside quotes as part of the value. Here the value
    /// ends at that quote, so the two would disagree about everything after.
    #[test]
    fn a_backslash_cannot_hide_a_uri_from_one_reader() {
        // The specification's reader: FOO is `a\`, BAR a quote left open.
        // FFmpeg's: FOO is `a",BAR=`, then a URI.
        refused("#EXT-X-KEY:METHOD=AES-128,FOO=\"a\\\",BAR=\",URI=http://192.0.2.7/key.bin");
        // Well-formed for the specification, and still two readings.
        refused("#EXT-X-KEY:METHOD=AES-128,FOO=\"a\\\",BAR=\"b\",URI=\"key.bin\"");
        refused("#EXT-X-MEDIA:NAME=\"a\\\\\\\",URI=\"audio.m3u8\"");
        // An even number is no escape of the quote.
        assert_eq!(
            line("#EXT-X-MEDIA:NAME=\"a\\\\\",URI=\"audio.m3u8\""),
            format!("#EXT-X-MEDIA:NAME=\"a\\\\\",URI=\"{ON_RELAY}audio.m3u8\"")
        );
        // A URI that ends in one is written without it.
        assert_eq!(
            line("#EXT-X-MAP:URI=\"init.mp4?a=\\\",BYTERANGE=\"1@0\""),
            format!("#EXT-X-MAP:URI=\"{ON_RELAY}init.mp4?a=%5C\",BYTERANGE=\"1@0\"")
        );
    }

    /// Media3 searches the line for `URI="`; the end of another value must
    /// not give it one.
    #[test]
    fn another_attributes_value_cannot_end_as_a_uri_would_start() {
        refused("#EXT-X-MEDIA:NAME=\"xURI=\",GROUP-ID=\"http://192.0.2.7/a\",URI=\"a.m3u8\"");
        refused("#EXT-X-MEDIA:NAME=\"xuri=\",URI=\"a.m3u8\"");
        // Inside a value it starts nothing.
        assert_eq!(
            line("#EXT-X-SESSION-DATA:DATA-ID=\"a\",VALUE=\"see URI=b here\""),
            "#EXT-X-SESSION-DATA:DATA-ID=\"a\",VALUE=\"see URI=b here\""
        );
        assert_eq!(
            line("#EXT-X-MAP:URI=\"init.mp4?nextURI=b\""),
            format!("#EXT-X-MAP:URI=\"{ON_RELAY}init.mp4?nextURI=b\"")
        );
    }

    #[test]
    fn uri_tail_cannot_create_a_nested_attribute_when_quoted() {
        for name in ["uri", "uRI", "X-ASSET-uRI"] {
            for value in ["part?nextURI=", "part?nextURI=#ignored"] {
                for value in [value.to_owned(), format!("\"{value}\"")] {
                    for tail in ["", ",BYTERANGE=\"16@0\"", ",URI=\"next.m3u8\""] {
                        refused(&format!("#EXT-X-MAP:{name}={value}{tail}"));
                    }
                }
            }
        }
        // The case-sensitive reader consumes a standard URI attribute from
        // its own name, so a marker inside that value starts no new match.
        for name in ["URI", "SERVER-URI", "X-ASSET-URI"] {
            assert_eq!(
                line(&format!("#EXT-X-MAP:{name}=\"part?nextURI=\"")),
                format!("#EXT-X-MAP:{name}=\"{ON_RELAY}part?nextURI=\"")
            );
        }
        assert_eq!(
            line("#EXT-X-MAP:uri=\"part?nextURI=value\""),
            format!("#EXT-X-MAP:uri=\"{ON_RELAY}part?nextURI=value\"")
        );
        assert_eq!(
            rewritten("#EXTM3U\npart?nextURI=\n").unwrap()[1],
            format!("{ON_RELAY}part?nextURI=")
        );
    }

    #[test]
    fn uri_tail_fuzz_regression_is_refused() {
        let body = include_bytes!("../../../../fuzz/seeds/hls/uri-tail-marker");
        for base in [
            "https://media.example.test/live/main.m3u8",
            "http://media.example.test:8080/a/b/main.mpd?token=synthetic",
            "http://media.example.test",
            "https://[2001:db8::1]:8443/v/",
        ] {
            assert!(rewrite_hls(body, &Url::parse(base).unwrap(), PREFIX).is_err());
        }
    }

    /// FFmpeg ends a line at a NUL byte: the rest of this tag line would be
    /// a segment address to it.
    #[test]
    fn a_nul_byte_cannot_end_a_line_for_one_reader() {
        refused("#EXT-X-DISCONTINUITY\0http://192.0.2.7/direct.ts");
        refused("#EXTINF:2,\nsegment.ts\0");
    }

    #[test]
    fn lines_that_name_no_uri_pass_as_they_are() {
        let body = "#EXTM3U\n#EXT-X-VERSION:7\n\
                    #EXT-X-STREAM-INF:BANDWIDTH=1280000,CODECS=\"avc1.4d401f,mp4a.40.2\"\n\
                    low/index.m3u8\n# a comment, not a tag\n#EXTINF:2.002,A title, with a comma\n\
                    http://192.0.2.7/segment.ts\n#EXT-X-ENDLIST\n";
        assert_eq!(
            rewritten(body).unwrap(),
            [
                "#EXTM3U",
                "#EXT-X-VERSION:7",
                "#EXT-X-STREAM-INF:BANDWIDTH=1280000,CODECS=\"avc1.4d401f,mp4a.40.2\"",
                &format!("{ON_RELAY}low/index.m3u8"),
                "# a comment, not a tag",
                "#EXTINF:2.002,A title, with a comma",
                "http://127.0.0.1:1/cap/http://192.0.2.7/segment.ts",
                "#EXT-X-ENDLIST",
            ]
        );
    }

    #[test]
    fn a_lone_carriage_return_cannot_hide_a_uri_inside_a_tag_line() {
        let body = "#EXTM3U\r\n#EXTINF:2,\rhttp://192.0.2.7/direct.ts\n\
                    #EXT-X-DISCONTINUITY\r#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\rnext.ts\r\n";
        let out =
            String::from_utf8(rewrite_hls(body.as_bytes(), &base(), PREFIX).unwrap()).unwrap();
        assert!(!out.contains('\r'), "{out:?}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines,
            [
                "#EXTM3U",
                "#EXTINF:2,",
                "http://127.0.0.1:1/cap/http://192.0.2.7/direct.ts",
                "#EXT-X-DISCONTINUITY",
                "#EXT-X-KEY:METHOD=AES-128,URI=\"http://127.0.0.1:1/cap/key/http://synthetic.example.test/live/key.bin\"",
                "http://127.0.0.1:1/cap/http://synthetic.example.test/live/next.ts",
            ]
        );
    }

    #[test]
    fn a_secure_playlist_cannot_name_a_cleartext_uri_in_either_form() {
        let base = Url::parse("https://synthetic.example.test/main.m3u8").unwrap();
        for body in [
            "#EXTM3U\nhttp://other.example.test/media.ts\n",
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"http://other.example.test/key\"\n",
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=http://other.example.test/key\n",
        ] {
            assert!(
                rewrite_hls(body.as_bytes(), &base, PREFIX).is_err(),
                "{body}"
            );
        }
    }
}
