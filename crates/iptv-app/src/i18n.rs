//! Platform locale discovery lives in the shell; lookup lives in the core.
//!
//! [`tr`] shows a fixed message in the interface language, [`trf`] one
//! with named placeholders and counts. A message kept as data and shown
//! later (an error's text, a notice sent as an event) is marked with
//! [`key`] where it is written, and translated with [`tr`] where it is
//! shown. `tests/app/catalog_lint.rs` checks that every literal in one
//! of these calls has a catalog row, and that no user-facing literal
//! goes around them.
use iptv_presentation::clock::{HourCycle, Style};
use iptv_presentation::i18n::{self, Args, Language};
use iptv_settings::{Target, keys};

#[cfg(feature = "test-support")]
mod pseudo;

/// A deliberately expanded fixed catalog label for synthetic layout scenarios.
#[cfg(feature = "test-support")]
pub fn pseudo_fixed_for_test(source: &str) -> Option<&'static str> {
    pseudo::fixed(source)
}

fn system() -> Option<&'static str> {
    static SYSTEM: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    SYSTEM.get_or_init(system_locale).as_deref()
}

#[cfg(test)]
thread_local! {
    // Wording fixtures only. Production preferences always come from the owner.
    static WORDING_SETTINGS: std::cell::RefCell<Option<std::sync::Arc<iptv_settings::Settings>>> = const { std::cell::RefCell::new(None) };
}

/// The owner's current preferences. Before an owner can be constructed (for
/// example, the private-storage error screen), use defaults and the device
/// locale without discovering paths, opening storage or creating an owner.
fn preferences() -> std::sync::Arc<iptv_settings::Settings> {
    #[cfg(test)]
    if let Some(settings) = WORDING_SETTINGS.with(|settings| settings.borrow().clone()) {
        return settings;
    }
    if crate::has_runtime() {
        crate::settings::get()
    } else {
        std::sync::Arc::new(iptv_settings::Settings::new())
    }
}

pub fn language() -> &'static Language {
    crate::settings::observe();
    let settings = preferences();
    let preference = settings.text(keys::LANGUAGE, &Target::Global);
    i18n::resolve(&preference, system())
}

/// How dates and times are written now: the interface language's names,
/// and the clock of **Settings → General → Clock**. Left on automatic, the
/// clock is the convention of the device's locale when the language
/// follows the device, and of the chosen language otherwise.
pub fn clock_style() -> Style {
    crate::settings::observe();
    let settings = preferences();
    let preference = settings.text(keys::LANGUAGE, &Target::Global);
    let locale = match preference.as_str() {
        "system" => system().unwrap_or("en"),
        chosen => chosen,
    };
    let hours = HourCycle::from_preference(settings.choice(keys::CLOCK, &Target::Global), locale);
    Style::new(i18n::resolve(&preference, system()), hours)
}

fn system_locale() -> Option<String> {
    // Headless scenarios pin a locale independently of the build machine.
    #[cfg(any(test, feature = "test-support"))]
    if let Ok(locale) = std::env::var("IPTV_RS_TEST_LOCALE") {
        return Some(locale);
    }
    #[cfg(test)]
    {
        Some("en".to_owned())
    }
    #[cfg(not(test))]
    {
        sys_locale::get_locale()
    }
}

/// Only call for application-owned messages, never provider content.
pub fn tr(source: &str) -> &str {
    #[cfg(feature = "test-support")]
    if pseudo::enabled()
        && let Some(expanded) = pseudo::fixed(source)
    {
        return expanded;
    }
    i18n::translate(language(), source)
}

/// As [`tr`], for a message with `{named}` placeholders and plural forms
/// (`iptv_presentation::i18n::message`): `args` fills them, and a count selects
/// its form by the interface language's rules. Provider content may be a
/// value; it is never the message.
pub fn trf(source: &str, args: &Args<'_>) -> String {
    i18n::format(language(), source, args)
}

/// Marks `source` as a catalog message that is translated where it is
/// shown, not here: the text of an error or of a notice that travels as
/// data. Returns it unchanged.
pub const fn key(source: &'static str) -> &'static str {
    source
}

pub fn language_name(tag: &str) -> &'static str {
    if tag == "system" {
        return tr("System language");
    }
    i18n::LANGUAGES
        .iter()
        .find(|l| l.tag == tag)
        .map_or("", |l| l.name)
}

/// Runs `run` with `language` as the interface language and `clock` as
/// the clock setting, for tests of wording: this thread's scoped fixture,
/// restored even when a nested wording assertion unwinds.
#[cfg(test)]
pub(crate) fn with_language<R>(language: &str, clock: &str, run: impl FnOnce() -> R) -> R {
    use iptv_settings::{Settings, Value};
    use std::sync::Arc;
    let mut settings = Settings::new();
    settings
        .set("general.language", Value::text(language))
        .unwrap();
    settings.set("general.clock", Value::choice(clock)).unwrap();
    struct Restore(Option<Arc<iptv_settings::Settings>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            WORDING_SETTINGS.set(self.0.take());
        }
    }
    let _restore = Restore(WORDING_SETTINGS.replace(Some(Arc::new(settings))));
    run()
}

/// As [`with_language`], with the clock left to the language.
#[cfg(test)]
pub(crate) fn in_language<R>(language: &str, run: impl FnOnce() -> R) -> R {
    with_language(language, "auto", run)
}

#[cfg(test)]
mod tests {
    use super::{clock_style, in_language, key, tr, trf, with_language};
    use iptv_presentation::clock::HourCycle;

    const CHANNELS: &str = "{count, plural, one {# channel} other {# channels}}";

    #[test]
    fn every_settings_help_message_has_real_locale_cells_without_copied_english_phrases() {
        use iptv_settings::catalog;
        let words = |text: &str| {
            text.split(|c: char| !c.is_ascii_alphabetic())
                .filter(|part| !part.is_empty())
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        };
        let rows: std::collections::HashMap<_, _> =
            include_str!("../../iptv-presentation/src/i18n/messages.tsv")
                .lines()
                .map(|line| {
                    let mut cells = line.split('\t');
                    (cells.next().unwrap(), cells.collect::<Vec<_>>())
                })
                .collect();
        for source in catalog::settings()
            .map(|setting| setting.summary)
            .chain(catalog::commands().map(|command| command.summary))
        {
            let cells = rows
                .get(source)
                .unwrap_or_else(|| panic!("missing {source}"));
            assert_eq!(cells.len(), iptv_presentation::i18n::LANGUAGES.len() - 1);
            let english = words(source);
            for (cell, language) in cells.iter().zip(&iptv_presentation::i18n::LANGUAGES[1..]) {
                assert!(!cell.trim().is_empty(), "{}: {source}", language.tag);
                // Font family and license proper names intentionally keep English.
                if source.starts_with("The version of this app.") {
                    continue;
                }
                let translated = words(cell);
                for phrase in english.windows(4).filter(|p| p.join(" ").len() >= 16) {
                    assert!(
                        !translated.windows(4).any(|candidate| candidate == phrase),
                        "{} copied an English phrase in {source}: {cell}",
                        language.tag
                    );
                }
            }
        }
    }

    #[test]
    fn wording_fixtures_restore_nested_preferences_after_unwinding() {
        assert_eq!(super::language().tag, "en");
        in_language("de", || {
            assert_eq!(super::language().tag, "de");
            let failed = std::panic::catch_unwind(|| {
                in_language("ru", || panic!("synthetic wording assertion"));
            });
            assert!(failed.is_err());
            assert_eq!(super::language().tag, "de");
        });
        assert_eq!(super::language().tag, "en");
    }

    #[test]
    fn messages_follow_the_language_setting() {
        assert_eq!(tr("Remote bindings"), "Remote bindings");
        assert_eq!(
            in_language("de", || tr("Remote bindings")),
            "Fernbedienungstasten"
        );
        assert_eq!(in_language("ru", || tr("Custom groups")), "Свои группы");
        let channels = |n: u32| trf(CHANNELS, &[("count", n.into())]);
        assert_eq!(channels(1), "1 channel");
        assert_eq!(channels(3), "3 channels");
        in_language("ru", || {
            assert_eq!(channels(1), "1 канал");
            assert_eq!(channels(3), "3 канала");
            assert_eq!(channels(5), "5 каналов");
            assert_eq!(channels(21), "21 канал");
        });
        assert_eq!(in_language("ja", || channels(3)), "3チャンネル");
        // A value is placed where the language wants it.
        let delete = || trf("Delete “{name}”?", &[("name", "Synthetic".into())]);
        assert_eq!(delete(), "Delete “Synthetic”?");
        assert_eq!(in_language("de", delete), "„Synthetic“ löschen?");
        assert_eq!(in_language("ja", delete), "「Synthetic」を削除しますか？");
        // A regional tag uses its language; an unknown one stays English.
        assert_eq!(
            in_language("pt-BR", || tr("Custom groups")),
            "Grupos personalizados"
        );
        assert_eq!(in_language("xx", || tr("Custom groups")), "Custom groups");
    }

    /// `key` only marks: the message is translated where it is shown.
    #[test]
    fn a_marked_message_is_translated_when_shown() {
        const NOTE: &str = key("Backup is too large");
        assert_eq!(NOTE, "Backup is too large");
        assert_eq!(in_language("de", || tr(NOTE)), "Sicherung ist zu groß");
    }

    #[test]
    fn the_clock_follows_the_language_unless_the_setting_fixes_it() {
        let hours =
            |language: &str, clock: &str| with_language(language, clock, || clock_style().hours);
        // Left alone: each language's own convention.
        assert_eq!(hours("en", "auto"), HourCycle::H12);
        assert_eq!(hours("en-GB", "auto"), HourCycle::H24);
        assert_eq!(hours("de", "auto"), HourCycle::H24);
        assert_eq!(hours("ko", "auto"), HourCycle::H12);
        assert_eq!(hours("zh-Hant", "auto"), HourCycle::H12);
        assert_eq!(hours("zh", "auto"), HourCycle::H24);
        // With the device's language (pinned to English in tests).
        assert_eq!(hours("system", "auto"), HourCycle::H12);
        // Fixed by the setting, whatever the language.
        for language in ["en", "de", "ko", "ja"] {
            assert_eq!(hours(language, "h24"), HourCycle::H24, "{language}");
            assert_eq!(hours(language, "h12"), HourCycle::H12, "{language}");
        }
        // The names are the interface language's either way.
        let style = with_language("de", "h12", clock_style);
        assert_eq!(style.language.tag, "de");
        assert_eq!(
            iptv_presentation::clock::date_time(iptv_core::guide::SYNTHETIC_EPOCH_UNIX, 0, style),
            "Di., 10. Nov.  6:00 PM"
        );
        let style = in_language("ja", clock_style);
        assert_eq!(
            iptv_presentation::clock::date_time(iptv_core::guide::SYNTHETIC_EPOCH_UNIX, 0, style),
            "11月10日(火)  18:00"
        );
    }

    /// The names of the controls that show an icon only (`.label(…)`, read
    /// by VoiceOver and TalkBack): each has a row in the catalog, so a
    /// screen reader speaks the interface language. A name that only
    /// reached `tr` without a row would stay English without a sign.
    #[test]
    fn accessibility_labels_are_in_the_catalog() {
        use iptv_presentation::i18n::{LANGUAGES, resolve, translate};
        const LABELS: [&str; 29] = [
            // The play bar.
            "Mute",
            "Volume",
            "Rewind",
            "Play or pause",
            "Fast-forward",
            "Favorite",
            "Subtitles and tracks",
            "Program details",
            "Picture size",
            "Channel list",
            "Multiview",
            "Back to multiview",
            "Picture in picture",
            "TV guide",
            "Full screen",
            "Guide",
            // The video, the choosers and the guide's way back.
            "Video",
            "Close the chooser",
            "Groups",
            "Menu",
            // Multiview's toolbar and tiles.
            "Add screen",
            "Exit multiview",
            "Reduce screen",
            "Enlarge screen",
            "Remove screen",
            // The picture-in-picture window.
            "Back to the app",
            "Close",
            "Play",
            "Pause",
        ];
        for label in LABELS {
            // Some languages spell a word as English does ("Menu"), but
            // none of these is the same in every language.
            let translated = LANGUAGES
                .iter()
                .filter(|language| translate(language, label) != label)
                .count();
            assert!(translated > 0, "no catalog row for {label:?}");
        }
        assert_eq!(translate(resolve("es", None), "Volume"), "Volumen");
        assert_eq!(translate(resolve("de", None), "Mute"), "Stumm");
        assert_eq!(translate(resolve("zh-TW", None), "Menu"), "選單");
    }

    #[test]
    fn picker_and_catalog_languages_match() {
        let choices = iptv_settings::catalog::choices::LANGUAGE.variants;
        assert_eq!(choices[0], "system");
        assert_eq!(
            &choices[1..],
            iptv_presentation::i18n::LANGUAGES
                .iter()
                .map(|l| l.tag)
                .collect::<Vec<_>>()
        );
    }
}
