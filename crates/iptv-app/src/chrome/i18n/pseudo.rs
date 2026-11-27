//! Test-support-only fixed-message expansion. No pseudo language is exposed in
//! the production picker or saved settings. Provider content never enters this map.
use std::collections::HashMap;
use std::sync::OnceLock;

pub(super) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("IPTV_RS_TEST_PSEUDOLOCALE").as_deref() == Ok("1"))
}

pub(super) fn fixed(source: &str) -> Option<&'static str> {
    static FIXED: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    FIXED
        .get_or_init(|| {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../iptv-presentation/src/i18n/messages.tsv"
            ))
            .lines()
            .filter_map(|line| {
                let source = line.split('\t').next()?;
                // Formatted/plural messages retain their real locale rules.
                // Separate Russian/CJK tests exercise those without altering
                // placeholders or expanding provider values.
                if source.contains(['{', '}']) {
                    return None;
                }
                let accented: String = source.chars().map(accent).collect();
                Some((source, format!("⟦{accented} {accented}⟧")))
            })
            .collect()
        })
        .get(source)
        .map(String::as_str)
}

fn accent(c: char) -> char {
    match c {
        'a' => 'à',
        'e' => 'ë',
        'i' => 'ï',
        'o' => 'ô',
        'u' => 'ü',
        'A' => 'À',
        'E' => 'Ë',
        'I' => 'Ï',
        'O' => 'Ô',
        'U' => 'Ü',
        'c' => 'ç',
        'C' => 'Ç',
        'n' => 'ñ',
        'N' => 'Ñ',
        _ => c,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn expansion_is_catalog_only_and_keeps_formatted_messages_out() {
        let source = "Play the last channel on start";
        let expanded = super::fixed(source).unwrap();
        assert!(expanded.chars().count() > source.chars().count() * 2);
        assert!(expanded.contains('ë'));
        assert!(super::fixed("Synthetic provider label").is_none());
        assert!(super::fixed("Track {number}").is_none());
    }
}
