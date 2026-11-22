//! Which one of several players is heard (#135): exactly one at a time,
//! pinned explicitly ("Listen to this"), never moved by focus.
//!
//! [`Heard`] keeps the pinned surface by its identity (`K`: a multiview
//! tile's id, a desktop player window's session or the main player), so
//! a presentation change (enlarging, reducing, a full-screen detour, a
//! window resized or made compact) never moves the sound. It also keeps
//! the surfaces heard before, most recent last, so that when the heard
//! one goes (a tile removed, a window closed) the sound goes back to the
//! one heard before it that is still there, else to the caller's
//! fallback (the main player). Nothing here plays anything: the caller
//! mutes every player but the heard one.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heard<K> {
    current: K,
    /// Surfaces heard before, the most recent last; never `current`, no
    /// duplicates.
    before: Vec<K>,
}

impl<K: Copy + PartialEq> Heard<K> {
    /// `current` is heard, nothing was before.
    pub fn new(current: K) -> Self {
        Heard {
            current,
            before: Vec::new(),
        }
    }

    /// The heard surface.
    pub fn current(&self) -> K {
        self.current
    }

    /// "Listen to this": `surface` is heard from now on; the one heard
    /// until now is remembered. Whether it changed.
    pub fn listen(&mut self, surface: K) -> bool {
        if surface == self.current {
            return false;
        }
        self.before.retain(|b| *b != surface);
        self.before.push(self.current);
        self.current = surface;
        true
    }

    /// `surface` went away. When it was the heard one the sound goes back
    /// to the most recent surface heard before it for which `present`
    /// holds, else to `fallback`.
    pub fn gone(&mut self, surface: K, fallback: K, present: impl Fn(K) -> bool) {
        self.before.retain(|b| *b != surface);
        if surface != self.current {
            return;
        }
        self.current = fallback;
        while let Some(previous) = self.before.pop() {
            if present(previous) {
                self.current = previous;
                break;
            }
        }
        let current = self.current;
        self.before.retain(|b| *b != current);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listening_moves_the_sound_and_remembers_where_it_was() {
        let mut h = Heard::new(0);
        assert!(!h.listen(0), "the heard one again changes nothing");
        assert!(h.listen(2));
        assert!(h.listen(3));
        assert_eq!(h.current(), 3);
        // Removing one heard before only forgets it.
        h.gone(2, 0, |_| true);
        assert_eq!(h.current(), 3);
        // The heard one gone: back to the one before it (2 is forgotten).
        h.gone(3, 9, |_| true);
        assert_eq!(h.current(), 0);
    }

    #[test]
    fn the_heard_one_gone_falls_back_past_surfaces_no_longer_there() {
        let mut h = Heard::new(1);
        h.listen(2);
        h.listen(3);
        // 2 went without being reported: skipped, then the fallback.
        h.gone(3, 7, |k| k != 2 && k != 1);
        assert_eq!(h.current(), 7);
        let mut h = Heard::new(1);
        h.listen(2);
        h.listen(1);
        h.listen(2);
        h.gone(2, 7, |_| true);
        assert_eq!(h.current(), 1, "no duplicate history");
        h.gone(1, 7, |_| true);
        assert_eq!(h.current(), 7);
    }
}
