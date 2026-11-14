//! Synthetic settings and playlist only; exercise the real choice editor and
//! reactive text bindings rather than rebuilding the application on a change.
use crate::personalization::{self, step};
use iptv_settings::{Settings, Target, Value, keys};
use std::time::Duration;

#[test]
fn interface_language_changes_live_persists_and_can_be_recovered() {
    let Some(home) = isolated!() else { return };
    let mut initial = Settings::new();
    initial.set("general.language", Value::text("en")).unwrap();
    let mut fixture = personalization::start(home, initial, false);
    let app = &mut fixture.app;
    // Keep the navigation rail mounted while settings are open, so a
    // language change must update its existing labels as well.
    step(app.press("text:/^☰  /"));
    step(app.press_key("left"));
    step(app.wait_for("text:Live TV"));
    step(app.press("label:Settings"));
    step(app.wait_for("text:Play the last channel on start"));
    step(app.press_key("right"));
    for _ in 0..15 {
        if app.screen().matches("text:Language").unwrap() && app.press("text:Language").is_ok() {
            break;
        }
        step(app.press_key("down"));
    }
    // The chooser's backdrop is mounted while closed; its name is read by
    // screen readers only.
    step(app.wait_for("label:Close the chooser"));
    step(app.wait_for("text:/Español/"));
    step(app.press("text:/Español/"));
    step(app.wait_for("text:Ajustes"));
    step(app.wait_for("text:Idioma"));
    step(app.wait_for("text:☰  Todos los canales"));
    step(app.wait_for("text:TV en directo"));
    step(app.wait_for("text:Reproducir el último canal al iniciar"));
    // Accessibility labels of mounted views follow: the rail's item, the
    // video's tap target and the chooser's backdrop.
    step(app.wait_for("label:Ajustes"));
    step(app.wait_for("label:Vídeo"));
    step(app.wait_for("label:Cerrar el selector"));
    assert!(!app.screen().matches("label:Close the chooser").unwrap());
    // A second switch while the settings remain mounted must update every
    // bound heading; native language names always remain readable.
    step(app.press("text:Idioma"));
    step(app.wait_for("text:/繁體中文/"));
    // Scroll through choices with the remote so the last entries are reachable.
    for _ in 0..7 {
        step(app.press_key("down"));
    }
    step(app.press("text:/繁體中文/"));
    step(app.wait_for("text:語言"));
    step(app.wait_for("text:直播電視"));
    step(app.wait_for("text:啟動時播放上次頻道"));
    step(app.wait_for("label:關閉選擇器"));
    let path = fixture.path.parent().unwrap().join("settings.json");
    let stored = personalization::saved_settings(app, &path, |settings| {
        settings.text(keys::LANGUAGE, &Target::Global) == "zh-Hant"
    });
    assert_eq!(stored.text(keys::LANGUAGE, &Target::Global), "zh-Hant");
    step(app.press("text:語言"));
    for _ in 0..12 {
        step(app.press_key("up"));
    }
    step(app.press("text:/系統語言/"));
    step(app.wait_for("text:Language"));
    step(app.wait_for("text:Settings >> nth=1"));
    step(app.wait_for("label:Close the chooser"));
    for _ in 0..3 {
        step(app.back());
    }
    step(app.wait_for_within("text:★ Local A", Duration::from_secs(5)));
}
