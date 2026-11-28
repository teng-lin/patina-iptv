//! Everything that opens above the guide and the video, in one list.
//!
//! Each screen keeps its own state and its own `is_open`; this module
//! wires [`Overlay`] to the live screens so code that asks "is something
//! open over the video?" asks one question instead of listing the screens
//! again (and forgetting the newest).
//!
//! The order of [`Overlay::ALL`] is the layers', topmost first
//! (`ui_layers` mounts them bottom to top). Key routing
//! (`input::context`) and Back (`go_back`) use that same topmost query.

pub(crate) use super::overlay::Overlay;
use crate::Ui;

impl Overlay {
    /// Whether it is open (a tracked read, like each screen's `is_open`).
    pub(crate) fn is_open(self, ui: Ui) -> bool {
        match self {
            Overlay::QuitConfirm => crate::chrome::desktop::quit_confirm::is_open(ui.chrome),
            Overlay::Leave => crate::chrome::chooser::is_open(ui.screens, true),
            Overlay::StoreStartupFailure => ui.chrome.store_startup_failure.with(Option::is_some),
            Overlay::Parental => crate::screens::parental_view::is_open(ui.screens),
            Overlay::CastPairing => crate::screens::cast_pairing::is_open(ui.cast),
            Overlay::RestoreReview => crate::library_ui::backup::is_open(ui.screens),
            Overlay::Wizard => crate::library_ui::wizard_view::is_open(ui.screens),
            Overlay::Vod => crate::screens::vod_view::is_open(ui.screens),
            Overlay::Recordings => crate::screens::recordings_view::is_open(ui.screens),
            Overlay::GuideRepair => crate::library_ui::guide_repair_view::is_open(ui.screens),
            Overlay::GuideInspection => crate::library_ui::guide_inspect_view::is_open(ui.screens),
            Overlay::GuideSources => crate::library_ui::epg_sources_view::is_open(ui.screens),
            Overlay::Organization => crate::library_ui::organization_view::is_open(ui.screens),
            Overlay::CustomGroups => crate::library_ui::custom_groups::is_open(ui.screens),
            Overlay::RemoteEditor => crate::chrome::remote_editor::is_open(ui.screens),
            Overlay::Playlists => crate::library_ui::playlists_view::is_open(ui.screens),
            Overlay::GuideAssign => crate::library_ui::guide_assign_view::is_open(ui.screens),
            Overlay::AppUpdateNotice => crate::screens::settings_view::updates::notice_open(ui),
            Overlay::AppUpdate => crate::screens::settings_view::updates::is_open(ui.screens),
            Overlay::Settings => crate::screens::settings_view::is_open(ui.screens),
            Overlay::FavoritesEditor => ui.screens.editor.with(Option::is_some),
            Overlay::Chooser => crate::chrome::chooser::is_open(ui.screens, false),
            Overlay::Description => ui.screens.description.with(Option::is_some),
            Overlay::PlaylistMenu => crate::chrome::menu_view::playlist_popup_open(ui.screens),
            Overlay::Menu => {
                ui.screens.menu.with(Option::is_some)
                    && !crate::chrome::menu_view::playlist_popup_open(ui.screens)
            }
            Overlay::Search => crate::screens::search_view::is_open(ui.screens),
            Overlay::ChannelPicker => crate::playback::multiview_picker::is_open(ui.playback),
            Overlay::ChannelList => crate::screens::channels_view::is_open(ui.playback),
        }
    }

    /// Whether a higher UI layer currently covers this one's controls.
    /// This follows the existing paint order only; it changes neither the
    /// input owner nor any screen's state. Screen readers must not activate
    /// controls through a modal backdrop.
    pub(crate) fn accessibility_covered(self, ui: Ui) -> bool {
        Self::ALL
            .into_iter()
            .take_while(|overlay| *overlay != self)
            .any(|overlay| overlay.is_open(ui))
    }

    /// A persistent UI subtree is exposed only while open and unobscured.
    pub(crate) fn accessibility_hidden(self, ui: Ui) -> bool {
        !self.is_open(ui) || self.accessibility_covered(ui)
    }
}

impl Ui {
    /// The visually highest open overlay owns keys and Back.
    pub(crate) fn top_overlay(self) -> Option<Overlay> {
        Overlay::top(|overlay| overlay.is_open(self))
    }

    /// Whether an overlay that `is` holds for is open.
    pub(crate) fn overlay_open(self, is: impl Fn(Overlay) -> bool) -> bool {
        Overlay::ALL
            .into_iter()
            .any(|overlay| is(overlay) && overlay.is_open(self))
    }

    /// Whether anything at all is open above the guide and the video.
    pub(crate) fn any_overlay_open(self) -> bool {
        self.overlay_open(|_| true)
    }
}
