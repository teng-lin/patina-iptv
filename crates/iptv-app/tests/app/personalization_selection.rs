use crate::personalization;
use iptv_settings::Settings;
use nsh_core::{MountItem, ViewKind};
use personalization::*;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
fn full(app: &crate::headless::Headless) -> bool {
    app.screen()
        .layout_nodes()
        .iter()
        .find(|n| n.kind == ViewKind::Native)
        .unwrap()
        .bounds
        .size
        .width
        .0
        > 1900.0
}
#[test]
fn default_explicit_choices_debounce_close_the_guide_and_list_and_cancel_on_back() {
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
                        && let Some(url) = v.as_ref().and_then(|v| v["url"].as_str())
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
    assert!(full(app));
    assert_eq!(*opened.lock().unwrap(), ["http://example.test/a"]);
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
    assert!(full(app));
    assert_eq!(
        opened.lock().unwrap().last().unwrap(),
        "http://example.test/b"
    );
    step(app.press_key("left"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.run_for(Duration::from_millis(500)));
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
