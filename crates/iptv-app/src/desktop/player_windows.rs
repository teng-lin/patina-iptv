//! Linux guide-menu prototype: auxiliary players belong to the app, independently
//! of the main guide's navigation and the multiview layout.
use crate::{Ui, i18n::tr};
use iptv_core::playback_windows::SessionId;
use nsh_core::prelude::*;

pub fn mounted(ui: Ui, surface: usize) -> bool {
    cfg!(target_os = "linux") && ui.player_windows.with(|w| w.on_surface(surface).is_some())
}

pub fn can_open(ui: Ui, channel: usize) -> bool {
    if !cfg!(target_os = "linux") || crate::multiview::session(ui) {
        return false;
    }
    let Some(key) = crate::schedule().channel_key(channel) else {
        return false;
    };
    if ui
        .player_windows
        .with(|w| w.sessions().iter().any(|s| s.channel == key))
    {
        return true;
    }
    let count = ui.player_windows.with(|w| w.sessions().len());
    count < crate::player::SURFACES - 1
        && crate::multiview::account_limit()
            .is_none_or(|limit| count + usize::from(ui.playing.get().is_some()) < limit as usize)
}

pub fn open(ui: Ui, channel: usize) {
    if !can_open(ui, channel) {
        return;
    }
    let Some(key) = crate::schedule().channel_key(channel) else {
        return;
    };
    if ui
        .player_windows
        .with_untracked(|w| w.sessions().iter().any(|s| s.channel == key))
    {
        return;
    }
    let mut surface = None;
    ui.player_windows.update(|w| {
        let audible = w.audible;
        if let Some(id) = w.open_auxiliary(key) {
            surface = w.sessions().iter().find(|s| s.id == id).map(|s| s.surface);
        }
        // Opening another channel leaves the main player's audio unchanged.
        w.audible = audible;
    });
    if let Some(surface) = surface {
        crate::player::start_surface(ui, surface, channel);
    }
}

pub fn close(ui: Ui, id: SessionId) {
    let surface = ui
        .player_windows
        .with_untracked(|w| w.sessions().iter().find(|s| s.id == id).map(|s| s.surface));
    if let Some(surface) = surface {
        crate::player::stop_surface(ui, surface);
        ui.player_windows.update(|w| w.close(id));
    }
}

pub fn close_all(ui: Ui) {
    let ids: Vec<_> = ui
        .player_windows
        .with_untracked(|w| w.sessions().iter().map(|s| s.id).collect());
    for id in ids {
        close(ui, id);
    }
}

pub fn bind(ui: Ui) {
    effect(move || {
        ui.data_version.get();
        untracked(|| {
            let sessions = ui.player_windows.with(|w| w.sessions().to_vec());
            let schedule = crate::schedule();
            for session in sessions {
                match schedule.find_channel(&session.channel) {
                    Some(channel) => ui.player.stream(session.surface).channel.set(Some(channel)),
                    None => close(ui, session.id),
                }
            }
        });
    });
}

pub fn windows(ui: Ui) -> impl IntoElement {
    view().children((1..crate::player::SURFACES).map(move |surface| {
        when(
            move || mounted(ui, surface),
            move || player_window(ui, surface),
        )
    }))
}

fn player_window(ui: Ui, surface: usize) -> impl IntoElement {
    let session = ui
        .player_windows
        .with_untracked(|w| w.on_surface(surface).cloned())
        .unwrap();
    let id = session.id;
    let mut attributes = nsh_core::desktop::WindowAttributes::default();
    attributes.frame = Some(nsh_core::desktop::WindowFrameStyle::Default);
    attributes.close_on_escape = Some(true);
    attributes.min_size = Some(Some(nsh_core::Size::new(px(640.0), px(360.0))));
    nsh_core::element::desktop::window()
        .id(format!("player-window-{}", id.0))
        .title(move || {
            ui.guide_data();
            ui.player
                .stream(surface)
                .channel
                .get()
                .map(crate::channel_name)
                .unwrap_or_default()
        })
        .default_size(nsh_core::Size::new(px(960.0), px(540.0)))
        .attributes(attributes)
        .on_open_change(move |_| close(ui, id))
        .content(move || player_content(ui, surface, id))
}

/// Reuse the app's actual full-screen player views with a window-local
/// presentation context. Its main stream aliases this auxiliary surface.
fn player_content(parent: Ui, surface: usize, id: SessionId) -> impl IntoElement {
    let mut local = parent;
    local.player_surface = surface;
    local.mode = signal(crate::ViewMode::Fullscreen);
    local.input = signal(crate::Input::Touch);
    local.playing = parent.player.stream(surface).channel;
    local.playback = signal(crate::Playback::Live);
    local.paused = signal(
        parent
            .player_windows
            .with_untracked(|w| w.on_surface(surface).is_some_and(|s| s.paused)),
    );
    local.player.streams[0] = parent.player.stream(surface);
    local.player.history = signal(Default::default());
    local.player.recorded = signal(None);
    local.recordings = signal(Default::default());
    local.recordings_open = computed(move || local.recordings.with(|s| s.open));
    local.player_status = signal(String::new());
    local.pending_zap = signal(None);
    local.browse_playback = signal(Default::default());
    local.focus = signal(parent.focus.get_untracked());
    local.panels = signal(Default::default());
    local.play_bar = signal(Default::default());
    local.channels = signal(None);
    local.channel_picker = signal(None);
    local.multiview = signal(None);
    local.player_windows = signal(Default::default());
    local.menu = signal(None);
    local.description = signal(None);
    local.chooser = signal(None);
    local.search = signal(None);
    local.wizard = signal(None);
    local.playlists = signal(None);
    local.settings_screen = signal(None);
    local.pip = signal(Default::default());
    local.seek_target = signal(None);
    local.sleep = signal(Default::default());
    local.media = crate::media::Media::new();
    let initial = parent
        .player_windows
        .with_untracked(|w| w.on_surface(surface).cloned())
        .unwrap();
    local.media.volume.set(initial.volume);
    local.media.delay_ms.set(initial.audio_delay);
    effect(move || {
        let paused = local.paused.get();
        untracked(|| parent.player_windows.update(|w| w.set_paused(id, paused)));
    });
    effect(move || {
        let volume = local.media.volume.get();
        untracked(|| parent.player_windows.update(|w| w.set_volume(id, volume)));
    });
    effect(move || {
        let delay = local.media.delay_ms.get();
        untracked(|| {
            parent
                .player_windows
                .update(|w| w.set_audio_delay(id, delay))
        });
    });
    effect(move || {
        let muted = parent.player_windows.with(|w| {
            w.on_surface(surface)
                .is_none_or(|s| s.volume.muted || w.audible != Some(id))
        });
        untracked(|| {
            if local.media.volume.with(|v| v.muted) != muted {
                local.media.volume.update(|v| v.muted = muted);
            }
        });
    });
    effect(move || {
        if let Some(key) = parent.player.stream(surface).key.get() {
            untracked(|| parent.player_windows.update(|w| w.retarget(id, key)));
        }
    });
    let window = nsh_core::element::desktop::current_window();
    let back_window = window.clone();
    effect(move || {
        if local.mode.get() != crate::ViewMode::Fullscreen {
            back_window.close();
        }
    });
    crate::browse_playback::bind(local);
    crate::play_bar::bind(local);
    crate::play_bar::pointer_moved(local);
    #[cfg(feature = "test-support")]
    if std::env::var_os("PATINA_GUIDE_WINDOW_WALKTHROUGH").is_some() {
        crate::play_bar::hover(local, true);
        let _reveal = set_timeout(std::time::Duration::from_secs(9), move || {
            crate::play_bar::pointer_moved(local)
        });
    }

    // Keep the controls visible for the first moments of a newly opened player.
    let _show = set_timeout(std::time::Duration::from_millis(250), move || {
        crate::play_bar::pointer_moved(local)
    });
    view()
        .flex_1()
        .test_id("auxiliary-player")
        .bg(rgb(0x000000))
        .on_mouse_move(move |event: &MouseEvent| crate::pointer_moved(local, event))
        .child(
            native(iptv_player_mpv::PIP_VIEW)
                .prop("surface", move || serde_json::json!(surface))
                .absolute()
                .inset_0(),
        )
        .child(
            view()
                .absolute()
                .inset_0()
                .label(move || tr("Video"))
                .on_press(move |event: &PressEvent| {
                    local.touched();
                    crate::toggle_panels(local);
                    if event.clicks == 2 {
                        let full = window
                            .state()
                            .is_some_and(|s| s.mode == nsh_core::desktop::WindowMode::FullScreen);
                        window.set_mode(if full {
                            nsh_core::desktop::WindowMode::Normal
                        } else {
                            nsh_core::desktop::WindowMode::FullScreen
                        });
                    }
                }),
        )
        .child(crate::player_panels::status(local))
        .child(crate::player_panels::layer(local))
        .child(crate::channels_view::layer(local))
        .child(crate::search_view::layer(local))
        .child(crate::menu_view::layer(local))
        .child(crate::chooser::layer(local))
        .child(crate::play_bar::layer(local))
}
