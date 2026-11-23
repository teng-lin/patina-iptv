use crate::personalization;
use iptv_settings::Settings;
use iptv_store_sqlite::{ChannelQuery, ChannelScope};
use personalization::*;
use std::time::Duration;

#[test]
fn copy_channels_from_the_guide_is_scoped_to_its_group() {
    let Some(home) = isolated!() else { return };
    let mut f = start(home, Settings::new(), false);
    let app = &mut f.app;
    group(app, "News");
    menu(app, "Copy channels to a group…");
    step(app.wait_for("text:0 selected · 2 matching · 2 total"));
    assert!(!app.shows("id:custom-groups-screen >> text:Local C"));
}

#[test]
fn custom_groups_filter_select_copy_move_rename_delete_and_restore_focus() {
    let Some(home) = isolated!() else { return };
    let mut f = start(home, Settings::new(), false);
    let app = &mut f.app;
    menu(app, "New group…");
    step(app.wait_for("id:custom-groups-screen"));
    // Blank and canceled drafts do not create groups.
    step(app.press("text:New group"));
    step(app.submit("text:Enter text"));
    assert!(
        crate::isolated::open_database(&f.path)
            .unwrap()
            .custom_groups(f.source)
            .unwrap()
            .is_empty()
    );
    step(app.press("text:Cancel"));
    new_group(app, "My group");
    new_group(app, "Second group");
    step(app.press("text:Browse all channels"));
    step(app.wait_for("text:Search · all channels"));
    step(app.press("text:Search · all channels"));
    step(app.type_text("text:Enter text", "local a"));
    step(app.submit("text:local a"));
    step(app.wait_for("text:0 selected · 1 matching · 3 total"));
    // Remote navigation reaches selection, clearing and individual rows.
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for("text:0 selected · 1 matching · 3 total"));
    step(app.press_key("down"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for("text:1 selected · 1 matching · 3 total"));
    step(app.press_key("up"));
    step(app.wait_for("text:Search · local a"));
    step(app.press("text:Search · local a"));
    step(app.clear("text:local a"));
    step(app.type_text("text:Enter text", "local b"));
    step(app.submit("text:local b"));
    step(app.wait_for("text:Select filtered"));
    step(app.press("text:Select filtered"));
    step(app.wait_for("text:2 selected · 1 matching · 3 total"));
    step(app.press("text:Copy selected"));
    step(app.back());
    step(app.wait_for("text:2 selected · 1 matching · 3 total"));
    step(app.press("text:Copy selected"));
    step(app.wait_for("text:My group · 0 channels"));
    step(app.press("text:My group · 0 channels"));
    step(app.wait_for("text:2 channels copied."));
    step(app.wait_for("text:0 selected · 1 matching · 3 total"));
    step(app.back());
    step(app.wait_for("text:My group · 2 channels"));
    step(app.press("text:My group · 2 channels"));
    step(app.wait_for("text:Edit members"));
    step(app.press("text:Edit members"));
    step(app.wait_for("text:Select filtered"));
    step(app.press("text:Select filtered"));
    step(app.wait_for("text:Move selected"));
    step(app.press("text:Move selected"));
    assert!(
        !app.shows("My group · 2 channels"),
        "cannot move to the source group"
    );
    step(app.press("text:Second group · 0 channels"));
    step(app.wait_for("text:2 channels moved."));
    step(app.wait_for("text:0 selected · 0 matching · 0 total"));
    let groups = crate::isolated::open_database(&f.path)
        .unwrap()
        .custom_groups(f.source)
        .unwrap();
    assert_eq!(groups[0].channel_count, 0);
    assert_eq!(groups[1].channel_count, 2);
    step(app.back());
    step(app.back());
    step(app.wait_for("text:Second group · 2 channels"));
    step(app.press("text:Second group · 2 channels"));
    step(app.wait_for("text:Rename group"));
    step(app.press("text:Rename group"));
    step(app.clear("text:Second group"));
    step(app.type_text("text:Enter text", "Renamed group"));
    step(app.submit("text:Renamed group"));
    step(app.wait_for("text:Reorder members"));
    step(app.press("text:Reorder members"));
    step(app.wait_for("id:organization-row-1"));
    step(app.drag(
        "id:reorder-handle-1",
        nsh_core::geometry::point(nsh_core::prelude::px(0.0), nsh_core::prelude::px(-52.0)),
    ));
    step(app.press("id:organization-screen >> text:Done"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    let s = crate::isolated::open_database(&f.path).unwrap();
    let id = s.custom_groups(f.source).unwrap()[1].id;
    let members = s
        .load_channels(
            f.source,
            &ChannelQuery {
                scope: ChannelScope::Custom(id),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(members[0].name, "Local B");
    drop(s);
    step(app.press("text:Edit members"));
    step(app.wait_for("text:Search · all channels"));
    step(app.press("text:Search · all channels"));
    step(app.type_text("text:Enter text", "local b"));
    step(app.submit("text:local b"));
    step(app.wait_for("text:Select filtered"));
    step(app.press("text:Select filtered"));
    step(app.wait_for("text:Remove selected"));
    step(app.press("text:Remove selected"));
    assert_eq!(
        crate::isolated::open_database(&f.path)
            .unwrap()
            .load_channels(
                f.source,
                &ChannelQuery {
                    scope: ChannelScope::Custom(id),
                    ..Default::default()
                }
            )
            .unwrap()
            .len(),
        1
    );
    step(app.back());
    // Delete confirmation defaults to Cancel; remote OK keeps the group.
    step(app.press("text:Delete group"));
    step(app.press_key("enter"));
    assert_eq!(
        crate::isolated::open_database(&f.path)
            .unwrap()
            .custom_groups(f.source)
            .unwrap()
            .len(),
        2
    );
    step(app.press("text:Delete group"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for("text:1 custom group"));
    assert_eq!(
        crate::isolated::open_database(&f.path)
            .unwrap()
            .load_channels(f.source, &ChannelQuery::default())
            .unwrap()
            .len(),
        3
    );
    step(app.back());
    step(app.wait_for_gone("id:custom-groups-screen", Duration::from_secs(2)));
    group(app, "My group");
    step(app.wait_for("text:This group has no channels."));
    // An empty custom list remains addressable in the guide and manageable.
    step(app.press("text:/^☰  /"));
    step(app.wait_for("text:All channels >> nth=0"));
    step(app.press("text:All channels >> nth=0"));
    step(app.press_key("right"));
    step(app.wait_for("text:★ Local A"));
}

#[test]
fn a_group_edit_finishes_after_its_editor_is_closed_during_write_contention() {
    let Some(home) = isolated!() else { return };
    let mut f = start(home, Settings::new(), false);
    let app = &mut f.app;
    menu(app, "New group…");
    step(app.wait_for("id:custom-groups-screen"));
    step(app.press("text:New group"));
    step(app.type_text("text:Enter text", "Pending synthetic group"));
    let lock = rusqlite::Connection::open(&f.path).unwrap();
    lock.pragma_update(None, "key", format!("x'{}'", "54".repeat(32)))
        .unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    step(app.submit("text:Pending synthetic group"));
    // Dismiss the text editor and the group screen while the commit is blocked.
    step(app.back());
    step(app.back());
    step(app.wait_for_gone("id:custom-groups-screen", Duration::from_secs(2)));
    lock.execute_batch("ROLLBACK").unwrap();
    // The late completion must update the guide, without reopening its editor.
    step(app.press("text:/^☰  /"));
    step(app.wait_for_within(
        "id:guide-groups >> text:Pending synthetic group",
        Duration::from_secs(10),
    ));
    assert!(!app.shows("id:custom-groups-screen"));
}

#[test]
fn guide_group_controls_and_channel_visibility_work_without_a_pin() {
    let Some(home) = isolated!() else { return };
    let mut f = start(home, Settings::new(), false);
    let app = &mut f.app;
    group(app, "News");
    let checked = |state| {
        move |screen: &nsh_testing::Screen| {
            screen
                .accessibility_state("id:organization-row-1")
                .is_ok_and(|row| row.checked == Some(state))
        }
    };
    menu(app, "Manage hidden channels…");
    step(app.wait_for("text:Show in guide"));
    step(app.wait_until(
        "visible channel switch",
        Duration::from_secs(5),
        checked(nsh_core::Checked::True),
    ));
    step(app.press_key("right"));
    assert!(checked(nsh_core::Checked::True)(app.screen()));
    step(app.press("text:Hide all"));
    step(app.wait_for("text:Confirm"));
    step(app.press_key("enter")); // Cancel is the default.
    step(app.wait_for_gone("text:Confirm", Duration::from_secs(5)));
    assert!(checked(nsh_core::Checked::True)(app.screen()));
    step(app.press("id:organization-row-1"));
    step(app.wait_until(
        "hidden channel switch",
        Duration::from_secs(5),
        checked(nsh_core::Checked::False),
    ));
    step(app.back());
    step(app.wait_for_gone("text:★ Local B", Duration::from_secs(5)));

    menu(app, "Manage hidden channels…");
    step(app.wait_until(
        "hidden channel switch",
        Duration::from_secs(5),
        checked(nsh_core::Checked::False),
    ));
    step(app.press("id:organization-row-1"));
    step(app.wait_until(
        "visible channel switch",
        Duration::from_secs(5),
        checked(nsh_core::Checked::True),
    ));
    step(app.back());
    step(app.wait_for("text:★ Local B"));

    menu(app, "Manage blocked channels…");
    step(app.wait_for("id:organization-row-0 >> text:Unblocked"));
    step(app.press_key("enter"));
    step(app.wait_for("id:organization-row-0 >> text:Blocked"));
    assert!(!app.shows("id:parental-screen"));
    step(app.back());
    step(app.press_key("enter"));
    step(app.wait_for("text:Unblock this channel to play it."));
    menu(app, "Unblock channel");
    step(app.wait_for("text:Content unblocked"));

    // The desktop's group context can be opened without first entering it.
    step(app.set_environment("desktop", |env| {
        env.os = nsh_core::platform::Os::Linux;
        env.idiom = nsh_core::platform::Idiom::Desktop;
    }));
    step(app.set_viewport(nsh_core::Size::new(
        nsh_core::prelude::px(1280.0),
        nsh_core::prelude::px(800.0),
    )));
    step(app.press_key("tab"));
    step(app.wait_for("id:guide-groups >> text:News"));
    step(app.press_key("shift+f10"));
    step(app.wait_for("id:menu-panel >> text:Manage groups"));
    step(app.back());
    step(app.right_click("id:guide-groups >> text:News"));
    step(app.press("id:menu-panel >> text:Rename group"));
    step(app.wait_for("text:Display name"));
    step(app.clear("text:News"));
    step(app.type_text("text:Empty restores the provider name", "Generated news"));
    step(app.submit("text:Generated news"));
    step(app.wait_for_gone("text:Display name", Duration::from_secs(5)));
    step(app.wait_for("id:guide-groups >> text:Generated news"));
    step(app.right_click("id:guide-groups >> text:Generated news"));
    step(app.press("id:menu-panel >> text:Restore group name"));
    step(app.wait_for("id:guide-groups >> text:News"));
    step(app.right_click("id:guide-groups >> text:News"));
    step(app.wait_for("id:menu-panel >> text:Hide group"));
    step(app.press("text:Block group"));
    step(app.wait_for("text:Content blocked"));
    step(app.press_key("right"));
    step(app.press_key("enter"));
    step(app.wait_for("text:Unblock the group to play this channel."));
    menu(app, "Group options…");
    step(app.wait_for("text:Unblock group"));
    step(app.press("text:Unblock group"));
    step(app.wait_for("text:Content unblocked"));

    menu(app, "Group options…");
    step(app.press("text:Hide group"));
    step(app.wait_for_gone("text:★ Local A", Duration::from_secs(5)));
    menu(app, "Group options…");
    step(app.press("text:Manage groups"));
    step(app.wait_for("id:organization-row-0 >> text:Hidden"));
    step(app.press("id:organization-row-0"));
    step(app.press("text:Show group"));
    step(app.wait_for("id:organization-row-0 >> text:Visible"));
    step(app.back());

    step(app.press_key("tab"));
    step(app.right_click("id:guide-groups >> text:Sport"));
    step(app.press("id:menu-panel >> text:Manage groups"));
    step(app.wait_for("id:organization-row-1 >> text:Sport"));
    step(app.press("id:organization-row-1"));
    step(app.press("text:Reorder groups"));
    step(app.wait_for("id:organization-row-1 >> text:Sport"));
    step(app.press_key("enter"));
    step(app.press_key("left"));
    step(app.back());
    step(app.wait_for("text:Save order"));
    step(app.press("text:Save order"));
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(5)));
    let store = crate::isolated::open_database(&f.path).unwrap();
    assert_eq!(store.guide_groups(f.source).unwrap()[0].name, "Sport");
    assert!(store.parental_credential().unwrap().is_none());
}

#[test]
fn a_repeated_notice_keeps_its_full_time_after_the_earlier_one_expires() {
    // A notice shows for 2.5 s of the app's clock.
    const SHOWN: Duration = Duration::from_millis(2_500);
    let Some(home) = isolated!() else { return };
    let mut f = start(home, Settings::new(), true);
    let app = &mut f.app;
    group(app, "News");
    menu(app, "Manage blocked channels…");
    step(app.wait_for("id:organization-row-0 >> text:Unblocked"));
    step(app.press_key("enter"));
    step(app.wait_for("text:Content blocked"));
    // No later than the first notice was shown: it ends by first + SHOWN.
    let first = app.clock().now();
    step(app.run_for(Duration::from_millis(1_000)));
    step(app.press_key("down"));
    // No earlier than the second notice is shown: it lasts past second + SHOWN.
    let second = app.clock().now();
    step(app.press_key("enter"));
    step(app.wait_for("id:organization-row-1 >> text:Blocked"));
    // Waiting on a manual clock moves it while the store writes: just past
    // the first notice's end must still be inside the second one's time.
    let past_first = first + SHOWN + Duration::from_millis(100);
    let now = app.clock().now();
    assert!(now < past_first);
    step(app.run_for(past_first - now));
    assert!(app.clock().now() < second + SHOWN);
    assert!(app.shows("Content blocked"), "{:?}", app.screen().texts());
    step(app.run_for(SHOWN));
    assert!(!app.shows("Content blocked"), "{:?}", app.screen().texts());
}

#[test]
fn standalone_group_preferences_apply_to_the_selected_group() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, Settings::new(), false);
    let app = &mut fixture.app;
    group(app, "News");
    step(app.long_press("id:guide-row >> text:/Local B/"));
    step(app.press("id:menu-panel >> text:Remove from favorites"));
    step(app.wait_for("id:guide-row >> text:Local B"));
    let news = "id:guide-groups >> text:News";
    let menu = "id:menu-panel";
    step(app.press("text:/^☰  /"));
    step(app.long_press(news));
    step(app.wait_for("id:menu-panel >> role:header[News]"));
    assert!(!app.shows("id:menu-panel >> text:Manage favorites…"));
    step(app.press("id:menu-panel >> text:Show favorites only"));
    step(app.wait_for_gone("id:guide-row >> text:/Local B/", Duration::from_secs(5)));
    step(app.long_press(news));
    step(app.press("id:menu-panel >> text:Guide time offset"));
    step(app.press_key("right"));
    step(app.wait_for(&format!("{menu} >> text:Guide time offset · +0:15")));
    step(app.press("id:menu-panel >> text:Save"));
    step(crate::isolated::wait_outside(
        app,
        "group offset saved",
        Duration::from_secs(5),
        || {
            let store = crate::isolated::open_database(&fixture.path).unwrap();
            let groups = store.groups(fixture.source, None).unwrap();
            groups.into_iter().any(|group| {
                group.name == "News"
                    && store
                        .guide_group_preferences(fixture.source, group.id.as_str())
                        .is_ok_and(|prefs| prefs.favorites_only && prefs.epg_offset_minutes == 15)
            })
        },
    ));
}
