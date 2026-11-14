use crate::personalization;
use iptv_settings::Settings;
use iptv_store_sqlite::{ChannelQuery, ChannelScope};
use personalization::*;
use std::time::Duration;
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
    step(app.press("id:organization-row-1"));
    step(app.wait_for("text:Move to first"));
    step(app.press("text:Move to first"));
    step(app.back());
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
