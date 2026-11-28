//! Overlay identifiers and their pure z-order / input-context metadata.
//!
//! Each screen keeps its own state and its own `is_open` (`overlays.rs`);
//! this module holds only the enum and pure predicates so feature trees and
//! building blocks never depend on the shell's screen dispatchers.

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
    pub(crate) fn top(is_open: impl Fn(Self) -> bool) -> Option<Self> {
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
