//! Navigation state and the guide layout for one player window.
use crate::ViewMode;
use iptv_presentation::layout::Class;
use iptv_settings::keys::BROWSE_MODE;
use nsh_core::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Guide,
    Overlay,
}

impl Layout {
    fn view(self) -> ViewMode {
        match self {
            Self::Guide => ViewMode::Guide,
            Self::Overlay => ViewMode::Overlay,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct BrowseLayout {
    default: Layout,
    over_video: Layout,
}

impl BrowseLayout {
    fn new(preference: &str, window: Class) -> Self {
        let over_video = if window == Class::Compact {
            Layout::Guide
        } else {
            Layout::Overlay
        };
        let default = if preference == "overlay" {
            over_video
        } else {
            Layout::Guide
        };
        Self {
            default,
            over_video,
        }
    }

    fn open(self, layout: Layout) -> Navigation {
        Navigation::Browse((layout != self.default).then_some(LayoutOverride {
            layout,
            default: self.default,
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LayoutOverride {
    layout: Layout,
    default: Layout,
}

impl LayoutOverride {
    fn fits(self, browse: BrowseLayout) -> bool {
        self.default == browse.default
            && (self.layout != Layout::Overlay || browse.over_video == Layout::Overlay)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Navigation {
    Fullscreen,
    Browse(Option<LayoutOverride>),
}

impl Navigation {
    fn view(self, browse: BrowseLayout) -> ViewMode {
        match self {
            Self::Fullscreen => ViewMode::Fullscreen,
            Self::Browse(layout) => layout
                .filter(|layout| layout.fits(browse))
                .map_or(browse.default, |layout| layout.layout)
                .view(),
        }
    }

    fn update_layout(self, browse: BrowseLayout) -> Self {
        match self {
            Self::Browse(Some(layout)) if !layout.fits(browse) => Self::Browse(None),
            _ => self,
        }
    }
}

pub(crate) enum Action {
    EnterFullscreen,
    ReturnToBrowse,
    ShowGuide,
    Back,
    ToggleOverlay,
    ToggleFullscreen,
    ReturnToPlayer,
    PipBackToApp,
    Restore(ViewMode),
}

fn transition(state: Navigation, action: Action, browse: BrowseLayout) -> Navigation {
    use Action::*;
    match action {
        EnterFullscreen | ReturnToPlayer | PipBackToApp | Restore(ViewMode::Fullscreen) => {
            Navigation::Fullscreen
        }
        ReturnToBrowse => Navigation::Browse(None),
        ShowGuide if state == Navigation::Fullscreen => browse.open(browse.over_video),
        ShowGuide => Navigation::Browse(None),
        Back => match state.view(browse) {
            ViewMode::Fullscreen => Navigation::Browse(None),
            ViewMode::Overlay => Navigation::Fullscreen,
            ViewMode::Guide => state,
        },
        ToggleOverlay => {
            let layout = if state.view(browse) == browse.over_video.view() {
                Layout::Guide
            } else {
                browse.over_video
            };
            browse.open(layout)
        }
        ToggleFullscreen if state == Navigation::Fullscreen => browse.open(Layout::Guide),
        ToggleFullscreen => Navigation::Fullscreen,
        Restore(ViewMode::Guide) => browse.open(Layout::Guide),
        Restore(ViewMode::Overlay) => browse.open(browse.over_video),
    }
}

/// The reactive view mode and the navigation intent that drives it.
#[derive(Clone, Copy)]
pub struct Mode {
    navigation: Signal<Navigation>,
    browse: Memo<BrowseLayout>,
    view: Memo<ViewMode>,
}

impl Mode {
    pub(crate) fn new(settings_revision: Signal<u64>) -> Self {
        let navigation = signal(Navigation::Browse(None));
        let browse = computed(move || {
            settings_revision.get();
            BrowseLayout::new(
                crate::wiring::settings::choice(BROWSE_MODE),
                crate::wiring::device::window_class(),
            )
        });
        let view = computed(move || navigation.get().view(browse.get()));

        // Retire an override once its default changes or its overlay no longer
        // fits. A later resize or settings edit must not bring it back.
        effect(move || {
            let layout = browse.get();
            let previous = navigation.get_untracked();
            let next = previous.update_layout(layout);
            if next != previous {
                navigation.set(next);
            }
        });

        Self {
            navigation,
            browse,
            view,
        }
    }

    /// The layout, subscribing the calling reactive closure.
    pub fn get(self) -> ViewMode {
        self.view.get()
    }

    /// The layout without a reactive subscription.
    pub fn get_untracked(self) -> ViewMode {
        self.view.get_untracked()
    }

    pub(crate) fn apply(self, action: Action) {
        let state = self.navigation.get_untracked();
        self.navigation
            .set(transition(state, action, self.browse.get_untracked()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BROWSING: Navigation = Navigation::Browse(None);
    const FULLSCREEN: Navigation = Navigation::Fullscreen;

    #[test]
    fn compact_browsing_shows_the_guide_beside_its_preview() {
        for window in [Class::Compact, Class::Medium, Class::Expanded] {
            assert_eq!(
                BROWSING.view(BrowseLayout::new("preview", window)),
                ViewMode::Guide
            );
            assert_eq!(
                BROWSING.view(BrowseLayout::new("overlay", window)),
                if window == Class::Compact {
                    ViewMode::Guide
                } else {
                    ViewMode::Overlay
                }
            );
        }
    }

    #[test]
    fn the_overlay_key_toggles_with_either_preference_and_respects_compact_windows() {
        for preference in ["preview", "overlay"] {
            let expanded = BrowseLayout::new(preference, Class::Expanded);
            let toggled = transition(BROWSING, Action::ToggleOverlay, expanded);
            assert_ne!(toggled.view(expanded), BROWSING.view(expanded));
            let toggled = transition(toggled, Action::ToggleOverlay, expanded);
            assert_eq!(toggled.view(expanded), BROWSING.view(expanded));

            let compact = BrowseLayout::new(preference, Class::Compact);
            for state in [BROWSING, FULLSCREEN] {
                assert_eq!(
                    transition(state, Action::ToggleOverlay, compact).view(compact),
                    ViewMode::Guide
                );
            }
        }
    }

    #[test]
    fn the_fullscreen_key_returns_to_preview_with_overlay_preferred() {
        let browse = BrowseLayout::new("overlay", Class::Expanded);
        let fullscreen = transition(BROWSING, Action::ToggleFullscreen, browse);
        assert_eq!(fullscreen.view(browse), ViewMode::Fullscreen);
        let guide = transition(fullscreen, Action::ToggleFullscreen, browse);
        assert_eq!(guide.view(browse), ViewMode::Guide);
    }

    #[test]
    fn a_compact_window_clears_the_explicit_overlay_until_the_user_requests_it_again() {
        let expanded = BrowseLayout::new("preview", Class::Expanded);
        let overlay = transition(FULLSCREEN, Action::ShowGuide, expanded);
        assert_eq!(overlay.view(expanded), ViewMode::Overlay);
        let compact = BrowseLayout::new("preview", Class::Compact);
        assert_eq!(overlay.view(compact), ViewMode::Guide);
        assert_eq!(
            overlay.update_layout(compact).view(expanded),
            ViewMode::Guide
        );
    }

    #[test]
    fn changing_the_default_clears_the_previous_layout_override() {
        let overlay = BrowseLayout::new("overlay", Class::Expanded);
        let preview = BrowseLayout::new("preview", Class::Expanded);
        let guide = transition(BROWSING, Action::ToggleOverlay, overlay);
        assert_eq!(guide.view(overlay), ViewMode::Guide);
        assert_eq!(
            guide.update_layout(preview).view(overlay),
            ViewMode::Overlay
        );
    }

    #[test]
    fn back_and_player_returns_resolve_the_browsing_preference() {
        let browse = BrowseLayout::new("overlay", Class::Expanded);
        assert_eq!(transition(FULLSCREEN, Action::Back, browse), BROWSING);
        assert_eq!(transition(BROWSING, Action::Back, browse), FULLSCREEN);
        assert_eq!(
            transition(FULLSCREEN, Action::ReturnToBrowse, browse).view(browse),
            ViewMode::Overlay
        );
        for action in [Action::ReturnToPlayer, Action::PipBackToApp] {
            assert_eq!(transition(BROWSING, action, browse), FULLSCREEN);
        }
        for view in [ViewMode::Guide, ViewMode::Overlay, ViewMode::Fullscreen] {
            assert_eq!(
                transition(BROWSING, Action::Restore(view), browse).view(browse),
                view
            );
        }
    }
}
