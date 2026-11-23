//! Explicit browsing policy for older player journeys. Default delay/close
//! behavior is exercised by the personalization integration tests.
pub fn two_step_preview(home: &std::path::Path) {
    let mut s = iptv_settings::Settings::new();
    s.set("guide.stay_on_guide", iptv_settings::Value::Bool(true))
        .unwrap();
    s.set("guide.stay_on_list", iptv_settings::Value::Bool(true))
        .unwrap();
    s.set("guide.switch_delay", iptv_settings::Value::ms(0))
        .unwrap();
    let dir = crate::isolated::data_dir(home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("settings.json"), s.to_json_string()).unwrap();
}

/// From the guide, open the rail beside the groups and tap one of its items.
#[track_caller]
pub fn open_rail_item(app: &mut crate::Headless, label: &str) {
    use crate::personalization::step;

    let item = format!("id:navigation-rail >> label:{label}");
    if !app.screen().matches(&item).unwrap() {
        step(app.press("id:guide-list-selector"));
        step(app.wait_for(&item));
    }
    step(app.press(&item));
}

/// Open the playlist form from the guide's navigation rail.
#[track_caller]
pub fn open_add_playlist(app: &mut crate::Headless) {
    use crate::personalization::step;

    open_rail_item(app, "Playlists");
    let add = "id:playlists-screen >> text:+ Add playlist";
    step(app.wait_for(add));
    step(app.press(add));
}

/// Start the focused live channel in the guide, then expand its preview.
#[track_caller]
pub fn choose_fullscreen(app: &mut crate::Headless) {
    activate_twice(app, |app| app.press_key("enter"));
}

/// Start a live channel by tapping its guide row, then expand its preview.
#[track_caller]
pub fn tap_fullscreen(app: &mut crate::Headless, row: &str) {
    activate_twice(app, |app| app.press(row));
}

#[track_caller]
fn activate_twice(
    app: &mut crate::Headless,
    activate: impl Fn(&mut crate::Headless) -> Result<(), String>,
) {
    use crate::personalization::step;
    use std::time::Duration;

    step(activate(app));
    step(app.wait_until(
        "the live channel opens in the preview",
        Duration::from_secs(5),
        |_| iptv_app::player_has_source(0),
    ));
    step(activate(app));
}
