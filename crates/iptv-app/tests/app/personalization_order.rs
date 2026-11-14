use crate::personalization;
use iptv_settings::Settings;
use iptv_store_sqlite::{ChannelQuery, ChannelScope};
use personalization::*;
use std::time::Duration;
fn reorder(app: &mut crate::headless::Headless, row: usize) {
    menu(app, "Reorder channels");
    step(app.press(&format!("id:organization-row-{row}")));
    step(app.wait_for("text:Move to first"));
    step(app.press("text:Move to first"));
    step(app.back());
}
#[test]
fn the_guide_uses_independent_favorite_and_provider_group_orders() {
    let Some(home) = isolated!() else { return };
    let mut f = start(home, Settings::new(), false);
    let app = &mut f.app;
    group(app, "Favourites");
    reorder(app, 2);
    step(crate::isolated::wait_outside(
        app,
        "favorites saved",
        Duration::from_secs(3),
        || {
            crate::isolated::open_database(&f.path)
                .unwrap()
                .load_channels(
                    f.source,
                    &ChannelQuery {
                        favourites_only: true,
                        ..Default::default()
                    },
                )
                .unwrap()[0]
                .name
                == "Local C"
        },
    ));
    // Reopening uses the just changed in-memory order, without restarting.
    menu(app, "Reorder channels");
    step(app.wait_for("id:organization-row-0 >> text:Local C"));
    step(app.back());
    group(app, "News");
    reorder(app, 1);
    let s = crate::isolated::open_database(&f.path).unwrap();
    let group = s
        .groups(f.source, None)
        .unwrap()
        .into_iter()
        .find(|g| g.name == "News")
        .unwrap();
    assert_eq!(
        s.load_channels(
            f.source,
            &ChannelQuery {
                scope: ChannelScope::Group(group.id),
                ..Default::default()
            }
        )
        .unwrap()[0]
            .name,
        "Local B"
    );
    assert_eq!(
        s.load_channels(f.source, &ChannelQuery::default()).unwrap()[0].name,
        "Local A"
    );
    menu(app, "Reorder channels");
    step(app.wait_for("id:organization-row-0 >> text:Local B"));
    step(app.press("id:organization-row-0"));
    for _ in 0..7 {
        step(app.press_key("down"));
    }
    step(app.press_key("enter"));
    step(app.back());
    menu(app, "Reorder channels");
    step(app.wait_for("id:organization-row-0 >> text:Local A"));
}
