//! Channel-name normalization for EPG matching.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Trailing tokens that describe the stream, not the channel.
const QUALITY: &[&str] = &[
    "hd", "fhd", "uhd", "sd", "hq", "lq", "4k", "8k", "hevc", "h264", "h265", "1080p", "1080i",
    "720p", "2160p", "50fps", "60fps", "hdr",
];

/// Folds a channel name to a matching key: compatibility decomposition,
/// diacritics removed, lowercase, `&` → `and`, `+` → `plus`, punctuation
/// and spaces dropped, and trailing quality tags (`HD`, `FHD`, `4K`, ...)
/// removed. Returns an empty string for names with no letters or digits.
///
/// ```
/// use iptv_epg::normalize_name;
/// assert_eq!(normalize_name("Ch\u{e2}ine Écho HD"), "chaineecho");
/// assert_eq!(normalize_name("Alpha-One (FHD) 4K"), "alphaone");
/// assert_eq!(normalize_name("Alpha +1"), "alphaplus1");
/// assert_ne!(normalize_name("Alpha +1"), normalize_name("Alpha 1"));
/// ```
pub fn normalize_name(name: &str) -> String {
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, tokens: &mut Vec<String>| {
        if !cur.is_empty() {
            tokens.push(std::mem::take(cur));
        }
    };
    for c in name.nfkd() {
        if is_combining_mark(c) {
            continue;
        }
        match c {
            '&' => {
                flush(&mut cur, &mut tokens);
                tokens.push("and".into());
            }
            '+' => {
                flush(&mut cur, &mut tokens);
                tokens.push("plus".into());
            }
            c if c.is_alphanumeric() => {
                for l in c.to_lowercase() {
                    match l {
                        'ß' => cur.push_str("ss"),
                        'æ' => cur.push_str("ae"),
                        'œ' => cur.push_str("oe"),
                        'þ' => cur.push_str("th"),
                        'ø' => cur.push('o'),
                        'ł' => cur.push('l'),
                        'đ' | 'ð' => cur.push('d'),
                        'ı' => cur.push('i'),
                        l => cur.push(l),
                    }
                }
            }
            _ => flush(&mut cur, &mut tokens),
        }
    }
    flush(&mut cur, &mut tokens);
    while tokens.len() > 1 && tokens.last().is_some_and(|t| QUALITY.contains(&t.as_str())) {
        tokens.pop();
    }
    tokens.concat()
}

/// Removes a leading country/group tag such as `UK: `, `US | `, `[DE] ` or
/// `FR - ` (two to four upper-case ASCII letters). `None` when there is no
/// such tag.
pub fn strip_tag_prefix(name: &str) -> Option<&str> {
    let s = name.trim_start();
    let (tag, rest) = if let Some(inner) = s.strip_prefix(['[', '(', '|']) {
        let close = inner.find([']', ')', '|'])?;
        (&inner[..close], &inner[close + 1..])
    } else if let Some(sep) = s.find([':', '|']) {
        (&s[..sep], &s[sep + 1..])
    } else {
        let sep = s.find(" - ")?;
        (&s[..sep], &s[sep + 3..])
    };
    let tag = tag.trim();
    let rest = rest.trim_start_matches([' ', ':', '|', '-']).trim();
    let tag_ok = (2..=4).contains(&tag.len()) && tag.bytes().all(|b| b.is_ascii_uppercase());
    (tag_ok && !rest.is_empty()).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding() {
        assert_eq!(normalize_name("  Ålpha   ÖNE  "), "alphaone");
        assert_eq!(normalize_name("Straße Zwei"), "strassezwei");
        assert_eq!(normalize_name("Łódź Info"), "lodzinfo");
        assert_eq!(normalize_name("Ｆｕｌｌｗｉｄｔｈ"), "fullwidth");
        assert_eq!(
            normalize_name("Alpha & Beta"),
            normalize_name("alpha and beta")
        );
        assert_eq!(normalize_name("Alpha.One HD"), "alphaone");
        assert_eq!(normalize_name("Alpha One UHD HEVC"), "alphaone");
        assert_eq!(normalize_name("Альфа ТВ"), "альфатв");
        // A name that is only a quality tag stays matchable.
        assert_eq!(normalize_name("HD"), "hd");
        assert_eq!(normalize_name("---"), "");
    }

    #[test]
    fn tag_prefixes() {
        assert_eq!(strip_tag_prefix("UK: Alpha One"), Some("Alpha One"));
        assert_eq!(strip_tag_prefix("US | Alpha"), Some("Alpha"));
        assert_eq!(strip_tag_prefix("[DE] Alpha"), Some("Alpha"));
        assert_eq!(strip_tag_prefix("|FR| Alpha"), Some("Alpha"));
        assert_eq!(strip_tag_prefix("FR - Alpha"), Some("Alpha"));
        assert_eq!(strip_tag_prefix("Alpha One"), None);
        assert_eq!(strip_tag_prefix("Alpha-One"), None);
        assert_eq!(strip_tag_prefix("Beta-Two"), None);
        assert_eq!(strip_tag_prefix("Beta: The Show"), None);
        assert_eq!(strip_tag_prefix("UK:"), None);
    }
}
