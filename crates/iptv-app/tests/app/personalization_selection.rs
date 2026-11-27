use crate::personalization;
use iptv_settings::Settings;
use nsh_core::MountItem;
use personalization::*;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
fn full(app: &crate::headless::Headless) -> bool {
    app.screen()
        .layout_nodes()
        .iter()
        .find(|n| n.test_id.as_deref() == Some("video-player"))
        .unwrap()
        .bounds
        .size
        .width
        .0
        > 1900.0
}
#[test]
fn explicit_guide_and_list_choices_debounce_and_cancel_when_focus_leaves() {
    let Some(home) = isolated!() else { return };
    let mut f = start(home, Settings::new(), true);
    let app = &mut f.app;
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = opened.clone();
    app.observe(move |_, b| {
        for item in &b.items {
            if let MountItem::ComponentProps { props, .. } = item {
                for (k, v) in props {
                    if k.as_ref() == "source"
                        && let Some(source) = v.as_ref().map(crate::support::upstream_source)
                        && let Some(url) = source["url"].as_str()
                    {
                        seen.lock().unwrap().push(url.into());
                    }
                }
            }
        }
    });
    assert!(!full(app));
    step(app.press_key("enter"));
    step(app.run_for(Duration::from_millis(499)));
    assert!(opened.lock().unwrap().is_empty());
    step(app.run_for(Duration::from_millis(1)));
    assert!(!full(app));
    loaded_stream(app, &opened, "http://example.test/a");
    assert_eq!(*opened.lock().unwrap(), ["http://example.test/a"]);
    step(app.press_key("enter"));
    assert!(full(app));
    step(app.back());
    step(app.back());
    assert!(!full(app));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.press_key("up"));
    step(app.run_for(Duration::from_secs(1)));
    assert!(!full(app));
    assert_eq!(
        opened.lock().unwrap().len(),
        1,
        "navigation canceled an explicit tune even with autoplay off"
    );
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.run_for(Duration::from_millis(500)));
    assert!(!full(app));
    loaded_stream(app, &opened, "http://example.test/b");
    step(app.press_key("enter"));
    assert!(full(app));
    assert_eq!(
        opened.lock().unwrap().last().unwrap(),
        "http://example.test/b"
    );
    step(app.press_key("left"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.run_for(Duration::from_millis(500)));
    loaded_stream(app, &opened, "http://example.test/c");
    assert_eq!(
        opened.lock().unwrap().last().unwrap(),
        "http://example.test/c"
    );
    // The list is closed: OK now opens the player's card, not another selection.
    step(app.press_key("enter"));
    step(app.wait_for("id:player-card"));
    step(app.press_key("left"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.back());
    let count = opened.lock().unwrap().len();
    step(app.run_for(Duration::from_secs(1)));
    assert_eq!(opened.lock().unwrap().len(), count);
}
