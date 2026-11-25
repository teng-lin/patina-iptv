//! The native accessibility props must exclude opacity-only UI and modal
//! underlays. Headless pointer/text selectors deliberately still see those
//! mounted nodes, so inspect the actual native props and their ancestors.
use crate::personalization::{self, Fixture, step};
use iptv_settings::{Settings, Value};
use nsh_core::{
    AccessibilityRole, MountBatch, MountItem, NodeId, Size, ViewKind, ViewProps, platform::Idiom,
    prelude::px,
};
use nsh_testing::Headless;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct NativeProps {
    views: HashMap<NodeId, ViewProps>,
    players: Vec<NodeId>,
    sources: Vec<String>,
    removed: Vec<NodeId>,
    inserts: HashMap<NodeId, usize>,
    scrolls: Vec<(NodeId, f32)>,
}

#[derive(Clone, Default)]
pub(super) struct Exposure(Arc<Mutex<NativeProps>>);

impl Exposure {
    /// Read the Tab parent's own native name, rather than a selector's
    /// fallback to its child text (which Android may visually ellipsize).
    #[track_caller]
    fn settings_tab(&self, app: &Headless, name: &str, selected: bool) -> NodeId {
        self.shown(app, "settings-content", true);
        let nodes = app.screen().layout_nodes();
        let content = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some("settings-content"))
            .unwrap()
            .id;
        let native = self.0.lock().unwrap();
        let text = nodes
            .iter()
            .find(|node| {
                node.text
                    .as_ref()
                    .is_some_and(|text| text.text.text == name)
                    && native
                        .views
                        .get(&node.parent)
                        .is_some_and(|props| props.accessibility.role == AccessibilityRole::Tab)
                    && {
                        let mut ancestor = node.id;
                        loop {
                            let Some(parent) = nodes.iter().find(|parent| parent.id == ancestor)
                            else {
                                break false;
                            };
                            if parent.hidden
                                || native
                                    .views
                                    .get(&parent.id)
                                    .is_some_and(|props| props.accessibility.hidden)
                            {
                                break false;
                            }
                            if parent.id == content {
                                break true;
                            }
                            ancestor = parent.parent;
                        }
                    }
            })
            .unwrap_or_else(|| panic!("missing tab text {name}"));
        let props = native
            .views
            .get(&text.parent)
            .expect("tab parent's native props");
        assert_eq!(props.accessibility.role, AccessibilityRole::Tab);
        assert_eq!(props.accessibility.accessible, Some(true));
        assert_eq!(props.accessibility.label.as_deref(), Some(name));
        assert_eq!(props.accessibility.state.selected, selected, "{name}");
        text.parent
    }

    pub(super) fn record(&self, batch: &MountBatch) {
        let mut native = self.0.lock().unwrap();
        for item in &batch.items {
            match item {
                MountItem::ViewProps { id, props } => {
                    native.views.insert(*id, (**props).clone());
                }
                MountItem::AccessibilityLabel { id, label } => {
                    native
                        .views
                        .get_mut(id)
                        .expect("label update follows initial native props")
                        .accessibility
                        .label = label.clone();
                }
                MountItem::Component { id, name } if name.as_ref() == "tv.player" => {
                    native.players.push(*id)
                }
                MountItem::ComponentProps { props, .. } => {
                    for (name, value) in props {
                        if name.as_ref() == "source"
                            && let Some(source) =
                                value.as_ref().map(crate::support::upstream_source)
                            && let Some(url) = source["url"].as_str()
                        {
                            native.sources.push(url.into());
                        }
                    }
                }
                MountItem::Insert { child, .. } => *native.inserts.entry(*child).or_default() += 1,
                MountItem::Remove { id } | MountItem::Delete { id } => native.removed.push(*id),
                MountItem::Command {
                    id,
                    command: nsh_core::Command::ScrollTo { y, .. },
                } => {
                    native.scrolls.push((*id, y.0));
                }
                _ => {}
            }
        }
    }

    pub(super) fn action_label(
        &self,
        app: &Headless,
        test_id: &str,
        action: &str,
    ) -> Option<String> {
        let nodes = app.screen().layout_nodes();
        let id = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some(test_id))?
            .id;
        self.0
            .lock()
            .unwrap()
            .views
            .get(&id)?
            .accessibility
            .actions
            .iter()
            .find(|item| item.name.as_ref() == action)?
            .label
            .as_deref()
            .map(str::to_owned)
    }

    #[track_caller]
    pub(super) fn shown(&self, app: &Headless, test_id: &str, expected: bool) {
        let nodes = app.screen().layout_nodes();
        let mut node = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some(test_id))
            .unwrap_or_else(|| panic!("{test_id} is not mounted"));
        let native = self.0.lock().unwrap();
        assert!(
            native.views.contains_key(&node.id),
            "observed the named container's native props"
        );
        let mut exposed = true;
        loop {
            // A plain ancestor with default props may have no ViewProps
            // item. Its accessibility-hidden value is false by default.
            exposed &= !native
                .views
                .get(&node.id)
                .is_some_and(|props| props.accessibility.hidden)
                && !node.hidden;
            let Some(parent) = nodes.iter().find(|parent| parent.id == node.parent) else {
                break;
            };
            node = parent;
        }
        assert_eq!(
            exposed, expected,
            "{test_id}: effective accessibility exposure"
        );
    }

    #[track_caller]
    fn selected_channel(&self, app: &Headless, name: &str) {
        self.shown(app, "guide-ui", true);
        let nodes = app.screen().layout_nodes();
        let guide = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some("guide-ui"))
            .unwrap()
            .id;
        let native = self.0.lock().unwrap();
        assert!(
            nodes.iter().any(|node| {
                if !native.views.get(&node.id).is_some_and(|props| {
                    props.accessibility.label.as_deref() == Some(name)
                        && props.accessibility.state.selected
                }) {
                    return false;
                }
                let mut ancestor = node.parent;
                while let Some(parent) = nodes.iter().find(|parent| parent.id == ancestor) {
                    if parent.id == guide {
                        return true;
                    }
                    ancestor = parent.parent;
                }
                false
            }),
            "the mounted guide's content focus should remain on {name}"
        );
    }

    #[track_caller]
    fn persistent_player(&self, app: &Headless, id: NodeId) {
        let native = self.0.lock().unwrap();
        assert_eq!(native.players, [id], "one persistent player component");
        assert!(
            !native.removed.contains(&id),
            "the player was never detached"
        );
        assert_eq!(
            native.inserts.get(&id),
            Some(&1),
            "the player was never reparented"
        );
        assert!(
            app.screen()
                .layout_nodes()
                .iter()
                .any(|node| node.id == id && !node.hidden)
        );
    }
}

fn fixture(home: crate::isolated::Home) -> (Fixture, Exposure) {
    fixture_with_settings(home, Settings::new())
}

fn fixture_with_settings(
    home: crate::isolated::Home,
    mut settings: Settings,
) -> (Fixture, Exposure) {
    let exposure = Exposure::default();
    let observed = exposure.clone();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    settings
        .set("guide.stay_on_guide", Value::Bool(true))
        .unwrap();
    let fixture = personalization::start_with_setup(home, settings, true, move |app| {
        app.observe(move |_, batch| observed.record(batch));
    });
    (fixture, exposure)
}

fn large_text() -> Settings {
    let mut settings = Settings::new();
    settings
        .set("appearance.font_size", Value::Enum("extra_large".into()))
        .unwrap();
    settings
}

fn inside(screen: &nsh_testing::Screen, control: &str, container: &str) -> bool {
    let nodes = screen.layout_nodes();
    let bounds = |id: &str| {
        nodes
            .iter()
            .find(|n| n.test_id.as_deref() == Some(id))
            .unwrap()
            .bounds
    };
    let c = bounds(control);
    let v = bounds(container);
    c.size.height.0 > 0.0
        && c.size.width.0 > 0.0
        && c.origin.x.0 >= v.origin.x.0 - 1.0
        && c.origin.y.0 >= v.origin.y.0 - 1.0
        && c.origin.x.0 + c.size.width.0 <= v.origin.x.0 + v.size.width.0 + 1.0
        && c.origin.y.0 + c.size.height.0 <= v.origin.y.0 + v.size.height.0 + 1.0
}

#[test]
fn settings_tabs_keep_full_native_names_at_largest_text_across_input_and_language() {
    fn here() {}
    let Some(home) = crate::isolated::run_with_env(
        std::any::type_name_of_val(&here),
        &[("IPTV_TEST_DEVICE_SIGNALS", "1")],
    ) else {
        return;
    };
    let (mut fixture, exposure) = fixture_with_settings(home, large_text());
    let app = &mut fixture.app;
    step(app.set_environment("largest TV text", |env| {
        env.idiom = Idiom::Tv;
        env.font_scale = 1.3;
    }));
    step(app.set_viewport(Size::new(px(960.0), px(540.0))));
    crate::device_signals::report(app, "tv", false, true);
    step(app.open_guide_groups());
    step(app.press("id:navigation-rail >> role:tab[Settings]"));
    step(app.wait_for("role:tab[General]"));
    exposure.settings_tab(app, "General", true);
    exposure.settings_tab(app, "Playlists", false);
    step(app.press_key("down"));
    step(app.press_key("down"));
    let guide = exposure.settings_tab(app, "TV guide data", true);
    step(app.press_key("right"));
    step(app.wait_for("text:Guide sources"));
    step(app.back());
    assert_eq!(exposure.settings_tab(app, "TV guide data", true), guide);
    step(app.press_key("down"));
    exposure.settings_tab(app, "Appearance", true);
    step(app.press_key("up"));
    assert_eq!(exposure.settings_tab(app, "TV guide data", true), guide);
    for _ in 0..3 {
        step(app.press_key("down"));
    }
    exposure.settings_tab(app, "Remote control", true);
    step(app.press_key("right"));
    step(app.wait_for("text:Edit remote bindings"));
    step(app.back());
    step(app.press("role:tab[TV guide data]"));
    assert_eq!(exposure.settings_tab(app, "TV guide data", true), guide);

    // A device/input change updates the keys page without moving the
    // selected guide page or replacing its native tab identity.
    step(app.set_environment("largest tablet text", |env| env.idiom = Idiom::Pad));
    step(app.set_viewport(Size::new(px(1280.0), px(800.0))));
    crate::device_signals::report(app, "tablet", true, false);
    assert_eq!(exposure.settings_tab(app, "TV guide data", true), guide);
    step(app.press("role:tab[Keyboard]"));
    let keys = exposure.settings_tab(app, "Keyboard", true);
    step(app.wait_for("text:Edit keyboard shortcuts"));
    crate::device_signals::report(app, "tablet", false, true);
    step(app.wait_for("text:Edit remote bindings"));
    assert_eq!(exposure.settings_tab(app, "Remote control", true), keys);

    // Change the real interface preference with its editor while the
    // sidebar remains mounted; existing parent names must translate too.
    step(app.press("role:tab[General]"));
    let guide = exposure.settings_tab(app, "TV guide data", false);
    step(app.press_key("right"));
    for _ in 0..15 {
        if app.screen().matches("text:Language").unwrap() && app.press("text:Language").is_ok() {
            break;
        }
        step(app.press_key("down"));
    }
    step(app.wait_for("text:/Español/"));
    step(app.press("text:/Español/"));
    let spanish = iptv_presentation::i18n::resolve("es", None);
    let translated = iptv_presentation::i18n::translate(spanish, "TV guide data");
    step(app.wait_for(&format!("role:tab[{translated}]")));
    step(app.wait_for(&format!("text:{translated}")));
    assert_eq!(exposure.settings_tab(app, translated, false), guide);
    let remote = iptv_presentation::i18n::translate(spanish, "Remote control");
    assert_eq!(exposure.settings_tab(app, remote, false), keys);
    step(app.press(&format!("role:tab[{translated}]")));
    assert_eq!(exposure.settings_tab(app, translated, true), guide);
    step(app.press_key("right"));
    let sources = iptv_presentation::i18n::translate(spanish, "Guide sources");
    step(app.wait_for(&format!("text:{sources}")));
    step(app.back());
    assert_eq!(exposure.settings_tab(app, translated, true), guide);
    step(app.press_key("down"));
    let appearance = iptv_presentation::i18n::translate(spanish, "Appearance");
    exposure.settings_tab(app, appearance, true);
}

#[test]
fn largest_text_pin_keeps_cancel_visible_and_dpad_has_no_invisible_stop() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, exposure) = fixture_with_settings(home, large_text());
    let app = &mut fixture.app;
    let player = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.kind == ViewKind::Native)
        .unwrap()
        .id;
    step(
        app.set_environment("enlarged text with a screen reader", |env| {
            env.screen_reader = true;
            env.font_scale = 1.3;
        }),
    );
    personalization::menu(app, "Protect channel with PIN");
    step(app.wait_for("text:Choose a new PIN"));
    exposure.shown(app, "parental-layer", true);
    exposure.shown(app, "guide-ui", false);
    exposure.shown(app, "menu-layer", false);
    for (idiom, width, height) in [
        (Idiom::Tv, 960.0, 540.0),
        (Idiom::Pad, 1280.0, 800.0),
        (Idiom::Desktop, 960.0, 540.0),
    ] {
        step(app.set_environment("input mode", move |env| env.idiom = idiom));
        step(app.set_viewport(Size::new(px(width), px(height))));
        assert!(
            inside(app.screen(), "parental-card", "parental-layer"),
            "dialog stays in {width}x{height}"
        );
        assert!(
            inside(app.screen(), "parental-cancel", "parental-card"),
            "Cancel remains visible"
        );
        assert!(app.screen().matches("role:button[/Cancel/]").unwrap());
    }
    // Editing a digit in the same phase must not reset a scrolled form.
    // Use a shorter window to force the keypad's lower row off screen.
    step(app.set_viewport(Size::new(px(960.0), px(320.0))));
    for key in ["down", "down", "down", "right", "right"] {
        step(app.press_key(key));
    }
    step(
        app.wait_until("Continue is revealed", Duration::from_secs(3), |screen| {
            inside(screen, "parental-continue", "parental-scroll")
        }),
    );
    let scroll = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.test_id.as_deref() == Some("parental-scroll"))
        .unwrap()
        .id;
    exposure.0.lock().unwrap().scrolls.clear();
    step(app.press_key("2"));
    assert!(
        !exposure
            .0
            .lock()
            .unwrap()
            .scrolls
            .iter()
            .any(|(id, y)| *id == scroll && *y == 0.0),
        "digit editing must not reset the scroll offset when the phase is unchanged"
    );
    step(app.set_viewport(Size::new(px(960.0), px(540.0))));
    // Saturating Down reaches the actual last control, with a visible focus
    // marker. An extra non-rendered keypad index used to swallow this focus.
    for _ in 0..6 {
        step(app.press_key("down"));
    }
    assert_eq!(
        app.screen()
            .span_style("id:parental-cancel >> text:Cancel")
            .unwrap()
            .color,
        nsh_core::Color((iptv_app::theme::FOCUSED_TEXT << 8) | 0xff)
    );
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:Choose a new PIN", Duration::from_secs(3)));
    exposure.shown(app, "guide-ui", true);
    exposure.selected_channel(app, "Local A");
    exposure.persistent_player(app, player);
}

#[test]
fn modal_back_restores_channel_identity_after_reordering_and_recovery_can_cancel() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, exposure) = fixture(home);
    let app = &mut fixture.app;
    step(app.press_key("down"));
    personalization::menu(app, "Reorder channels");
    step(app.wait_for("id:guide-reorder-handle"));
    exposure.shown(app, "guide-ui", true);
    exposure.shown(app, "guide-program", false);
    step(app.wait_for("id:guide-row >> nth=1 >> text:★ Local B"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.press_key("left"));
    step(app.wait_for("id:guide-row >> nth=0 >> text:★ Local C"));
    step(app.back());
    step(app.wait_for("text:Save order"));
    exposure.shown(app, "guide-ui", false);
    step(app.press("text:Save order"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    step(app.wait_for("id:guide-row >> nth=0 >> text:★ Local C"));
    exposure.shown(app, "guide-ui", true);
    exposure.selected_channel(app, "Local B");
    // Recovery is a different form, with a different number of controls.
    // It must reset keypad focus and let D-pad OK activate Cancel.
    personalization::menu(app, "Protect channel with PIN");
    step(app.wait_for("text:Choose a new PIN"));
    step(app.press("text:Forgot PIN?"));
    assert!(app.screen().matches("label:Recovery code").unwrap());
    step(app.press_key("enter"));
    step(app.press_key("1"));
    step(app.press_key("2"));
    assert_eq!(
        app.screen()
            .input_text("id:parental-recovery-input")
            .unwrap(),
        Some("12"),
        "D-pad OK focuses the native input and keyboard digits edit recovery text"
    );
    // Each vertical arrow visits the next stacked control, including Continue.
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for("text:Enter the 32-character recovery code"));
    step(app.press_key("up"));
    step(app.wait_until(
        "recovery field returns into view",
        Duration::from_secs(3),
        |screen| inside(screen, "parental-recovery-input", "parental-scroll"),
    ));
    step(app.press_key("down"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:Recover parental PIN", Duration::from_secs(3)));
    exposure.shown(app, "guide-ui", true);
    exposure.selected_channel(app, "Local B");
}

#[test]
fn first_run_large_text_exposes_source_choices_and_scrolls_focused_fields() {
    let Some(home) = isolated!() else { return };
    std::fs::create_dir_all(home.data_dir()).unwrap();
    std::fs::write(
        home.data_dir().join("settings.json"),
        large_text().to_json_string(),
    )
    .unwrap();
    let mut config = nsh_engine::EngineConfig::new(nsh_core::element::Root::of::<iptv_app::App>());
    config.viewport = Size::new(px(960.0), px(540.0));
    config.environment.idiom = Idiom::Tv;
    config.environment.font_scale = 1.3;
    config.environment.screen_reader = true;
    let mut app = crate::headless::Headless::start(config).unwrap();
    step(app.wait_for("id:wizard-welcome"));
    step(app.press("role:button[/^Add a playlist$/]"));
    step(app.wait_for("id:wizard-form"));
    step(app.press_key("enter"));
    step(app.wait_for("id:wizard-source-picker"));
    assert!(
        app.screen()
            .matches("role:button[/Playlist link/]")
            .unwrap()
    );
    assert!(app.screen().matches("role:button[/Xtream Codes/]").unwrap());
    for _ in 0..3 {
        step(app.press_key("down"));
    }
    step(app.press_key("enter"));
    step(app.wait_for("id:wizard-form"));
    let viewport = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.test_id.as_deref() == Some("setup-scroll"))
        .unwrap()
        .bounds;
    assert!(viewport.origin.x.0 >= 0.0 && viewport.origin.y.0 >= 0.0);
    assert!(viewport.origin.x.0 + viewport.size.width.0 <= 960.0);
    assert!(viewport.origin.y.0 + viewport.size.height.0 <= 540.0);
    assert!(!app.screen().matches("id:wizard-field-EpgUrl").unwrap());
    for key in ["down", "down", "enter", "down", "down"] {
        step(app.press_key(key));
    }
    step(app.wait_until(
        "last field scrolls into view",
        Duration::from_secs(3),
        |screen| inside(screen, "wizard-field-EpgUrl", "setup-scroll"),
    ));
    let guidance = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter_map(|node| node.text)
        .find(|text| text.text.text == "Optional: an XMLTV link (the provider's guide otherwise)")
        .expect("guide fallback is displayed in the form");
    assert_eq!(guidance.text.max_lines, 0, "the fallback can wrap");
    assert!(!guidance.text.no_wrap);
    step(app.press_key("down"));
    step(app.wait_until(
        "submit scrolls into view",
        Duration::from_secs(3),
        |screen| inside(screen, "wizard-submit", "setup-scroll"),
    ));
    assert!(app.screen().matches("role:button[/Add playlist/]").unwrap());
    step(app.press_key("enter"));
    assert!(
        app.screen()
            .matches("role:button[/Portal.*Fill this in/]")
            .unwrap(),
        "the field's validation error remains in its accessible name"
    );
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for("id:wizard-source-picker"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for("id:wizard-form"));
    assert!(app.screen().matches("role:button[/^Username$/]").unwrap());
    assert!(!app.screen().matches("id:wizard-paste").unwrap());
    assert!(
        app.screen().matches("role:button[/^Password$/]").unwrap(),
        "empty credentials must not announce invented bullet values"
    );
    step(app.back());
    step(app.wait_for("id:wizard-welcome"));
}

#[test]
fn inactive_player_ui_stays_out_of_accessibility_and_returns_without_recreating_video() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, exposure) = fixture(home);
    let app = &mut fixture.app;
    step(app.set_environment("a TV with TalkBack", |env| {
        env.idiom = Idiom::Tv;
        env.screen_reader = true;
    }));
    let player = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.kind == ViewKind::Native)
        .unwrap()
        .id;
    exposure.shown(app, "guide-ui", true);
    for inactive in [
        "video",
        "channel-list-layer",
        "chooser-layer",
        "favorites-editor",
        "menu-layer",
        "description-layer",
        "player-panels",
    ] {
        exposure.shown(app, inactive, false);
    }
    step(app.press_key("down"));
    exposure.selected_channel(app, "Local B");
    step(app.press_key("enter"));
    step(app.wait_for("text:▶"));
    step(app.wait_until(
        "Local B source is published",
        Duration::from_secs(5),
        |_| {
            exposure
                .0
                .lock()
                .unwrap()
                .sources
                .last()
                .is_some_and(|url| url == "http://example.test/b")
        },
    ));
    let opens = iptv_app::player_opens(0);
    step(app.press_key("enter"));
    step(app.wait_for("id:player-banner"));
    exposure.shown(app, "guide-ui", false);
    exposure.shown(app, "video", true);
    exposure.shown(app, "player-panels", true);
    exposure.shown(app, "player-top-bar", false);
    exposure.shown(app, "player-controls", false);
    // With the reader on, the banner stays past its delay (#281) until
    // the viewer replaces or hides it.
    step(app.run_for(Duration::from_secs(4)));
    exposure.shown(app, "player-panels", true);
    step(app.press_key("i"));
    step(app.wait_for("id:player-card"));
    exposure.shown(app, "player-card", true);
    exposure.shown(app, "player-controls", false);
    step(app.press_key("enter"));
    exposure.shown(app, "player-panels", false);
    exposure.shown(app, "play-bar-layer", true);
    step(app.play_bar_option("Display"));
    exposure.shown(app, "chooser-layer", true);
    exposure.shown(app, "player-panels", false);
    exposure.shown(app, "video", false);
    step(app.back());
    exposure.shown(app, "chooser-layer", false);
    exposure.shown(app, "play-bar-layer", true);
    step(app.back());
    step(app.back());
    exposure.shown(app, "guide-ui", true);
    exposure.shown(app, "video", false);
    exposure.selected_channel(app, "Local B");
    assert_eq!(
        iptv_app::player_opens(0),
        opens,
        "opening UI does not reopen the stream"
    );
    exposure.persistent_player(app, player);
}

#[test]
fn modal_screens_and_editors_hide_only_their_underlay_and_restore_it_on_back() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, exposure) = fixture(home);
    let app = &mut fixture.app;
    step(app.open_guide_groups());
    step(app.press_key("left"));
    step(app.press("label:Settings"));
    step(app.wait_for("id:settings-screen"));
    exposure.shown(app, "guide-ui", false);
    exposure.shown(app, "settings-screen", true);
    exposure.shown(app, "settings-content", true);
    step(app.press_key("right"));
    for _ in 0..15 {
        if app.screen().matches("text:Language").unwrap() && app.press("text:Language").is_ok() {
            break;
        }
        step(app.press_key("down"));
    }
    step(app.wait_for("id:settings-editor"));
    exposure.shown(app, "settings-editor", true);
    exposure.shown(app, "settings-content", false);
    step(app.back());
    exposure.shown(app, "settings-content", true);
    step(app.back());
    step(app.back());
    exposure.shown(app, "guide-ui", true);
    step(app.press("label:Playlists"));
    step(app.wait_for("id:playlists-screen"));
    exposure.shown(app, "guide-ui", false);
    exposure.shown(app, "playlists-list", true);
    step(app.press("text:Synthetic personalization"));
    exposure.shown(app, "playlists-list", false);
    step(app.back());
    exposure.shown(app, "playlists-list", true);
    step(app.press("id:playlists-screen >> text:+ Add playlist"));
    step(app.wait_for("text:Playlist link"));
    exposure.shown(app, "playlists-screen", false);
    // Settings alerts are siblings of its screen and can outlive it. A
    // higher opaque screen must hide the whole layer, including an alert.
    exposure.shown(app, "settings-layer", false);
    step(app.back());
    exposure.shown(app, "playlists-screen", true);
    step(app.back());
    exposure.shown(app, "guide-ui", true);
    exposure.shown(app, "settings-layer", true);
}

#[test]
fn a_popup_hides_its_underlay_while_the_pointer_bar_keeps_its_visible_controls() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, exposure) = fixture(home);
    let app = &mut fixture.app;
    step(app.press_key("down"));
    step(app.press_key("m"));
    exposure.shown(app, "menu-panel", true);
    exposure.shown(app, "guide-ui", false);
    step(app.press("text:Manage favorites…"));
    exposure.shown(app, "favorites-editor", true);
    exposure.shown(app, "menu-layer", false);
    exposure.shown(app, "guide-ui", false);
    step(app.back());
    exposure.shown(app, "guide-ui", true);
    exposure.selected_channel(app, "Local B");
    step(app.press_key("enter"));
    step(app.press_key("enter"));
    step(app.press("label:Video"));
    step(app.wait_for("id:play-bar-play-pause"));
    exposure.shown(app, "play-bar-layer", true);
    step(app.play_bar_option("Display"));
    exposure.shown(app, "chooser-layer", true);
    exposure.shown(app, "play-bar-layer", true);
    exposure.shown(app, "video", false);
    step(app.press("label:Close the chooser"));
    exposure.shown(app, "chooser-layer", false);
    step(app.play_bar_option("Program details"));
    exposure.shown(app, "description-layer", true);
    exposure.shown(app, "menu-layer", true);
    exposure.shown(app, "menu-panel", false);
    exposure.shown(app, "play-bar-layer", true);
    step(app.press("id:description-layer"));
    exposure.shown(app, "description-layer", false);
    exposure.shown(app, "menu-layer", false);
    step(app.press("label:Guide"));
    exposure.shown(app, "guide-ui", true);
    exposure.selected_channel(app, "Local B");
}

/// A screen reader's activation of `id` (TalkBack's double tap, VoiceOver's
/// and Narrator's activate): the platform's accessibility action turns into
/// a bare `onPress`, with no finger down, no release and no position
/// (`A11y.performAccessibilityAction` on Android).
fn activate(app: &mut Headless, id: NodeId) {
    app.engine().control().dispatch_event(
        id,
        nsh_core::EventKind::Press,
        nsh_core::EventPayload::None,
    );
    step(app.run_for(Duration::from_millis(500)));
}

/// #192: what a screen reader activates plays as a tap does. TalkBack's
/// double tap on a guide row, a program cell or the play bar's buttons
/// reaches the app as a bare press; the play bar it opens over full-screen
/// video stays while the reader moves through its controls.
#[test]
fn a_screen_readers_activation_plays_from_the_guide_and_keeps_the_play_bar() {
    let Some(home) = isolated!() else { return };
    let exposure = Exposure::default();
    let observed = exposure.clone();
    let mut settings = Settings::new();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    let guide_file = home.path().join("screen-reader-guide.xml");
    std::fs::write(
        &guide_file,
        crate::support::guide(
            crate::support::now(),
            &[("c", "Local C", "Local C bulletin")],
        ),
    )
    .unwrap();
    let mut fixture = personalization::launch_prepared(
        home,
        settings,
        true,
        move |path, source| {
            let mut store = crate::isolated::open_database(path).unwrap();
            store
                .set_guide_sources(
                    source,
                    &[iptv_store_sqlite::GuideSource {
                        feed: iptv_epg::FeedId(1001),
                        provider_index: None,
                        url: iptv_providers::SecretUrl::new(guide_file.to_string_lossy()),
                        name: Some("Synthetic screen reader guide".into()),
                        enabled: true,
                        shift_minutes: 0,
                    }],
                )
                .unwrap();
        },
        |_| {},
        move |app| app.observe(move |_, batch| observed.record(batch)),
    );
    let app = &mut fixture.app;
    let live_cell = "id:guide-ui >> role:cell[Local C bulletin]";
    step(app.wait_for_within(live_cell, Duration::from_secs(20)));
    step(app.open_guide_groups());
    step(app.press_key("right"));
    step(app.set_environment("a tablet with TalkBack", |env| {
        env.idiom = Idiom::Pad;
        env.screen_reader = true;
    }));
    // VoiceOver exposes custom actions only when explicitly declared;
    // its native adapter does not infer them from long-press handlers.
    let title = app.screen().id("id:guide-header-program").unwrap();
    assert!(
        exposure.0.lock().unwrap().views[&title]
            .accessibility
            .actions
            .iter()
            .any(|action| action.name.as_ref() == "longpress"
                && action.label.as_deref() == Some("More options")),
        "the program title exposes a localized menu action to screen readers"
    );
    app.engine().control().dispatch_event(
        title,
        nsh_core::EventKind::AccessibilityAction,
        nsh_core::EventPayload::Map(vec![(
            "actionName",
            nsh_core::PayloadValue::Str("longpress".into()),
        )]),
    );
    step(app.wait_for("id:menu-panel >> text:Manage favorites…"));
    assert_eq!(iptv_app::player_opens(0), 0, "a menu never tunes");
    step(app.back());
    step(app.wait_for_gone(
        "id:menu-panel >> text:Manage favorites…",
        Duration::from_secs(3),
    ));
    exposure.shown(app, "menu-panel", false);
    // A channel row plays in the guide, then its second activation expands it.
    let row = app.screen().id("role:button[/^Local B$/]").unwrap();
    activate(app, row);
    step(app.wait_until(
        "Local B opens in the preview",
        Duration::from_secs(5),
        |_| iptv_app::player_has_source(0) && iptv_app::player_opens(0) == 1,
    ));
    exposure.shown(app, "guide-ui", true);
    activate(app, row);
    exposure.shown(app, "guide-ui", false);
    exposure.shown(app, "video", true);
    // The video opens the play bar, which stays however long the reader
    // takes to reach a control.
    let video = app.screen().id("id:video").unwrap();
    activate(app, video);
    exposure.shown(app, "play-bar-layer", true);
    step(app.run_for(Duration::from_secs(
        4 * iptv_presentation::play_bar::HIDE_AFTER_MS / 1000,
    )));
    exposure.shown(app, "play-bar-layer", true);
    // Its Guide button goes back to the guide, on the playing channel.
    let guide = app.screen().id("label:Guide").unwrap();
    activate(app, guide);
    exposure.shown(app, "guide-ui", true);
    exposure.selected_channel(app, "Local B");
    // A program cell on another row plays its channel.
    let cell = app.screen().id(live_cell).unwrap();
    activate(app, cell);
    step(app.wait_until(
        "Local C opens in the preview",
        Duration::from_secs(5),
        |_| iptv_app::player_has_source(0) && iptv_app::player_opens(0) == 2,
    ));
    exposure.shown(app, "guide-ui", true);
    let cell = app.screen().id(live_cell).unwrap();
    activate(app, cell);
    exposure.shown(app, "guide-ui", false);
    step(app.wait_for("text:/Local C  ▶/"));
    // Without the reader the bar hides after its delay again.
    step(app.set_environment("TalkBack off", |env| env.screen_reader = false));
    activate(app, video);
    exposure.shown(app, "play-bar-layer", true);
    step(app.run_for(Duration::from_millis(
        iptv_presentation::play_bar::HIDE_AFTER_MS + 500,
    )));
    step(app.wait_for_gone("id:play-bar-layer", Duration::from_secs(3)));
}
