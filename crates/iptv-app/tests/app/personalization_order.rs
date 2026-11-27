use crate::personalization;
use iptv_settings::Settings;
use iptv_store_sqlite::{ChannelQuery, ChannelScope};
use personalization::*;
use std::time::Duration;

#[track_caller]
pub(super) fn wait_visible_order(app: &mut crate::Headless, expected: &[&str]) {
    step(app.wait_until(
        &format!("guide rows in order: {expected:?}"),
        Duration::from_secs(5),
        |screen| {
            let nodes = screen.layout_nodes();
            let mut previous_y = f32::NEG_INFINITY;
            expected.iter().all(|name| {
                let Some(row) = nodes.iter().find(|node| {
                    node.text.as_ref().is_some_and(|text| {
                        text.text.text.trim().trim_start_matches('★').trim() == *name
                    })
                }) else {
                    return false;
                };
                let y = row.bounds.origin.y.0;
                let next = y > previous_y;
                previous_y = y;
                next
            })
        },
    ));
}

#[test]
fn group_and_channel_sort_choices_apply_and_preserve_custom_positions() {
    use iptv_core::organization::Sort;
    use iptv_library::OrganizationScope;
    use iptv_providers::model::ContentKind;
    use nsh_core::platform::{Idiom, Os};

    let Some(home) = isolated!() else { return };
    let playlist = r#"#EXTM3U
#EXTINF:-1 group-title="Sport",Local C
http://example.test/c
#EXTINF:-1 group-title="Sport",Local A
http://example.test/a
#EXTINF:-1 group-title="Sport",Local B
http://example.test/b
#EXTINF:-1 group-title="Arts",Local D
http://example.test/d
#EXTINF:-1 group-title="News",Local E
http://example.test/e
"#;
    let mut fixture = launch_playlist(
        home,
        Settings::new(),
        false,
        playlist,
        |path, source| {
            let mut store = crate::isolated::open_database(path).unwrap();
            let catalog = iptv_providers::m3u::parse_m3u(playlist);
            let sport = catalog
                .groups
                .iter()
                .find(|group| group.name == "Sport")
                .unwrap();
            let news = catalog
                .groups
                .iter()
                .find(|group| group.name == "News")
                .unwrap();
            store.move_group(source, &news.id, 0).unwrap();
            let scope = OrganizationScope::GroupChannels(sport.id.clone());
            let provider = ["Local C", "Local A", "Local B"].map(|name| {
                catalog
                    .channels
                    .items
                    .iter()
                    .find(|channel| channel.name == name)
                    .unwrap()
                    .id
                    .as_str()
                    .to_owned()
            });
            let custom = [
                provider[2].clone(),
                provider[0].clone(),
                provider[1].clone(),
            ];
            store
                .save_guide_order(source, &scope, &provider, &custom)
                .unwrap();
            for (name, last, elapsed) in [
                ("Local A", 3, 1_000),
                ("Local B", 1, 9_000),
                ("Local C", 2, 4_000),
            ] {
                let channel = catalog
                    .channels
                    .items
                    .iter()
                    .find(|channel| channel.name == name)
                    .unwrap();
                store
                    .record_watch(source, &channel.id, ContentKind::Live, last, None)
                    .unwrap();
                store
                    .add_channel_watch_time(source, &channel.id, elapsed)
                    .unwrap();
            }
        },
        |config| config.environment.idiom = Idiom::Desktop,
        |_| {},
    );
    let app = &mut fixture.app;
    step(app.set_environment("desktop", |env| {
        env.idiom = Idiom::Desktop;
        env.os = Os::Linux;
    }));
    let db = crate::isolated::open_database(&fixture.path).unwrap();
    let sport = db
        .groups(fixture.source, Some(ContentKind::Live))
        .unwrap()
        .into_iter()
        .find(|group| group.name == "Sport")
        .unwrap();
    let sport_scope = OrganizationScope::GroupChannels(sport.id);

    let pick = |app: &mut crate::Headless, label, sort| {
        step(app.press(&format!("id:organization-sort >> text:/^(✓ )?{label}$/")));
        if sort == Sort::Manual {
            step(app.press("id:organization-screen >> text:Done"));
        }
        step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    };
    let group_heading = "id:guide-groups >> role:tab[Sport]";
    let mut group_mode = "Custom order";
    for (option, label, sort, groups) in [
        (
            "Playlist order",
            "Playlist order",
            Sort::Provider,
            ["Sport", "Arts", "News"],
        ),
        (
            r"Name \(A–Z\)",
            "Name (A–Z)",
            Sort::Name,
            ["Arts", "News", "Sport"],
        ),
        (
            r"Name \(Z–A\)",
            "Name (Z–A)",
            Sort::NameDescending,
            ["Sport", "News", "Arts"],
        ),
        (
            "Custom order",
            "Custom order",
            Sort::Manual,
            ["News", "Sport", "Arts"],
        ),
    ] {
        step(app.open_guide_groups());
        step(app.right_click(group_heading));
        step(app.press(&format!("id:menu-panel >> text:Group order: {group_mode}")));
        pick(app, option, sort);
        step(app.open_guide_groups());
        wait_visible_order(app, &groups);
        assert_eq!(
            db.guide_sort(fixture.source, &OrganizationScope::Groups)
                .unwrap(),
            sort
        );
        group_mode = label;
    }

    group(app, "Sport");
    let cell = "id:guide-ui >> role:cell[No information] >> nth=1";
    let mut channel_mode = "Custom order";
    for (option, label, sort, channels) in [
        (
            "Playlist order",
            "Playlist order",
            Sort::Provider,
            ["Local C", "Local A", "Local B"],
        ),
        (
            r"Name \(A–Z\)",
            "Name (A–Z)",
            Sort::Name,
            ["Local A", "Local B", "Local C"],
        ),
        (
            r"Name \(Z–A\)",
            "Name (Z–A)",
            Sort::NameDescending,
            ["Local C", "Local B", "Local A"],
        ),
        (
            r"Last watched \(newest first\)",
            "Last watched (newest first)",
            Sort::LastWatched,
            ["Local A", "Local C", "Local B"],
        ),
        (
            r"Last watched \(oldest first\)",
            "Last watched (oldest first)",
            Sort::OldestWatched,
            ["Local B", "Local C", "Local A"],
        ),
        (
            r"Watch time \(most first\)",
            "Watch time (most first)",
            Sort::MostWatched,
            ["Local B", "Local C", "Local A"],
        ),
        (
            r"Watch time \(least first\)",
            "Watch time (least first)",
            Sort::LeastWatched,
            ["Local A", "Local C", "Local B"],
        ),
        (
            "Custom order",
            "Custom order",
            Sort::Manual,
            ["Local B", "Local C", "Local A"],
        ),
    ] {
        step(app.right_click(cell));
        let row = format!("Channel order: {channel_mode}");
        let texts = app.panel_texts("menu-panel");
        assert!(
            texts.ends_with(&["Reorder channel".to_owned(), row.clone()]),
            "{texts:?}"
        );
        step(app.press(&format!("id:menu-panel >> text:{row}")));
        pick(app, option, sort);
        for (index, name) in channels.iter().enumerate() {
            step(app.wait_for(&format!("id:guide-row >> nth={index} >> text:★ {name}")));
        }
        assert_eq!(db.guide_sort(fixture.source, &sport_scope).unwrap(), sort);
        channel_mode = label;
    }
    step(app.right_click(cell));
    step(app.wait_for("id:menu-panel >> text:Channel order: Custom order"));
}

#[track_caller]
fn open_inline_order(app: &mut crate::headless::Headless) {
    menu(app, "Reorder channel");
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
    menu(app, "/^Channel order:/");
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
    menu(app, "/^Channel order:/");
    step(app.wait_for("text:/Custom order$/"));
    step(app.press("text:/Custom order$/"));
    step(app.wait_for("id:reorder-handle-0"));
    step(app.wait_for("id:organization-row-0 >> text:Local B"));
    // Custom positions are only activated by saving the editor.
    step(app.press("id:organization-screen >> text:Close"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    menu(app, "/^Channel order:/");
    step(app.wait_for("text:✓ Playlist order"));
    step(app.press("text:Custom order"));
    step(app.wait_for("id:organization-row-0 >> text:Local B"));
    step(app.press("id:organization-screen >> text:Done"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    menu(app, "/^Channel order:/");
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
    menu(app, "Reorder channel");
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
    menu(app, "/^Channel order:/");
    step(app.wait_for("text:Playlist order"));
    step(app.press("text:Playlist order"));
    step(app.wait_for_gone("id:organization-sort", Duration::from_secs(5)));
    menu(app, "/^Channel order:/");
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
    step(app.press("id:menu-panel >> text:/^Channel order:/"));
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
    step(app.wait_for("id:menu-panel >> text:/^Channel order:/"));
    step(app.press("id:menu-panel >> text:/^Channel order:/"));
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
            .find(|node| node.test_id.as_deref() == Some("video-player"))
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
fn tablet_channel_grip_keeps_a_diagonal_drag_across_the_timeline() {
    use nsh_core::{geometry::point, platform::Idiom, prelude::px};

    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, Settings::new(), false);
    let app = &mut fixture.app;
    step(app.set_environment("Android tablet", |env| {
        env.idiom = Idiom::Pad;
        env.os = nsh_core::platform::Os::Android;
    }));
    open_inline_order(app);
    let strip = "id:reorder-grip-strip";
    step(app.wait_for(strip));
    let second = "id:guide-row >> nth=1 >> text:★ Local B";
    step(app.drag(second, point(px(0.0), px(-50.0))));
    step(app.wait_for(second));
    // The three handles share a hit target outside the native list. Its
    // center is B. Start sideways across the timeline's pan threshold.
    step(app.drag(strip, point(px(90.0), px(-50.0))));
    let first = "id:guide-row >> nth=0 >> text:★ Local B";
    step(app.wait_for(first));
    step(app.press("id:organization-screen >> text:Save"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    step(app.wait_for(first));
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
