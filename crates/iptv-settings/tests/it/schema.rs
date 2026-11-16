//! Integrity of the schema and catalogue.

use std::collections::{BTreeMap, BTreeSet};

use iptv_settings::backup::{MIGRATIONS, VERSION};
use iptv_settings::catalogue::{self, choices};
use iptv_settings::schema::{Effect, Test, ValueType};
use iptv_settings::validate::{check, is_token};
use iptv_settings::{Scope, Section, Settings, Target, Value, keys};

fn all() -> Vec<&'static iptv_settings::Setting> {
    catalogue::settings().collect()
}

#[test]
fn keys_are_unique_across_settings_and_commands() {
    let mut seen = BTreeSet::new();
    for key in all()
        .iter()
        .map(|s| s.key)
        .chain(catalogue::commands().map(|c| c.key))
    {
        assert!(seen.insert(key), "duplicate key {key}");
    }
}

#[test]
fn keys_are_dotted_tokens_under_a_section() {
    for key in all()
        .iter()
        .map(|s| s.key)
        .chain(catalogue::commands().map(|c| c.key))
    {
        let parts: Vec<&str> = key.split('.').collect();
        assert!(parts.len() >= 2, "{key}: needs section.name");
        assert!(
            parts.iter().all(|p| is_token(p)),
            "{key}: segments must be tokens"
        );
        assert!(
            Section::from_key(parts[0]).is_some(),
            "{key}: unknown section prefix"
        );
    }
}

#[test]
fn settings_sit_in_their_sections_table() {
    for section in Section::ALL {
        for s in catalogue::section(section) {
            assert_eq!(s.section, section, "{}", s.key);
        }
    }
}

#[test]
fn defaults_are_valid_for_their_type() {
    for s in all() {
        assert_eq!(check(s, &s.default_value()), Ok(()), "{}", s.key);
    }
}

#[test]
fn numeric_ranges_are_well_formed() {
    for s in all() {
        let (min, max, step) = match s.ty {
            ValueType::Int { min, max, step, .. } => (min, max, step),
            ValueType::Duration {
                min_ms,
                max_ms,
                step_ms,
                ..
            } => (min_ms, max_ms, step_ms),
            _ => continue,
        };
        assert!(step > 0, "{}: step", s.key);
        assert!(min < max, "{}: range", s.key);
        assert_eq!(
            (max - min) % step,
            0,
            "{}: max must be on the step grid",
            s.key
        );
    }
}

#[test]
fn enum_and_colour_defaults_are_members() {
    for s in all() {
        let (choices, default) = match s.ty {
            ValueType::Enum { choices, default } => (choices, default),
            ValueType::Colour { palette, default } => (palette, default),
            _ => continue,
        };
        assert!(choices.contains(default), "{}: {default}", s.key);
    }
}

#[test]
fn choice_families_have_unique_token_variants() {
    for family in choices::ALL {
        assert!(is_token(family.family), "{}", family.family);
        let unique: BTreeSet<_> = family.variants.iter().collect();
        assert_eq!(unique.len(), family.variants.len(), "{}", family.family);
        assert!(
            family.variants.iter().all(|v| is_token(v)),
            "{}",
            family.family
        );
    }
    // Every family a setting uses is registered (so label extraction sees it).
    for s in all() {
        if let Some(c) = s.ty.choices() {
            assert!(
                choices::ALL.iter().any(|f| std::ptr::eq(*f, c)),
                "{}",
                s.key
            );
        }
    }
}

#[test]
fn dependencies_refer_to_existing_settings_of_a_matching_type() {
    for s in all() {
        for dep in s.depends {
            assert!(!dep.any_of.is_empty(), "{}: empty dependency", s.key);
            for w in dep.any_of {
                assert_ne!(w.key, s.key, "{}: depends on itself", s.key);
                let target = catalogue::setting(w.key)
                    .unwrap_or_else(|| panic!("{} depends on unknown {}", s.key, w.key));
                let ok = match (w.test, &target.ty) {
                    (Test::IsTrue, ValueType::Bool { .. }) => true,
                    (Test::EnumIn(vs), ty) => {
                        let c = ty.choices().expect("EnumIn needs an enum or colour");
                        !vs.is_empty() && vs.iter().all(|v| c.contains(v))
                    }
                    (Test::NotEmpty, ValueType::Text { .. }) => true,
                    _ => false,
                };
                assert!(ok, "{} -> {}: test does not fit the type", s.key, w.key);
            }
        }
    }
}

#[test]
fn dependencies_are_acyclic() {
    fn visit(key: &'static str, path: &mut Vec<&'static str>) {
        assert!(!path.contains(&key), "dependency cycle: {path:?} -> {key}");
        path.push(key);
        let s = catalogue::setting(key).unwrap();
        for w in s.depends.iter().flat_map(|d| d.any_of) {
            visit(w.key, path);
        }
        path.pop();
    }
    for s in all() {
        visit(s.key, &mut Vec::new());
    }
}

/// `Settings::is_visible` and `is_enabled` read one level of conditions,
/// so a setting whose condition refers to a setting with conditions of the
/// same effect names those too (the frame-rate option that needs the
/// refresh-rate switch also needs frame-rate matching itself).
#[test]
fn dependencies_repeat_their_parents_conditions() {
    for s in all() {
        for dep in s.depends {
            for w in dep.any_of {
                let parent = catalogue::setting(w.key).unwrap();
                for inherited in parent.depends.iter().filter(|d| d.effect == dep.effect) {
                    assert!(
                        s.depends.contains(inherited),
                        "{} depends on {}, so it needs {:?} too",
                        s.key,
                        parent.key,
                        inherited
                    );
                }
            }
        }
    }
}

#[test]
fn subsections_are_declared_and_used() {
    let mut used: BTreeMap<(Section, &str), usize> = BTreeMap::new();
    for s in all() {
        if let Some(sub) = s.subsection {
            assert!(
                s.section.subsections().contains(&sub),
                "{}: subsection {sub}",
                s.key
            );
            *used.entry((s.section, sub)).or_default() += 1;
        }
    }
    for c in catalogue::commands() {
        if let Some(sub) = c.subsection {
            assert!(
                c.section.subsections().contains(&sub),
                "{}: subsection {sub}",
                c.key
            );
            *used.entry((c.section, sub)).or_default() += 1;
        }
    }
    for section in Section::ALL {
        for sub in section.subsections() {
            assert!(
                used.contains_key(&(section, *sub)),
                "{}.{sub} is empty",
                section.key()
            );
        }
    }
}

#[test]
fn label_keys_are_unique() {
    let mut labels = BTreeSet::new();
    let mut add = |l: String| assert!(labels.insert(l.clone()), "duplicate label key {l}");
    for section in Section::ALL {
        add(section.label_key());
        for sub in section.subsections() {
            add(section.subsection_label_key(sub));
        }
    }
    for s in all() {
        add(s.label_key());
        add(s.help_key());
    }
    for c in catalogue::commands() {
        add(c.label_key());
        add(c.help_key());
    }
    // Choice labels are shared per family by design; just check the scheme.
    assert_eq!(
        choices::SIZE.label_key("medium"),
        "settings.choice.size.medium"
    );
}

#[test]
fn summaries_are_present() {
    for s in all() {
        assert!(!s.summary.trim().is_empty(), "{}", s.key);
    }
    for c in catalogue::commands() {
        assert!(!c.summary.trim().is_empty(), "{}", c.key);
    }
}

#[test]
fn commands_reset_existing_settings() {
    for c in catalogue::commands() {
        for key in c.resets {
            assert!(
                catalogue::setting(key).is_some(),
                "{} resets unknown {key}",
                c.key
            );
        }
    }
}

#[test]
fn typed_keys_match_the_schema() {
    for (key, ty) in keys::ALL {
        let s = catalogue::setting(key).unwrap_or_else(|| panic!("typed key {key} missing"));
        assert_eq!(s.ty.type_name(), *ty, "{key}");
    }
    // Every typed getter works on defaults.
    let st = Settings::new();
    let g = &Target::Global;
    st.duration_ms(keys::LONG_PRESS, g);
    st.duration_ms(keys::DIGIT_TIMEOUT, g);
    st.key_bindings(keys::KEY_OVERRIDES, g);
    st.duration_ms(keys::INFO_PANEL_TIMEOUT, g);
    st.duration_ms(keys::CONTROLS_PANEL_TIMEOUT, g);
    st.duration_ms(keys::SWITCH_DELAY, g);
    st.duration_ms(keys::FULLSCREEN_TIMEOUT, g);
    st.bool(keys::AUTOPLAY, g);
    st.choice(keys::BROWSE_MODE, g);
    st.colour(keys::ACCENT, g);
    st.colour(keys::BACKGROUND, g);
    st.choice(keys::FONT_SIZE, g);
    st.choice(keys::CHANNEL_SORT, g);
    st.choice(keys::GROUP_SORT, g);
    st.int(keys::RECENT_COUNT, g);
    st.text(keys::USER_AGENT, g);
}

#[test]
fn input_and_appearance_defaults_match_the_other_crates() {
    let st = Settings::new();
    let g = &Target::Global;
    assert_eq!(st.duration_ms(keys::LONG_PRESS, g), 400); // iptv-nav DEFAULT_LONG_PRESS_MS
    assert_eq!(st.duration_ms(keys::INFO_PANEL_TIMEOUT, g), 4_000); // iptv-core Timeouts
    assert_eq!(st.duration_ms(keys::CONTROLS_PANEL_TIMEOUT, g), 7_000);
    assert_eq!(st.colour(keys::ACCENT, g), "blue"); // theme.rs default: Blue on Dark
    assert_eq!(st.colour(keys::BACKGROUND, g), "dark");
    assert_eq!(st.get("general.user_agent"), Ok(Value::text("")));
}

/// The palette keys mirror the bundle names in iptv-app's Patina
/// theme (read at compile time; no dependency on the NSH crate).
#[test]
fn palettes_mirror_the_theme_names() {
    let theme = include_str!("../../../iptv-app/src/theme.rs");
    let names = |ctor: &str| -> Vec<String> {
        theme
            .lines()
            .filter_map(|l| l.trim().strip_prefix(ctor))
            .filter_map(|rest| rest.split('"').next())
            .map(|n| n.to_lowercase().replace(' ', "_"))
            .collect()
    };
    assert_eq!(names("a(key(\""), choices::ACCENT.variants, "accents");
    assert_eq!(
        names("b(key(\""),
        choices::BACKGROUND.variants,
        "backgrounds"
    );
}

#[test]
fn migrations_are_contiguous_up_to_the_current_version() {
    let froms: Vec<u32> = MIGRATIONS.iter().map(|m| m.from).collect();
    let expected: Vec<u32> = (1..VERSION).collect();
    assert_eq!(froms, expected);
}

#[test]
fn every_scope_and_effect_is_exercised() {
    let scopes: BTreeSet<Scope> = all().iter().map(|s| s.scope).collect();
    assert_eq!(scopes.len(), 3);
    let effects: Vec<Effect> = all()
        .iter()
        .flat_map(|s| s.depends)
        .map(|d| d.effect)
        .collect();
    assert!(effects.contains(&Effect::Visible) && effects.contains(&Effect::Enabled));
    assert!(all().iter().any(|s| s.flags.requires_restart));
    assert!(all().iter().any(|s| s.flags.advanced));
}

#[test]
fn tree_lists_every_setting_once() {
    let count = |advanced: bool| -> Vec<&str> {
        catalogue::tree(advanced)
            .iter()
            .flat_map(|sec| &sec.groups)
            .flat_map(|g| g.settings.iter().map(|s| s.key))
            .collect()
    };
    let full = count(true);
    let unique: BTreeSet<_> = full.iter().collect();
    assert_eq!(full.len(), all().len());
    assert_eq!(unique.len(), full.len());
    let basic = count(false);
    assert_eq!(
        basic.len(),
        all().iter().filter(|s| !s.flags.advanced).count()
    );
    let commands: usize = catalogue::tree(false)
        .iter()
        .flat_map(|sec| &sec.groups)
        .map(|g| g.commands.len())
        .sum();
    assert_eq!(commands, catalogue::commands().count());
    // Groups follow the declared subsection order, unsectioned first.
    for sec in catalogue::tree(true) {
        let order: Vec<Option<&str>> = sec.groups.iter().map(|g| g.subsection).collect();
        let declared: Vec<Option<&str>> = std::iter::once(None)
            .chain(sec.section.subsections().iter().copied().map(Some))
            .filter(|s| order.contains(s))
            .collect();
        assert_eq!(order, declared, "{}", sec.section.key());
    }
}

#[test]
fn time_constructors_convert_their_unit_to_milliseconds() {
    use iptv_settings::schema::{TimeUnit, days, dur, hours, mins, offset_hm, offset_hms, secs};
    let ms = |ty: ValueType| match ty {
        ValueType::Duration {
            min_ms,
            max_ms,
            step_ms,
            unit,
            default_ms,
        } => (unit, [min_ms, max_ms, step_ms, default_ms]),
        other => panic!("not a duration: {other:?}"),
    };
    assert_eq!(
        ms(dur(TimeUnit::Millis, -20, 40, 10, 0)),
        (TimeUnit::Millis, [-20, 40, 10, 0])
    );
    assert_eq!(
        ms(secs(1, 30, 1, 4)),
        (TimeUnit::Seconds, [1_000, 30_000, 1_000, 4_000])
    );
    assert_eq!(
        ms(mins(0, 2, 1, 1)),
        (TimeUnit::Minutes, [0, 120_000, 60_000, 60_000])
    );
    assert_eq!(
        ms(hours(0, 2, 1, 1)),
        (TimeUnit::Hours, [0, 7_200_000, 3_600_000, 3_600_000])
    );
    assert_eq!(
        ms(days(1, 2, 1, 2)),
        (
            TimeUnit::Days,
            [86_400_000, 172_800_000, 86_400_000, 172_800_000]
        )
    );
    // Offsets are signed.
    assert_eq!(
        ms(offset_hm(-90, 90, 15, -30)),
        (
            TimeUnit::HoursMinutes,
            [-5_400_000, 5_400_000, 900_000, -1_800_000]
        )
    );
    assert_eq!(
        ms(offset_hms(-60, 60, 1, 5)),
        (
            TimeUnit::HoursMinutesSeconds,
            [-60_000, 60_000, 1_000, 5_000]
        )
    );
}

#[test]
fn types_name_themselves_and_give_their_default() {
    use iptv_settings::schema::{
        IntUnit, Setting, TextKind, bindings, boolean, choice, colour, int, secs, text,
    };
    let cases = [
        (boolean(true), "bool", Value::Bool(true)),
        (int(0, 10, 2, IntUnit::Percent, 4), "integer", Value::Int(4)),
        (secs(0, 9, 1, 3), "duration", Value::ms(3_000)),
        (
            choice(&choices::SIZE, "large"),
            "enum",
            Value::choice("large"),
        ),
        (text(TextKind::Plain, 8, "abc"), "text", Value::text("abc")),
        (
            colour(&choices::ACCENT, "red"),
            "colour",
            Value::colour("red"),
        ),
        (bindings(4), "key_bindings", Value::KeyBindings(vec![])),
    ];
    for (ty, name, default) in cases {
        assert_eq!(ty.type_name(), name);
        let s = Setting::new(Section::Search, "", "search.example", ty, "Synthetic.");
        assert_eq!(s.default_value(), default, "{name}");
        assert_eq!(s.default_value().type_name(), name, "value and type agree");
        // Choices exist exactly for enums and colours.
        assert_eq!(ty.choices().is_some(), matches!(name, "enum" | "colour"));
    }
    assert_eq!(
        choice(&choices::SIZE, "large").choices(),
        Some(&choices::SIZE)
    );
    assert_eq!(
        colour(&choices::ACCENT, "red").choices(),
        Some(&choices::ACCENT)
    );
}

#[test]
fn a_new_setting_is_plain_until_the_builders_add_to_it() {
    use iptv_settings::schema::{
        Dependency, Flags, Setting, When, boolean, enabled_if, visible_if, when,
    };
    static SHOWN: [When; 1] = [When {
        key: "search.show_history",
        test: Test::IsTrue,
    }];
    let plain = Setting::new(Section::Search, "", "search.example", boolean(false), "S.");
    assert_eq!(plain.section, Section::Search);
    assert_eq!(plain.subsection, None, "an empty subsection is none");
    assert_eq!(plain.scope, Scope::Global);
    assert_eq!(plain.flags, Flags::default());
    assert!(plain.depends.is_empty());
    assert_eq!(plain.label_key(), "settings.search.example");
    assert_eq!(plain.help_key(), "settings.search.example.help");

    assert_eq!(when("search.show_history", Test::IsTrue), SHOWN[0]);
    let visible = visible_if(&SHOWN);
    let enabled = enabled_if(&SHOWN);
    assert_eq!(
        (visible, enabled),
        (
            Dependency {
                effect: Effect::Visible,
                any_of: &SHOWN
            },
            Dependency {
                effect: Effect::Enabled,
                any_of: &SHOWN
            }
        )
    );

    let built = Setting::new(
        Section::Player,
        "clock",
        "player.example",
        boolean(true),
        "S.",
    )
    .scope(Scope::Group)
    .restart()
    .advanced()
    .depends(&[]);
    assert_eq!(built.subsection, Some("clock"));
    assert_eq!(built.scope, Scope::Group);
    assert!(built.flags.requires_restart && built.flags.advanced);
    // Each builder sets only its own part.
    let restart =
        Setting::new(Section::Player, "", "player.example", boolean(true), "S.").restart();
    assert!(restart.flags.requires_restart && !restart.flags.advanced);
    assert_eq!(restart.scope, Scope::Global);
}

#[test]
fn sections_and_choices_resolve_their_keys() {
    for section in Section::ALL {
        assert_eq!(Section::from_key(section.key()), Some(section));
        assert_eq!(
            section.label_key(),
            format!("settings.section.{}", section.key())
        );
    }
    assert_eq!(Section::from_key("Player"), None, "keys are lower case");
    assert_eq!(Section::from_key("nonexistent"), None);
    assert_eq!(
        Section::Player.subsection_label_key("clock"),
        "settings.section.player.clock"
    );

    let size = &choices::SIZE;
    assert!(size.contains("large") && !size.contains("huge"));
    // Interning hands back the schema's own copy of a member.
    let owned = String::from("large");
    let interned = size.intern(&owned).unwrap();
    assert_eq!(interned, "large");
    assert!(size.variants.iter().any(|v| std::ptr::eq(*v, interned)));
    assert_eq!(size.intern("huge"), None);
    assert_eq!(
        choices::ACCENT.label_key("red"),
        "settings.choice.accent.red"
    );
}
