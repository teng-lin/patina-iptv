//! Synthetic settings and playlist only; exercise the real choice editor and
//! reactive text bindings rather than rebuilding the application on a change.
//!
//! The scenarios after the first start in a language and walk the flows
//! that are translated in full: what they expect on screen is the
//! catalogue's text for that language, formatted by the core as the app
//! formats it, so a reworded translation does not fail them and a message
//! that stayed English does.
use crate::personalization::{self, Fixture, step};
use iptv_core::i18n::{self, Arg, Language};
use iptv_settings::{Settings, Target, Value, keys};
use nsh_testing::{DialogAnswer, Headless};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CHANNELS: &str = "{count, plural, one {# channel} other {# channels}}";
/// How long an import or a refresh of a local file may take.
const IMPORT: Duration = Duration::from_secs(30);

/// The catalogue in one language.
#[derive(Clone, Copy)]
struct Words(&'static Language);

impl Words {
    fn of(tag: &str) -> Self {
        let language = i18n::resolve(tag, None);
        assert_eq!(language.tag, tag);
        Self(language)
    }

    /// A message in this language, which is not its English.
    #[track_caller]
    fn t(self, source: &str) -> String {
        let text = i18n::translate(self.0, source);
        assert_ne!(text, source, "{} has no translation", self.0.tag);
        text.to_owned()
    }

    /// As [`Words::t`], with values and counts.
    #[track_caller]
    fn f(self, source: &str, args: &[(&str, Arg<'_>)]) -> String {
        assert!(i18n::is_catalogued(source), "no row for {source:?}");
        let text = i18n::format(self.0, source, args);
        assert_ne!(
            text,
            i18n::format(&i18n::LANGUAGES[0], source, args),
            "{} has no translation of {source:?}",
            self.0.tag
        );
        text
    }
}

/// The app with the personalization playlist, in `language` on `clock`.
fn start_in(home: crate::isolated::Home, language: &str, clock: &str) -> Fixture {
    let mut initial = Settings::new();
    initial
        .set("general.language", Value::text(language))
        .unwrap();
    initial.set("general.clock", Value::choice(clock)).unwrap();
    personalization::start(home, initial, false)
}

#[track_caller]
fn shown(app: &mut Headless, text: &str) {
    step(app.wait_for(&format!("text:{text}")));
}

#[track_caller]
fn press(app: &mut Headless, text: &str) {
    shown(app, text);
    step(app.press(&format!("text:{text}")));
}

/// A text as the pattern of a `text:/…/` selector.
fn pattern(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if r"\.+*?()|[]{}^$/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// From the guide to the navigation rail.
fn rail(app: &mut Headless) {
    step(app.press("text:/^☰  /"));
    step(app.press_key("left"));
    step(app.press_key("left"));
}

#[test]
fn interface_language_changes_live_persists_and_can_be_recovered() {
    let Some(home) = isolated!() else { return };
    let mut initial = Settings::new();
    initial.set("general.language", Value::text("en")).unwrap();
    let mut fixture = personalization::start(home, initial, false);
    let app = &mut fixture.app;
    // Keep the navigation rail mounted while settings are open, so a
    // language change must update its existing labels as well.
    step(app.press("text:/^☰  /"));
    step(app.press_key("left"));
    step(app.wait_for("text:Live TV"));
    step(app.press("label:Settings"));
    step(app.wait_for("text:Play the last channel on start"));
    step(app.press_key("right"));
    for _ in 0..15 {
        if app.screen().matches("text:Language").unwrap() && app.press("text:Language").is_ok() {
            break;
        }
        step(app.press_key("down"));
    }
    // The chooser's backdrop is mounted while closed; its name is read by
    // screen readers only.
    step(app.wait_for("label:Close the chooser"));
    step(app.wait_for("text:/Español/"));
    step(app.press("text:/Español/"));
    step(app.wait_for("text:Ajustes"));
    step(app.wait_for("text:Idioma"));
    step(app.wait_for("text:☰  Todos los canales"));
    step(app.wait_for("text:TV en directo"));
    step(app.wait_for("text:Reproducir el último canal al iniciar"));
    // Accessibility labels of mounted views follow: the rail's item, the
    // video's tap target and the chooser's backdrop.
    step(app.wait_for("label:Ajustes"));
    step(app.wait_for("label:Vídeo"));
    step(app.wait_for("label:Cerrar el selector"));
    assert!(!app.screen().matches("label:Close the chooser").unwrap());
    // A second switch while the settings remain mounted must update every
    // bound heading; native language names always remain readable.
    step(app.press("text:Idioma"));
    step(app.wait_for("text:/繁體中文/"));
    // Scroll through choices with the remote so the last entries are reachable.
    for _ in 0..7 {
        step(app.press_key("down"));
    }
    step(app.press("text:/繁體中文/"));
    step(app.wait_for("text:語言"));
    step(app.wait_for("text:直播電視"));
    step(app.wait_for("text:啟動時播放上次頻道"));
    step(app.wait_for("label:關閉選擇器"));
    let path = fixture.path.parent().unwrap().join("settings.json");
    let stored = personalization::saved_settings(app, &path, |settings| {
        settings.text(keys::LANGUAGE, &Target::Global) == "zh-Hant"
    });
    assert_eq!(stored.text(keys::LANGUAGE, &Target::Global), "zh-Hant");
    step(app.press("text:語言"));
    for _ in 0..12 {
        step(app.press_key("up"));
    }
    step(app.press("text:/系統語言/"));
    step(app.wait_for("text:Language"));
    step(app.wait_for("text:Settings >> nth=1"));
    step(app.wait_for("label:Close the chooser"));
    for _ in 0..3 {
        step(app.back());
    }
    step(app.wait_for_within("text:★ Local A", Duration::from_secs(5)));
}

/// The playlists screen in German: counts, the organisation screen, a
/// refresh that fails and an import that fails.
#[test]
fn playlists_organisation_and_failures_speak_the_interface_language() {
    let Some(home) = isolated!() else { return };
    let de = Words::of("de");
    let mut fixture = start_in(home, "de", "auto");
    let playlist = fixture.home.path().join("synthetic.m3u");
    let app = &mut fixture.app;
    // The ruler's date: German names and order, and its 24-hour clock.
    step(app.wait_for(r"text:/^(Mo|Di|Mi|Do|Fr|Sa|So)\., \d{1,2}\. \S+  \d\d:\d\d$/"));
    rail(app);
    step(app.press(&format!("text:{} >> nth=0", de.t("Playlists"))));
    shown(app, "Synthetic personalization");
    // A count in the row: "3 Sender", not "3 channels".
    let three = de.f(CHANNELS, &[("count", 3u32.into())]);
    shown(app, &format!("{} · {three}", de.t("M3U playlist")));
    assert!(!app.shows("3 channels"));
    // Its actions.
    step(app.press("text:Synthetic personalization"));
    let yes = de.t("Yes");
    shown(
        app,
        &de.f(
            "Playlist enabled: {answer}",
            &[("answer", yes.as_str().into())],
        ),
    );
    shown(app, &de.t("Refresh now"));
    shown(app, &de.t("Custom groups"));
    // Organisation (the seventh action, by remote as it is below the fold).
    for _ in 0..6 {
        step(app.press_key("down"));
    }
    step(app.press_key("enter"));
    step(app.wait_for("id:organization-screen"));
    shown(app, &de.t("Organize channels"));
    shown(
        app,
        &de.t("Includes hidden items. OK opens actions; select items for bulk hide/show. Back returns."),
    );
    shown(app, &format!("{} >> nth=0", de.t("Visible")));
    step(app.press("id:organization-row-0"));
    shown(app, &de.t("Hide item"));
    shown(app, &de.t("Move to first"));
    shown(app, &de.t("Use provider name"));
    press(app, &de.t("Hide item"));
    shown(app, &de.t("Hidden"));
    step(app.press("id:organization-row-0"));
    press(app, &de.t("Show item"));
    step(app.back());
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(3)));
    // A refresh that fails says so, and why, in German.
    std::fs::remove_file(&playlist).unwrap();
    step(app.press("text:Synthetic personalization >> nth=0"));
    press(app, &de.t("Refresh now"));
    let failed = de.f("Refresh failed: {reason}", &[("reason", "".into())]);
    step(app.wait_for_within(&format!("text:/^{}/", pattern(&failed)), IMPORT));
    assert!(!app.shows("Refresh failed"));
    // An import that fails: a file that is not a playlist.
    app.on_pick(|_| crate::support::picked("not a playlist".to_owned()));
    step(app.press(&format!(
        "text:{} >> nth=1",
        pattern_free(&de.t("+ Add playlist"))
    )));
    press(app, &de.t("Playlist file"));
    press(app, &de.t("Choose a file…"));
    shown(app, "synthetic.m3u");
    press(app, &de.t("Add playlist"));
    // The form comes back with the reason, whichever of these the reader
    // gives for such a file.
    let why = [
        "This is not a playlist the app can read. Check the address.",
        "The playlist loaded but has nothing to watch in it.",
        "The file could not be read. Choose it again.",
    ];
    let german: Vec<_> = why.iter().map(|source| pattern(&de.t(source))).collect();
    step(app.wait_for_within(&format!("text:/^({})$/", german.join("|")), IMPORT));
    assert!(why.iter().all(|source| !app.shows(source)));
}

/// A selector's text as written (no pattern): only checks there is none
/// of the characters a selector reads.
fn pattern_free(text: &str) -> &str {
    assert!(!text.contains(">>") && !text.starts_with('/'));
    text
}

/// The guide's menu in Russian: custom groups with their counts in three
/// plural forms, and the guide-data chooser's notice.
#[test]
fn custom_groups_and_guide_assignment_speak_the_interface_language() {
    let Some(home) = isolated!() else { return };
    let ru = Words::of("ru");
    let mut fixture = start_in(home, "ru", "auto");
    let app = &mut fixture.app;
    step(app.wait_for(r"text:/^(пн|вт|ср|чт|пт|сб|вс), \d{1,2} \S+  \d\d:\d\d$/"));
    step(app.press_key("m"));
    shown(app, &ru.t("Manage favourites…"));
    press(app, &ru.t("New group…"));
    step(app.wait_for("id:custom-groups-screen"));
    shown(app, &ru.t("Custom groups"));
    shown(
        app,
        &ru.t("Copy adds membership. Move transfers between custom groups. Channels and provider groups are retained."),
    );
    let groups = "{count, plural, one {# custom group} other {# custom groups}}";
    // Zero is "many" in Russian: "0 своих групп".
    shown(app, &ru.f(groups, &[("count", 0u32.into())]));
    press(app, &ru.t("New group"));
    // A blank name is refused in Russian.
    step(app.submit(&format!("text:{}", ru.t("Enter text"))));
    shown(app, &ru.t("Enter a group name."));
    step(app.type_text(&format!("text:{}", ru.t("Enter text")), "Синтетика"));
    step(app.submit("text:Синтетика"));
    let row = "{name} · {count, plural, one {# channel} other {# channels}}";
    shown(
        app,
        &ru.f(row, &[("name", "Синтетика".into()), ("count", 0u32.into())]),
    );
    // One group: "1 своя группа".
    shown(app, &ru.f(groups, &[("count", 1u32.into())]));
    press(app, &ru.t("Browse all channels"));
    let all = ru.t("all channels");
    let search = ru.f("Search · {query}", &[("query", all.as_str().into())]);
    shown(app, &search);
    let summary = "{selected} selected · {matching} matching · {total} total";
    let counts = |selected: u32| {
        ru.f(
            summary,
            &[
                ("selected", selected.into()),
                ("matching", 3u32.into()),
                ("total", 3u32.into()),
            ],
        )
    };
    shown(app, &counts(0));
    press(app, &ru.t("Select filtered"));
    shown(app, &counts(3));
    press(app, &ru.t("Copy selected"));
    press(
        app,
        &ru.f(row, &[("name", "Синтетика".into()), ("count", 0u32.into())]),
    );
    // Three channels copied: the "few" form, "3 канала скопировано."
    let copied = "{count, plural, one {# channel copied.} other {# channels copied.}}";
    shown(app, &ru.f(copied, &[("count", 3u32.into())]));
    assert!(!app.shows("3 channels copied."));
    for _ in 0..4 {
        if !app.screen().matches("id:custom-groups-screen").unwrap() {
            break;
        }
        step(app.back());
    }
    step(app.wait_for_gone("id:custom-groups-screen", Duration::from_secs(3)));
    // No guide is stored: the chooser says so instead of opening.
    step(app.press_key("m"));
    shown(app, &ru.t("Manage favourites…"));
    press(app, &ru.t("Choose guide data…"));
    shown(
        app,
        &ru.t("No guide data is stored yet: update the guide first"),
    );
}

/// Settings in Japanese on a 12-hour clock: the clock and its setting,
/// the backup and restore dialogs, and the remote-key editor.
#[test]
fn backup_remote_keys_and_the_clock_speak_the_interface_language() {
    let Some(home) = isolated!() else { return };
    let ja = Words::of("ja");
    let mut fixture = start_in(home, "ja", "h12");
    let app = &mut fixture.app;
    // The ruler: the Japanese date, then a 12-hour time with its half of
    // the day first.
    step(
        app.wait_for(
            r"text:/^\d{1,2}月\d{1,2}日\((月|火|水|木|金|土|日)\)  午(前|後)\d{1,2}:\d\d$/",
        ),
    );
    step(app.press_key("m"));
    shown(app, &ja.t("Manage favourites…"));
    step(app.press(&format!("text:{} >> nth=1", ja.t("Settings"))));
    shown(app, &ja.t("Back up"));
    // Backing up asks first, in Japanese; the answer here is "Cancel".
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    let cancel = ja.t("Cancel");
    app.on_dialog(move |dialog| {
        seen.lock().unwrap().push((
            dialog.title.clone().unwrap_or_default(),
            dialog.message.clone().unwrap_or_default(),
            dialog
                .buttons
                .iter()
                .map(|button| button.text.clone())
                .collect::<Vec<_>>(),
        ));
        DialogAnswer::Press(cancel.clone())
    });
    press(app, &ja.t("Back up"));
    let (title, message, buttons) = asked.lock().unwrap().pop().expect("no dialog was shown");
    assert_eq!(title, ja.t("Back up Patina"));
    assert_eq!(
        message,
        ja.t("The backup includes playlist logins and passwords, settings, channel arrangements, history and cached guide data. Keep the file private.")
    );
    assert_eq!(buttons, [ja.t("Cancel"), ja.t("Choose file…")]);
    // Restoring explains itself before any file is chosen.
    press(app, &ja.t("Restore a backup"));
    shown(
        app,
        &ja.t("Choose a backup to apply when you next open Patina. You can cancel the scheduled restore before closing the app."),
    );
    step(app.back());
    // The clock setting names its choice.
    for _ in 0..15 {
        if app
            .screen()
            .matches(&format!("text:{}", ja.t("Clock")))
            .unwrap()
        {
            break;
        }
        step(app.press_key("down"));
    }
    shown(app, &ja.t("Clock"));
    shown(app, &ja.t("12-hour"));
    // The remote-key editor.
    press(app, &ja.t("Remote control"));
    press(app, &ja.t("Edit remote bindings"));
    step(app.wait_for("id:remote-editor-screen"));
    shown(app, &ja.t("Remote bindings"));
    shown(
        app,
        &ja.t("Up/Down chooses a row; Left/Right or tap changes it. Save applies immediately. Back remains available."),
    );
    let guide = ja.t("Guide");
    shown(
        app,
        &ja.f("Context: {context}", &[("context", guide.as_str().into())]),
    );
    let short = ja.t("Short");
    shown(
        app,
        &ja.f("Press: {press}", &[("press", short.as_str().into())]),
    );
    // "Currently: … · default binding", whatever the key does.
    let now = ja.f(
        "Currently: {action} · default binding",
        &[("action", "\u{1}".into())],
    );
    let (before, after) = now.split_once('\u{1}').unwrap();
    step(app.wait_for(&format!("text:/^{}.*{}$/", pattern(before), pattern(after))));
    // The press kind changes with a tap: "Hold", in Japanese.
    step(app.press("id:remote-Press"));
    let hold = ja.t("Hold");
    shown(
        app,
        &ja.f("Press: {press}", &[("press", hold.as_str().into())]),
    );
    assert!(!app.shows("Press: Hold"));
}
