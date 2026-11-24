//! Player panels over full-screen video: which panels show, which button
//! has focus, and when they hide on their own.
//!
//! As observed in the reference UX: OK shows the info card (logo, number
//! and name, program, times with one progress line, next program,
//! stream badges) with a row of action cards beneath it; Left/Right move
//! along the cards, OK presses one, OK with none focused (or Back) hides
//! them. Long OK or Menu shows the full control strip. A channel change
//! shows a compact banner that takes no input and hides after a few
//! seconds. A replayed program's card shows its seek bar in place of the
//! progress line ([`Line`]): one time line on screen, never two.
//!
//! Every input restarts the auto-hide timer. Times are milliseconds on the
//! caller's monotonic clock, so this stays portable (no clock access here).

/// What is on screen over the video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Hidden,
    /// Channel change feedback: number, name, the program on now and
    /// its progress, in one compact panel; no input.
    Banner,
    /// The info card alone (the Info key, a seek in a replay): no input,
    /// so the seek keys go on seeking.
    Info,
    /// OK or a tap: the top bar, the info card and the action cards, which
    /// own Left/Right/OK.
    Card,
    /// Long OK or Menu: the control strip in place of the card.
    Controls,
}

/// The action cards under the info card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickAction {
    /// Back to the guide, the video in its preview.
    Guide,
    /// The recently played channels.
    History,
    /// The channel list over the video.
    Channels,
    /// The playing channel in the favorites, or out of them.
    Favorite,
    /// Several channels at once.
    Multiview,
    /// The full control strip (what Long OK and Menu show).
    Controls,
}

pub const QUICK_ACTIONS: [QuickAction; 6] = [
    QuickAction::Guide,
    QuickAction::History,
    QuickAction::Channels,
    QuickAction::Favorite,
    QuickAction::Multiview,
    QuickAction::Controls,
];

/// Buttons in the full-screen control strip, in the order observed in the
/// reference UX's strip. Play/pause and live are on the
/// remote's media keys, not in the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Search,
    ChannelList,
    Recordings,
    Multiview,
    PictureInPicture,
    VideoTracks,
    AudioTracks,
    AudioOffset,
    Captions,
    Scaling,
    SleepTimer,
    Favorite,
    ChannelOptions,
    Settings,
    Record,
}

// [`Panels`] keeps the controls it leaves out as a `u16` mask.
const _: () = assert!(CONTROLS.len() <= 16 && QUICK_ACTIONS.len() <= 16);

pub const CONTROLS: [Control; 15] = [
    Control::Search,
    Control::ChannelList,
    Control::Recordings,
    Control::Multiview,
    Control::PictureInPicture,
    Control::VideoTracks,
    Control::AudioTracks,
    Control::AudioOffset,
    Control::Captions,
    Control::Scaling,
    Control::SleepTimer,
    Control::Favorite,
    Control::ChannelOptions,
    Control::Settings,
    Control::Record,
];

/// The first index of [`CONTROLS`] in `step`'s direction from `from`
/// (exclusive) whose control is offered.
fn next_offered(omitted: u16, from: usize, step: isize) -> Option<usize> {
    if step == 0 {
        return None;
    }
    let mut at = from as isize;
    loop {
        at += step.signum();
        if at < 0 || at as usize >= CONTROLS.len() {
            return None;
        }
        if omitted & (1 << at) == 0 {
            return Some(at as usize);
        }
    }
}

/// The one time line a panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// Live: how far the program on now is.
    Progress,
    /// A replay: where it plays (or the seek being chosen) along its length.
    Seek,
}

/// Auto-hide delays. iptv-rs defaults; most are settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// The banner after a channel change.
    pub banner_ms: u64,
    /// The card alone.
    pub info_ms: u64,
    /// The card with its action cards, or the control strip.
    pub controls_ms: u64,
    /// Shown by the mouse: once it stops moving.
    pub pointer_ms: u64,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            banner_ms: 3_000,
            info_ms: 4_000,
            controls_ms: 7_000,
            pointer_ms: 3_000,
        }
    }
}

/// What OK did over the panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// An action card was pressed.
    Quick(QuickAction),
    /// A control strip button was pressed.
    Control(Control),
    /// No card had the focus: the panels hid.
    Hidden,
    /// Nothing takes OK here.
    Nothing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Panels {
    pub shown: Shown,
    /// Index into [`QUICK_ACTIONS`] of the focused action card; none until
    /// Left or Right reaches the row.
    pub quick: Option<usize>,
    /// Index into [`CONTROLS`] of the focused control: always one the
    /// strip offers ([`Self::offer`]).
    pub focus: usize,
    /// Controls the host can never offer, by their [`CONTROLS`] index: the
    /// strip leaves them out rather than drawing them disabled.
    omitted: u16,
    pub timeouts: Timeouts,
    /// The mouse showed (or last kept) them.
    by_pointer: bool,
    hide_at_ms: Option<u64>,
    /// A held remote key decides tap or hold at this time, over the
    /// panels shown when it went down: apart from the idle deadline, so
    /// input meanwhile (a zap's banner) does not pull it in.
    hold: Option<(Shown, u64)>,
}

impl Default for Panels {
    fn default() -> Self {
        Panels {
            shown: Shown::Hidden,
            quick: None,
            focus: 0,
            omitted: 0,
            timeouts: Timeouts::default(),
            by_pointer: false,
            hide_at_ms: None,
            hold: None,
        }
    }
}

impl Panels {
    /// OK in full screen (or a tap): the card with its action cards, none
    /// focused. Open buttons stay as they are.
    pub fn show_card(&mut self, now_ms: u64) {
        if !self.captures_input() {
            self.shown = Shown::Card;
            self.quick = None;
        }
        self.by_pointer = false;
        self.touch(now_ms);
    }

    /// Long OK or Menu: the control strip at once, focus on its first
    /// control unless it was already open.
    pub fn show_controls(&mut self, now_ms: u64) {
        if self.shown != Shown::Controls {
            self.focus = self.first_offered();
        }
        self.shown = Shown::Controls;
        self.by_pointer = false;
        self.touch(now_ms);
    }

    /// Menu: the control strip, or away again when it shows.
    pub fn toggle_controls(&mut self, now_ms: u64) {
        if self.shown == Shown::Controls {
            self.hide();
        } else {
            self.show_controls(now_ms);
        }
    }

    /// The card alone (the Info key, a seek). Keeps open buttons.
    pub fn show_info(&mut self, now_ms: u64) {
        if matches!(self.shown, Shown::Hidden | Shown::Banner) {
            self.shown = Shown::Info;
        }
        self.touch(now_ms);
    }

    /// A channel change: the banner, unless buttons are open (they stay,
    /// showing the new channel).
    pub fn show_banner(&mut self, now_ms: u64) {
        if !self.captures_input() {
            self.shown = Shown::Banner;
        }
        self.touch(now_ms);
    }

    /// The mouse moved over the video: the card with its buttons, hidden
    /// again once the mouse rests ([`Timeouts::pointer_ms`]).
    pub fn pointer_moved(&mut self, now_ms: u64) {
        if !self.captures_input() {
            self.shown = Shown::Card;
        }
        self.by_pointer = true;
        self.touch(now_ms);
    }

    pub fn hide(&mut self) {
        self.shown = Shown::Hidden;
        self.quick = None;
        self.focus = self.first_offered();
        self.by_pointer = false;
        self.hide_at_ms = None;
        self.hold = None;
    }

    /// Whether a row of buttons owns Left/Right/OK.
    pub fn captures_input(&self) -> bool {
        matches!(self.shown, Shown::Card | Shown::Controls)
    }

    /// Whether the info card shows (the strip and the banner replace it).
    pub fn card_shown(&self) -> bool {
        matches!(self.shown, Shown::Info | Shown::Card)
    }

    /// The time line on screen: one at most, on the card or the banner;
    /// a replay's (`replay`) is its seek bar.
    pub fn line(&self, replay: bool) -> Option<Line> {
        match self.shown {
            Shown::Banner | Shown::Info | Shown::Card if replay => Some(Line::Seek),
            Shown::Banner | Shown::Info | Shown::Card => Some(Line::Progress),
            Shown::Hidden | Shown::Controls => None,
        }
    }

    pub fn focused(&self) -> Control {
        CONTROLS[self.focus]
    }

    /// Says which controls the strip shows: the rest are left out, as
    /// controls the host can never offer. A focused control that is left
    /// out passes the focus to the nearest one shown.
    pub fn offer(&mut self, offered: impl Fn(Control) -> bool) {
        self.omitted = CONTROLS
            .iter()
            .enumerate()
            .filter(|&(_, &control)| !offered(control))
            .fold(0, |mask, (at, _)| mask | (1 << at));
        if self.omitted & (1 << self.focus) != 0 {
            self.focus = next_offered(self.omitted, self.focus, -1)
                .or_else(|| next_offered(self.omitted, self.focus, 1))
                .unwrap_or(0);
        }
    }

    /// The controls the strip shows, in order.
    pub fn controls(&self) -> impl Iterator<Item = Control> + '_ {
        CONTROLS
            .iter()
            .enumerate()
            .filter(|&(at, _)| self.omitted & (1 << at) == 0)
            .map(|(_, &control)| control)
    }

    /// A pointer or touch focused `control` (if the strip shows it).
    pub fn focus_control(&mut self, control: Control) {
        if let Some(at) = CONTROLS.iter().position(|&c| c == control)
            && self.omitted & (1 << at) == 0
        {
            self.focus = at;
        }
    }

    fn first_offered(&self) -> usize {
        if self.omitted & 1 == 0 {
            0
        } else {
            next_offered(self.omitted, 0, 1).unwrap_or(0)
        }
    }

    pub fn quick_focused(&self) -> Option<QuickAction> {
        self.quick.map(|i| QUICK_ACTIONS[i])
    }

    /// Moves focus within the row that has it (clamped, no wrap); the
    /// first press over the card focuses its first action.
    pub fn move_focus(&mut self, step: isize, now_ms: u64) {
        let clamp =
            |at: usize, len: usize| (at as isize + step).clamp(0, len as isize - 1) as usize;
        match self.shown {
            Shown::Card => {
                self.quick = Some(self.quick.map_or(0, |at| clamp(at, QUICK_ACTIONS.len())));
            }
            Shown::Controls => {
                for _ in 0..step.unsigned_abs() {
                    match next_offered(self.omitted, self.focus, step) {
                        Some(at) => self.focus = at,
                        None => break,
                    }
                }
            }
            Shown::Hidden | Shown::Banner | Shown::Info => {}
        }
        self.by_pointer = false;
        self.touch(now_ms);
    }

    /// OK over the panels: presses the focused card or control; over the
    /// card with no card focused, hides it.
    pub fn select(&mut self, now_ms: u64) -> Pick {
        self.by_pointer = false;
        let pick = match (self.shown, self.quick_focused()) {
            (Shown::Card, Some(action)) => Pick::Quick(action),
            (Shown::Card, None) => {
                self.hide();
                return Pick::Hidden;
            }
            (Shown::Controls, _) => Pick::Control(self.focused()),
            _ => Pick::Nothing,
        };
        self.touch(now_ms);
        pick
    }

    /// A remote key is held and decides tap or hold at `at_ms`: the
    /// panels shown now stay at least until then, so the press finds them
    /// on release, whatever restarts their own timer meanwhile.
    pub fn keep_until(&mut self, at_ms: u64) {
        if self.shown != Shown::Hidden {
            self.hold = Some((self.shown, at_ms));
        }
    }

    /// The held key's press was decided or dropped: only the idle
    /// deadline counts again.
    pub fn release_hold(&mut self) {
        self.hold = None;
    }

    /// Whether a held key keeps the panels ([`Self::keep_until`]).
    pub fn holding(&self) -> bool {
        self.hold.is_some()
    }

    /// Any input while shown: restart the auto-hide timer.
    pub fn touch(&mut self, now_ms: u64) {
        let t = self.timeouts;
        self.hide_at_ms = match self.shown {
            Shown::Hidden => None,
            _ if self.by_pointer => Some(now_ms + t.pointer_ms),
            Shown::Banner => Some(now_ms + t.banner_ms),
            Shown::Info => Some(now_ms + t.info_ms),
            Shown::Card | Shown::Controls => Some(now_ms + t.controls_ms),
        };
    }

    /// When the panels hide unless something happens (for scheduling):
    /// their idle deadline, or a held key's if later and still over the
    /// panels it was held on.
    pub fn hide_at_ms(&self) -> Option<u64> {
        let hold = self
            .hold
            .filter(|(shown, _)| *shown == self.shown)
            .map(|(_, at)| at);
        match (self.hide_at_ms, hold) {
            (Some(idle), Some(hold)) => Some(idle.max(hold)),
            (idle, None) => idle,
            (None, hold) => hold.filter(|_| self.shown != Shown::Hidden),
        }
    }

    /// Hides once the deadline has passed. Returns whether it hid.
    pub fn tick(&mut self, now_ms: u64) -> bool {
        match self.hide_at_ms() {
            Some(at) if now_ms >= at => {
                self.hide();
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests;
