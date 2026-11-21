//! Backup JSON: round trip, stability, forward/backward compatibility and
//! migrations. All ids and values are synthetic.

use iptv_settings::backup::{FORMAT, VERSION};
use iptv_settings::{Entry, KeyBinding, LoadError, PressKind, Settings, Target, Value, keys};
use serde_json::json;

const G: &Target = &Target::Global;

fn sample() -> Settings {
    let mut st = Settings::new();
    st.set("guide.autoplay", Value::Bool(true)).unwrap();
    st.set("history.recent_count", Value::Int(15)).unwrap();
    st.set("epg.time_offset", Value::ms(-30 * 60_000)).unwrap();
    st.set("appearance.accent", Value::color("teal")).unwrap();
    st.set("appearance.font_size", Value::choice("large"))
        .unwrap();
    st.set("logos.folder", Value::text("/storage/logos"))
        .unwrap();
    st.set(
        "remote.key_overrides",
        Value::KeyBindings(vec![
            KeyBinding::new(None, "red", PressKind::Short, "show_guide"),
            KeyBinding::new(Some("player"), "ok", PressKind::Long, "show_menu"),
        ]),
    )
    .unwrap();
    st.set_in(
        "channels.group_sort",
        &Target::playlist("pl-a"),
        Value::choice("name"),
    )
    .unwrap();
    st.set_in(
        "general.user_agent",
        &Target::playlist("pl-b"),
        Value::text("SyntheticAgent/1.0"),
    )
    .unwrap();
    st.set_in(
        "channels.channel_sort",
        &Target::group("pl-a", "grp-1"),
        Value::choice("manual"),
    )
    .unwrap();
    st
}

fn load(doc: serde_json::Value) -> (Settings, iptv_settings::LoadReport) {
    Settings::from_json(&doc.to_string()).expect("loads")
}

fn entry(target: Target, key: &str) -> Entry {
    Entry {
        target,
        key: key.to_owned(),
    }
}

#[test]
fn round_trip_preserves_every_value_type_and_level() {
    let st = sample();
    let (back, report) = Settings::from_json(&st.to_json_string()).unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(back, st);
    assert_eq!(
        back.choice(keys::CHANNEL_SORT, &Target::group("pl-a", "grp-1")),
        "manual"
    );
}

#[test]
fn serialization_is_stable_and_minimal() {
    let st = sample();
    let text = st.to_json_string();
    assert_eq!(text, st.clone().to_json_string());
    let (back, _) = Settings::from_json(&text).unwrap();
    assert_eq!(back.to_json_string(), text);

    // Insertion order does not matter.
    let mut other = Settings::new();
    other
        .set_in(
            "channels.channel_sort",
            &Target::group("pl-a", "grp-1"),
            Value::choice("manual"),
        )
        .unwrap();
    other.set("guide.autoplay", Value::Bool(true)).unwrap();
    let mut reverse = Settings::new();
    reverse.set("guide.autoplay", Value::Bool(true)).unwrap();
    reverse
        .set_in(
            "channels.channel_sort",
            &Target::group("pl-a", "grp-1"),
            Value::choice("manual"),
        )
        .unwrap();
    assert_eq!(other.to_json_string(), reverse.to_json_string());

    // Defaults are never written.
    assert_eq!(
        Settings::new().to_json(),
        json!({"format": FORMAT, "version": VERSION, "global": {}, "playlists": {}, "groups": {}})
    );
}

#[test]
fn values_use_the_documented_json_shapes() {
    let doc = sample().to_json();
    assert_eq!(doc["global"]["guide.autoplay"], json!(true));
    assert_eq!(doc["global"]["epg.time_offset"], json!(-1_800_000));
    assert_eq!(doc["global"]["appearance.accent"], json!("teal"));
    assert_eq!(
        doc["global"]["remote.key_overrides"],
        json!([
            {"key": "red", "press": "short", "action": "show_guide"},
            {"context": "player", "key": "ok", "press": "long", "action": "show_menu"}
        ])
    );
    assert_eq!(
        doc["playlists"]["pl-a"]["channels.group_sort"],
        json!("name")
    );
    assert_eq!(
        doc["groups"]["pl-a"]["grp-1"]["channels.channel_sort"],
        json!("manual")
    );
}

#[test]
fn unknown_keys_are_kept_reported_and_written_back() {
    let doc = json!({
        "format": FORMAT,
        "version": VERSION,
        "global": {"guide.autoplay": true, "future.hologram_mode": {"depth": 3}},
        "playlists": {"pl-a": {"future.per_playlist": 1}},
        "groups": {"pl-a": {"grp-1": {"future.per_group": "x"}}},
        "profiles": [{"name": "Profile A"}]
    });
    let (st, report) = load(doc.clone());
    assert!(st.bool(keys::AUTOPLAY, G));
    assert_eq!(
        report.unknown,
        vec![
            entry(Target::Global, "future.hologram_mode"),
            entry(Target::playlist("pl-a"), "future.per_playlist"),
            entry(Target::group("pl-a", "grp-1"), "future.per_group"),
        ]
    );
    assert_eq!(report.unknown_members, ["profiles"]);
    assert_eq!(st.to_json(), doc);

    // Resetting everything drops them.
    let mut st = st;
    st.reset_all();
    assert_eq!(st, Settings::new());
}

#[test]
fn numbers_outside_a_changed_range_are_clamped() {
    let doc = json!({
        "format": FORMAT,
        "version": VERSION,
        "global": {
            "player.info_panel_timeout": 120_000,   // above max (30 s)
            "history.recent_count": 0,              // below min (1)
            "guide.switch_delay": 1_100,            // off the 250 ms grid
            "appearance.ui_transparency": 95        // above max (80)
        }
    });
    let (st, report) = load(doc);
    assert_eq!(st.duration_ms(keys::INFO_PANEL_TIMEOUT, G), 30_000);
    assert_eq!(st.int(keys::RECENT_COUNT, G), 1);
    assert_eq!(st.duration_ms(keys::SWITCH_DELAY, G), 1_000);
    assert_eq!(st.get("appearance.ui_transparency"), Ok(Value::Int(80)));
    assert_eq!(report.clamped.len(), 4);
    assert!(report.rejected.is_empty());
}

#[test]
fn invalid_values_are_rejected_but_kept_until_overwritten() {
    let doc = json!({
        "format": FORMAT,
        "version": VERSION,
        "global": {
            "appearance.font_size": "gigantic",     // variant this build lacks
            "guide.autoplay": "yes",                // wrong JSON type
            "player.info_panel_timeout": 4.5,       // not a whole number
            "general.udp_proxy": "no-port"          // malformed
        },
        "playlists": {"pl-a": {"appearance.accent": "red"}} // global-only key
    });
    let (mut st, report) = load(doc);
    let rejected: Vec<&str> = report
        .rejected
        .iter()
        .map(|(e, _)| e.key.as_str())
        .collect();
    assert_eq!(
        rejected,
        [
            "appearance.font_size",
            "general.udp_proxy",
            "guide.autoplay",
            "player.info_panel_timeout",
            "appearance.accent"
        ]
    );
    // Defaults apply…
    assert_eq!(st.choice(keys::FONT_SIZE, G), "medium");
    assert!(!st.bool(keys::AUTOPLAY, G));
    // …and the raw entries survive a save (a newer build may understand them).
    let saved = st.to_json();
    assert_eq!(saved["global"]["appearance.font_size"], json!("gigantic"));
    assert_eq!(
        saved["playlists"]["pl-a"]["appearance.accent"],
        json!("red")
    );
    // Setting the key replaces the preserved entry.
    st.set("appearance.font_size", Value::choice("small"))
        .unwrap();
    assert_eq!(
        st.to_json()["global"]["appearance.font_size"],
        json!("small")
    );
    // Resetting it drops the preserved entry too.
    st.reset("guide.autoplay").unwrap();
    assert!(st.to_json()["global"].get("guide.autoplay").is_none());
}

#[test]
fn version_1_documents_are_migrated() {
    let v1 = json!({
        "format": FORMAT,
        "version": 1,
        "global": {"player.panel_timeout_secs": 9, "guide.autoplay": true}
    });
    let (st, report) = load(v1);
    assert_eq!(report.version, 1);
    assert_eq!(report.migrations, [1, 2, 3]);
    assert!(report.unknown.is_empty());
    assert_eq!(st.duration_ms(keys::CONTROLS_PANEL_TIMEOUT, G), 9_000);
    assert!(st.bool(keys::AUTOPLAY, G));

    // The result saves as the current version and reloads without
    // migrating again (safe on repeated startup).
    let saved = st.to_json();
    assert_eq!(saved["version"], json!(VERSION));
    assert!(saved["global"].get("player.panel_timeout_secs").is_none());
    let (again, report) = load(saved);
    assert!(report.is_clean());
    assert_eq!(again, st);
}

#[test]
fn version_2_folds_the_overlay_stay_choice_into_one() {
    let v2 = |global: serde_json::Value| json!({"format": FORMAT, "version": 2, "global": global});
    let stay = |global| load(v2(global)).0.bool(keys::GUIDE_STAY_ON_GUIDE, G);

    // The overlay layout's old default (on) was never written: off now.
    assert!(!stay(json!({"guide.browse_mode": "overlay"})));
    // The choice for the layout in use wins.
    assert!(stay(json!({
        "guide.browse_mode": "overlay",
        "guide.stay_on_guide": false,
        "guide.stay_on_guide_overlay": true
    })));
    assert!(!stay(json!({
        "guide.stay_on_guide": false,
        "guide.stay_on_guide_overlay": true
    })));
    // Otherwise an explicit choice for the other layout carries over.
    assert!(stay(json!({"guide.stay_on_guide_overlay": true})));
    assert!(stay(
        json!({"guide.browse_mode": "overlay", "guide.stay_on_guide": true})
    ));

    // Only booleans count: an invalid old value never replaces a valid
    // choice, and an invalid shared value does not block a valid old one.
    assert!(stay(json!({
        "guide.browse_mode": "overlay",
        "guide.stay_on_guide": true,
        "guide.stay_on_guide_overlay": null
    })));
    assert!(stay(json!({
        "guide.stay_on_guide": null,
        "guide.stay_on_guide_overlay": true
    })));
    assert!(!stay(json!({
        "guide.browse_mode": "overlay",
        "guide.stay_on_guide": true,
        "guide.stay_on_guide_overlay": false
    })));

    let (st, report) = load(v2(json!({"guide.stay_on_guide_overlay": false})));
    assert_eq!(report.migrations, [2, 3]);
    assert!(report.unknown.is_empty(), "the old key is gone, not kept");
    let saved = st.to_json();
    assert!(saved["global"].get("guide.stay_on_guide_overlay").is_none());
    assert!(load(saved).1.is_clean());
}

/// A backup written before US spelling (#139) restores every choice.
#[test]
fn version_3_documents_keep_their_choices_in_us_spelling() {
    let v3 = json!({
        "format": FORMAT,
        "version": 3,
        "global": {
            "guide.show_programmes": false,
            "appearance.accent": "blue_grey",
            "appearance.background": "dark_grey",
            "guide.now_highlight_style": "colour"
        },
        "playlists": {"pl-1": {"guide.show_programmes": false}},
        "groups": {"pl-1": {"grp-1": {"appearance.accent": "grey"}}}
    });
    let (st, report) = load(v3);
    assert_eq!(report.version, 3);
    assert_eq!(report.migrations, [3]);
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    // Both settings are global only: the overrides are kept (renamed) but
    // not applied, as for any override at a level a setting does not allow.
    let rejected: Vec<&Entry> = report.rejected.iter().map(|(e, _)| e).collect();
    assert_eq!(
        rejected,
        [
            &entry(Target::playlist("pl-1"), "guide.show_programs"),
            &entry(Target::group("pl-1", "grp-1"), "appearance.accent"),
        ]
    );
    assert_eq!(st.get("guide.show_programs").unwrap(), Value::Bool(false));
    assert_eq!(st.color(keys::ACCENT, G), "blue_gray");
    assert_eq!(st.color(keys::BACKGROUND, G), "dark_gray");
    assert_eq!(
        st.get("guide.now_highlight_style").unwrap(),
        Value::Enum("color".into())
    );
    let saved = st.to_json();
    assert_eq!(saved["version"], json!(VERSION));
    assert_eq!(
        saved["global"],
        json!({
            "appearance.accent": "blue_gray",
            "appearance.background": "dark_gray",
            "guide.now_highlight_style": "color",
            "guide.show_programs": false
        })
    );
    assert_eq!(
        saved["playlists"]["pl-1"],
        json!({"guide.show_programs": false})
    );
    assert_eq!(
        saved["groups"]["pl-1"]["grp-1"],
        json!({"appearance.accent": "gray"})
    );
    let (again, report) = load(saved);
    assert!(report.migrations.is_empty() && report.unknown.is_empty());
    assert_eq!(again, st);
}

#[test]
fn version_3_keeps_a_value_already_under_the_us_key() {
    let v3 = json!({
        "format": FORMAT,
        "version": 3,
        "global": {"guide.show_programmes": false, "guide.show_programs": true}
    });
    let (st, report) = load(v3);
    assert!(report.unknown.is_empty());
    assert_eq!(st.get("guide.show_programs").unwrap(), Value::Bool(true));
}

#[test]
fn migration_keeps_a_value_already_under_the_new_key() {
    let v1 = json!({
        "format": FORMAT,
        "version": 1,
        "global": {"player.panel_timeout_secs": 9, "player.controls_panel_timeout": 5_000}
    });
    let (st, _) = load(v1);
    assert_eq!(st.duration_ms(keys::CONTROLS_PANEL_TIMEOUT, G), 5_000);
}

#[test]
fn migrated_values_are_validated_like_any_other() {
    // 90 s exceeds the controls timeout's 60 s maximum: clamped after migrating.
    let v1 = json!({"format": FORMAT, "version": 1, "global": {"player.panel_timeout_secs": 90}});
    let (st, report) = load(v1);
    assert_eq!(st.duration_ms(keys::CONTROLS_PANEL_TIMEOUT, G), 60_000);
    assert_eq!(
        report.clamped,
        [entry(Target::Global, "player.controls_panel_timeout")]
    );
}

#[test]
fn newer_documents_load_best_effort() {
    let doc = json!({
        "format": FORMAT,
        "version": VERSION + 5,
        "global": {"guide.autoplay": true, "future.key": 1}
    });
    let (st, report) = load(doc);
    assert!(report.newer);
    assert!(report.migrations.is_empty());
    assert!(st.bool(keys::AUTOPLAY, G));
    assert_eq!(report.unknown, [entry(Target::Global, "future.key")]);
}

#[test]
fn malformed_documents_fail_to_load() {
    let cases = [
        ("not json", LoadError::NotADocument(String::new())),
        ("[1, 2]", LoadError::NotADocument(String::new())),
        (r#"{"version": 2}"#, LoadError::WrongFormat),
        (
            r#"{"format": "other.app", "version": 2}"#,
            LoadError::WrongFormat,
        ),
        (r#"{"format": "iptv-rs.settings"}"#, LoadError::BadVersion),
        (
            r#"{"format": "iptv-rs.settings", "version": 0}"#,
            LoadError::BadVersion,
        ),
        (
            r#"{"format": "iptv-rs.settings", "version": "2"}"#,
            LoadError::BadVersion,
        ),
        (
            r#"{"format": "iptv-rs.settings", "version": 2, "global": []}"#,
            LoadError::BadLayer("global".into()),
        ),
        (
            r#"{"format": "iptv-rs.settings", "version": 2, "groups": {"p": {"g": 1}}}"#,
            LoadError::BadLayer("groups.p.g".into()),
        ),
    ];
    for (text, expected) in cases {
        let err = Settings::from_json(text).expect_err(text);
        match (&err, &expected) {
            (LoadError::NotADocument(_), LoadError::NotADocument(_)) => {}
            _ => assert_eq!(err, expected, "{text}"),
        }
    }
}

#[test]
fn load_errors_say_what_was_wrong() {
    let says = |text: &str| Settings::from_json(text).unwrap_err().to_string();
    assert!(
        says("not json").starts_with("not a settings document: "),
        "{}",
        says("not json")
    );
    assert_eq!(says("[1]"), "not a settings document: not an object");
    assert_eq!(
        says(r#"{"format": "other.app", "version": 2}"#),
        "not an iptv-rs settings document"
    );
    assert_eq!(
        says(r#"{"format": "iptv-rs.settings", "version": -1}"#),
        "missing or invalid version"
    );
    assert_eq!(
        says(r#"{"format": "iptv-rs.settings", "version": 2, "playlists": {"pl-1": 3}}"#),
        "playlists.pl-1 is not an object of settings"
    );
    assert_eq!(
        says(r#"{"format": "iptv-rs.settings", "version": 2, "groups": {"pl-1": []}}"#),
        "groups.pl-1 is not an object of settings"
    );
    // Every version up to the current one has its step today, so this one
    // only shows when a step goes missing.
    assert_eq!(
        LoadError::NoMigration { from: 7 }.to_string(),
        "no migration from version 7"
    );
    let boxed: Box<dyn std::error::Error> = Box::new(LoadError::WrongFormat);
    assert_eq!(boxed.to_string(), "not an iptv-rs settings document");
}

#[test]
fn rejected_entries_say_why() {
    let doc = json!({
        "format": FORMAT,
        "version": VERSION,
        "global": {
            "guide.autoplay": "yes",
            "appearance.font_size": "gigantic",
            "remote.key_overrides": [{"key": "ok", "press": "hold", "action": "play"}]
        },
        "playlists": {"pl-1": {"appearance.accent": "red"}}
    });
    let (_, report) = load(doc);
    let reasons: Vec<(&str, &str)> = report
        .rejected
        .iter()
        .map(|(e, why)| (e.key.as_str(), why.as_str()))
        .collect();
    assert_eq!(
        reasons,
        [
            (
                "appearance.font_size",
                r#"appearance.font_size: "gigantic" is not a valid choice"#
            ),
            ("guide.autoplay", "guide.autoplay: expected bool"),
            (
                "remote.key_overrides",
                "remote.key_overrides: expected key_bindings"
            ),
            (
                "appearance.accent",
                r#"appearance.accent cannot be overridden for playlist "pl-1""#
            ),
        ]
    );
}

#[test]
fn binding_entries_need_every_field_but_the_context() {
    let load_bindings = |list: serde_json::Value| {
        let doc = json!({"format": FORMAT, "version": VERSION,
            "global": {"remote.key_overrides": list}});
        let (st, report) = load(doc);
        (
            st.key_bindings(keys::KEY_OVERRIDES, G),
            report.rejected.len(),
        )
    };
    let everywhere = KeyBinding::new(None, "ok", PressKind::Short, "play");
    for context in [json!(null), json!("absent")] {
        let mut b = json!({"key": "ok", "press": "short", "action": "play"});
        if context.is_null() {
            b["context"] = context;
        }
        assert_eq!(load_bindings(json!([b])), (vec![everywhere.clone()], 0));
    }
    let broken = [
        json!(["ok"]),
        json!([{"context": 5, "key": "ok", "press": "short", "action": "play"}]),
        json!([{"press": "short", "action": "play"}]),
        json!([{"key": "ok", "action": "play"}]),
        json!([{"key": "ok", "press": "short"}]),
        json!([{"key": "ok", "press": "short", "action": 1}]),
    ];
    for list in broken {
        assert_eq!(load_bindings(list.clone()), (vec![], 1), "{list}");
    }
}

#[test]
fn migrations_reach_every_layer() {
    let v1 = json!({
        "format": FORMAT,
        "version": 1,
        "global": {"guide.autoplay": true},
        "playlists": {
            "pl-1": {"player.panel_timeout_secs": 8},
            "pl-2": {"channels.group_sort": "name"}
        },
        "groups": {"pl-1": {"grp-1": {"player.panel_timeout_secs": 6}}}
    });
    let (st, report) = load(v1);
    assert_eq!(report.migrations, [1, 2, 3]);
    // The renamed key is global-only, so the overrides are kept but not
    // applied, under the new name.
    let rejected: Vec<&Entry> = report.rejected.iter().map(|(e, _)| e).collect();
    assert_eq!(
        rejected,
        [
            &entry(Target::playlist("pl-1"), "player.controls_panel_timeout"),
            &entry(
                Target::group("pl-1", "grp-1"),
                "player.controls_panel_timeout"
            ),
        ]
    );
    let saved = st.to_json();
    assert_eq!(
        saved["playlists"]["pl-1"],
        json!({"player.controls_panel_timeout": 8_000})
    );
    assert_eq!(
        saved["groups"]["pl-1"]["grp-1"],
        json!({"player.controls_panel_timeout": 6_000})
    );
    assert_eq!(
        st.choice(keys::GROUP_SORT, &Target::playlist("pl-2")),
        "name"
    );
}

#[test]
fn for_each_layer_visits_every_settings_object() {
    use iptv_settings::backup::for_each_layer;
    let doc = json!({
        "format": FORMAT,
        "global": {"a.global": 1},
        "playlists": {"pl-1": {"a.playlist": 1}, "pl-2": "not a layer"},
        "groups": {"pl-1": {"grp-1": {"a.group": 1}, "grp-2": 3}, "pl-2": []}
    });
    let serde_json::Value::Object(mut doc) = doc else {
        unreachable!()
    };
    let mut seen = Vec::new();
    for_each_layer(&mut doc, &mut |layer| {
        seen.extend(layer.keys().cloned());
        layer.insert("visited".into(), json!(true));
    });
    assert_eq!(seen, ["a.global", "a.playlist", "a.group"]);
    assert_eq!(doc["groups"]["pl-1"]["grp-1"]["visited"], json!(true));
    assert_eq!(doc["playlists"]["pl-2"], json!("not a layer"));
}

#[test]
fn a_missing_layer_is_empty() {
    let (st, report) = load(json!({"format": FORMAT, "version": VERSION}));
    assert!(report.is_clean());
    assert_eq!(st, Settings::new());
}
