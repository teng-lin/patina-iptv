//! Experimental desktop workspace for the multi-window UX demo.
//! Playback sessions outlive browsing and carry stable identities.
//!
//! Exactly one player is heard (#135): the main player (`None`) or one
//! session's window, kept by the session's identity ([`crate::heard`]),
//! so resizing, full screen or the compact presentation never move it,
//! nor does the keyboard focus. Opening a window does not take the sound;
//! Listen to this ([`Workspace::listen`], [`Workspace::listen_to_main`])
//! or a window's own sound controls do; closing the heard window hands
//! it back to the player heard before it that is still open, else to
//! the main player.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session<C> {
    pub id: SessionId,
    pub surface: usize,
    pub channel: C,
    pub separate: bool,
    /// Compact PiP presentation in this session's existing player window.
    pub compact: bool,
    pub paused: bool,
    pub volume: crate::sound::Volume,
    pub audio_delay: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace<C> {
    sessions: Vec<Session<C>>,
    next_id: u64,
    pub browsing: Option<C>,
    /// The heard player: a session, or the main player (`None`).
    heard: crate::heard::Heard<Option<SessionId>>,
}

impl<C> Default for Workspace<C> {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            next_id: 1,
            browsing: None,
            heard: crate::heard::Heard::new(None),
        }
    }
}

impl<C: Clone + PartialEq> Workspace<C> {
    pub fn sessions(&self) -> &[Session<C>] {
        &self.sessions
    }
    /// The session whose window is heard; `None`: the main player.
    pub fn audible(&self) -> Option<SessionId> {
        self.heard.current()
    }
    pub fn on_surface(&self, surface: usize) -> Option<&Session<C>> {
        self.sessions.iter().find(|s| s.surface == surface)
    }
    pub fn browse(&mut self, channel: C) {
        self.browsing = Some(channel);
    }

    /// Reopening a channel finds its existing player, without a new stream.
    /// A new one does not take the sound (an auxiliary one opens muted).
    pub fn open(&mut self, channel: C) -> Option<SessionId> {
        self.open_from(channel, 0)
    }

    /// The current app reserves surface zero for its main player.
    pub fn open_auxiliary(&mut self, channel: C) -> Option<SessionId> {
        self.open_from(channel, 1)
    }

    fn open_from(&mut self, channel: C, first_surface: usize) -> Option<SessionId> {
        if let Some(session) = self.sessions.iter_mut().find(|s| s.channel == channel) {
            session.separate = true;
            return Some(session.id);
        }
        let surface = (first_surface..crate::multiview::MAX_SURFACES)
            .find(|surface| self.on_surface(*surface).is_none())?;
        let id = SessionId(self.next_id);
        self.next_id += 1;
        self.sessions.push(Session {
            id,
            surface,
            channel,
            separate: true,
            compact: false,
            paused: false,
            volume: crate::sound::Volume {
                muted: first_surface > 0,
                ..Default::default()
            },
            audio_delay: 0,
        });
        Some(id)
    }

    pub fn return_to_main(&mut self, id: SessionId) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.separate = false;
        }
    }
    pub fn toggle_pause(&mut self, id: SessionId) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.paused = !s.paused;
        }
    }
    pub fn set_compact(&mut self, id: SessionId, compact: bool) {
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == id) {
            session.compact = compact;
        }
    }
    /// Listen to this: session `id`'s window is heard (unmuted), the
    /// others are not.
    pub fn listen(&mut self, id: SessionId) {
        if !self.sessions.iter().any(|s| s.id == id) {
            return;
        }
        for s in &mut self.sessions {
            s.volume.muted = s.id != id;
        }
        self.heard.listen(Some(id));
    }
    /// The main player's Listen to this and audio controls reclaim sound
    /// from auxiliary windows.
    pub fn listen_to_main(&mut self) {
        self.heard.listen(None);
        for session in &mut self.sessions {
            session.volume.muted = true;
        }
    }
    pub fn retarget(&mut self, id: SessionId, channel: C) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.channel = channel;
        }
    }
    pub fn set_paused(&mut self, id: SessionId, paused: bool) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.paused = paused;
        }
    }
    pub fn set_volume(&mut self, id: SessionId, volume: crate::sound::Volume) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.volume = volume;
            // Its own Mute gives the sound to the main player; its
            // volume (or unmuting) takes it.
            if volume.muted {
                if self.heard.current() == Some(id) {
                    self.heard.listen(None);
                }
            } else if self.heard.listen(Some(id)) {
                for other in self.sessions.iter_mut().filter(|s| s.id != id) {
                    other.volume.muted = true;
                }
            }
        }
    }
    pub fn set_audio_delay(&mut self, id: SessionId, delay: i32) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.audio_delay = delay;
        }
    }
    /// Closes session `id`; when its window was heard, the sound goes
    /// back to the player heard before it that is still open, else to the
    /// main player.
    pub fn close(&mut self, id: SessionId) {
        self.sessions.retain(|s| s.id != id);
        let sessions = &self.sessions;
        self.heard.gone(Some(id), None, |k| {
            k.is_none_or(|k| sessions.iter().any(|s| s.id == k))
        });
        if let Some(heard) = self.heard.current()
            && let Some(s) = self.sessions.iter_mut().find(|s| s.id == heard)
        {
            s.volume.muted = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_presentation_is_per_session_and_stale_actions_cannot_target_a_reused_surface() {
        let mut w = Workspace::default();
        let first = w.open_auxiliary("one").unwrap();
        let second = w.open_auxiliary("two").unwrap();
        w.set_compact(first, true);
        assert!(w.on_surface(1).unwrap().compact);
        assert!(!w.on_surface(2).unwrap().compact);
        w.set_compact(second, true);
        w.set_compact(first, false);
        assert!(w.on_surface(2).unwrap().compact);
        w.close(first);
        let replacement = w.open_auxiliary("three").unwrap();
        w.set_compact(first, true);
        assert_eq!(w.on_surface(1).unwrap().id, replacement);
        assert!(!w.on_surface(1).unwrap().compact);
    }
    #[test]
    fn main_audio_selection_mutes_auxiliaries_without_closing_them() {
        let mut w = Workspace::default();
        let id = w.open_auxiliary("one").unwrap();
        w.set_volume(id, crate::sound::Volume::default());
        assert_eq!(w.audible(), Some(id));
        w.listen_to_main();
        assert_eq!(w.audible(), None);
        assert!(w.on_surface(1).unwrap().volume.muted);
        w.set_volume(id, crate::sound::Volume::default());
        assert_eq!(w.audible(), Some(id), "sound can be selected again");
        // Taking it with another window's volume mutes this one.
        let other = w.open_auxiliary("two").unwrap();
        w.set_volume(other, crate::sound::Volume::default());
        assert_eq!(w.audible(), Some(other));
        assert!(w.on_surface(1).unwrap().volume.muted);
    }
    #[test]
    fn exactly_one_player_is_heard_and_closing_it_hands_the_sound_back() {
        let mut w = Workspace::default();
        let a = w.open_auxiliary("a").unwrap();
        let b = w.open_auxiliary("b").unwrap();
        assert_eq!(
            w.audible(),
            None,
            "opening windows keeps the main player heard"
        );
        assert!(w.sessions().iter().all(|s| s.volume.muted));
        // Compact or not, a window's presentation moves no sound.
        w.set_compact(a, true);
        assert_eq!(w.audible(), None);
        w.listen(a);
        assert_eq!(w.audible(), Some(a));
        assert!(!w.on_surface(1).unwrap().volume.muted);
        w.listen(b);
        assert_eq!(w.audible(), Some(b));
        assert!(w.on_surface(1).unwrap().volume.muted, "the other one muted");
        // Closing a window not heard changes nothing.
        let c = w.open_auxiliary("c").unwrap();
        w.close(c);
        assert_eq!(w.audible(), Some(b));
        // The heard one closed: the one heard before it, unmuted.
        w.close(b);
        assert_eq!(w.audible(), Some(a));
        assert!(!w.on_surface(1).unwrap().volume.muted);
        // Then the main player.
        w.close(a);
        assert_eq!(w.audible(), None);
        // A window heard before but closed meanwhile is skipped.
        let d = w.open_auxiliary("d").unwrap();
        let e = w.open_auxiliary("e").unwrap();
        w.listen(d);
        w.listen(e);
        w.close(d);
        w.close(e);
        assert_eq!(w.audible(), None);
        // Listen to this on the main player, then a window's back: the
        // main one heard before it gets the sound when it closes.
        let f = w.open_auxiliary("f").unwrap();
        let g = w.open_auxiliary("g").unwrap();
        w.listen(f);
        w.listen_to_main();
        w.listen(g);
        w.close(g);
        assert_eq!(w.audible(), None);
        assert!(w.on_surface(1).unwrap().volume.muted);
        // Stale identities move nothing.
        w.listen(g);
        assert_eq!(w.audible(), None);
    }
    #[test]
    fn auxiliary_players_reserve_the_main_surface_and_enforce_the_limit() {
        let mut w = Workspace::default();
        let one = w.open_auxiliary("one").unwrap();
        assert!(w.on_surface(0).is_none());
        assert_eq!(w.on_surface(1).unwrap().id, one);
        w.open_auxiliary("two").unwrap();
        w.open_auxiliary("three").unwrap();
        assert_eq!(w.open_auxiliary("four"), None);
        assert_eq!(w.open_auxiliary("one"), Some(one));
    }
    #[test]
    fn browsing_and_returning_preserve_independent_sessions() {
        let mut w = Workspace::default();
        let first = w.open("one").unwrap();
        let second = w.open("two").unwrap();
        let before = w.sessions().to_vec();
        w.browse("three");
        assert_eq!(w.sessions(), before);
        w.return_to_main(first);
        assert!(!w.sessions()[0].separate);
        assert_eq!(w.sessions()[0].id, first);
        assert!(w.sessions()[1].separate);
        assert_eq!(w.open("one"), Some(first));
        w.toggle_pause(second);
        assert!(!w.sessions()[0].paused && w.sessions()[1].paused);
        w.listen(second);
        assert_eq!(w.audible(), Some(second));
    }
    #[test]
    fn closing_one_releases_only_its_surface_and_old_actions_cannot_close_a_new_session() {
        let mut w = Workspace::default();
        let first = w.open("one").unwrap();
        let second = w.open("two").unwrap();
        w.close(first);
        let replacement = w.open("three").unwrap();
        assert_ne!(first, replacement);
        w.close(first);
        assert_eq!(w.sessions().len(), 2);
        assert_eq!(w.on_surface(0).unwrap().id, replacement);
        assert_eq!(w.on_surface(1).unwrap().id, second);
    }
}
