//! Experimental desktop workspace for the multi-window UX demo.
//! Playback sessions outlive browsing and carry stable identities.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session<C> {
    pub id: SessionId,
    pub surface: usize,
    pub channel: C,
    pub separate: bool,
    pub paused: bool,
    pub volume: crate::sound::Volume,
    pub audio_delay: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace<C> {
    sessions: Vec<Session<C>>,
    next_id: u64,
    pub browsing: Option<C>,
    pub audible: Option<SessionId>,
}

impl<C> Default for Workspace<C> {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            next_id: 1,
            browsing: None,
            audible: None,
        }
    }
}

impl<C: Clone + PartialEq> Workspace<C> {
    pub fn sessions(&self) -> &[Session<C>] {
        &self.sessions
    }
    pub fn on_surface(&self, surface: usize) -> Option<&Session<C>> {
        self.sessions.iter().find(|s| s.surface == surface)
    }
    pub fn browse(&mut self, channel: C) {
        self.browsing = Some(channel);
    }

    /// Reopening a channel finds its existing player, without a new stream.
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
            paused: false,
            volume: crate::sound::Volume {
                muted: first_surface > 0,
                ..Default::default()
            },
            audio_delay: 0,
        });
        if self.audible.is_none() {
            self.audible = Some(id);
        }
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
    pub fn listen(&mut self, id: SessionId) {
        if self.sessions.iter().any(|s| s.id == id) {
            self.audible = Some(id);
        }
    }
    /// The main player's audio controls reclaim sound from auxiliary windows.
    pub fn listen_to_main(&mut self) {
        self.audible = None;
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
            if volume.muted {
                if self.audible == Some(id) {
                    self.audible = None;
                }
            } else {
                self.audible = Some(id);
            }
        }
    }
    pub fn set_audio_delay(&mut self, id: SessionId, delay: i32) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.audio_delay = delay;
        }
    }
    pub fn close(&mut self, id: SessionId) {
        self.sessions.retain(|s| s.id != id);
        if self.audible == Some(id) {
            self.audible = self.sessions.iter().find(|s| !s.volume.muted).map(|s| s.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn main_audio_selection_mutes_auxiliaries_without_closing_them() {
        let mut w = Workspace::default();
        let id = w.open_auxiliary("one").unwrap();
        w.set_volume(id, crate::sound::Volume::default());
        assert_eq!(w.audible, Some(id));
        w.listen_to_main();
        assert_eq!(w.audible, None);
        assert!(w.on_surface(1).unwrap().volume.muted);
        w.set_volume(id, crate::sound::Volume::default());
        assert_eq!(w.audible, Some(id), "sound can be selected again");
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
        assert_eq!(w.audible, Some(second));
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
