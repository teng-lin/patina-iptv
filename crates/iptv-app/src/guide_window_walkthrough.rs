//! Explicit synthetic walkthrough of the real App. Absent from release builds.
use crate::Ui;
use nsh_core::prelude::*;
use std::time::Duration;
pub fn bind(ui: Ui) {
    if std::env::var_os("PATINA_GUIDE_WINDOW_WALKTHROUGH").is_none() {
        return;
    }
    let _guide = set_timeout(Duration::from_secs(2), move || {
        ui.mode.apply(crate::mode::Action::WalkthroughPreview);
        println!("DEMO: the actual guide; the main channel continues in its preview");
    });
    let _menu = set_timeout(Duration::from_secs(4), move || {
        ui.mode.apply(crate::mode::Action::WalkthroughPreview);
        let schedule = crate::runtime_owner::schedule();
        if let Some(channel) = std::env::var("PATINA_GUIDE_SECOND_KEY")
            .ok()
            .and_then(|key| schedule.find_channel(&key))
        {
            crate::menu_view::open_at(ui, channel);
        }
        println!("DEMO: channel context menu offers Open in new window");
    });
    let _open = set_timeout(Duration::from_secs(9), move || {
        crate::menu_view::perform(ui, iptv_presentation::menu::MenuItem::WatchInNewWindow);
        println!("DEMO: channel two plays in a new window; main playback unchanged");
    });
    let _next_menu = set_timeout(Duration::from_secs(14), move || {
        ui.mode.apply(crate::mode::Action::WalkthroughPreview);
        let schedule = crate::runtime_owner::schedule();
        if let Some(channel) = std::env::var("PATINA_GUIDE_THIRD_KEY")
            .ok()
            .and_then(|key| schedule.find_channel(&key))
        {
            crate::menu_view::open_at(ui, channel);
        }
    });
    let _next = set_timeout(Duration::from_secs(18), move || {
        crate::menu_view::perform(ui, iptv_presentation::menu::MenuItem::WatchInNewWindow);
        println!("DEMO: two independent windows and the original guide/player in one process");
    });
}
