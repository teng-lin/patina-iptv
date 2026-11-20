//! What the player does when the host's audio is interrupted (#88): another
//! app takes the audio focus, a call or a navigation prompt takes it for a
//! while, or the sound's route goes away (headphones unplugged, a Bluetooth
//! headset disconnected). The host reports each ([`Interruption`]); this
//! policy says whether to pause or resume, free of the platform so it is
//! unit-tested on the host.
//!
//! - Another app took the audio for good: pause; nothing resumes it.
//! - Taken for a while (a call): pause, and resume when it comes back, only
//!   if this policy paused it and the viewer did nothing to playback since.
//! - The route went away: pause, never resume by itself (playing aloud
//!   from the speaker is what the viewer was spared).
//!
//! It only pauses and resumes what plays: it never opens, retunes or
//! restarts a stream, so a stream the viewer stopped stays stopped, and
//! resuming opens no second provider connection.

/// What the host reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interruption {
    /// Another app took the audio focus for good.
    FocusLoss,
    /// Another app took it for a while (a call, a navigation prompt).
    FocusLossTransient,
    /// The audio focus came back.
    FocusGain,
    /// The audio's route went away (headphones, a Bluetooth headset).
    Noisy,
}

/// What to do about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    Pause,
    Resume,
}

/// Remembers whether a pause was the policy's, to resume it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Interruptions {
    /// The stream (its token) a transient loss paused, to resume on gain.
    paused: Option<u64>,
}

impl Interruptions {
    /// The host reported `what` while stream `token` is open (0: none) and
    /// `playing` (open and not paused).
    pub fn report(&mut self, what: Interruption, token: u64, playing: bool) -> Option<Response> {
        let open = token != 0;
        match what {
            Interruption::FocusLoss | Interruption::Noisy => {
                self.paused = None;
                (open && playing).then_some(Response::Pause)
            }
            Interruption::FocusLossTransient => {
                if open && playing {
                    self.paused = Some(token);
                    Some(Response::Pause)
                } else {
                    // Already paused or stopped: the viewer's state, kept.
                    None
                }
            }
            Interruption::FocusGain => {
                let resume = self.paused.take() == Some(token) && open && !playing;
                resume.then_some(Response::Resume)
            }
        }
    }

    /// The viewer played, paused, stopped or chose something else: a
    /// paused stream is theirs now and the focus coming back leaves it.
    pub fn viewer_acted(&mut self) {
        self.paused = None;
    }

    /// Whether the focus coming back would resume playback.
    pub fn resumes(&self) -> bool {
        self.paused.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Interruption::*;

    #[test]
    fn a_call_pauses_and_its_end_resumes() {
        let mut i = Interruptions::default();
        assert_eq!(i.report(FocusLossTransient, 7, true), Some(Response::Pause));
        assert!(i.resumes());
        assert_eq!(i.report(FocusGain, 7, false), Some(Response::Resume));
        assert!(!i.resumes());
        // A second gain does nothing.
        assert_eq!(i.report(FocusGain, 7, true), None);
    }

    #[test]
    fn another_app_or_unplugged_headphones_pause_for_good() {
        for what in [FocusLoss, Noisy] {
            let mut i = Interruptions::default();
            assert_eq!(i.report(what, 7, true), Some(Response::Pause), "{what:?}");
            assert_eq!(i.report(FocusGain, 7, false), None, "{what:?}");
        }
        // A permanent loss during a call's pause cancels its resume.
        let mut i = Interruptions::default();
        i.report(FocusLossTransient, 7, true);
        assert_eq!(i.report(Noisy, 7, false), None);
        assert_eq!(i.report(FocusGain, 7, false), None);
    }

    #[test]
    fn what_the_viewer_did_is_never_undone() {
        // Paused by the viewer before the call: stays paused.
        let mut i = Interruptions::default();
        assert_eq!(i.report(FocusLossTransient, 7, false), None);
        assert_eq!(i.report(FocusGain, 7, false), None);
        // Stopped: nothing to pause or resume, and nothing restarts.
        assert_eq!(i.report(FocusLossTransient, 0, false), None);
        assert_eq!(i.report(FocusGain, 0, false), None);
        assert_eq!(i.report(Noisy, 0, false), None);
        // The viewer played or paused during the call.
        let mut i = Interruptions::default();
        i.report(FocusLossTransient, 7, true);
        i.viewer_acted();
        assert_eq!(i.report(FocusGain, 7, false), None);
        // The viewer chose another stream during the call.
        let mut i = Interruptions::default();
        i.report(FocusLossTransient, 7, true);
        assert_eq!(i.report(FocusGain, 8, false), None);
        // The viewer stopped during the call: the focus coming back opens
        // nothing.
        let mut i = Interruptions::default();
        i.report(FocusLossTransient, 7, true);
        assert_eq!(i.report(FocusGain, 0, false), None);
    }
}
