//! Root NSH application component, persistent video layer, and top-level UI
//! layer stack.

use std::rc::Rc;
use std::time::Duration;

use nsh_core::prelude::*;

use crate::guide::{fav_editor, favorites, grid, guide_clock, header, lists, rail};
use crate::host;
use crate::kit::i18n::{self, tr};
use crate::kit::{fonts, keyboard, overlay::Overlay, startup, toast};
use crate::library_ui::{
    backup, custom_groups, epg_sources_view, guide_assign_view, guide_inspect_view,
    guide_repair_view, organization_view, playlists_view, wizard_view,
};
use crate::playback::{
    browse_playback, interruption, multiview, multiview_picker, multiview_view, panels, pip,
    pip_window, play_bar, player, player_panels, player_windows, touch_zap, video,
};
use crate::screens::{
    cast_pairing, channels_view, parental_view, recordings_view, search_view, settings_view,
    vod_view,
};
use crate::shell::back::{bind_back, go_back};
use crate::shell::{
    chooser, desktop, exit_confirmation, input, menu_view, remote_editor, startup_view,
};
use crate::wiring::runtime_owner::{self, runtime};
use crate::wiring::{data, device_wiring, runtime_wiring, settings, settings_wiring};
#[cfg(feature = "test-support")]
use crate::wiring::{guide_window_walkthrough, performance_probe};
use crate::{IOS, Input, SharedUi, Ui, ViewMode, pointer_down, pointer_input};

#[component]
pub fn App() -> impl IntoElement {
    startup::mark("rust root");
    host::log_library_base();
    fonts::register();
    let directory = match host::initialize() {
        Ok(directory) => directory,
        Err(error) => {
            log::error!("iptv storage initialization failed: {error}");
            return view()
                .flex_1()
                .items_center()
                .justify_center()
                .child(text(i18n::tr("Private app storage is unavailable")))
                .child(text(i18n::tr(
                    "Check your home directory and file permissions, then reopen Patina.",
                )));
        }
    };
    // This root's owner becomes the shell's handle, and the one the host's
    // stop callback flushes. Only a root does this: constructing a runtime
    // elsewhere changes neither.
    let owner = Rc::new(runtime_owner::new_runtime(directory));
    host::flush_on_host_stop(owner.host_handle());
    runtime_owner::install_root(owner);
    if let Err(error) = runtime().lock_storage() {
        let message = error.message();
        return view()
            .flex_1()
            .items_center()
            .justify_center()
            .child(text(i18n::tr(message)));
    }
    // Restore validation can decrypt/migrate SQLite and copied documents. It
    // must finish off the UI thread before settings or the live store open.
    let outcome = signal(None::<Result<(), &'static str>>);
    let (tx, rx) = futures_channel::oneshot::channel();
    let started = host::background("iptv-bootstrap", {
        let activate = runtime().restore_activation();
        move || {
            let result = match activate() {
                Ok(()) => Ok(()),
                Err(error) => {
                    log::warn!("iptv backup: {}", error.error);
                    if error.fatal {
                        Err(error.error.message())
                    } else {
                        Ok(())
                    }
                }
            };
            let _ = tx.send(result);
        }
    });
    if started.is_err() {
        outcome.set(Some(Err(i18n::key(
            "The startup task could not be started",
        ))));
    } else {
        // Detach under this root owner; disposing the root cancels the reply
        // task, while recovery itself safely completes on its worker.
        drop(spawn(async move {
            let stopped = Err(i18n::key("The startup task stopped"));
            outcome.set(Some(rx.await.unwrap_or(stopped)));
        }));
    }
    // Keep startup branding above the app while its library and player start.
    // Removing this sibling overlay never remounts or hides a player surface.
    let minimum_elapsed = signal(false);
    let _splash_timer = set_timeout(Duration::from_millis(1500), move || {
        minimum_elapsed.set(true);
    });
    let splash = move || match outcome.get() {
        None => true,
        Some(Ok(())) => !minimum_elapsed.get(),
        Some(Err(_)) => false,
    };
    let input = stored(None);
    input::window(view().flex_1(), input)
        .child(
            view()
                .flex_1()
                .accessibility_elements_hidden(splash)
                .child(dynamic(move || {
                    let result = outcome.get();
                    // Only the one-shot outcome controls this branch. Reads while building
                    // ReadyApp must not subscribe the branch and recreate its video surface.
                    untracked(move || match result {
                        None => view()
                            .flex_1()
                            .items_center()
                            .justify_center()
                            .into_element(),
                        Some(Err(message)) => view()
                            .flex_1()
                            .items_center()
                            .justify_center()
                            .child(text(i18n::tr("Saved data could not be recovered")))
                            .child(text(i18n::tr(message)))
                            .child(text(i18n::tr("Close Patina and try opening it again.")))
                            .into_element(),
                        Some(Ok(())) => ready_app(input).into_element(),
                    })
                })),
        )
        .child(when(splash, startup_view::splash))
}

/// Constructed once after recovery; all player surfaces belong to this owner.
fn ready_app(input: Stored<Option<input::InputScope>>) -> impl IntoElement {
    settings::load();
    startup::mark("settings loaded");
    // Establish host-report signals before eager feature bindings subscribe.
    let device_bridge = device_wiring::bridge();
    let ui = Ui::new(SharedUi::new(), player::Player::new());
    settings_wiring::bind(ui);
    browse_playback::bind(ui);
    interruption::bind(ui);
    desktop::bind(ui);
    desktop::menus(ui);
    // Leaving full screen closes the panels.
    effect(move || {
        if ui.chrome.mode.get() != ViewMode::Fullscreen {
            log::info!("iptv panels hidden: left full screen");
            ui.playback.panels.update(|p| p.hide());
        }
    });
    favorites::reset();
    input.replace(Some(input::bind(ui)));
    #[cfg(feature = "test-support")]
    performance_probe::bind(ui);
    runtime_wiring::start(ui);
    guide_clock::start(ui);
    settings_view::updates::bind(ui);
    player::bind(ui);
    player::watch_time::bind(ui);
    data::pages::bind(ui);
    play_bar::bind(ui);
    lists::bind(ui);
    multiview::bind(ui);
    pip::bind(ui);
    player_windows::bind(ui);
    bind_back(ui);
    #[cfg(feature = "test-support")]
    guide_window_walkthrough::bind(ui);

    let rows = element_ref();
    let groups = element_ref();
    let channel = computed(move || ui.guide.focus.with(|f| f.channel));
    let group = computed(move || ui.guide.focus.with(|f| f.group));
    let shown = computed(move || lists::columns_shown(ui.guide));
    // Keys scroll only when the focus comes within EDGE rows of the
    // viewport's edge (touch scrolls on its own).
    const EDGE: usize = 2;
    effect(move || {
        let ch = channel.get();
        if !ui.keys_shown() {
            return;
        }
        let Some((first, last)) = ui.guide.visible_rows.get_untracked() else {
            return;
        };
        if ch < first + EDGE {
            rows.scroll_to_index(
                ch.saturating_sub(EDGE) as u32,
                percentage(0.0),
                px(0.0),
                false,
            );
        } else if ch + EDGE > last {
            let len = ui.guide.list.with_untracked(|l| l.len());
            let target = (ch + EDGE).min(len.saturating_sub(1));
            rows.scroll_to_index(target as u32, percentage(1.0), px(0.0), false);
        }
    });
    // The groups column shows the selected group when it opens or moves.
    effect(move || {
        if shown.get()
            && let Some(position) = lists::visible_entries(ui.app).position(group.get())
        {
            groups.scroll_to_index(position as u32, percentage(0.5), px(0.0), false);
        }
    });

    view()
        .flex_1()
        .bg(move || {
            if video::backdrop_clear(ui) {
                rgba(0)
            } else {
                rgb(ui.app.theme.with(|t| t.background.window))
            }
        })
        // A moving mouse over full-screen video shows the player panels.
        .on_mouse_move(move |e: &MouseEvent| pointer_moved(ui, e))
        .on_mouse_enter(move |e: &MouseEvent| {
            desktop::pointer_entered(
                ui.playback.player_surface,
                e.page_position.x.0,
                e.page_position.y.0,
            )
        })
        // Which pointer presses the controls: touch or pointer mode.
        .on_touch(move |e: &nsh_core::TouchEvent| pointer_down(ui.chrome, e))
        // Layers, bottom to top: opaque backdrop, video (multiview's other
        // surfaces while their tiles play), UI, full-screen tap catcher.
        // The picture-in-picture window shows only the video (`pip.rs`).
        .child(backup::service_layer(ui))
        .child(keyboard::bridge())
        .child(device_bridge)
        .child(panels::backdrop(ui))
        .child(video::layer(ui, 0))
        .children((1..player::SURFACES).map(move |s| {
            when(
                move || {
                    multiview::mounted(ui.playback, s) || player_windows::mounted(ui.playback, s)
                },
                move || video::layer(ui, s),
            )
        }))
        .child(ui_layers(ui, groups, rows))
        // The desktops' picture-in-picture window, while it is open.
        .child(pip_window::window(ui))
        .child(player_windows::windows(ui, go_back))
}

/// Everything over the video: the guide, the full-screen tap catcher,
/// the player panels and every screen and popup. None of it shows in the
/// picture-in-picture window.
pub(crate) fn guide_chrome_pointer_events(screens: crate::ScreensUi) -> PointerEvents {
    if organization_view::guide_order_open(screens) {
        PointerEvents::None
    } else {
        PointerEvents::Auto
    }
}

fn ui_layers(ui: Ui, groups: ElementRef, rows: ElementRef) -> View {
    let fullscreen = move || ui.chrome.mode.get() == ViewMode::Fullscreen;
    let shown = move || pip::ui_shown(ui.playback);
    let gesture = touch_zap::Gesture::new();
    view()
        .test_id("app-ui")
        .on_touch(move |event: &nsh_core::TouchEvent| gesture.observe(ui, event))
        .absolute()
        .inset_0()
        .accessibility_elements_hidden(move || !shown())
        .opacity(move || if shown() { 1.0_f32 } else { 0.0 })
        .pointer_events(move || {
            if shown() {
                PointerEvents::BoxNone
            } else {
                PointerEvents::None
            }
        })
        .child(
            view()
                .test_id("guide-ui")
                .flex_1()
                .accessibility_elements_hidden(move || {
                    fullscreen()
                        || (ui.any_overlay_open() && !organization_view::guide_order_accessible(ui))
                })
                .opacity(move || if fullscreen() { 0.0_f32 } else { 1.0 })
                .pointer_events(move || {
                    if fullscreen()
                        || (organization_view::is_open(ui.screens)
                            && !(organization_view::guide_order_accessible(ui)
                                && ui.drag_editing()))
                    {
                        PointerEvents::None
                    } else {
                        PointerEvents::Auto
                    }
                })
                .child(header::header(ui))
                .child(
                    view()
                        .flex_1()
                        .flex_row()
                        .child(rail::column(ui))
                        .child(
                            when(
                                move || organization_view::group_column_order_open(ui.screens),
                                move || view().w(move || px(panels::groups_width(ui.guide))),
                            )
                            .otherwise(move || panels::group_column(ui, groups)),
                        )
                        .child(grid::guide(ui, rows)),
                ),
        )
        .child(
            // Made again when live video turns into a replay or back: only
            // live video has the channel actions (`touch_zap.rs`).
            when(
                move || touch_zap::changes_channel(ui.playback),
                move || video_catcher(ui, true, gesture),
            )
            .otherwise(move || video_catcher(ui, false, gesture)),
        )
        .child(touch_zap::layer(ui))
        // Where the video was while the desktops' picture-in-picture
        // window shows it.
        .child(pip_window::placeholder(ui))
        .child(player_panels::status(ui))
        .child(player_panels::layer(ui))
        .child(multiview_view::layer(ui))
        .child(channels_view::layer(ui))
        .child(multiview_picker::layer(ui))
        .child(search_view::layer(ui))
        .child(menu_view::layer(ui))
        .child(chooser::layer(ui, false))
        // Over the popups it opens (choosers, program details), so its
        // buttons stay usable and toggle them.
        .child(play_bar::layer(ui, go_back))
        .child(fav_editor::layer(ui))
        .child(settings_view::layer(ui))
        .child(menu_view::playlist_layer(ui))
        .child(vod_view::layer(ui))
        .child(recordings_view::layer(ui))
        .child(guide_assign_view::layer(ui))
        // The playlists screen opens over settings, a guide sources screen
        // over both.
        .child(playlists_view::layer(ui))
        .child(remote_editor::layer(ui))
        .child(custom_groups::layer(ui))
        .child(organization_view::layer(ui))
        .child(epg_sources_view::layer(ui))
        .child(guide_inspect_view::layer(ui))
        .child(guide_repair_view::layer(ui))
        .child(wizard_view::layer(ui))
        .child(backup::layer(ui))
        .child(settings_view::updates::layer(ui))
        .child(cast_pairing::layer(ui))
        .child(parental_view::layer(ui))
        .child(startup_view::store_failure_layer(ui))
        .child(desktop::quit_confirm::layer(ui))
        // The question before leaving, over every screen it can be asked
        // from (the first run's welcome screen too); mounted only while it
        // is asked.
        .child(nsh_core::prelude::when(
            move || chooser::is_open(ui.screens, true),
            move || chooser::layer(ui, true),
        ))
        .child(toast::layer())
        .child(exit_confirmation::layer(ui))
}

/// Full screen: a tap toggles the play bar (the panels where there is no
/// pointer); over live video a vertical swipe and the accessibility
/// actions change channel (`touch_zap.rs`). It lies above the video, which
/// it never wraps.
fn video_catcher(ui: Ui, live: bool, gesture: touch_zap::Gesture) -> View {
    let fullscreen = move || ui.chrome.mode.get() == ViewMode::Fullscreen;
    let swiped = gesture.swiped;
    let catcher = view()
        .test_id("video")
        .accessibility_elements_hidden(move || {
            !fullscreen() || ui.any_overlay_open() || multiview::is_open(ui.playback)
        })
        .absolute()
        .inset_0()
        .pointer_events(move || {
            if fullscreen() {
                PointerEvents::Auto
            } else {
                PointerEvents::None
            }
        })
        .label(move || tr("Video"))
        .on_press(move |e: &PressEvent| {
            // The finger swiped (`touch_zap.rs`): its lift is no tap. Only
            // a pointer's press is the swipe's (a keyboard's or a screen
            // reader's has no clicks), and only the first one after it.
            if e.clicks != 0 && swiped.with(std::mem::take) {
                return;
            }
            ui.touched();
            toggle_panels(ui);
            // A double click on the desktop: window full screen (the
            // first click's panels go again).
            if e.clicks == 2 && desktop::DESKTOP {
                desktop::toggle_fullscreen();
            }
        });
    if !live {
        return catcher;
    }
    touch_zap::watch(catcher, ui, gesture)
        .accessibility_actions(touch_zap::actions())
        .on_accessibility_action(move |action: &str| touch_zap::act(ui, action))
}

/// A tap on full-screen video: the play bar on a device with a pointer,
/// else the card and its buttons; beside multiview's tiles, its pointer
/// chrome (the tiles take their own taps).
pub(crate) fn toggle_panels(ui: Ui) {
    // A screen of its own covers the video: a tap that fell through its
    // background is not one on the picture.
    if untracked(|| ui.overlay_open(Overlay::covers_video)) {
        return;
    }
    // An iPad's tap on the video beside the channel list closes it.
    if IOS && untracked(|| channels_view::is_open(ui.playback)) {
        channels_view::close(ui.playback);
        return;
    }
    if untracked(|| multiview::is_open(ui.playback)) {
        // Beside the tiles: the menu closes first.
        if multiview::dismiss(ui) {
            return;
        }
        if play_bar::pointer_available() {
            play_bar::tapped(ui);
        }
        return;
    }
    if play_bar::pointer_available() {
        play_bar::tapped(ui);
        return;
    }
    if ui.playback.panels.with(|p| p.shown) == iptv_presentation::panels::Shown::Hidden {
        ui.update_panels(|p, now| p.show_card(now));
    } else {
        log::info!("iptv panels hidden by a tap");
        ui.playback.panels.update(|p| p.hide());
    }
}

/// The mouse moved: over full-screen video with nothing open above the
/// panels, it shows the play bar (multiview's pointer chrome over its
/// grid) and keeps it while it moves.
pub(crate) fn pointer_moved(ui: Ui, e: &MouseEvent) {
    let open = untracked(|| ui.overlay_open(Overlay::keeps_pointer_from_play_bar));
    let (x, y) = (e.page_position.x.0, e.page_position.y.0);
    // Anywhere else (the guide, lists, Recordings), a mouse that really
    // moved after keys switches to Pointer, so the ⋯ controls show: no
    // focus moves and no playback is scheduled or canceled.
    if desktop::pointer_traveled(ui.playback.player_surface, x, y) {
        pointer_input(ui.chrome);
    }
    if ui.chrome.mode.get_untracked() != ViewMode::Fullscreen
        || open
        || !desktop::pointer_moved(x, y, ui.now_ms())
    {
        return;
    }
    if ui.chrome.input.get_untracked() != Input::Pointer {
        ui.chrome.last_press.set(Input::Pointer);
        ui.touched();
    }
    play_bar::pointer_moved(ui);
}
