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
            compact: false,
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
    pub fn set_compact(&mut self, id: SessionId, compact: bool) {
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == id) {
            session.compact = compact;
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

/// A rectangle in screen points: `(x, y, width, height)`.
pub type Rect = (f32, f32, f32, f32);

/// How much of a window's top edge must stay on a display for the viewer
/// to grab it again (points): a title bar's height, and a width enough to
/// aim at.
pub const GRAB_HEIGHT: f32 = 32.0;
pub const GRAB_WIDTH: f32 = 96.0;

/// Where a window must go so the viewer can reach it again, after a
/// display it was on went away or the displays were arranged anew
/// (`docs/desktop.md`, "Displays"): `None` when enough of its top edge
/// is still on one of `areas` (the displays' usable areas), or when the
/// host reports no display to measure against. Otherwise the window
/// moves onto the area nearest to it, the first (primary) one on a tie,
/// keeping its size unless it is larger than that area. A presentation
/// change only: the caller moves the window, its player plays on.
pub fn reachable(window: Rect, areas: &[Rect]) -> Option<Rect> {
    let finite = |r: &Rect| [r.0, r.1, r.2, r.3].into_iter().all(f32::is_finite);
    if !finite(&window) {
        return None;
    }
    let areas: Vec<Rect> = areas
        .iter()
        .copied()
        .filter(|a| finite(a) && a.2 > 0.0 && a.3 > 0.0)
        .collect();
    if areas.is_empty() {
        return None;
    }
    let (x, y, w, h) = window;
    let grab_w = GRAB_WIDTH.min(w.max(1.0));
    let grab_h = GRAB_HEIGHT.min(h.max(1.0));
    let grabbable = areas.iter().any(|&(ax, ay, aw, ah)| {
        let across = (x + w).min(ax + aw) - x.max(ax);
        let down = (y + grab_h).min(ay + ah) - y.max(ay);
        across >= grab_w && down >= grab_h
    });
    if grabbable {
        return None;
    }
    // The area whose rectangle is closest to the window's centre.
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let distance = |&(ax, ay, aw, ah): &Rect| {
        let dx = (ax - cx).max(cx - (ax + aw)).max(0.0);
        let dy = (ay - cy).max(cy - (ay + ah)).max(0.0);
        dx * dx + dy * dy
    };
    let mut nearest = areas[0];
    for area in &areas[1..] {
        if distance(area) < distance(&nearest) {
            nearest = *area;
        }
    }
    let (ax, ay, aw, ah) = nearest;
    let (w, h) = (w.min(aw), h.min(ah));
    Some((x.clamp(ax, ax + aw - w), y.clamp(ay, ay + ah - h), w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    const PRIMARY: Rect = (0.0, 0.0, 1920.0, 1040.0);
    const LEFT: Rect = (-1512.0, -200.0, 1512.0, 945.0);

    #[test]
    fn a_window_with_its_top_edge_on_a_display_stays_where_the_viewer_put_it() {
        assert_eq!(reachable((100.0, 80.0, 960.0, 540.0), &[PRIMARY]), None);
        // Mostly off the right edge, but its title bar can still be grabbed.
        assert_eq!(reachable((1800.0, 900.0, 960.0, 540.0), &[PRIMARY]), None);
        // On the second display, left of and above the primary.
        assert_eq!(
            reachable((-1400.0, -150.0, 960.0, 540.0), &[PRIMARY, LEFT]),
            None
        );
        // No display reported (a host that tells none): leave it alone.
        assert_eq!(reachable((-9000.0, 0.0, 640.0, 360.0), &[]), None);
    }

    #[test]
    fn removing_a_display_brings_its_windows_onto_the_nearest_remaining_one() {
        // The left display went away: the window comes onto the primary's
        // left edge, at the same size.
        let moved = reachable((-1400.0, -150.0, 960.0, 540.0), &[PRIMARY]).unwrap();
        assert_eq!(moved, (0.0, 0.0, 960.0, 540.0));
        // A window only a sliver of which shows keeps its size and lands
        // inside, nearest to where it was.
        let moved = reachable((1900.0, 500.0, 640.0, 360.0), &[PRIMARY]).unwrap();
        assert_eq!(moved, (1280.0, 500.0, 640.0, 360.0));
        // Its title bar above the top of every display is unreachable too.
        let moved = reachable((200.0, -500.0, 640.0, 360.0), &[PRIMARY]).unwrap();
        assert_eq!(moved, (200.0, 0.0, 640.0, 360.0));
    }

    #[test]
    fn a_window_larger_than_the_remaining_display_shrinks_to_fit_it() {
        let small = (100.0, 30.0, 1280.0, 690.0);
        let moved = reachable((-1500.0, -190.0, 1500.0, 900.0), &[small]).unwrap();
        assert_eq!(moved, small);
    }

    #[test]
    fn the_nearest_display_wins_and_reconnecting_needs_no_move() {
        let right = (1920.0, 0.0, 2560.0, 1400.0);
        // Off the far right: the right display is nearer than the primary.
        let moved = reachable((5000.0, 100.0, 640.0, 360.0), &[PRIMARY, right]).unwrap();
        assert_eq!(moved, (4480.0 - 640.0, 100.0, 640.0, 360.0));
        // After the display returns, the restored placement is reachable.
        assert_eq!(
            reachable((-1400.0, -150.0, 960.0, 540.0), &[PRIMARY, LEFT]),
            None
        );
        // Garbage from a host never moves a window.
        assert_eq!(reachable((f32::NAN, 0.0, 640.0, 360.0), &[PRIMARY]), None);
        assert_eq!(
            reachable((-5000.0, 0.0, 640.0, 360.0), &[(0.0, 0.0, 0.0, 0.0)]),
            None
        );
    }
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
