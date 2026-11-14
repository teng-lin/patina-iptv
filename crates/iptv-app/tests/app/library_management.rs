//! Synthetic end-to-end editing; all storage is inside a temporary HOME.
use crate::headless::Headless;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_store_sqlite::{ChannelQuery, SourcePolicy};
use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use std::time::{Duration, Instant};

const PLAYLIST: &str = "#EXTM3U\n\
                        #EXTINF:-1 group-title=\"News\",Local A\n\
                        http://stream.example.test/a\n\
                        #EXTINF:-1 group-title=\"News\",Local B\n\
                        http://stream.example.test/b\n\
                        #EXTINF:-1 group-title=\"Sport\",Local C\n\
                        http://stream.example.test/c\n";
#[track_caller]
fn step(result: Result<(), String>) {
    if let Err(error) = result {
        panic!("{error}")
    }
}
fn rail(app: &mut Headless) {
    step(app.press("text:☰  All channels"));
    step(app.press_key("left"));
    step(app.press_key("left"));
}
// Use remote navigation for actions beyond the visible portion of the sheet.
fn edit(app: &mut Headless, row: usize, label: &str, index: usize) {
    step(app.press(&format!("id:organization-row-{row}")));
    for _ in 0..index {
        step(app.press_key("down"));
    }
    step(app.wait_for(&format!("text:{label}")));
    step(app.press_key("enter"));
}
fn channels(path: &std::path::Path, source: iptv_store_sqlite::SourceId) -> Vec<String> {
    crate::isolated::open_database(path)
        .unwrap()
        .load_channels(source, &ChannelQuery::default())
        .unwrap()
        .into_iter()
        .map(|r| r.name)
        .collect()
}
fn read_bindings(dir: &std::path::Path) -> Vec<iptv_settings::KeyBinding> {
    let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    let (settings, _) = iptv_settings::Settings::from_json(&text).unwrap();
    settings.key_bindings(
        iptv_settings::keys::KEY_OVERRIDES,
        &iptv_settings::Target::Global,
    )
}

fn saved_bindings(
    app: &mut Headless,
    dir: &std::path::Path,
    expected: impl Fn(&[iptv_settings::KeyBinding]) -> bool,
) -> Vec<iptv_settings::KeyBinding> {
    let deadline = Instant::now() + crate::isolated::scaled_duration(Duration::from_secs(10));
    loop {
        // The UI edits optimistically; its custom/default marker is not a
        // durability acknowledgement. Observe the actual atomic publication.
        if let Ok(text) = std::fs::read_to_string(dir.join("settings.json"))
            && let Ok((settings, _)) = iptv_settings::Settings::from_json(&text)
        {
            let bindings = settings.key_bindings(
                iptv_settings::keys::KEY_OVERRIDES,
                &iptv_settings::Target::Global,
            );
            if expected(&bindings) {
                return bindings;
            }
        }
        assert!(
            Instant::now() < deadline,
            "remote bindings did not reach the expected durable state"
        );
        step(app.wait(Duration::from_millis(10)));
    }
}
#[test]
fn organization_playlist_policies_and_remote_bindings_are_editable_and_persisted() {
    let Some(home) = isolated!() else { return };
    let dir = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let playlist_path = home.path().join("synthetic.m3u");
    std::fs::write(&playlist_path, PLAYLIST).unwrap();
    let m = iptv_providers::m3u::parse_m3u(PLAYLIST);
    let mut store = crate::isolated::open_database(&path).unwrap();
    let source = store
        .add_source(&Source {
            name: "Synthetic library".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(playlist_path.to_string_lossy()),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    store
        .replace_catalog(source, &m.groups, &m.channels.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(
            source,
            SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    step(app.wait_for_within("text:Local A", Duration::from_secs(20)));
    rail(&mut app);
    step(app.press("text:Playlists >> nth=0"));
    step(app.wait_for("text:Synthetic library"));
    step(app.press("text:Synthetic library"));
    // Channel editor is available by remote; hidden items remain in this manager.
    for _ in 0..6 {
        step(app.press_key("down"));
    }
    step(app.press_key("enter"));
    step(app.wait_for("text:Organize channels"));
    step(app.press("id:organization-row-0"));
    step(app.wait_for("text:Hide item"));
    step(app.press("text:Hide item"));
    assert!(
        crate::isolated::open_database(&path)
            .unwrap()
            .load_channels(
                source,
                &ChannelQuery {
                    include_hidden: true,
                    ..Default::default()
                }
            )
            .unwrap()[0]
            .hidden
    );
    step(app.press("id:organization-row-0"));
    step(app.wait_for("text:Show item"));
    step(app.press("text:Show item"));
    step(app.wait_for("id:organization-row-2"));
    step(app.press("id:organization-row-2"));
    step(app.wait_for("text:Move to first"));
    step(app.press("text:Move to first"));
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .load_channels(source, &ChannelQuery::default())
            .unwrap()[0]
            .name,
        "Local C"
    );
    // Rename, cancel and reset change overlays without changing provider identity.
    edit(&mut app, 0, "Rename", 1);
    step(app.clear("text:Local C"));
    step(app.type_text("text:Empty restores the provider name", "Renamed C"));
    step(app.submit("text:Renamed C"));
    assert_eq!(channels(&path, source)[0], "Renamed C");
    edit(&mut app, 0, "Rename", 1);
    step(app.type_text("text:Renamed C", " unsaved"));
    step(app.back());
    assert_eq!(channels(&path, source)[0], "Renamed C");
    step(app.back());
    edit(&mut app, 0, "Rename", 1);
    step(app.type_text("text:Renamed C", " discarded"));
    step(app.wait_for("text:Cancel"));
    step(app.press("text:Cancel"));
    assert_eq!(channels(&path, source)[0], "Renamed C");
    step(app.back());
    edit(&mut app, 0, "Rename", 1);
    step(app.clear("text:Renamed C"));
    step(app.submit("text:Empty restores the provider name"));
    assert_eq!(channels(&path, source)[0], "Local C");
    edit(&mut app, 0, "Use provider name", 2);
    assert_eq!(channels(&path, source)[0], "Local C");
    edit(&mut app, 0, "Move earlier", 3);
    assert_eq!(channels(&path, source)[0], "Local C");
    edit(&mut app, 0, "Move to last", 6);
    assert_eq!(channels(&path, source)[2], "Local C");
    edit(&mut app, 2, "Move earlier", 3);
    assert_eq!(channels(&path, source)[1], "Local C");
    edit(&mut app, 1, "Move later", 4);
    assert_eq!(channels(&path, source)[2], "Local C");
    edit(&mut app, 2, "Restore provider order", 7);
    assert_eq!(channels(&path, source), ["Local A", "Local B", "Local C"]);
    // Bulk selection preserves the unselected channel; hiding everything remains recoverable.
    edit(&mut app, 0, "Select / deselect for bulk visibility", 8);
    edit(&mut app, 1, "Select / deselect for bulk visibility", 8);
    edit(&mut app, 0, "Hide selected", 9);
    assert_eq!(channels(&path, source), ["Local C"]);
    edit(&mut app, 0, "Show selected", 10);
    assert_eq!(channels(&path, source).len(), 3);
    edit(&mut app, 0, "Select / deselect for bulk visibility", 8);
    edit(&mut app, 0, "Hide all", 11);
    assert!(channels(&path, source).is_empty());
    assert!(app.screen().matches("id:organization-row-2").unwrap());
    edit(&mut app, 0, "Show all", 12);
    assert_eq!(channels(&path, source).len(), 3);
    step(app.back());
    step(app.wait_for_gone("id:organization-screen", Duration::from_secs(2)));
    // Group editor uses stable provider identities and supports recovery too.
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for("text:Organize groups"));
    step(app.press("id:organization-row-0"));
    step(app.wait_for("text:Hide item"));
    step(app.press("text:Hide item"));
    assert!(
        crate::isolated::open_database(&path)
            .unwrap()
            .library_groups(source)
            .unwrap()[0]
            .hidden
    );
    step(app.press("id:organization-row-0"));
    step(app.wait_for("text:Show item"));
    step(app.press("text:Show item"));
    edit(&mut app, 0, "Rename", 1);
    step(app.clear("text:News"));
    step(app.type_text("text:Empty restores the provider name", "Local news"));
    step(app.submit("text:Local news"));
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .library_groups(source)
            .unwrap()[0]
            .name,
        "Local news"
    );
    edit(&mut app, 0, "Use provider name", 2);
    edit(&mut app, 0, "Move to last", 6);
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .library_groups(source)
            .unwrap()[1]
            .name,
        "News"
    );
    edit(&mut app, 1, "Move earlier", 3);
    edit(&mut app, 0, "Move later", 4);
    edit(&mut app, 1, "Move to first", 5);
    edit(&mut app, 0, "Restore provider order", 7);
    edit(&mut app, 0, "Hide all", 11);
    assert!(
        crate::isolated::open_database(&path)
            .unwrap()
            .library_groups(source)
            .unwrap()
            .iter()
            .all(|g| g.hidden)
    );
    edit(&mut app, 0, "Show all", 12);
    step(app.press("id:organization-screen >> text:Close"));
    // Back to actions, refresh interval and startup setting are persisted.
    for _ in 0..3 {
        step(app.press_key("up"));
    }
    step(app.press_key("enter"));
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .source_policy(source)
            .unwrap()
            .refresh_seconds,
        3600
    );
    step(app.press_key("down"));
    step(app.press_key("enter"));
    assert!(
        crate::isolated::open_database(&path)
            .unwrap()
            .source_policy(source)
            .unwrap()
            .refresh_on_start
    );
    step(app.press_key("up"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    assert!(
        !crate::isolated::open_database(&path)
            .unwrap()
            .source_policy(source)
            .unwrap()
            .enabled
    );
    step(app.press_key("enter"));
    assert!(
        crate::isolated::open_database(&path)
            .unwrap()
            .source_policy(source)
            .unwrap()
            .enabled
    );
    // Show the enabled playlist, open settings via the guide's Menu.
    for _ in 0..3 {
        step(app.press_key("up"));
    }
    step(app.press_key("enter"));
    step(app.wait_for_within("text:Local A", Duration::from_secs(20)));
    rail(&mut app);
    step(app.press("text:Settings >> nth=0"));
    step(app.wait_for("text:Remote control"));
    step(app.press("text:Remote control"));
    step(app.wait_for("text:Edit remote bindings"));
    step(app.press("text:Edit remote bindings"));
    step(app.wait_for("text:Remote bindings"));
    step(app.press("id:remote-Action")); // Toggle favourite -> Nothing.
    step(app.wait_for("id:remote-Save"));
    step(app.press("id:remote-Save"));
    step(app.wait_for("text:/^Currently: .* · custom binding$/"));
    let bindings = saved_bindings(&mut app, &dir, |bindings| bindings.len() == 1);
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].action, "ignore");
    // Replacing the same key requires confirmation; Back keeps the previous value.
    step(app.press("id:remote-Action"));
    step(app.wait_for("id:remote-Save"));
    step(app.press("id:remote-Save"));
    step(app.wait_for("text:Replace the existing binding?"));
    step(app.back());
    step(app.wait_for_gone("text:Replace the existing binding?", Duration::from_secs(2)));
    step(app.press("id:remote-Clear"));
    step(app.wait_for("text:/^Currently: .* · default binding$/"));
    assert!(saved_bindings(&mut app, &dir, |bindings| bindings.is_empty()).is_empty());
    // Separate contexts/presses, physical capture, accepted replacement, and reset confirmation.
    step(app.press("id:remote-Save"));
    step(app.wait_for("text:/^Currently: .* · custom binding$/"));
    assert_eq!(
        saved_bindings(&mut app, &dir, |bindings| bindings.len() == 1).len(),
        1
    );
    step(app.press("id:remote-Context"));
    step(app.wait_for("id:remote-Press"));
    step(app.press("id:remote-Press"));
    step(app.wait_for("id:remote-Capture"));
    step(app.press("id:remote-Capture"));
    step(app.press_key("green"));
    step(app.wait_for("text:Key: Green"));
    // A quiet headless batch does not acknowledge a queued settings action.
    // Observe the captured key's unsaved state, then its optimistic saved
    // marker, and separately wait for the durable file publication.
    step(app.wait_for("text:/^Currently: .* · default binding$/"));
    step(app.press("id:remote-Save"));
    step(app.wait_for("text:/^Currently: .* · custom binding$/"));
    let saved = saved_bindings(&mut app, &dir, |bindings| bindings.len() == 2);
    assert_eq!(saved.len(), 2);
    assert!(saved.iter().any(|b| b.context.as_deref() == Some("groups")
        && b.key == "green"
        && b.press == iptv_settings::PressKind::Long));
    step(app.press("id:remote-Capture"));
    step(app.press_key("back")); // Android then delivers its platform Back event.
    step(app.back());
    assert!(app.screen().matches("text:Remote bindings").unwrap());
    assert_eq!(read_bindings(&dir), saved);
    step(app.press("id:remote-Action"));
    step(app.wait_for("id:remote-Save"));
    step(app.press("id:remote-Save"));
    step(app.wait_for("text:Replace the existing binding?"));
    step(app.press_key("left"));
    step(app.press_key("enter"));
    // Replacement keeps the custom marker, so observe the confirmation closing
    // after save(), then observe the new action's file publication.
    step(app.wait_for_gone("text:Replace the existing binding?", Duration::from_secs(2)));
    let replaced = saved_bindings(&mut app, &dir, |bindings| {
        bindings.len() == 2
            && bindings
                .iter()
                .any(|b| b.key == "green" && b.action == "move_down")
    });
    assert_eq!(replaced.len(), 2);
    assert!(
        replaced
            .iter()
            .any(|b| b.key == "green" && b.action == "move_down")
    );
    step(app.press("id:remote-Reset"));
    step(app.wait_for("text:Restore all defaults?"));
    step(app.press("text:Cancel"));
    step(app.wait_for_gone("text:Restore all defaults?", Duration::from_secs(2)));
    assert_eq!(read_bindings(&dir).len(), 2);
    step(app.press("id:remote-Reset"));
    step(app.wait_for("text:Restore all defaults?"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:Restore all defaults?", Duration::from_secs(2)));
    step(app.wait_for("text:/^Currently: .* · default binding$/"));
    assert!(saved_bindings(&mut app, &dir, |bindings| bindings.is_empty()).is_empty());
    // Back closes the editor; reopening starts a fresh draft with defaults intact.
    step(app.back());
    step(app.wait_for_gone("id:remote-editor-screen", Duration::from_secs(2)));
    step(app.press("text:Edit remote bindings"));
    step(app.wait_for("id:remote-Key"));
    step(app.press("id:remote-Key"));
    step(app.wait_for("text:Key: Green"));
    step(app.press_key("left"));
    step(app.wait_for("text:Key: Red"));
    // The edited map must act immediately, rather than merely being serialized.
    for _ in 0..50 {
        if app.screen().matches("text:Action: Search").unwrap() {
            break;
        }
        step(app.press("id:remote-Action"));
    }
    step(app.wait_for("text:Action: Search"));
    step(app.press("id:remote-Save"));
    step(app.wait_for("text:/^Currently: .* · custom binding$/"));
    let bindings = saved_bindings(&mut app, &dir, |bindings| {
        bindings.len() == 1 && bindings[0].action == "open_search"
    });
    assert_eq!(bindings[0].action, "open_search");
    step(app.press("id:remote-editor-screen >> text:Close"));
    step(app.back());
    step(app.back());
    step(app.wait_for("text:☰  All channels"));
    step(app.press("text:☰  All channels"));
    step(app.press_key("right"));
    step(app.press_key("red"));
    step(app.wait_for("id:search-field"));
}
