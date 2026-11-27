//! Everything that opens above the guide and the video, in one list.
//!
//! Each screen keeps its own state and its own `is_open`; this module
//! only names them all, so code that asks "is something open over the
//! video?" asks one question instead of listing the screens again (and
//! forgetting the newest). A new overlay gets a variant here: the
//! exhaustive matches below then ask what it is, and every caller follows.
//!
//! The order of [`Overlay::ALL`] is the layers', topmost first
//! (`ui_layers` mounts them bottom to top). Key routing
//! (`input::context`) and Back (`go_back`) use that same topmost query.

use crate::Ui;

/// A screen, popup or list that can be open above the guide and the video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Overlay {
    /// The question before quitting during a capture (desktops).
    QuitConfirm,
    /// The question before leaving (`crate::confirm_leaving`): a chooser
    /// of its own layer, over every screen it can be asked from.
    Leave,
    StoreStartupFailure,
    Parental,
    CastPairing,
    RestoreReview,
    AppUpdate,
    AppUpdateNotice,
    Wizard,
    Recordings,
    Vod,
    GuideSources,
    GuideInspection,
    GuideRepair,
    Organization,
    CustomGroups,
    RemoteEditor,
    Playlists,
    GuideAssign,
    PlaylistMenu,
    Settings,
    FavoritesEditor,
    /// A chooser the play bar or a menu opened (tracks, picture size…).
    Chooser,
    /// The program details popup.
    Description,
    Menu,
    Search,
    /// Multiview's channel picker.
    ChannelPicker,
    /// The channel list over (or beside) full-screen video.
    ChannelList,
}

impl Overlay {
    fn top(is_open: impl Fn(Self) -> bool) -> Option<Self> {
        Self::ALL.into_iter().find(|overlay| is_open(*overlay))
    }

    pub(crate) fn key_context(self) -> iptv_nav::Context {
        use iptv_nav::Context;
        match self {
            Self::QuitConfirm => Context::Menu,
            Self::StoreStartupFailure
            | Self::RestoreReview
            | Self::AppUpdate
            | Self::AppUpdateNotice
            | Self::CastPairing
            | Self::Wizard
            | Self::Vod
            | Self::Recordings
            | Self::GuideRepair
            | Self::GuideInspection
            | Self::GuideSources
            | Self::Organization
            | Self::CustomGroups
            | Self::RemoteEditor
            | Self::Settings
            | Self::FavoritesEditor => Context::Editor,
            Self::Leave
            | Self::Parental
            | Self::Playlists
            | Self::GuideAssign
            | Self::Chooser
            | Self::Description
            | Self::PlaylistMenu
            | Self::Menu => Context::Menu,
            Self::Search => Context::Search,
            Self::ChannelPicker | Self::ChannelList => Context::ChannelList,
        }
    }

    /// Every overlay, topmost layer first.
    pub(crate) const ALL: [Overlay; 28] = [
        Overlay::QuitConfirm,
        Overlay::Leave,
        Overlay::StoreStartupFailure,
        Overlay::Parental,
        Overlay::CastPairing,
        Overlay::AppUpdate,
        Overlay::AppUpdateNotice,
        Overlay::RestoreReview,
        Overlay::Wizard,
        Overlay::Recordings,
        Overlay::Vod,
        Overlay::GuideRepair,
        Overlay::GuideInspection,
        Overlay::GuideSources,
        Overlay::Organization,
        Overlay::CustomGroups,
        Overlay::RemoteEditor,
        Overlay::Playlists,
        Overlay::GuideAssign,
        Overlay::PlaylistMenu,
        Overlay::Settings,
        Overlay::FavoritesEditor,
        Overlay::Chooser,
        Overlay::Description,
        Overlay::Menu,
        Overlay::Search,
        Overlay::ChannelPicker,
        Overlay::ChannelList,
    ];

    /// Whether it is open (a tracked read, like each screen's `is_open`).
    pub(crate) fn is_open(self, ui: Ui) -> bool {
        match self {
            Overlay::QuitConfirm => crate::chrome::desktop::quit_confirm::is_open(ui),
            Overlay::Leave => crate::chrome::chooser::is_open(ui, true),
            Overlay::StoreStartupFailure => ui.chrome.store_startup_failure.with(Option::is_some),
            Overlay::Parental => crate::screens::parental_view::is_open(ui),
            Overlay::CastPairing => crate::screens::cast_pairing::is_open(ui),
            Overlay::RestoreReview => crate::library_ui::backup::is_open(ui),
            Overlay::Wizard => crate::library_ui::wizard_view::is_open(ui),
            Overlay::Vod => crate::screens::vod_view::is_open(ui),
            Overlay::Recordings => crate::screens::recordings_view::is_open(ui),
            Overlay::GuideRepair => crate::library_ui::guide_repair_view::is_open(ui),
            Overlay::GuideInspection => crate::library_ui::guide_inspect_view::is_open(ui),
            Overlay::GuideSources => crate::library_ui::epg_sources_view::is_open(ui),
            Overlay::Organization => crate::library_ui::organization_view::is_open(ui),
            Overlay::CustomGroups => crate::library_ui::custom_groups::is_open(ui),
            Overlay::RemoteEditor => crate::chrome::remote_editor::is_open(ui),
            Overlay::Playlists => crate::library_ui::playlists_view::is_open(ui),
            Overlay::GuideAssign => crate::library_ui::guide_assign_view::is_open(ui),
            Overlay::AppUpdateNotice => crate::screens::settings_view::updates::notice_open(ui),
            Overlay::AppUpdate => crate::screens::settings_view::updates::is_open(ui),
            Overlay::Settings => crate::screens::settings_view::is_open(ui),
            Overlay::FavoritesEditor => ui.screens.editor.with(Option::is_some),
            Overlay::Chooser => crate::chrome::chooser::is_open(ui, false),
            Overlay::Description => ui.screens.description.with(Option::is_some),
            Overlay::PlaylistMenu => crate::chrome::menu_view::playlist_popup_open(ui),
            Overlay::Menu => {
                ui.screens.menu.with(Option::is_some)
                    && !crate::chrome::menu_view::playlist_popup_open(ui)
            }
            Overlay::Search => crate::screens::search_view::is_open(ui),
            Overlay::ChannelPicker => crate::playback::multiview_picker::is_open(ui),
            Overlay::ChannelList => crate::screens::channels_view::is_open(ui),
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

    /// Whether it is a screen of its own that covers the video, as
    /// opposed to a popup or a list shown over the picture. A tap that
    /// falls through such a screen's background is not a tap on the video.
    pub(crate) fn covers_video(self) -> bool {
        match self {
            Overlay::QuitConfirm
            | Overlay::StoreStartupFailure
            | Overlay::Parental
            | Overlay::CastPairing
            | Overlay::RestoreReview
            | Overlay::AppUpdate
            | Overlay::Wizard
            | Overlay::Vod
            | Overlay::Recordings
            | Overlay::GuideRepair
            | Overlay::GuideInspection
            | Overlay::GuideSources
            | Overlay::Organization
            | Overlay::CustomGroups
            | Overlay::RemoteEditor
            | Overlay::Playlists
            | Overlay::GuideAssign
            | Overlay::Settings
            | Overlay::FavoritesEditor
            | Overlay::Search => true,
            Overlay::Leave
            | Overlay::AppUpdateNotice
            | Overlay::Chooser
            | Overlay::Description
            | Overlay::PlaylistMenu
            | Overlay::Menu
            | Overlay::ChannelPicker
            | Overlay::ChannelList => false,
        }
    }

    /// Whether a mouse moving over full-screen video leaves the play bar
    /// alone while it is open. Everything does but the program details:
    /// the bar is layered over them and held while they are open
    /// (`play_bar::bind`), and its own button toggles them, so the bar
    /// still comes up over them. (A chooser holds the bar too, but a bar
    /// that is not showing does not come up over one.)
    pub(crate) fn keeps_pointer_from_play_bar(self) -> bool {
        match self {
            Overlay::Description => false,
            Overlay::QuitConfirm
            | Overlay::Leave
            | Overlay::StoreStartupFailure
            | Overlay::Parental
            | Overlay::CastPairing
            | Overlay::RestoreReview
            | Overlay::AppUpdate
            | Overlay::AppUpdateNotice
            | Overlay::Wizard
            | Overlay::Vod
            | Overlay::Recordings
            | Overlay::GuideRepair
            | Overlay::GuideInspection
            | Overlay::GuideSources
            | Overlay::Organization
            | Overlay::CustomGroups
            | Overlay::RemoteEditor
            | Overlay::Playlists
            | Overlay::GuideAssign
            | Overlay::Settings
            | Overlay::FavoritesEditor
            | Overlay::Chooser
            | Overlay::PlaylistMenu
            | Overlay::Menu
            | Overlay::Search
            | Overlay::ChannelPicker
            | Overlay::ChannelList => true,
        }
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

#[cfg(test)]
mod tests {
    use super::Overlay;

    #[test]
    fn nested_screens_keep_input_on_the_visible_top_layer() {
        use iptv_nav::Context;
        let mut open = vec![Overlay::Settings, Overlay::Playlists, Overlay::GuideSources];
        for (expected, context) in [
            (Overlay::GuideSources, Context::Editor),
            (Overlay::Playlists, Context::Menu),
            (Overlay::Settings, Context::Editor),
        ] {
            let top = Overlay::top(|overlay| open.contains(&overlay)).unwrap();
            assert_eq!(top, expected);
            assert_eq!(top.key_context(), context);
            open.retain(|overlay| *overlay != top);
        }
        assert_eq!(Overlay::top(|overlay| open.contains(&overlay)), None);
        assert_eq!(
            Overlay::top(|overlay| [Overlay::Settings, Overlay::GuideAssign].contains(&overlay)),
            Some(Overlay::GuideAssign),
        );
    }

    #[test]
    fn every_overlay_is_listed_once() {
        for (at, overlay) in Overlay::ALL.into_iter().enumerate() {
            assert_eq!(
                Overlay::ALL.iter().position(|o| *o == overlay),
                Some(at),
                "{overlay:?}"
            );
        }
        // A variant missing from `ALL` would not be asked about: the match
        // makes adding one without counting it here a compile error.
        let listed = |overlay: Overlay| match overlay {
            Overlay::QuitConfirm
            | Overlay::Leave
            | Overlay::StoreStartupFailure
            | Overlay::Parental
            | Overlay::CastPairing
            | Overlay::RestoreReview
            | Overlay::AppUpdate
            | Overlay::AppUpdateNotice
            | Overlay::Wizard
            | Overlay::Vod
            | Overlay::Recordings
            | Overlay::GuideRepair
            | Overlay::GuideInspection
            | Overlay::GuideSources
            | Overlay::Organization
            | Overlay::CustomGroups
            | Overlay::RemoteEditor
            | Overlay::Playlists
            | Overlay::GuideAssign
            | Overlay::Settings
            | Overlay::FavoritesEditor
            | Overlay::Chooser
            | Overlay::Description
            | Overlay::PlaylistMenu
            | Overlay::Menu
            | Overlay::Search
            | Overlay::ChannelPicker
            | Overlay::ChannelList => Overlay::ALL.contains(&overlay),
        };
        assert!(Overlay::ALL.into_iter().all(listed));
        assert_eq!(Overlay::ALL.len(), 28);
    }

    #[test]
    fn the_pointer_and_taps_on_video_respect_what_is_open() {
        // The lists and popups over the picture leave taps to the video;
        // the screens that cover it do not.
        let over_the_picture = [
            Overlay::Leave,
            Overlay::AppUpdateNotice,
            Overlay::Chooser,
            Overlay::Description,
            Overlay::PlaylistMenu,
            Overlay::Menu,
            Overlay::ChannelPicker,
            Overlay::ChannelList,
        ];
        for overlay in Overlay::ALL {
            assert_eq!(
                overlay.covers_video(),
                !over_the_picture.contains(&overlay),
                "{overlay:?}"
            );
            // Only the program details let a moving mouse bring the
            // play bar up; the channel list and the custom groups screen
            // keep it away like every other overlay.
            assert_eq!(
                overlay.keeps_pointer_from_play_bar(),
                overlay != Overlay::Description,
                "{overlay:?}"
            );
        }
    }
}
