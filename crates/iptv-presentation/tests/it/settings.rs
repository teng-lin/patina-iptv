//! The settings screen's state machine.

use iptv_presentation::settings::screen::{
    Ctx, Editor, Effect, Key, Offer, Page, Pane, Part, Row, Screen, Status, grid, rows,
};
use iptv_settings::{Section, Settings, Value};

static PAGES: &[Page] = &[
    Page {
        key: "general",
        parts: &[Part::Section(Section::General)],
    },
    Page {
        key: "playlists",
        parts: &[Part::Keys(&["playlists.update_all"])],
    },
    Page {
        key: "epg",
        parts: &[Part::Section(Section::Epg)],
    },
    Page {
        key: "appearance",
        parts: &[
            Part::Section(Section::Appearance),
            Part::Section(Section::Guide),
        ],
    },
    Page {
        key: "playback",
        parts: &[
            Part::Section(Section::Player),
            Part::Section(Section::Catchup),
        ],
    },
    Page {
        key: "remote",
        parts: &[Part::Section(Section::Remote)],
    },
    Page {
        key: "other",
        parts: &[Part::Section(Section::Search), Part::Section(Section::Vod)],
    },
];

/// What the app offers (a sample of each kind and value type).
fn offer(key: &str) -> Offer {
    match key {
        "general.resume_last_channel"
        | "epg.update_interval"
        | "epg.keep_past"
        | "epg.update_now"
        | "epg.clear"
        | "appearance.accent"
        | "appearance.ui_transparency"
        | "catchup.seek_keys"
        | "catchup.forward_step"
        | "guide.highlight_now"
        | "guide.now_highlight_style"
        | "player.clock"
        | "player.clock_date"
        | "remote.customize"
        | "remote.key_overrides"
        | "search.favorites_first" => Offer::Active,
        "general.about" => Offer::ReadOnly,
        "general.start_on_boot" | "general.start_on_wake" | "search.prefer_voice" => Offer::Hidden,
        k if k.starts_with("vod.") => Offer::Later,
        _ => Offer::NotYet,
    }
}

fn everything(_: &str) -> Offer {
    Offer::Active
}

fn ctx(settings: &Settings) -> Ctx<'_> {
    Ctx {
        settings,
        offer: &offer,
    }
}

fn page(key: &str) -> &'static Page {
    PAGES.iter().find(|p| p.key == key).unwrap()
}

fn press(screen: &mut Screen, settings: &Settings, keys: &[Key]) -> Effect {
    let mut last = Effect::None;
    for k in keys {
        last = screen.key(*k, &ctx(settings));
    }
    last
}

/// Applies a `Set` effect like the app does.
fn apply(settings: &mut Settings, effect: Effect) {
    match effect {
        Effect::Set { key, value } => {
            settings.set(key, value).unwrap();
        }
        other => panic!("expected a value to store, got {other:?}"),
    }
}

fn keys_of(rows: &[Row]) -> Vec<&'static str> {
    rows.iter().filter_map(Row::key).collect()
}

fn row(settings: &Settings, page_key: &str, key: &str) -> Row {
    rows(page(page_key), &ctx(settings))
        .into_iter()
        .find(|r| r.key() == Some(key))
        .unwrap()
}

#[test]
fn pages_choose_and_back_closes() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "nothing");
    assert_eq!((screen.page().key, screen.pane()), ("general", Pane::Pages));
    press(&mut screen, &s, &[Key::Up]);
    assert_eq!(screen.page().key, "general", "no wrap at the top");
    press(&mut screen, &s, &[Key::Down, Key::Down]);
    assert_eq!(screen.page().key, "epg");
    press(&mut screen, &s, &[Key::PageDown, Key::PageDown]);
    assert_eq!(screen.page().key, "other", "stops at the last");
    assert_eq!(press(&mut screen, &s, &[Key::Back]), Effect::Close);
    assert_eq!(Screen::new(PAGES, "remote").page_index(), 5);
}

#[test]
fn rows_leave_out_what_is_not_offered() {
    let s = Settings::new();
    let general = keys_of(&rows(page("general"), &ctx(&s)));
    assert!(!general.contains(&"general.start_on_boot"));
    assert!(general.contains(&"general.confirm_exit"));
    assert!(
        !general.contains(&"general.confirm_exit_window"),
        "advanced"
    );
    let other = keys_of(&rows(page("other"), &ctx(&s)));
    assert!(!other.contains(&"search.prefer_voice"));
    // Named rows are shown in their order.
    assert_eq!(
        keys_of(&rows(page("playlists"), &ctx(&s))),
        ["playlists.update_all"]
    );
}

#[test]
fn headings_name_subsections_and_sections_of_shared_pages() {
    let s = Settings::new();
    let epg = rows(page("epg"), &ctx(&s));
    assert!(
        matches!(epg[0], Row::Command(_)),
        "no heading without a subsection"
    );
    assert!(epg.contains(&Row::Heading {
        section: Section::Epg,
        subsection: Some("updates")
    }));
    // Search has no subsections: on a shared page its name heads its rows.
    let other = rows(page("other"), &ctx(&s));
    assert_eq!(
        other[0],
        Row::Heading {
            section: Section::Search,
            subsection: None
        }
    );
}

#[test]
fn statuses_follow_the_offer() {
    let mut s = Settings::new();
    let c = ctx(&s);
    assert_eq!(c.status(&row(&s, "epg", "epg.keep_past")), Status::Active);
    assert_eq!(c.status(&row(&s, "epg", "epg.time_offset")), Status::NotYet);
    assert_eq!(c.status(&row(&s, "other", "vod.sort")), Status::Later);
    assert_eq!(
        c.status(&row(&s, "general", "general.about")),
        Status::ReadOnly
    );
    // An active key-binding list has no editor: read-only.
    let bindings = row(&s, "remote", "remote.key_overrides");
    assert_eq!(c.status(&bindings), Status::ReadOnly);
    // Switched off by another setting.
    s.set("player.clock", Value::Bool(false)).unwrap();
    let date = row(&s, "playback", "player.clock_date");
    assert_eq!(ctx(&s).status(&date), Status::Disabled);
    let mut screen = Screen::new(PAGES, "playback");
    assert_eq!(screen.tap_row("player.clock_date", &ctx(&s)), Effect::None);
    assert_eq!(screen.editor(), None);
}

#[test]
fn entering_the_rows_focuses_the_first_row_the_app_acts_on() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "general");
    // General starts with commands the app cannot run yet.
    assert!(matches!(
        rows(page("general"), &ctx(&s))[0],
        Row::Command(_)
    ));
    press(&mut screen, &s, &[Key::Right]);
    assert_eq!(screen.pane(), Pane::Rows);
    assert_eq!(screen.focus(), Some("general.resume_last_channel"));
    // Left goes back to the pages; Right returns to the same row.
    press(&mut screen, &s, &[Key::Down, Key::Left]);
    assert_eq!(screen.pane(), Pane::Pages);
    press(&mut screen, &s, &[Key::Ok]);
    assert_eq!(screen.focus(), Some("general.confirm_exit"));
    // A page without any row the app acts on: its first row.
    let mut screen = Screen::new(PAGES, "playlists");
    press(&mut screen, &s, &[Key::Ok]);
    assert_eq!(screen.focus(), Some("playlists.update_all"));
}

#[test]
fn rows_skip_headings_and_stop_at_the_ends() {
    let s = Settings::new();
    let keys = keys_of(&rows(page("epg"), &ctx(&s)));
    let mut screen = Screen::new(PAGES, "epg");
    press(&mut screen, &s, &[Key::Right]);
    assert_eq!(screen.focus(), Some("epg.update_interval"));
    press(&mut screen, &s, &[Key::PageUp, Key::Up]);
    assert_eq!(screen.focus(), Some(keys[0]));
    press(&mut screen, &s, &[Key::PageDown; 10]);
    assert_eq!(screen.focus(), keys.last().copied());
    assert_eq!(press(&mut screen, &s, &[Key::Down]), Effect::None);
    assert_eq!(screen.focus(), keys.last().copied());
}

#[test]
fn ok_flips_a_switch() {
    let mut s = Settings::new();
    let mut screen = Screen::new(PAGES, "general");
    let effect = press(&mut screen, &s, &[Key::Right, Key::Ok]);
    assert_eq!(
        effect,
        Effect::Set {
            key: "general.resume_last_channel",
            value: Value::Bool(false)
        }
    );
    apply(&mut s, effect);
    assert_eq!(screen.editor(), None);
    assert_eq!(
        press(&mut screen, &s, &[Key::Ok]),
        Effect::Set {
            key: "general.resume_last_channel",
            value: Value::Bool(true)
        }
    );
}

#[test]
fn a_choice_opens_a_list_on_the_current_value() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "playback");
    screen.tap_row("catchup.seek_keys", &ctx(&s));
    let Some(Editor::Choice {
        focus, selected, ..
    }) = screen.editor().cloned()
    else {
        panic!("a choice list opens");
    };
    assert_eq!((focus, selected), (0, 0), "left_right is the default");
    // Left/Right do nothing in the list; Up stops at the top.
    press(
        &mut screen,
        &s,
        &[Key::Left, Key::Right, Key::Up, Key::Down],
    );
    assert_eq!(
        press(&mut screen, &s, &[Key::Ok]),
        Effect::Set {
            key: "catchup.seek_keys",
            value: Value::choice("down_up")
        }
    );
    assert_eq!(screen.editor(), None);
    // Back closes the list without a change.
    press(&mut screen, &s, &[Key::Ok, Key::Down]);
    assert_eq!(press(&mut screen, &s, &[Key::Back]), Effect::None);
    assert_eq!((screen.editor(), screen.pane()), (None, Pane::Rows));
}

#[test]
fn colors_are_picked_as_colors() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "appearance");
    assert_eq!(screen.tap_row("appearance.accent", &ctx(&s)), Effect::None);
    assert_eq!(
        screen.pick(13),
        Effect::Set {
            key: "appearance.accent",
            value: Value::color("red")
        }
    );
}

#[test]
fn a_stepper_moves_in_steps_within_range_and_keeps_on_ok() {
    let mut s = Settings::new();
    let mut screen = Screen::new(PAGES, "epg");
    press(&mut screen, &s, &[Key::Right, Key::Ok]);
    let hour = 3_600_000;
    let value = |screen: &Screen| match screen.editor() {
        Some(Editor::Number { value, .. }) => *value,
        other => panic!("stepper expected, got {other:?}"),
    };
    assert_eq!(value(&screen), 24 * hour);
    press(&mut screen, &s, &[Key::Right, Key::Up]);
    assert_eq!(value(&screen), 26 * hour);
    press(
        &mut screen,
        &s,
        &[Key::PageDown, Key::PageDown, Key::PageDown],
    );
    assert_eq!(value(&screen), 0, "stops at the minimum");
    screen.step(-1);
    assert_eq!(value(&screen), 0);
    for _ in 0..20 {
        screen.step(10);
    }
    assert_eq!(value(&screen), 168 * hour, "stops at the maximum");
    let effect = press(&mut screen, &s, &[Key::Left, Key::Ok]);
    assert_eq!(
        effect,
        Effect::Set {
            key: "epg.update_interval",
            value: Value::Duration(167 * hour)
        }
    );
    apply(&mut s, effect);
    // Back cancels: nothing to store, the old value stays.
    press(&mut screen, &s, &[Key::Ok, Key::Right]);
    assert_eq!(press(&mut screen, &s, &[Key::Back]), Effect::None);
    assert_eq!(screen.editor(), None);
    assert_eq!(
        s.get("epg.update_interval"),
        Ok(Value::Duration(167 * hour))
    );
}

#[test]
fn accepting_automatic_connections_preserves_absence_but_explicit_limits_are_saved() {
    static CONNECTIONS: &[Page] = &[Page {
        key: "connections",
        parts: &[Part::Keys(&["playlists.max_connections"])],
    }];
    let mut settings = Settings::new();
    fn context(settings: &Settings) -> Ctx<'_> {
        Ctx {
            settings,
            offer: &everything,
        }
    }
    let mut screen = Screen::new(CONNECTIONS, "connections");
    screen.tap_row("playlists.max_connections", &context(&settings));
    assert_eq!(screen.commit(), Effect::None);
    assert!(screen.editor().is_none());
    assert!(
        settings
            .stored("playlists.max_connections", &iptv_settings::Target::Global)
            .is_none()
    );
    // Either choosing the lower boundary or returning to it after stepping up
    // deliberately selects one; it must not silently retain Automatic.
    for steps in [&[-1][..], &[1, -1][..]] {
        screen.tap_row("playlists.max_connections", &context(&settings));
        for &by in steps {
            screen.step(by);
        }
        let selected = screen.commit();
        assert_eq!(
            selected,
            Effect::Set {
                key: "playlists.max_connections",
                value: Value::Int(1)
            }
        );
        let mut explicit = settings.clone();
        apply(&mut explicit, selected);
        assert_eq!(
            explicit.stored("playlists.max_connections", &iptv_settings::Target::Global),
            Some(&Value::Int(1))
        );
    }
    screen.tap_row("playlists.max_connections", &context(&settings));
    screen.step(1);
    let changed = screen.commit();
    assert_eq!(
        changed,
        Effect::Set {
            key: "playlists.max_connections",
            value: Value::Int(2)
        }
    );
    apply(&mut settings, changed);
    screen.tap_row("playlists.max_connections", &context(&settings));
    assert_eq!(
        screen.commit(),
        Effect::Set {
            key: "playlists.max_connections",
            value: Value::Int(2)
        }
    );
    // A previously explicit one must also remain explicit when accepted unchanged.
    settings
        .set("playlists.max_connections", Value::Int(1))
        .unwrap();
    screen.tap_row("playlists.max_connections", &context(&settings));
    assert_eq!(
        screen.commit(),
        Effect::Set {
            key: "playlists.max_connections",
            value: Value::Int(1)
        }
    );
}

#[test]
fn integers_step_as_integers() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "appearance");
    screen.tap_row("appearance.ui_transparency", &ctx(&s));
    screen.step(1);
    assert_eq!(
        screen.commit(),
        Effect::Set {
            key: "appearance.ui_transparency",
            value: Value::Int(25)
        }
    );
}

#[test]
fn rows_the_app_does_not_act_on_do_nothing() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "general");
    let c = ctx(&s);
    assert_eq!(screen.tap_row("general.language", &c), Effect::None);
    assert_eq!(screen.focus(), Some("general.language"));
    assert_eq!(screen.editor(), None);
    assert_eq!(screen.tap_row("general.backup", &c), Effect::None);
    assert_eq!(
        screen.tap_row("general.about", &c),
        Effect::None,
        "read-only"
    );
    let mut screen = Screen::new(PAGES, "other");
    assert_eq!(screen.tap_row("vod.sort", &c), Effect::None, "later");
}

#[test]
fn commands_run_after_a_confirmation_when_asked() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "epg");
    assert_eq!(
        screen.tap_row("epg.update_now", &ctx(&s)),
        Effect::Run("epg.update_now")
    );
    // Clearing the guide asks first; Back keeps everything.
    assert_eq!(screen.tap_row("epg.clear", &ctx(&s)), Effect::None);
    assert!(matches!(screen.editor(), Some(Editor::Confirm { .. })));
    assert_eq!(
        press(&mut screen, &s, &[Key::Left, Key::Back]),
        Effect::None
    );
    assert_eq!(screen.editor(), None);
    screen.tap_row("epg.clear", &ctx(&s));
    assert_eq!(press(&mut screen, &s, &[Key::Ok]), Effect::Run("epg.clear"));
    assert_eq!(screen.editor(), None);
}

#[test]
fn the_remote_bindings_command_runs_without_an_editor_of_its_own() {
    // The app opens its own bindings editor for it.
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "remote");
    assert_eq!(
        screen.tap_row("remote.customize", &ctx(&s)),
        Effect::Run("remote.customize")
    );
    assert_eq!((screen.editor(), screen.pane()), (None, Pane::Rows));
}

#[test]
fn focus_follows_the_row_not_its_position() {
    let mut s = Settings::new();
    s.set("guide.highlight_now", Value::Bool(false)).unwrap();
    let mut screen = Screen::new(PAGES, "appearance");
    screen.tap_row("guide.now_line_full_height", &ctx(&s));
    // A row appears above the focused one.
    s.set("guide.highlight_now", Value::Bool(true)).unwrap();
    assert_eq!(screen.focus(), Some("guide.now_line_full_height"));
    press(&mut screen, &s, &[Key::Up]);
    assert_eq!(screen.focus(), Some("guide.now_highlight_style"));
    // The focused row disappears: movement starts from the first row.
    s.set("guide.highlight_now", Value::Bool(false)).unwrap();
    press(&mut screen, &s, &[Key::Down]);
    let keys = keys_of(&rows(page("appearance"), &ctx(&s)));
    assert_eq!(screen.focus(), Some(keys[1]));
}

#[test]
fn text_is_kept_only_when_valid() {
    let s = Settings::new();
    let c = Ctx {
        settings: &s,
        offer: &everything,
    };
    let mut screen = Screen::new(PAGES, "general");
    assert_eq!(screen.tap_row("general.user_agent", &c), Effect::None);
    screen.set_text("SyntheticPlayer/1.0");
    assert_eq!(
        screen.key(Key::Ok, &c),
        Effect::Set {
            key: "general.user_agent",
            value: Value::text("SyntheticPlayer/1.0")
        }
    );
    screen.tap_row("general.user_agent", &c);
    screen.set_text(&"x".repeat(513));
    assert_eq!(screen.commit(), Effect::None);
    assert!(matches!(
        screen.editor(),
        Some(Editor::Text { invalid: true, .. })
    ));
    screen.set_text("SyntheticPlayer/2.0");
    assert!(matches!(
        screen.editor(),
        Some(Editor::Text { invalid: false, .. })
    ));
    assert_eq!(
        screen.commit(),
        Effect::Set {
            key: "general.user_agent",
            value: Value::text("SyntheticPlayer/2.0")
        }
    );
}

#[test]
fn headings_are_never_acted_on() {
    let s = Settings::new();
    let heading = Row::Heading {
        section: Section::Epg,
        subsection: Some("updates"),
    };
    assert_eq!(heading.key(), None);
    assert_eq!(ctx(&s).status(&heading), Status::ReadOnly);
}

#[test]
fn only_numbers_have_a_grid() {
    let grid_of = |key: &str| grid(&iptv_settings::catalog::setting(key).unwrap().ty);
    assert_eq!(grid_of("appearance.ui_transparency"), Some((0, 80, 5)));
    assert_eq!(
        grid_of("epg.time_offset"),
        Some((-720 * 60_000, 840 * 60_000, 15 * 60_000))
    );
    for key in [
        "guide.autoplay",
        "appearance.font_size",
        "appearance.accent",
        "general.user_agent",
        "remote.key_overrides",
    ] {
        assert_eq!(grid_of(key), None, "{key}");
    }
}

#[test]
fn editors_name_what_they_edit() {
    let s = Settings::new();
    let c = Ctx {
        settings: &s,
        offer: &everything,
    };
    let mut screen = Screen::new(PAGES, "playback");
    let opened = |screen: &mut Screen, key: &str| {
        screen.tap_row(key, &c);
        let editor = screen.editor().cloned().expect("an editor opens");
        (editor.key(), editor.setting().map(|s| s.key))
    };
    for key in ["catchup.seek_keys", "catchup.forward_step"] {
        assert_eq!(opened(&mut screen, key), (key, Some(key)));
    }
    let mut screen = Screen::new(PAGES, "general");
    assert_eq!(
        opened(&mut screen, "general.user_agent"),
        ("general.user_agent", Some("general.user_agent"))
    );
    assert_eq!(
        opened(&mut screen, "general.restore"),
        ("general.restore", None),
        "a confirmation edits no setting"
    );
}

#[test]
fn the_focused_row_is_only_in_the_rows_pane() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "epg");
    assert_eq!(screen.pages().len(), PAGES.len());
    assert_eq!(screen.focused_row(&ctx(&s)), None);
    press(&mut screen, &s, &[Key::Right]);
    assert_eq!(
        screen.focused_row(&ctx(&s)).and_then(|r| r.key()),
        Some("epg.update_interval")
    );
    // Back in the pages, the row is remembered but not focused.
    press(&mut screen, &s, &[Key::Back]);
    assert_eq!(screen.focus(), Some("epg.update_interval"));
    assert_eq!(screen.focused_row(&ctx(&s)), None);
}

#[test]
fn page_keys_and_side_keys_in_the_pages_list() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "other");
    press(&mut screen, &s, &[Key::PageUp]);
    assert_eq!(screen.page().key, "playlists", "five up");
    press(&mut screen, &s, &[Key::PageUp]);
    assert_eq!(screen.page().key, "general", "stops at the first");
    // Left has nowhere to go from the pages.
    assert_eq!(press(&mut screen, &s, &[Key::Left]), Effect::None);
    assert_eq!((screen.page().key, screen.pane()), ("general", Pane::Pages));
    // Right in the rows does nothing either.
    press(&mut screen, &s, &[Key::Right, Key::Right]);
    assert_eq!(
        (screen.pane(), screen.focus()),
        (Pane::Rows, Some("general.resume_last_channel"))
    );
}

/// Pages whose rows can be empty.
static SPARSE: &[Page] = &[
    Page {
        key: "nothing",
        parts: &[Part::Keys(&["no.such_key"])],
    },
    Page {
        key: "logos",
        parts: &[Part::Section(Section::Logos)],
    },
];

fn only_inexact_match(key: &str) -> Offer {
    if key == "logos.inexact_match" {
        Offer::Active
    } else {
        Offer::Hidden
    }
}

#[test]
fn a_page_without_rows_cannot_be_entered() {
    let s = Settings::new();
    let c = Ctx {
        settings: &s,
        offer: &only_inexact_match,
    };
    for page in SPARSE {
        assert_eq!(rows(page, &c), [], "{}", page.key);
        let mut screen = Screen::new(SPARSE, page.key);
        assert_eq!(screen.key(Key::Ok, &c), Effect::None);
        assert_eq!((screen.pane(), screen.focus()), (Pane::Pages, None));
    }
}

#[test]
fn the_rows_pane_closes_when_its_last_row_hides() {
    let mut s = Settings::new();
    s.set("logos.folder", Value::text("/storage/example"))
        .unwrap();
    let mut screen = Screen::new(SPARSE, "logos");
    let c = Ctx {
        settings: &s,
        offer: &only_inexact_match,
    };
    screen.key(Key::Ok, &c);
    assert_eq!(screen.focus(), Some("logos.inexact_match"));
    s.set("logos.folder", Value::text("")).unwrap();
    let c = Ctx {
        settings: &s,
        offer: &only_inexact_match,
    };
    assert_eq!(screen.key(Key::Down, &c), Effect::None);
    assert_eq!(screen.pane(), Pane::Pages);
}

#[test]
fn page_keys_move_further_in_editors() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "appearance");
    // appearance.accent: blue is variant 2 of the palette.
    screen.tap_row("appearance.accent", &ctx(&s));
    press(
        &mut screen,
        &s,
        &[Key::PageDown, Key::PageDown, Key::PageUp],
    );
    assert!(matches!(
        screen.editor(),
        Some(Editor::Choice {
            focus: 7,
            selected: 2,
            ..
        })
    ));
    // The stepper: Page Up is ten steps more, Page Down ten less.
    screen.tap_row("appearance.ui_transparency", &ctx(&s));
    press(&mut screen, &s, &[Key::PageUp]);
    assert!(matches!(
        screen.editor(),
        Some(Editor::Number { value: 70, .. })
    ));
    press(&mut screen, &s, &[Key::PageDown, Key::PageUp]);
    assert!(matches!(
        screen.editor(),
        Some(Editor::Number { value: 70, .. })
    ));
}

#[test]
fn ok_keeps_whatever_the_editor_shows() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "playback");
    screen.tap_row("catchup.seek_keys", &ctx(&s));
    press(&mut screen, &s, &[Key::Down, Key::Down]);
    assert_eq!(
        screen.commit(),
        Effect::Set {
            key: "catchup.seek_keys",
            value: Value::choice("transport")
        }
    );
    // Nothing open: OK does nothing.
    assert_eq!(screen.commit(), Effect::None);
    assert_eq!(screen.editor(), None);
}

#[test]
fn picking_needs_an_open_list_and_an_entry() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "playback");
    assert_eq!(screen.pick(0), Effect::None, "no list open");
    screen.tap_row("catchup.seek_keys", &ctx(&s));
    let open = screen.editor().cloned();
    assert_eq!(screen.pick(99), Effect::None);
    assert_eq!(screen.editor().cloned(), open, "the list stays open");
    // A stepper is no list either.
    screen.tap_row("catchup.forward_step", &ctx(&s));
    assert_eq!(screen.pick(0), Effect::None);
    assert!(matches!(screen.editor(), Some(Editor::Number { .. })));
}

#[test]
fn tapping_a_row_that_is_not_shown_does_nothing() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "general");
    press(&mut screen, &s, &[Key::Right]);
    let before = screen.clone();
    for key in ["general.start_on_boot", "guide.autoplay", "no.such_key"] {
        assert_eq!(screen.tap_row(key, &ctx(&s)), Effect::None, "{key}");
        assert_eq!(screen, before, "{key}");
    }
}

#[test]
fn tapping_a_page_leaves_any_editor() {
    let s = Settings::new();
    let mut screen = Screen::new(PAGES, "epg");
    screen.tap_row("epg.keep_past", &ctx(&s));
    assert!(screen.editor().is_some());
    screen.tap_page(6);
    assert_eq!(
        (
            screen.page().key,
            screen.pane(),
            screen.editor(),
            screen.focus()
        ),
        ("other", Pane::Pages, None, None)
    );
}

#[test]
fn language_picker_keeps_text_storage_cancel_and_focus_identity() {
    let mut settings = Settings::new();
    settings.set("general.language", Value::text("fr")).unwrap();
    fn ctx(settings: &Settings) -> Ctx<'_> {
        Ctx {
            settings,
            offer: &everything,
        }
    }
    let mut screen = Screen::new(PAGES, "general");
    screen.tap_row("general.language", &ctx(&settings));
    let variants = iptv_settings::catalog::choices::LANGUAGE.variants;
    let french = variants.iter().position(|v| *v == "fr").unwrap();
    assert!(
        matches!(screen.editor(), Some(Editor::Choice { focus, selected, .. }) if *focus == french && *selected == french)
    );
    screen.key(Key::Down, &ctx(&settings));
    screen.key(Key::Back, &ctx(&settings));
    assert_eq!(settings.get("general.language").unwrap(), Value::text("fr"));
    assert_eq!(screen.focus(), Some("general.language"));
    screen.tap_row("general.language", &ctx(&settings));
    assert_eq!(screen.pick(usize::MAX), Effect::None);
    let chinese = variants.iter().position(|v| *v == "zh-Hant").unwrap();
    apply(&mut settings, screen.pick(chinese));
    let (restored, report) = Settings::from_json(&settings.to_json_string()).unwrap();
    assert!(report.is_clean());
    assert_eq!(
        restored.get("general.language").unwrap(),
        Value::text("zh-Hant")
    );
    screen.tap_row("general.language", &ctx(&restored));
    apply(&mut settings, screen.pick(0));
    assert_eq!(
        settings.get("general.language").unwrap(),
        Value::text("system")
    );
}

#[test]
fn unlisted_locale_is_preserved_until_a_suggestion_is_chosen() {
    let mut settings = Settings::new();
    settings
        .set("general.language", Value::text("es-MX"))
        .unwrap();
    let ctx = Ctx {
        settings: &settings,
        offer: &everything,
    };
    let mut screen = Screen::new(PAGES, "general");
    screen.tap_row("general.language", &ctx);
    assert!(matches!(
        screen.editor(),
        Some(Editor::Choice {
            focus: 0,
            selected: usize::MAX,
            ..
        })
    ));
    screen.key(Key::Back, &ctx);
    assert_eq!(
        settings.get("general.language").unwrap(),
        Value::text("es-MX")
    );
}

#[test]
fn numeric_stepper_accepts_full_width_step_counts() {
    let settings = Settings::default();
    let mut screen = Screen::new(PAGES, "playback");
    screen.tap_row(
        "player.info_panel_timeout",
        &Ctx {
            settings: &settings,
            offer: &everything,
        },
    );
    screen.step(i64::MAX);
    assert!(matches!(
        screen.editor(),
        Some(Editor::Number { value: 30_000, .. })
    ));
    screen.step(i64::MIN);
    assert!(matches!(
        screen.editor(),
        Some(Editor::Number { value: 1_000, .. })
    ));
}
