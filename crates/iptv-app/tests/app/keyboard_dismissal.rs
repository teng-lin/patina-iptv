//! A floating IME reports visibility changes with an inset height of zero.

use crate::{Headless, personalization};
use personalization::step;
use std::time::Duration;

use nsh_core::{Command, EventKind, EventPayload, NodeId};
use nsh_tv::RemoteKey;

fn node(app: &Headless, test_id: &str) -> NodeId {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(test_id))
        .unwrap_or_else(|| panic!("missing {test_id}"))
        .id
}

fn visibility(app: &mut Headless, visible: bool) {
    app.engine().control().dispatch_event(
        node(app, "ime-visibility"),
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "visibility".into(),
            data: visible.into(),
        },
    );
    app.settle();
    step(app.wait(Duration::ZERO));
}

fn unfocused(app: &Headless, field: NodeId) -> bool {
    app.screen()
        .commands()
        .iter()
        .any(|(id, command)| *id == field && matches!(command, Command::Blur))
}

fn fixture(here: &str) -> Option<personalization::Fixture> {
    let home = crate::isolated::run_with_env(here, &[("IPTV_TEST_FLOATING_IME", "1")])?;
    Some(personalization::start(
        home,
        iptv_settings::Settings::new(),
        false,
    ))
}

#[test]
fn search_blurs_only_after_the_floating_keyboard_was_shown_and_hidden() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here)) else {
        return;
    };
    let app = &mut fixture.app;

    step(app.press_remote(RemoteKey::Search));
    step(app.wait_for("id:search-field"));
    let field = node(app, "search-field");
    app.clear_commands();
    visibility(app, false);
    assert!(!unfocused(app, field), "initial hidden state keeps editing");
    visibility(app, true);
    step(app.type_text("id:search-field", "Local"));
    let tab = "id:search-screen >> text:Channels · 3";
    step(app.wait_for(tab));
    assert!(!unfocused(app, field), "typing keeps the editor focused");

    visibility(app, false);
    assert!(unfocused(app, field), "dismissal must end native input");
    assert_eq!(
        app.screen().input_text("id:search-field").unwrap(),
        Some("Local")
    );
    let before = app.screen().span_style(tab).unwrap().color;
    step(app.press_key("down"));
    assert_ne!(
        app.screen().span_style(tab).unwrap().color,
        before,
        "the first Down focuses the Search tab"
    );

    step(app.press_key("up"));
    step(app.press_key("enter"));
    visibility(app, true);
    app.clear_commands();
    visibility(app, false);
    assert!(
        unfocused(app, field),
        "the reopened editor is dismissed again"
    );
}

#[test]
fn multiview_picker_blurs_after_the_floating_keyboard_is_hidden() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here)) else {
        return;
    };
    let app = &mut fixture.app;

    step(app.press("text:★ Local A"));
    step(app.wait_for("text:▶"));
    step(app.press("text:★ Local A"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Multiview"));
    step(app.press("label:Multiview"));
    let field = "id:multiview-channel-search";
    step(app.wait_for(field));
    step(app.type_text(field, "Local B"));
    let row = "id:multiview-picker-channels >> text:2  Local B";
    step(app.wait_for(row));
    let field_id = node(app, "multiview-channel-search");
    visibility(app, true);
    app.clear_commands();

    visibility(app, false);
    assert!(unfocused(app, field_id), "dismissal must end native input");
    assert_eq!(app.screen().input_text(field).unwrap(), Some("Local B"));
    let before = app.screen().span_style(row).unwrap().color;
    step(app.press_key("down"));
    assert_ne!(
        app.screen().span_style(row).unwrap().color,
        before,
        "the first Down focuses the filtered channel"
    );
}
