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
    Wizard,
    GuideSources,
    Organization,
    CustomGroups,
    RemoteEditor,
    Playlists,
    GuideAssign,
    Settings,
    FavoritesEditor,
    /// A chooser the play bar or a menu opened (tracks, picture size…).
    Chooser,
    /// The programme details popup.
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
            Self::GuideSources
            | Self::Organization
            | Self::CustomGroups
            | Self::RemoteEditor
            | Self::Settings
            | Self::FavoritesEditor => Context::Editor,
            Self::Wizard
            | Self::Playlists
            | Self::GuideAssign
            | Self::Chooser
            | Self::Description
            | Self::Menu => Context::Menu,
            Self::Search => Context::Search,
            Self::ChannelPicker | Self::ChannelList => Context::ChannelList,
        }
    }

    /// Every overlay, topmost layer first.
    pub(crate) const ALL: [Overlay; 15] = [
        Overlay::Wizard,
        Overlay::GuideSources,
        Overlay::Organization,
        Overlay::CustomGroups,
        Overlay::RemoteEditor,
        Overlay::Playlists,
        Overlay::GuideAssign,
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
            Overlay::Wizard => crate::wizard_view::is_open(ui),
            Overlay::GuideSources => crate::epg_sources_view::is_open(ui),
            Overlay::Organization => crate::organization_view::is_open(ui),
            Overlay::CustomGroups => crate::custom_groups::is_open(ui),
            Overlay::RemoteEditor => crate::remote_editor::is_open(ui),
            Overlay::Playlists => crate::playlists_view::is_open(ui),
            Overlay::GuideAssign => crate::guide_assign_view::is_open(ui),
            Overlay::Settings => crate::settings_view::is_open(ui),
            Overlay::FavoritesEditor => ui.editor.with(Option::is_some),
            Overlay::Chooser => ui.chooser.with(Option::is_some),
            Overlay::Description => ui.description.with(Option::is_some),
            Overlay::Menu => ui.menu.with(Option::is_some),
            Overlay::Search => crate::search_view::is_open(ui),
            Overlay::ChannelPicker => crate::multiview_picker::is_open(ui),
            Overlay::ChannelList => crate::channels_view::is_open(ui),
        }
    }

    /// Whether it is a screen of its own that covers the video, as
    /// opposed to a popup or a list shown over the picture. A tap that
    /// falls through such a screen's background is not a tap on the video.
    pub(crate) fn covers_video(self) -> bool {
        match self {
            Overlay::Wizard
            | Overlay::GuideSources
            | Overlay::Organization
            | Overlay::CustomGroups
            | Overlay::RemoteEditor
            | Overlay::Playlists
            | Overlay::GuideAssign
            | Overlay::Settings
            | Overlay::FavoritesEditor
            | Overlay::Search => true,
            Overlay::Chooser
            | Overlay::Description
            | Overlay::Menu
            | Overlay::ChannelPicker
            | Overlay::ChannelList => false,
        }
    }

    /// Whether a mouse moving over full-screen video leaves the play bar
    /// alone while it is open. Everything does but the programme details:
    /// the bar is layered over them and held while they are open
    /// (`play_bar::bind`), and its own button toggles them, so the bar
    /// still comes up over them. (A chooser holds the bar too, but a bar
    /// that is not showing does not come up over one.)
    pub(crate) fn keeps_pointer_from_play_bar(self) -> bool {
        match self {
            Overlay::Description => false,
            Overlay::Wizard
            | Overlay::GuideSources
            | Overlay::Organization
            | Overlay::CustomGroups
            | Overlay::RemoteEditor
            | Overlay::Playlists
            | Overlay::GuideAssign
            | Overlay::Settings
            | Overlay::FavoritesEditor
            | Overlay::Chooser
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
            Overlay::Wizard
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
            | Overlay::Menu
            | Overlay::Search
            | Overlay::ChannelPicker
            | Overlay::ChannelList => Overlay::ALL.contains(&overlay),
        };
        assert!(Overlay::ALL.into_iter().all(listed));
        assert_eq!(Overlay::ALL.len(), 15);
    }

    #[test]
    fn the_pointer_and_taps_on_video_respect_what_is_open() {
        // The lists and popups over the picture leave taps to the video;
        // the screens that cover it do not.
        let over_the_picture = [
            Overlay::Chooser,
            Overlay::Description,
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
            // Only the programme details let a moving mouse bring the
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
