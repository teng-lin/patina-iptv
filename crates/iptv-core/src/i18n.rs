//! Interface languages and translations. Pure, deterministic and platform independent.
//! English source messages are keys; untranslated messages retain their source text.
//!
//! [`translate`] looks a fixed message up. [`format()`] does the same for a
//! [message] with named placeholders and plural forms, chosen by
//! the language's own [plural rules](plural). [`calendar`] has each
//! language's day and month names; [`clock`](crate::clock) writes dates
//! and times with them.

pub mod calendar;
pub mod message;
pub mod plural;

pub use message::{Arg, Args};

/// A supported interface language. Names stay in their own language so the
/// language picker remains usable after an accidental selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Language {
    pub tag: &'static str,
    pub name: &'static str,
}

/// Column order in the UTF-8 catalog (English is the source column).
pub const LANGUAGES: &[Language] = &[
    Language {
        tag: "en",
        name: "English",
    },
    Language {
        tag: "es",
        name: "Español",
    },
    Language {
        tag: "fr",
        name: "Français",
    },
    Language {
        tag: "de",
        name: "Deutsch",
    },
    Language {
        tag: "it",
        name: "Italiano",
    },
    // The Portuguese catalog uses European Portuguese vocabulary (e.g.
    // "ecrã", "ficheiro", "definições") and CLDR pt-PT plural rules (zero
    // other, one one, million many). Regional locales like pt-BR resolve
    // to it as the closest match.
    Language {
        tag: "pt",
        name: "Português",
    },
    Language {
        tag: "ru",
        name: "Русский",
    },
    Language {
        tag: "zh",
        name: "简体中文",
    },
    Language {
        tag: "zh-Hant",
        name: "繁體中文",
    },
    Language {
        tag: "ja",
        name: "日本語",
    },
    Language {
        tag: "ko",
        name: "한국어",
    },
];

/// Resolve regional/script tags, including POSIX locale names. Unsupported
/// languages fall back to English; Chinese regions select the right script.
/// The single supported Portuguese catalog is European (vocabulary and
/// pt-PT plural rules); Brazilian locales (`pt-BR`) resolve to it.
pub fn resolve(preference: &str, system: Option<&str>) -> &'static Language {
    let tag = if preference == "system" {
        system.unwrap_or("en")
    } else {
        preference
    };
    let normalized = tag.trim().replace('_', "-").to_ascii_lowercase();
    let tag = normalized.split(['.', '@']).next().unwrap_or("");
    let parts: Vec<_> = tag.split('-').collect();
    let base = parts.first().copied().unwrap_or("");
    let language = if base == "zh" {
        if parts.contains(&"hant")
            || (!parts.contains(&"hans") && parts.iter().any(|p| matches!(*p, "tw" | "hk" | "mo")))
        {
            "zh-Hant"
        } else {
            "zh"
        }
    } else {
        base
    };
    LANGUAGES
        .iter()
        .find(|l| l.tag == language)
        .unwrap_or(&LANGUAGES[0])
}

// The catalog is outside the module only where a browser host installs its
// reader (wasm32): a native build with the feature on (`--all-features`)
// keeps the embedded catalog, or it would show English everywhere. This
// crate's own tests read the external catalog natively too.
#[cfg(not(all(feature = "external-catalog", any(target_family = "wasm", test))))]
const CATALOG: &str = include_str!("i18n/messages.tsv");

#[cfg(not(all(feature = "external-catalog", any(target_family = "wasm", test))))]
type Catalog = std::collections::HashMap<&'static str, Vec<&'static str>>;
#[cfg(not(all(feature = "external-catalog", any(target_family = "wasm", test))))]
static MESSAGES: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();

#[cfg(not(all(feature = "external-catalog", any(target_family = "wasm", test))))]
fn messages() -> &'static Catalog {
    MESSAGES.get_or_init(|| {
        CATALOG
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let columns: Vec<_> = line.split('\t').collect();
                (columns[0], columns)
            })
            .collect()
    })
}

/// The catalog's text for `source` in `language`: `None` for English
/// (the source is the text), an unknown message or an unknown language.
fn find(language: &Language, source: &str) -> Option<&'static str> {
    let column = LANGUAGES.iter().position(|l| l.tag == language.tag)?;
    if column == 0 {
        return None;
    }
    #[cfg(not(all(feature = "external-catalog", any(target_family = "wasm", test))))]
    let text = messages()
        .get(source)
        .and_then(|row| row.get(column))
        .copied();
    #[cfg(all(feature = "external-catalog", any(target_family = "wasm", test)))]
    let text = external::column(column).get(source).copied();
    text.filter(|s| !s.is_empty())
}

/// The catalog outside the module (feature `external-catalog` on wasm32,
/// for a browser package, whose module size is its launch time): one file per
/// language, read when the language is first shown through the reader
/// the host installs ([`set_catalog_reader`]). A language's file has a
/// line per message, its English source and its text separated by a tab;
/// English's has the sources alone (for [`is_cataloged`]).
#[cfg(all(feature = "external-catalog", any(target_family = "wasm", test)))]
mod external {
    use super::LANGUAGES;
    use std::collections::HashMap;
    use std::sync::OnceLock;

    pub(super) type Reader = fn(tag: &str) -> Option<String>;

    pub(super) static READER: OnceLock<Reader> = OnceLock::new();

    type Column = HashMap<&'static str, &'static str>;

    /// Language `column`'s messages by source, read once. A file that
    /// cannot be read leaves the language in English (and is not read
    /// again).
    pub(super) fn column(column: usize) -> &'static Column {
        static COLUMNS: [OnceLock<Column>; LANGUAGES.len()] =
            [const { OnceLock::new() }; LANGUAGES.len()];
        COLUMNS[column].get_or_init(|| {
            let Some(text) = READER.get().and_then(|read| read(LANGUAGES[column].tag)) else {
                return Column::new();
            };
            // Kept for the process's life, as the embedded catalog is.
            let text: &'static str = Box::leak(text.into_boxed_str());
            text.lines()
                .filter(|line| !line.is_empty())
                .map(|line| line.split_once('\t').unwrap_or((line, "")))
                .collect()
        })
    }
}

/// Installs how the catalog of a language is read (feature
/// `external-catalog` on wasm32): `read(tag)` returns that language's file (see the
/// `external` module), `None` when there is none. Only the first call
/// counts; set it before the first message is shown.
#[cfg(all(feature = "external-catalog", any(target_family = "wasm", test)))]
pub fn set_catalog_reader(read: fn(tag: &str) -> Option<String>) {
    let _ = external::READER.set(read);
}

/// Translate only application-owned messages, never playlist/channel/EPG text.
/// Unknown source text and unsupported languages safely retain English.
pub fn translate<'a>(language: &Language, source: &'a str) -> &'a str {
    find(language, source).unwrap_or(source)
}

/// As [`translate`], for a [message] with placeholders: `source`
/// in `language` with `args` filled in and each plural's form chosen by
/// the count it names.
///
/// A message the catalog lacks is formatted from its English source by
/// English rules, so a count never selects a form the text does not have.
pub fn format(language: &Language, source: &str, args: &Args<'_>) -> String {
    match find(language, source) {
        Some(translated) => message::render(translated, language.tag, args),
        None => message::render(source, LANGUAGES[0].tag, args),
    }
}

/// Whether the catalog has a row for `source`: for the checks that no
/// message reaches [`translate`] or [`format()`] without one.
pub fn is_cataloged(source: &str) -> bool {
    #[cfg(not(all(feature = "external-catalog", any(target_family = "wasm", test))))]
    return messages().contains_key(source);
    #[cfg(all(feature = "external-catalog", any(target_family = "wasm", test)))]
    return external::column(0).contains_key(source);
}

#[cfg(all(test, feature = "external-catalog"))]
mod external_tests {
    use super::*;

    fn read(tag: &str) -> Option<String> {
        match tag {
            "en" => Some("Settings\nSearch\n".to_owned()),
            "fr" => Some("Settings\tParamètres\n{count, plural, one {# channel} other {# channels}}\t{count, plural, one {# chaîne} other {# chaînes}}\n".to_owned()),
            _ => None,
        }
    }

    #[test]
    fn languages_are_read_from_the_host_when_first_shown() {
        set_catalog_reader(read);
        let fr = resolve("fr", None);
        assert_eq!(translate(fr, "Settings"), "Paramètres");
        assert_eq!(translate(fr, "Search"), "Search", "no row: English");
        assert_eq!(
            format(
                fr,
                "{count, plural, one {# channel} other {# channels}}",
                &[("count", Arg::Count(2))]
            ),
            "2 chaînes"
        );
        assert_eq!(
            translate(resolve("de", None), "Settings"),
            "Settings",
            "no file: English"
        );
        assert!(is_cataloged("Search"));
        assert!(!is_cataloged("Paramètres"));
    }
}

#[cfg(all(test, not(feature = "external-catalog")))]
mod tests {
    use super::*;

    #[test]
    fn regional_and_system_preferences_resolve() {
        for (tag, expected) in [
            ("es-MX", "es"),
            ("pt_BR.UTF-8", "pt"),
            ("ZH_tw", "zh-Hant"),
            ("zh-HK", "zh-Hant"),
            ("zh-Hans-TW", "zh"),
            ("zh-Hant-CN", "zh-Hant"),
            ("ja-JP", "ja"),
            ("xx", "en"),
            ("C", "en"),
        ] {
            assert_eq!(resolve(tag, None).tag, expected, "{tag}");
        }
        assert_eq!(resolve("system", Some("fr-CA")).tag, "fr");
        assert_eq!(resolve("de", Some("es")).tag, "de");
        assert_eq!(resolve("system", None).tag, "en");
    }

    #[test]
    fn catalog_is_complete_unique_and_has_no_blank_translations() {
        let mut seen = std::collections::HashSet::new();
        for line in CATALOG.lines() {
            let columns: Vec<_> = line.split('\t').collect();
            assert_eq!(columns.len(), LANGUAGES.len(), "{line}");
            assert!(seen.insert(columns[0]), "duplicate: {}", columns[0]);
            assert!(columns.iter().all(|c| !c.trim().is_empty()), "{line}");
            for language in LANGUAGES {
                assert!(!translate(language, columns[0]).is_empty());
            }
        }
        assert!(seen.len() >= 100);
    }

    /// A translation has the placeholders of its source, no more and no
    /// fewer, and each plural has the forms its language needs. Without
    /// this a count would silently fall back to `other`, or a value would
    /// show as `{name}`.
    #[test]
    fn translations_keep_their_placeholders_and_have_their_plural_forms() {
        let mut formatted = 0;
        for line in CATALOG.lines() {
            let columns: Vec<_> = line.split('\t').collect();
            let source = columns[0];
            if !source.contains(['{', '}']) {
                // Fixed text: `translate` shows it as written, and no
                // translation may turn it into a message.
                for (language, text) in LANGUAGES.iter().zip(&columns) {
                    assert!(!text.contains(['{', '}']), "{}: {source}", language.tag);
                }
                continue;
            }
            formatted += 1;
            let expected = message::shape(source)
                .unwrap_or_else(|e| panic!("{source}: {} at {}", e.why, e.at));
            for (language, text) in LANGUAGES.iter().zip(&columns) {
                let tag = language.tag;
                let shape = message::shape(text)
                    .unwrap_or_else(|e| panic!("{tag}: {text}: {} at {}", e.why, e.at));
                assert_eq!(shape.names, expected.names, "{tag}: {text}");
                let selects_on = |shape: &message::Shape| {
                    let mut names: Vec<_> =
                        shape.plurals.iter().map(|(name, _)| name.clone()).collect();
                    names.sort();
                    names
                };
                assert_eq!(selects_on(&shape), selects_on(&expected), "{tag}: {text}");
                let forms = plural::forms(tag).unwrap();
                for (name, categories) in &shape.plurals {
                    for required in forms.required {
                        assert!(
                            categories.contains(required),
                            "{tag}: {text}: {name} lacks its {} form",
                            required.keyword()
                        );
                    }
                    for category in categories {
                        assert!(
                            forms.required.contains(category) || forms.optional.contains(category),
                            "{tag}: {text}: {name} has a {} form the language never selects",
                            category.keyword()
                        );
                    }
                }
            }
        }
        assert!(formatted >= 40, "{formatted} messages with placeholders");
    }

    #[test]
    fn formats_counts_by_each_languages_rules() {
        const CHANNELS: &str = "{count, plural, one {# channel} other {# channels}}";
        let of = |tag: &str, n: u64| format(resolve(tag, None), CHANNELS, &[("count", n.into())]);
        assert_eq!(of("en", 1), "1 channel");
        assert_eq!(of("en", 5), "5 channels");
        assert_eq!(of("es", 1), "1 canal");
        assert_eq!(of("es", 5), "5 canales");
        assert_eq!(of("fr", 0), "0 chaîne");
        assert_eq!(of("fr", 2), "2 chaînes");
        assert_eq!(of("de", 1), "1 Sender");
        assert_eq!(of("pt", 0), "0 canais");
        assert_eq!(of("pt", 1), "1 canal");
        assert_eq!(of("pt", 2), "2 canais");
        assert_eq!(of("pt-BR", 0), "0 canais");
        assert_eq!(of("ru", 1), "1 канал");
        assert_eq!(of("ru", 2), "2 канала");
        assert_eq!(of("ru", 5), "5 каналов");
        assert_eq!(of("ru", 21), "21 канал");
        assert_eq!(of("ru", 112), "112 каналов");
        assert_eq!(of("zh", 5), "5 个频道");
        assert_eq!(of("zh-TW", 5), "5 個頻道");
        assert_eq!(of("ja", 1), "1チャンネル");
        assert_eq!(of("ko", 3), "채널 3개");
    }

    #[test]
    fn formats_placeholders_where_each_language_puts_them() {
        let args = [("name", "Synthetic".into())];
        let of = |tag: &str| format(resolve(tag, None), "Delete “{name}”?", &args);
        assert_eq!(of("en"), "Delete “Synthetic”?");
        assert_eq!(of("es"), "¿Eliminar «Synthetic»?");
        assert_eq!(of("de"), "„Synthetic“ löschen?");
        assert_eq!(of("ja"), "「Synthetic」を削除しますか？");
    }

    #[test]
    fn an_uncataloged_message_is_formatted_by_english_rules() {
        const NEW: &str = "{count, plural, one {# synthetic row} other {# synthetic rows}}";
        assert!(!is_cataloged(NEW));
        // 21 is singular in Russian; the English text has no such form.
        let russian = resolve("ru", None);
        assert_eq!(
            format(russian, NEW, &[("count", 21u32.into())]),
            "21 synthetic rows"
        );
        assert_eq!(
            format(russian, NEW, &[("count", 1u32.into())]),
            "1 synthetic row"
        );
        assert!(is_cataloged("Settings"));
        assert!(!is_cataloged("Ajustes"), "only the source column is a key");
    }

    #[test]
    fn translates_and_preserves_unknown_messages() {
        assert_eq!(translate(resolve("es", None), "Settings"), "Ajustes");
        assert_eq!(translate(resolve("zh-TW", None), "Settings"), "設定");
        assert_eq!(translate(resolve("en", None), "Settings"), "Settings");
        assert_eq!(
            translate(resolve("fr", None), "Untranslated message"),
            "Untranslated message"
        );
    }
}
