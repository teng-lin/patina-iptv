use crate::personalization;
use iptv_settings::Settings;
use nsh_core::MountItem;
use personalization::*;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
#[test]
fn repeated_player_zaps_skip_intermediate_streams_and_menu_cancels_the_last() {
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
    step(app.press_key("enter"));
    step(app.run_for(Duration::from_millis(500)));
    loaded_stream(app, &opened, "http://example.test/a");
    step(app.press_key("enter"));
    opened.lock().unwrap().clear();
    step(app.press_key("down"));
    step(app.run_for(Duration::from_millis(150)));
    step(app.press_key("down"));
    step(app.run_for(Duration::from_millis(499)));
    assert!(opened.lock().unwrap().is_empty());
    step(app.run_for(Duration::from_millis(1)));
    loaded_stream(app, &opened, "http://example.test/c");
    assert_eq!(*opened.lock().unwrap(), ["http://example.test/c"]);
    step(app.press_key("up"));
    step(app.press_key("m"));
    step(app.run_for(Duration::from_secs(1)));
    assert_eq!(*opened.lock().unwrap(), ["http://example.test/c"]);
}
