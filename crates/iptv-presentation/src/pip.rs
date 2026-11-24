//! Picture in picture: the player in a small window of the system's over
//! other apps (Android's PiP), showing only the video.
//!
//! The host reports whether it can ([`Pip::supported`]), whether the
//! device is a TV, and when the window opens or closes ([`Pip::report`]);
//! the app asks it to open one now ([`Pip::request`]: the control strip's
//! button) or to open one by itself when the viewer leaves the app
//! ([`Pip::auto_enter`]: Home while a channel plays full screen, setting
//! [`OnLeave`]). A TV never has it, whatever its host reports: Android TV's
//! guidelines keep media playback out of its picture in picture, and Home
//! there returns to the launcher with playback stopped. While Android's
//! window is open the app shows only the video ([`Pip::ui_shown`]); the
//! desktops' window is one of the app's own beside its main window
//! ([`Pip::separate`], `pip_window`), whose UI stays.

/// `player.pip_on_leave`: whether leaving the app while a channel plays
/// full screen opens the picture-in-picture window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnLeave {
    /// On phones and tablets (a TV has no window at all).
    Auto,
    /// The same as [`OnLeave::Auto`] now that a TV has no window; kept so
    /// a stored choice still reads.
    Always,
    Never,
}

impl OnLeave {
    /// The setting's stored choice; anything else is [`OnLeave::Auto`].
    pub fn from_key(key: &str) -> OnLeave {
        match key {
            "always" => OnLeave::Always,
            "never" => OnLeave::Never,
            _ => OnLeave::Auto,
        }
    }
}

/// What a change reported by the host means for the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    /// The window opened: only the video shows.
    Entered,
    /// It closed (expanded back to full screen, or dismissed).
    Left,
}

/// Picture-in-picture state, as the host reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pip {
    /// A native host has reported support (false support is distinct from no report).
    pub reported: bool,
    /// The host can open a picture-in-picture window: Android phones and
    /// tablets with the system feature, the desktops' window of their own;
    /// never a TV ([`Pip::report`]).
    pub supported: bool,
    /// The device is a TV (Android's leanback feature).
    pub tv: bool,
    /// The window is open.
    pub active: bool,
    /// Requests to open one so far: a new value asks the host to.
    pub requests: u64,
    /// The window is apart from the app's (the desktops' window of its
    /// own): the app's UI stays while it is open.
    pub separate: bool,
}

impl Pip {
    /// The host's report: `supported`, `tv`, `active`. A TV is never
    /// supported, whatever its system feature says.
    pub fn report(&mut self, supported: bool, tv: bool, active: bool) -> Change {
        self.reported = true;
        self.supported = supported && !tv;
        self.tv = tv;
        let was = std::mem::replace(&mut self.active, active);
        match (was, active) {
            (false, true) => Change::Entered,
            (true, false) => Change::Left,
            _ => Change::None,
        }
    }

    /// Whether the control strip's button can open the window now:
    /// `playing` is a channel playing full screen.
    pub fn can_enter(&self, playing: bool) -> bool {
        crate::capabilities::pip(self, playing).available()
    }

    /// Asks the host to open the window; false when it cannot now.
    pub fn request(&mut self, playing: bool) -> bool {
        if !self.can_enter(playing) {
            return false;
        }
        self.requests += 1;
        true
    }

    /// Whether leaving the app opens the window by itself now: `playing`
    /// is a channel playing full screen (not the guide's preview), and
    /// never on a TV.
    pub fn auto_enter(&self, on_leave: OnLeave, playing: bool) -> bool {
        let wanted = on_leave != OnLeave::Never;
        self.reported && self.supported && !self.tv && playing && wanted
    }

    /// Whether the app's UI shows over the video: not in Android's
    /// window (the app's own is that window), always beside the desktops'.
    pub fn ui_shown(&self) -> bool {
        !self.active || self.separate
    }

    /// Whether the player shows in a window apart from the app's now.
    pub fn in_window(&self) -> bool {
        self.active && self.separate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(tv: bool) -> Pip {
        let mut pip = Pip::default();
        pip.report(true, tv, false);
        pip
    }

    #[test]
    fn the_button_needs_a_host_that_can_and_a_channel_playing() {
        assert!(!Pip::default().can_enter(true), "no host support");
        assert!(!host(false).can_enter(false), "nothing plays");
        assert!(host(false).can_enter(true));
        assert!(!host(true).can_enter(true), "never on a TV");
    }

    #[test]
    fn a_tv_reports_no_window_and_takes_no_request() {
        let mut pip = host(true);
        assert!(pip.reported && pip.tv && !pip.supported);
        assert!(!pip.request(true));
        assert_eq!(pip.requests, 0);
    }

    #[test]
    fn a_request_counts_only_when_it_can_enter() {
        let mut pip = Pip::default();
        assert!(!pip.request(true));
        assert_eq!(pip.requests, 0);
        let mut pip = host(false);
        assert!(pip.request(true));
        assert_eq!(pip.requests, 1);
        pip.report(true, false, true);
        assert!(!pip.request(true), "already in the window");
        assert_eq!(pip.requests, 1);
    }

    #[test]
    fn leaving_enters_by_default_on_tablets_never_on_tvs() {
        assert!(host(false).auto_enter(OnLeave::Auto, true));
        assert!(host(false).auto_enter(OnLeave::Always, true));
        assert!(!host(true).auto_enter(OnLeave::Auto, true));
        assert!(
            !host(true).auto_enter(OnLeave::Always, true),
            "a stored choice from before does not bring it back"
        );
        assert!(!host(false).auto_enter(OnLeave::Never, true), "opted out");
        assert!(
            !host(false).auto_enter(OnLeave::Always, false),
            "nothing plays"
        );
        assert!(
            !Pip::default().auto_enter(OnLeave::Always, true),
            "no support"
        );
    }

    #[test]
    fn the_window_hides_the_ui_until_it_closes() {
        let mut pip = host(false);
        assert!(pip.ui_shown());
        assert_eq!(pip.report(true, false, true), Change::Entered);
        assert!(!pip.ui_shown());
        assert_eq!(pip.report(true, false, true), Change::None);
        assert_eq!(pip.report(true, false, false), Change::Left);
        assert!(pip.ui_shown());
    }

    #[test]
    fn the_desktops_window_leaves_the_ui_beside_it() {
        let mut pip = Pip {
            separate: true,
            ..host(false)
        };
        assert!(!pip.in_window());
        assert!(pip.request(true));
        assert_eq!(pip.report(true, false, true), Change::Entered);
        assert!(pip.ui_shown(), "the main window stays usable");
        assert!(pip.in_window());
        assert!(!pip.can_enter(true), "one window");
        assert_eq!(pip.report(true, false, false), Change::Left);
        assert!(!pip.in_window());
    }

    #[test]
    fn unknown_choices_are_auto() {
        assert_eq!(OnLeave::from_key("always"), OnLeave::Always);
        assert_eq!(OnLeave::from_key("never"), OnLeave::Never);
        assert_eq!(OnLeave::from_key("auto"), OnLeave::Auto);
        assert_eq!(OnLeave::from_key("?"), OnLeave::Auto);
    }
}
