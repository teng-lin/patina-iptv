use crate::personalization;
use iptv_settings::Settings;
use iptv_store_sqlite::{ChannelQuery, ChannelScope};
use personalization::*;
use std::time::Duration;

#[track_caller]
fn open_inline_order(app: &mut crate::headless::Headless) {
    menu(app, "Reorder channels");
    step(app.wait_for("id:guide-reorder-handle"));
    assert!(!app.screen().matches("id:organization-row-0").unwrap());
}

fn reorder_favorites(app: &mut crate::headless::Headless, row: usize) {
    open_inline_order(app);
    step(app.drag(
        &format!("id:guide-row >> nth={row} >> id:guide-reorder-handle"),
        nsh_core::geometry::point(nsh_core::prelude::px(0.0), nsh_core::prelude::px(-105.0)),
    ));
    step(app.wait_for("id:guide-row >> nth=0 >> text:★ Local C"));
    step(app.press("id:organization-screen >> text:Save"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
}
#[test]
fn the_guide_uses_independent_favorite_and_provider_group_orders() {
    let Some(home) = isolated!() else { return };
    let mut f = start(home, Settings::new(), false);
    let app = &mut f.app;
    group(app, "Favorites");
    reorder_favorites(app, 2);
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
                        favorites_only: true,
                        ..Default::default()
                    },
                )
                .unwrap()[0]
                .name
                == "Local C"
        },
    ));
    // Reopening uses the just changed in-memory order, without restarting.
    open_inline_order(app);
    step(app.wait_for("id:guide-row >> nth=0 >> text:★ Local C"));
    step(app.back());
    group(app, "News");
    open_inline_order(app);
    step(app.wait_for("id:guide-row >> nth=0 >> text:★ Local A"));
    step(app.press_key("down"));
    step(app.press_key("enter")); // Select Local B.
    step(app.press_key("left"));
    step(app.wait_for("id:guide-row >> nth=0 >> text:★ Local B"));
    step(app.back());
    step(app.wait_for("text:Save order"));
    step(app.press_key("enter"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
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
    open_inline_order(app);
    step(app.wait_for("id:guide-row >> nth=0 >> text:★ Local B"));
    step(app.back());
    menu(app, "Channel order");
    step(app.wait_for("text:Playlist order"));
    step(app.press("text:Playlist order"));
    step(app.wait_for_gone("id:organization-sort", Duration::from_secs(5)));
    open_inline_order(app);
    step(app.wait_for("id:guide-row >> nth=0 >> text:★ Local A"));
    // Leaving with a draft preserves the saved provider policy if discarded.
    step(app.press_key("enter"));
    step(app.press_key("right"));
    step(app.press("id:organization-screen >> text:Cancel"));
    step(app.wait_for("text:Discard changes"));
    step(app.press("text:Discard changes"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    open_inline_order(app);
    step(app.wait_for("id:guide-row >> nth=0 >> text:★ Local A"));
    step(app.back());
    menu(app, "Channel order");
    step(app.wait_for("text:/Custom order$/"));
    step(app.press("text:/Custom order$/"));
    step(app.wait_for("id:reorder-handle-0"));
    step(app.wait_for("id:organization-row-0 >> text:Local B"));
    // Custom positions are only activated by saving the editor.
    step(app.press("id:organization-screen >> text:Close"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    menu(app, "Channel order");
    step(app.wait_for("text:✓ Playlist order"));
    step(app.press("text:Custom order"));
    step(app.wait_for("id:organization-row-0 >> text:Local B"));
    step(app.press("id:organization-screen >> text:Done"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    menu(app, "Channel order");
    step(app.wait_for("text:✓ Custom order"));
}

#[test]
fn tv_reorders_channels_in_the_guide_and_grabs_only_after_ok() {
    use nsh_core::platform::Idiom;

    let Some(home) = isolated!() else { return };
    let mut f = start(home, Settings::new(), false);
    let app = &mut f.app;
    step(app.set_environment("a TV", |env| env.idiom = Idiom::Tv));
    step(app.press_key("tab"));
    step(app.press_key("down"));
    step(app.press_key("right"));
    menu(app, "Reorder channels");
    step(app.wait_for("text:OK picks up or places · ↑/↓ move · ←/→ jump to either end"));
    step(app.wait_for("id:guide-row"));
    assert!(!app.screen().matches("id:organization-row-0").unwrap());
    assert!(!app.screen().matches("id:reorder-handle-0").unwrap());
    step(app.press_key("down"));
    step(app.wait_for("id:guide-row >> nth=0 >> text:★ Local A"));
    step(app.wait_for("text:OK picks up or places · ↑/↓ move · ←/→ jump to either end"));
    step(app.press_key("enter"));
    step(app.wait_for("text:OK picks up or places · ↑/↓ move · ←/→ jump to either end"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.back());
    step(app.wait_for("text:Save order"));
    step(app.press_key("enter"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    let db = crate::isolated::open_database(&f.path).unwrap();
    let group = db
        .groups(f.source, None)
        .unwrap()
        .into_iter()
        .find(|g| g.name == "News")
        .unwrap();
    let channels = db
        .load_channels(
            f.source,
            &ChannelQuery {
                scope: ChannelScope::Group(group.id.clone()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(channels[0].name, "Local B");
    let scope = iptv_library::OrganizationScope::GroupChannels(group.id);
    menu(app, "Channel order");
    step(app.wait_for("text:Playlist order"));
    step(app.press("text:Playlist order"));
    step(app.wait_for_gone("id:organization-sort", Duration::from_secs(5)));
    menu(app, "Channel order");
    step(app.wait_for("text:Custom order"));
    step(app.press("text:Custom order"));
    step(app.wait_for("text:OK picks up or places · ↑/↓ move · ←/→ jump to either end"));
    // Activating the saved Custom order is itself a change, even with no drag.
    step(app.back());
    step(app.wait_for("text:Save order"));
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:Save order", Duration::from_secs(5)));
    assert_eq!(
        db.guide_sort(f.source, &scope).unwrap(),
        iptv_core::organization::Sort::Manual
    );
}

#[test]
fn a_held_grip_scrolls_at_the_edge_and_moves_the_row_past_the_viewport() {
    use nsh_core::{Size, geometry::point, prelude::px};

    let Some(home) = isolated!() else { return };
    let mut f = start(home, Settings::new(), true);
    let app = &mut f.app;
    open_inline_order(app);
    let first_grip = "role:row[Local A] >> id:guide-reorder-handle";
    step(app.set_viewport(Size::new(px(720.0), px(205.0))));
    step(app.wait_for(first_grip));
    let nodes = app.screen().layout_nodes();
    let grip = nodes
        .iter()
        .filter(|n| n.test_id.as_deref() == Some("guide-reorder-handle"))
        .min_by(|a, b| a.bounds.origin.y.0.total_cmp(&b.bounds.origin.y.0))
        .unwrap();
    let mut node = grip;
    while node.scroll.is_none() {
        node = nodes.iter().find(|n| n.id == node.parent).unwrap();
    }
    let pitch = grip.bounds.size.height.0;
    let bottom = node.bounds.origin.y.0 + node.bounds.size.height.0;
    let distance_to_bottom = bottom - grip.bounds.center().y.0;
    assert!(
        distance_to_bottom < 2.0 * pitch,
        "the last row's center starts below the viewport"
    );
    // Reach the edge without moving far enough to place A in the last row
    // until the list itself scrolls under the held grip.
    let drag_y = (distance_to_bottom - 5.0).min(1.25 * pitch);
    assert!(drag_y > 0.0, "the first grip starts inside the viewport");
    step(app.drag_and_hold(first_grip, point(px(0.0), px(drag_y))));
    step(app.run_for(Duration::from_millis(550)));
    step(app.drop_held());
    step(app.set_viewport(Size::new(px(1920.0), px(1080.0))));
    step(app.wait_for("id:guide-row >> nth=2 >> text:★ Local A"));
    step(app.press("id:organization-screen >> text:Save"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    let rows = crate::isolated::open_database(&f.path)
        .unwrap()
        .load_channels(f.source, &ChannelQuery::default())
        .unwrap();
    assert_eq!(rows.last().unwrap().name, "Local A");
}

#[test]
fn custom_channel_order_from_a_group_menu_opens_the_drag_editor() {
    use nsh_core::{
        Size,
        geometry::point,
        platform::{Idiom, Os},
        prelude::px,
    };

    let Some(home) = isolated!() else { return };
    let mut f = start(home, Settings::new(), false);
    let app = &mut f.app;
    step(app.set_environment("a Linux desktop", |env| {
        env.os = Os::Linux;
        env.idiom = Idiom::Desktop;
    }));
    step(app.set_viewport(Size::new(px(1280.0), px(800.0))));
    step(app.press_key("tab"));
    step(app.wait_for("id:guide-groups >> text:News"));
    step(app.right_click("id:guide-groups >> text:News"));
    step(app.press("id:menu-panel >> text:Channel order"));
    for option in [
        "Playlist order",
        "Name (A–Z)",
        "Name (Z–A)",
        "Last watched (newest first)",
        "Last watched (oldest first)",
        "Watch time (most first)",
        "Watch time (least first)",
    ] {
        step(app.wait_for(&format!("id:organization-sort >> text:{option}")));
    }
    assert!(!app.screen().matches("id:organization-row-0").unwrap());
    step(app.wait_for("text:/Custom order$/"));
    step(app.press("text:/Custom order$/"));
    step(app.wait_for("id:organization-row-1 >> text:Local B"));
    step(app.wait_for("id:reorder-handle-1"));
    assert!(
        !app.screen()
            .matches("id:organization-screen >> text:Visible")
            .unwrap()
    );

    step(app.drag("id:reorder-handle-1", point(px(0.0), px(-52.0))));
    step(app.wait_for("id:organization-row-0 >> text:Local B"));
    step(app.press("id:organization-screen >> text:Done"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));

    let db = crate::isolated::open_database(&f.path).unwrap();
    let news = db
        .groups(f.source, None)
        .unwrap()
        .into_iter()
        .find(|group| group.name == "News")
        .unwrap();
    let channels = db
        .load_channels(
            f.source,
            &ChannelQuery {
                scope: ChannelScope::Group(news.id),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(channels[0].name, "Local B");
}

#[test]
fn fullscreen_channel_order_preserves_the_focused_channel() {
    let Some(home) = isolated!() else { return };
    fullscreen_channel_order(home, false);
}

#[test]
fn fullscreen_organization_returns_to_the_selected_overlay_without_reopening_video() {
    let Some(home) = isolated!() else { return };
    fullscreen_channel_order(home, true);
}

fn fullscreen_channel_order(home: crate::isolated::Home, overlay: bool) {
    let mut settings = Settings::new();
    settings
        .set("guide.stay_on_list", iptv_settings::Value::Bool(true))
        .unwrap();
    if overlay {
        settings
            .set("guide.browse_mode", iptv_settings::Value::choice("overlay"))
            .unwrap();
    }
    let visibility = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let observed = visibility.clone();
    let mut f = start_with_setup(home, settings, false, move |app| {
        app.observe(move |_, batch| {
            for item in &batch.items {
                if let nsh_core::MountItem::ViewProps { id, props } = item {
                    observed
                        .lock()
                        .unwrap()
                        .insert(*id, props.accessibility.hidden);
                }
            }
        });
    });
    let app = &mut f.app;
    let list_is_hidden = |screen: &nsh_testing::Screen| {
        screen
            .layout_nodes()
            .iter()
            .find(|node| node.test_id.as_deref() == Some("channel-list-layer"))
            .and_then(|node| visibility.lock().unwrap().get(&node.id).copied())
    };
    group(app, "News");
    crate::browse_fixture::choose_fullscreen(app);
    step(app.press_key("left"));
    step(
        app.wait_until("the channel list opens", Duration::from_secs(5), |screen| {
            list_is_hidden(screen) == Some(false)
        }),
    );
    step(app.wait_for("id:channel-list >> text:/^1  Local A/"));
    step(app.press_key("m"));
    step(app.press("id:menu-panel >> text:More options"));
    step(app.wait_for("id:menu-panel >> text:Channel order"));
    step(app.press("id:menu-panel >> text:Channel order"));
    step(app.wait_for("text:Name (Z–A)"));
    step(app.press("text:Name (Z–A)"));
    step(app.wait_for("id:channel-list >> text:/^2  Local A/"));

    // OK on the already playing, still focused A closes the list.
    step(app.press_key("enter"));
    step(app.wait_until(
        "the channel list closes",
        Duration::from_secs(5),
        |screen| list_is_hidden(screen) == Some(true),
    ));

    // Target B from the player list while A keeps playing.
    let opens = iptv_app::player_opens(0);
    let player = |app: &crate::Headless| {
        app.screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.kind == nsh_core::ViewKind::Native)
            .expect("persistent player")
    };
    let id = player(app).id;
    step(app.press_key("left"));
    step(app.wait_until(
        "the channel list reopens",
        Duration::from_secs(5),
        |screen| list_is_hidden(screen) == Some(false),
    ));
    step(app.press_key("up"));
    step(app.press_key("m"));
    step(app.press("id:menu-panel >> text:Reorder channel"));
    step(app.wait_for("id:organization-screen"));
    step(app.wait_for("id:guide-reorder-handle"));
    assert!(!app.shows("id:organization-row-0"));
    step(app.wait_for("id:guide-row >> nth=0 >> text:★ Local B"));
    step(app.press("id:organization-screen >> text:Cancel"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    assert!(
        app.screen()
            .accessibility_state("id:guide-ui >> label:Local B")
            .unwrap()
            .selected,
        "the guide keeps the channel targeted in the player list"
    );
    assert_eq!(player(app).id, id);
    assert_eq!(player(app).bounds.size.width.0 == 1920.0, overlay);
    assert_eq!(iptv_app::player_opens(0), opens);
    assert!(
        iptv_app::player_has_source(0),
        "the original stream stays open"
    );
}

#[test]
fn channel_context_reorders_inline_with_the_pointer() {
    use nsh_core::{geometry::point, platform::Idiom, prelude::px};

    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, Settings::new(), false);
    let app = &mut fixture.app;
    step(app.set_environment("desktop input", |env| {
        env.idiom = Idiom::Desktop;
        env.os = nsh_core::platform::Os::Linux;
    }));
    group(app, "News");
    step(app.right_click("id:guide-row >> text:/Local B/"));
    step(app.wait_for("id:menu-panel >> role:header[Local B]"));
    step(app.press("id:menu-panel >> text:Reorder channel"));
    let grip = "id:guide-row >> nth=1 >> id:guide-reorder-handle";
    step(app.wait_for(grip));
    step(app.drag(grip, point(px(0.0), px(-51.0))));
    step(app.wait_for("id:guide-row >> nth=0 >> text:/Local B/"));
    step(app.press("id:organization-screen >> text:Save"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    let store = crate::isolated::open_database(&fixture.path).unwrap();
    let news = store
        .groups(fixture.source, None)
        .unwrap()
        .into_iter()
        .find(|group| group.name == "News")
        .unwrap();
    let rows = store
        .load_channels(
            fixture.source,
            &ChannelQuery {
                scope: ChannelScope::Group(news.id),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(rows[0].name, "Local B");
}
