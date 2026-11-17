//! Desktop guide-menu prototype: auxiliary players belong to the app, independently
//! of the main guide's navigation and the multiview layout.
use crate::{Ui, i18n::tr};
use iptv_core::playback_windows::SessionId;
use nsh_core::prelude::*;

pub fn mounted(ui: Ui, surface: usize) -> bool {
    cfg!(any(target_os = "linux", target_os = "macos"))
        && ui.player_windows.with(|w| w.on_surface(surface).is_some())
}

pub fn can_open(ui: Ui, channel: usize) -> bool {
    if ui.player_surface != 0
        || !cfg!(any(target_os = "linux", target_os = "macos"))
        || crate::multiview::session(ui)
    {
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
    admit_session(ui.player_windows, key, |surface| {
        crate::player::start_surface(ui, surface, channel)
    });
}

fn admit_session(
    workspace: Signal<iptv_core::playback_windows::Workspace<String>>,
    key: String,
    start: impl FnOnce(usize) -> bool,
) {
    batch(|| {
        let mut session = None;
        workspace.update(|w| {
            let audible = w.audible;
            if let Some(id) = w.open_auxiliary(key) {
                session = w.sessions().iter().find(|s| s.id == id).cloned();
            }
            // Opening another channel leaves the main player's audio unchanged.
            w.audible = audible;
        });
        if let Some(session) = session
            && !start(session.surface)
        {
            // Admission can fail when recordings own the remaining connections.
            // Publish no window until both session allocation and admission succeed.
            workspace.update(|w| w.close(session.id));
        }
    });
}

fn follow_clock(
    parent: Signal<iptv_core::guide::Focus>,
    local: Signal<iptv_core::guide::Focus>,
    view: impl Fn() -> iptv_core::lists::ListView + 'static,
) {
    let now = computed(move || parent.with(|f| f.now));
    effect(move || {
        let now = now.get();
        untracked(|| {
            local.update(|f| {
                f.tick(now, &view());
            });
        });
    });
}

fn follow_lists(local: Ui) {
    let catalog = std::cell::RefCell::new(crate::schedule());
    effect(move || {
        local.data_version.get();
        local.list_version.get();
        untracked(|| {
            let next = crate::schedule();
            let previous = catalog.replace(next.clone());
            if !std::rc::Rc::ptr_eq(&previous, &next) {
                let kept = crate::lists::keep_in(local, previous);
                crate::lists::restore(local, kept);
                // These open presentations also contain catalogue row indices.
                local.channels.set(None);
                local.menu.set(None);
                local.description.set(None);
                local.chooser.set(None);
            } else if matches!(
                iptv_core::lists::Entry::at(local.focus.with_untracked(|f| f.group)),
                iptv_core::lists::Entry::Favorites | iptv_core::lists::Entry::Recent
            ) {
                crate::lists::rebuild(local, true);
            }
        });
    });
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
    let compact = move || {
        ui.player_windows
            .with(|w| w.on_surface(surface).is_some_and(|s| s.compact))
    };
    let window = nsh_core::element::desktop::window()
        .id(format!("player-window-{}", id.0))
        .title(move || {
            if compact() {
                return iptv_core::pip_window::TITLE.to_owned();
            }
            ui.guide_data();
            ui.player
                .stream(surface)
                .channel
                .get()
                .map(crate::channel_name)
                .unwrap_or_default()
        })
        .default_size(nsh_core::Size::new(px(960.0), px(540.0)))
        .attributes(move || window_attributes(compact()))
        .on_open_change(move |_| close(ui, id))
        .content(move || player_content(ui, surface, id));
    // A starting position only: resizing/fullscreen/control updates never
    // snap a window back after the viewer moves it.
    #[cfg(target_os = "macos")]
    let window = match untracked(|| opening_position(surface)) {
        Some(position) => window.default_position(position),
        None => window, // NSH's native cascade remains the fallback.
    };
    window
}

fn window_attributes(compact: bool) -> nsh_core::desktop::WindowAttributes {
    use nsh_core::desktop::{WindowAttributes, WindowFrameStyle, WindowLevel, Workspaces};
    let mut attributes = WindowAttributes::default();
    attributes.frame = Some(if compact {
        WindowFrameStyle::None
    } else {
        WindowFrameStyle::Default
    });
    attributes.level = Some(if compact {
        WindowLevel::Floating
    } else {
        WindowLevel::Normal
    });
    attributes.workspaces = Some(if compact {
        Workspaces::All
    } else {
        Workspaces::Current
    });
    attributes.taskbar = Some(!compact);
    attributes.minimizable = Some(!compact);
    attributes.maximizable = Some(!compact);
    // A window-local key binding restores compact mode on Escape.
    attributes.close_on_escape = Some(false);
    let (w, h) = if compact {
        (240.0, 135.0)
    } else {
        (640.0, 360.0)
    };
    attributes.min_size = Some(Some(nsh_core::Size::new(px(w), px(h))));
    attributes
}

fn bind_pip(parent: Ui, local: Ui, id: SessionId, window: nsh_core::element::desktop::WindowRef) {
    use nsh_core::desktop::{WindowFrame, WindowMode};
    let saved = std::cell::Cell::new(None);
    let was_compact = std::cell::Cell::new(false);
    let key_window = window.clone();
    on_key("escape", move |_| {
        if crate::pip::exit(local, iptv_core::pip_window::Exit::BackToApp) {
            return;
        }
        if local.top_overlay().is_some() {
            crate::go_back(local);
            return;
        }
        if key_window
            .state()
            .is_some_and(|s| s.mode == WindowMode::FullScreen)
        {
            key_window.set_mode(WindowMode::Normal);
            return;
        }
        key_window.close();
    });
    effect(move || {
        let compact = crate::pip::in_window(local);
        if was_compact.replace(compact) == compact {
            return;
        }
        untracked(|| {
            parent.player_windows.update(|w| w.set_compact(id, compact));
            if local.mode.get() != crate::ViewMode::Fullscreen {
                return; // The compact Close action is disposing this session.
            }
            window.set(window_attributes(compact));
            if compact {
                saved.set(window.state().map(|s| s.placement));
                let (w, h) = iptv_core::pip_window::size_for(local.player.main().size.get());
                window.set_mode(WindowMode::Normal);
                window.set_bounds(WindowFrame {
                    width: Some(px(w)),
                    height: Some(px(h)),
                    ..Default::default()
                });
            } else {
                if let Some(placement) = saved.take() {
                    window.set_placement(placement);
                }
                crate::play_bar::pointer_moved(local);
            }
            window.focus();
        });
    });
}

#[cfg(target_os = "macos")]
fn opening_position(surface: usize) -> Option<nsh_core::desktop::WindowPosition> {
    let state = super::with_window(|main| main.state()).flatten()?;
    let area = nsh_core::element::desktop::display(state.display?)?.visible_bounds;
    Some(cascade_position(surface, area))
}

#[cfg(any(target_os = "macos", test))]
fn cascade_position(
    surface: usize,
    area: nsh_core::Bounds<nsh_core::Points>,
) -> nsh_core::desktop::WindowPosition {
    // Separate title bars on the main display, including screens to the
    // left/above the primary. Leave room for the native title bar too.
    let x_step = ((area.size.width.0 - 960.0).max(0.0) / 4.0).min(72.0);
    let y_step = ((area.size.height.0 - 572.0).max(0.0) / 4.0).min(56.0);
    nsh_core::desktop::WindowPosition::At(nsh_core::geometry::point(
        px(area.origin.x.0 + x_step * surface as f32),
        px(area.origin.y.0 + y_step * surface as f32),
    ))
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
    local.channel_number = signal(Default::default());
    local.search_history_revision = signal(0);
    local.player.vod = signal(None);
    local.browse_playback = signal(Default::default());
    local.focus = signal(parent.focus.get_untracked());
    local.list = signal(parent.list.get_untracked());
    follow_clock(parent.focus, local.focus, move || crate::lists::view(local));
    local.guide_panning = signal(false);
    local.visible_rows = signal(None);
    local.stats = signal(Default::default());
    local.back_guard = signal(Default::default());
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
    local.parental = signal(None);
    // These screens belong to the main window. Its open state must never
    // capture keys or cover controls in an auxiliary player's presentation.
    local.editor = signal(None);
    local.organization = signal(None);
    local.custom_groups = signal(None);
    local.remote_editor = signal(None);
    local.guide_sources = signal(None);
    local.guide_inspect = signal(None);
    local.guide_repair = signal(None);
    local.guide_assign = signal(None);
    local.guide_repair_cancel = stored(None);
    local.guide_inspect_cancel = stored(None);
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
    follow_lists(local);
    crate::input::bind(local);
    crate::pip::bind(local);
    bind_pip(parent, local, id, window.clone());
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
        if std::env::var_os("PATINA_GUIDE_PIP_WALKTHROUGH").is_some() {
            let _compact = set_timeout(std::time::Duration::from_secs(2), move || {
                crate::pip::enter(local);
            });
        }
    }

    // Keep the controls visible for the first moments of a newly opened player.
    let _show = set_timeout(std::time::Duration::from_millis(250), move || {
        crate::play_bar::pointer_moved(local)
    });
    view()
        .flex_1()
        .test_id(&format!("auxiliary-player-{}", id.0))
        .bg(rgb(0x000000))
        .on_mouse_enter(move |_: &MouseEvent| {
            if !crate::pip::in_window(local)
                && !untracked(|| {
                    local.overlay_open(crate::overlays::Overlay::keeps_pointer_from_play_bar)
                })
            {
                local.touched();
                crate::play_bar::pointer_moved(local);
            }
        })
        .on_mouse_move(move |event: &MouseEvent| {
            if !crate::pip::in_window(local) {
                crate::pointer_moved(local, event);
            }
        })
        .child(
            native(iptv_player_mpv::PIP_VIEW)
                .prop("surface", move || serde_json::json!(surface))
                .prop("compact", move || {
                    serde_json::json!(crate::pip::in_window(local))
                })
                .absolute()
                .inset_0(),
        )
        .child(when(
            move || crate::pip::in_window(local),
            move || super::pip_window::chrome(local),
        ))
        .child(when(
            move || !crate::pip::in_window(local),
            move || {
                let window = window.clone();
                view()
                    .absolute()
                    .inset_0()
                    .child(
                        view()
                            .absolute()
                            .inset_0()
                            .label(move || tr("Video"))
                            .on_press(move |event: &PressEvent| {
                                local.touched();
                                crate::toggle_panels(local);
                                if event.clicks == 2 {
                                    let full = window.state().is_some_and(|s| {
                                        s.mode == nsh_core::desktop::WindowMode::FullScreen
                                    });
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
            },
        ))
}

#[cfg(test)]
mod tests {
    use super::{admit_session, cascade_position, follow_clock};
    use nsh_core::desktop::WindowPosition;
    use nsh_core::geometry::{Bounds, point, px, size};
    use nsh_core::prelude::*;

    #[component]
    fn LocalStateHarness() -> impl IntoElement {
        use iptv_core::guide::{Focus, Schedule, SyntheticSchedule};
        use iptv_core::lists::{ALL, ChannelList, ListView};
        use std::rc::Rc;
        let parent = signal(Focus::at(0));
        let mut focus = Focus::at(0);
        focus.channel = 2;
        let local = signal(focus);
        let catalog: Rc<dyn Schedule> = Rc::new(SyntheticSchedule);
        let list = Rc::new(ChannelList::build(ALL, &*catalog, &[], &[]));
        follow_clock(parent, local, move || {
            ListView::new(catalog.clone(), list.clone())
        });
        let workspace = signal(Default::default());
        view()
            .child(text(move || {
                local.with(|f| format!("minute {} channel {}", f.now, f.channel))
            }))
            .child(
                view()
                    .label("Next minute")
                    .w(px(220.0))
                    .h(px(40.0))
                    .on_press(move || parent.update(|f| f.now += 60)),
            )
            .child(text(move || {
                workspace.with(|w: &iptv_core::playback_windows::Workspace<String>| {
                    format!("sessions {}", w.sessions().len())
                })
            }))
            .child(
                view()
                    .label("Refuse session")
                    .w(px(220.0))
                    .h(px(40.0))
                    .on_press(move || admit_session(workspace, "generated".into(), |_| false)),
            )
            .child(
                view()
                    .label("Admit session")
                    .w(px(220.0))
                    .h(px(40.0))
                    .on_press(move || admit_session(workspace, "generated".into(), |_| true)),
            )
    }

    #[test]
    fn window_clock_follows_parent_without_replacing_local_channel_focus() {
        let mut app = nsh_testing::Headless::start(nsh_engine::EngineConfig::new(
            nsh_core::element::Root::of::<LocalStateHarness>(),
        ))
        .unwrap();
        app.wait_for("text:minute 0 channel 2").unwrap();
        app.press("label:Next minute").unwrap();
        app.wait_for("text:minute 60 channel 2").unwrap();
    }

    #[test]
    fn refused_admission_leaves_no_session_and_the_same_channel_can_be_opened_again() {
        let mut app = nsh_testing::Headless::start(nsh_engine::EngineConfig::new(
            nsh_core::element::Root::of::<LocalStateHarness>(),
        ))
        .unwrap();
        app.wait_for("text:sessions 0").unwrap();
        app.press("label:Refuse session").unwrap();
        assert!(app.screen().matches("text:sessions 0").unwrap());
        app.press("label:Admit session").unwrap();
        app.wait_for("text:sessions 1").unwrap();
    }

    #[test]
    fn opening_positions_separate_title_bars_on_an_offset_display() {
        let area = Bounds::new(point(px(-1512.0), px(-200.0)), size(px(1512.0), px(900.0)));
        let positions: Vec<_> = (1..=3)
            .map(|surface| match cascade_position(surface, area) {
                WindowPosition::At(at) => at,
                _ => unreachable!(),
            })
            .collect();
        for pair in positions.windows(2) {
            assert!(pair[0].x.0 < pair[1].x.0 && pair[0].y.0 < pair[1].y.0);
        }
        for at in positions {
            assert!(at.x.0 >= -1512.0 && at.x.0 + 960.0 <= 0.0);
            assert!(at.y.0 >= -200.0 && at.y.0 + 572.0 <= 700.0);
        }
    }

    #[test]
    fn opening_positions_do_not_push_small_display_title_bars_offscreen() {
        let area = Bounds::new(point(px(100.0), px(30.0)), size(px(960.0), px(540.0)));
        assert_eq!(cascade_position(3, area), WindowPosition::At(area.origin));
    }
}
