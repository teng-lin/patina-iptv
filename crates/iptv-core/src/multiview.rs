//! Multiview: up to four channels at once, one of them *active* (the
//! remote's focus is on it) and one *heard* (its sound plays, the others
//! are muted). Focus never moves the sound (#135): the heard tile changes
//! only by its menu's Listen to this ([`Multiview::listen`]), unless the
//! viewer turned on "Sound follows the selected tile" (#179,
//! [`Multiview::set_sound_follows_focus`]): then each focus move of the
//! viewer's (an arrow, a tap on another tile, a tile added, a picked
//! channel already on another tile) is a Listen to this on the tile it
//! lands on. A focus that stays (changing the focused tile's channel) takes
//! nothing. Enlarging or reducing, a detour and a tile removed move the
//! focus by themselves and keep the rules below either way. Opening
//! keeps the sound on the channel that was playing (the first tile);
//! adding or replacing a tile does not take it (a replaced heard tile
//! stays heard, with its new channel); removing the heard tile hands it
//! back to the tile heard before it, else to the one on surface 0 (the
//! full-screen player), else the first ([`crate::heard`]). The heard tile
//! is kept by its identity ([`Tile::id`]), so enlarging, reducing,
//! reflowing and a detour never move it; on a detour the tile watched
//! full screen is heard, and the grid coming back brings the sound back
//! to the heard tile ([`Multiview::audible_surface`]).
//!
//! The screen holds just the tiles ([`Layout`]): no empty cells, and the
//! tiles flow again at once when one comes or goes. One tile sits centered
//! at half the width, two side by side, three as two above one, four two
//! by two. A tile can be *enlarged*: big at the top, the others in a strip
//! under it; enlarging another moves the big place to it, reducing it
//! brings the grid back, and moving the focus leaves it be.
//!
//! The arrows move between tiles; OK (or a tap on the active tile) opens
//! its menu (`menu.rs`): try again (when it failed), add a screen from the
//! channel list or the search screen (while another fits), change
//! channel, pause or play that tile alone, enlarge or reduce (with two or
//! more), Listen to this (with two or more, on a tile not heard), full
//! screen, remove (with two or more). Long OK (a long press on
//! a tile) enlarges or reduces directly. A tap on another tile makes it
//! active. Remove acts at once; removing the heard tile hands the sound
//! back as above.
//! Back closes a dialog, else leaves multiview to the active tile in full
//! screen.
//!
//! Full screen is a *detour*: that tile plays full screen while the
//! session stays as it was (tiles, channels, the enlarged one, the active
//! one), and [`Multiview::resume`] brings the grid back. The tile moves
//! to surface 0 (the full-screen player) for it, trading surfaces with the
//! tile that had it; the other tiles keep playing, hidden and muted, so
//! the way back opens nothing. What ends a detour is the caller's rule.
//!
//! Opening starts
//! with the playing channel as the only tile and asks for the next one at
//! once (canceling leaves the one tile); a short key hint shows until a
//! key or a few seconds pass (the caller times it).
//!
//! Each tile keeps the player surface it was given (0 to [`MAX_SURFACES`]
//! − 1; the first tile starts on surface 0, the full-screen player), so
//! adding, removing, replacing, enlarging or reflowing never disturbs the
//! other tiles' streams. Adding past the account's connection limit asks
//! first; once the viewer insists it does not ask again in this multiview.
//! Channels are the caller's (`C`); nothing here plays anything.

#[cfg(test)]
mod heard_tests;
mod layout;
mod menu;
#[cfg(test)]
mod tests;

pub use layout::{Layout, Rect, THUMB, VIDEO};

use crate::heard::Heard;
pub use menu::{Dialog, TileAction, WARNING_CHOICES, WarningChoice};

/// The most tiles (and player surfaces) multiview uses.
pub const MAX_SURFACES: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile<C> {
    /// Its identity for the session: kept while it plays, whatever its
    /// place, surface or channel; never reused.
    id: u32,
    /// The player surface it plays on (0 to [`MAX_SURFACES`] − 1).
    pub surface: usize,
    pub channel: C,
    /// Its player holds (the menu's Pause); the others play on.
    pub paused: bool,
}

impl<C> Tile<C> {
    /// Its identity for the session (what the heard tile is kept by).
    pub fn id(&self) -> u32 {
        self.id
    }
}

/// What the channel being picked is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Add,
    /// The channel of the tile at this index.
    Replace(usize),
}

/// The remote's keys as multiview takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Ok,
    /// A long OK: enlarges or reduces the active tile.
    LongOk,
    Back,
    /// The active tile's menu.
    Menu,
}

/// What the caller does after a step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<C> {
    Stay,
    /// Open the channel list; the choice goes to [`Multiview::picked`]
    /// (closing it without one to [`Multiview::cancel_pick`]).
    PickChannel,
    /// Open the search screen to pick, as for [`Outcome::PickChannel`].
    SearchChannel,
    /// Open the stream on this surface again.
    Retry(usize),
    /// A detour began: the detour tile (now on surface 0) plays full
    /// screen, the session waits ([`Multiview::detour`]).
    FullScreen,
    /// Leave multiview; this channel (on `surface`) plays full screen.
    Leave {
        channel: C,
        surface: usize,
    },
}

/// What bounds the number of tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Streams this device decodes at once (1 to [`MAX_SURFACES`]).
    pub decoders: usize,
    /// Streams the account allows at once, when it says.
    pub connections: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Multiview<C> {
    tiles: Vec<Tile<C>>,
    /// The tile the remote's focus is on.
    active: usize,
    /// The tile whose sound plays, by [`Tile::id`].
    heard: Heard<u32>,
    /// Focus moves of the viewer's make the tile heard (#179).
    sound_follows_focus: bool,
    /// The next tile's id.
    next_id: u32,
    /// The enlarged tile (only with two or more).
    enlarged: Option<usize>,
    limits: Limits,
    /// The viewer chose to go past the account's limit.
    insisted: bool,
    dialog: Option<Dialog>,
    pick: Option<Pick>,
    /// A source-specific limit can only be checked after choosing a channel.
    pending_pick: Option<(Pick, C)>,
    /// The key hint shows (until a key, or the caller's timeout).
    hint: bool,
    /// The tile watched full screen, the grid waiting.
    detour: Option<usize>,
}

impl<C: Clone + PartialEq> Multiview<C> {
    /// Multiview with `channel` (playing full screen, on surface 0) as its
    /// only tile, active. The caller then usually starts adding
    /// ([`Multiview::add`]).
    pub fn open(channel: C, limits: Limits) -> Self {
        let limits = Limits {
            decoders: limits.decoders.clamp(1, MAX_SURFACES),
            ..limits
        };
        Multiview {
            tiles: vec![Tile {
                id: 0,
                surface: 0,
                channel,
                paused: false,
            }],
            active: 0,
            heard: Heard::new(0),
            sound_follows_focus: false,
            next_id: 1,
            enlarged: None,
            limits,
            insisted: false,
            dialog: None,
            pick: None,
            pending_pick: None,
            hint: true,
            detour: None,
        }
    }

    pub fn tiles(&self) -> &[Tile<C>] {
        &self.tiles
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Another tile fits on this device.
    pub fn can_add(&self) -> bool {
        self.tiles.len() < self.limits.decoders
    }

    pub fn layout(&self) -> Layout {
        let tiles = self.tiles.len();
        match self.enlarged {
            Some(big) if tiles >= 2 => Layout::Enlarged { tiles, big },
            _ => Layout::Grid { tiles },
        }
    }

    /// The enlarged tile, by index.
    pub fn enlarged(&self) -> Option<usize> {
        self.enlarged
    }

    /// The tile the remote's focus is on.
    pub fn active(&self) -> &Tile<C> {
        &self.tiles[self.active]
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    /// The tile whose sound plays in the grid, by index (focus never
    /// changes it).
    pub fn heard_index(&self) -> usize {
        let id = self.heard.current();
        self.tiles.iter().position(|t| t.id == id).unwrap_or(0)
    }

    /// The surface heard now: the heard tile's, or on a detour the one
    /// watched full screen (surface 0).
    pub fn audible_surface(&self) -> usize {
        if self.detour.is_some() {
            return 0;
        }
        self.tiles.get(self.heard_index()).map_or(0, |t| t.surface)
    }

    /// Listen to this: tile `index` is heard from now on (also while the
    /// focus goes elsewhere). Whether it changed.
    pub fn listen(&mut self, index: usize) -> bool {
        let Some(id) = self.tiles.get(index).map(|t| t.id) else {
            return false;
        };
        self.dialog = None;
        self.heard.listen(id)
    }

    /// "Sound follows the selected tile" (#179): from now on, each focus
    /// move of the viewer's makes the tile it lands on heard (an arrow, a
    /// tap on another tile, a tile added, a picked channel already on
    /// another tile). A focus that stays where it is takes nothing, and
    /// turning it on moves nothing by itself.
    pub fn set_sound_follows_focus(&mut self, on: bool) {
        self.sound_follows_focus = on;
    }

    pub fn sound_follows_focus(&self) -> bool {
        self.sound_follows_focus
    }

    /// The viewer moved the focus to tile `index`; with the sound
    /// following the focus it is heard too. Focus that stays where it is
    /// (changing the focused tile's channel, picking the channel it
    /// already plays) moves no sound, so a tile chosen with Listen to
    /// this keeps it.
    fn focus(&mut self, index: usize) {
        let moved = index != self.active;
        self.active = index;
        if moved
            && self.sound_follows_focus
            && let Some(id) = self.tiles.get(index).map(|t| t.id)
        {
            self.heard.listen(id);
        }
    }

    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
    }

    pub fn pick(&self) -> Option<Pick> {
        self.pick
    }

    /// Whether the key hint still shows.
    pub fn hint(&self) -> bool {
        self.hint
    }

    /// The hint's time is up.
    pub fn dismiss_hint(&mut self) {
        self.hint = false;
    }

    /// The tile watched full screen while the grid waits (a detour).
    pub fn detour(&self) -> Option<usize> {
        self.detour
    }

    /// Back from a detour: the grid as it was.
    pub fn resume(&mut self) {
        self.detour = None;
    }

    /// The tile on `surface`, by index.
    pub fn tile_on(&self, surface: usize) -> Option<usize> {
        self.tiles.iter().position(|t| t.surface == surface)
    }

    /// What each surface plays (by surface number).
    pub fn assignments(&self) -> [Option<C>; MAX_SURFACES] {
        let mut out: [Option<C>; MAX_SURFACES] = Default::default();
        for t in &self.tiles {
            out[t.surface] = Some(t.channel.clone());
        }
        out
    }

    /// Adding one more stream goes past the account's limit.
    pub fn over_connections(&self) -> Option<u32> {
        self.limits
            .connections
            .filter(|&allowed| self.tiles.len() as u32 >= allowed)
    }

    /// A key; `failed(surface)` says whether a surface's stream failed
    /// (its menu then offers Retry).
    pub fn key(&mut self, key: Key, failed: impl Fn(usize) -> bool) -> Outcome<C> {
        self.hint = false;
        if self.detour.is_some() {
            return Outcome::Stay;
        }
        if let Some(dialog) = &mut self.dialog {
            let (focus, len, across) = match dialog {
                Dialog::Options { items, focus, .. } => (focus, items.len(), false),
                Dialog::Warning { focus, .. } => (focus, WARNING_CHOICES.len(), true),
            };
            let step = |focus: &mut usize, by: isize| {
                *focus = (*focus as isize + by).clamp(0, len as isize - 1) as usize;
            };
            match key {
                Key::Up if !across => step(focus, -1),
                Key::Down if !across => step(focus, 1),
                Key::Left if across => step(focus, -1),
                Key::Right if across => step(focus, 1),
                Key::Ok => {
                    let at = *focus;
                    return self.choose(at);
                }
                Key::Back | Key::Menu => {
                    self.dialog = None;
                    self.pending_pick = None;
                }
                _ => {}
            }
            return Outcome::Stay;
        }
        let way = match key {
            Key::Left => Some((-1, 0)),
            Key::Right => Some((1, 0)),
            Key::Up => Some((0, -1)),
            Key::Down => Some((0, 1)),
            Key::Ok | Key::Menu => {
                self.open_options(self.active, &failed);
                return Outcome::Stay;
            }
            Key::LongOk => {
                self.toggle_enlarge(self.active);
                return Outcome::Stay;
            }
            Key::Back => return self.leave(self.active),
        };
        if let Some((dx, dy)) = way
            && let Some(to) = self.layout().neighbor(self.active, dx, dy)
        {
            self.focus(to);
        }
        Outcome::Stay
    }

    /// A tap on tile `index`: another tile becomes active, the active one
    /// opens its menu. With a dialog open, any tap (also beside the tiles)
    /// only closes it.
    pub fn tap(&mut self, index: usize, failed: impl Fn(usize) -> bool) -> Outcome<C> {
        if self.dialog.take().is_some() || index >= self.tiles.len() || self.detour.is_some() {
            self.pending_pick = None;
            return Outcome::Stay;
        }
        if index == self.active {
            self.open_options(index, &failed);
        } else {
            self.focus(index);
        }
        Outcome::Stay
    }

    /// Removes tile `index` at once (a tile's remove button); never the
    /// last one. Whether it went.
    pub fn remove_tile(&mut self, index: usize) -> bool {
        if self.tiles.len() < 2 || index >= self.tiles.len() {
            return false;
        }
        self.dialog = None;
        self.remove(index);
        true
    }

    /// Enlarges tile `index` (taking the big place from another), or
    /// reduces it when it is the enlarged one; it becomes active. Nothing
    /// with a single tile.
    pub fn toggle_enlarge(&mut self, index: usize) {
        if self.tiles.len() < 2 || index >= self.tiles.len() {
            return;
        }
        self.dialog = None;
        self.enlarged = if self.enlarged == Some(index) {
            None
        } else {
            Some(index)
        };
        self.active = index;
    }

    /// Pauses (or plays again) tile `index` alone.
    pub fn set_paused(&mut self, index: usize, paused: bool) {
        if let Some(t) = self.tiles.get_mut(index) {
            t.paused = paused;
        }
    }

    /// Leaves multiview to the active tile in full screen (the pointer's
    /// toolbar).
    pub fn exit(&mut self) -> Outcome<C> {
        self.dialog = None;
        self.leave(self.active)
    }

    /// Defer a chosen channel behind the usual connection warning. The caller
    /// counts streams for its owning account, excluding any replaced tile.
    pub fn picked_with_warning(&mut self, channel: C, exceeded: Option<u32>) -> Outcome<C> {
        if let (Some(pick), Some(allowed)) = (self.pick, exceeded)
            && !self.tiles.iter().any(|t| t.channel == channel)
        {
            self.pick = None;
            self.pending_pick = Some((pick, channel));
            self.dialog = Some(Dialog::Warning {
                allowed,
                focus: 1,
                search: false,
            });
            return Outcome::Stay;
        }
        self.picked(channel)
    }

    /// The channel list chose `channel` for the pending pick. A channel
    /// already on a tile only makes that tile active.
    pub fn picked(&mut self, channel: C) -> Outcome<C> {
        let Some(pick) = self.pick.take() else {
            return Outcome::Stay;
        };
        if let Some(at) = self.tiles.iter().position(|t| t.channel == channel) {
            self.focus(at);
            return Outcome::Stay;
        }
        match pick {
            Pick::Add if self.can_add() => {
                let surface = (0..MAX_SURFACES)
                    .find(|s| self.tile_on(*s).is_none())
                    .unwrap_or(0);
                let id = self.next_id;
                self.next_id += 1;
                self.tiles.push(Tile {
                    id,
                    surface,
                    channel,
                    paused: false,
                });
                self.focus(self.tiles.len() - 1);
            }
            Pick::Add => {}
            Pick::Replace(tile) => {
                if let Some(t) = self.tiles.get_mut(tile) {
                    t.channel = channel;
                    t.paused = false;
                    self.focus(tile);
                }
            }
        }
        Outcome::Stay
    }

    /// The selected channel awaiting confirmation of a connection-limit warning.
    pub fn pending_pick_channel(&self) -> Option<&C> {
        self.pending_pick.as_ref().map(|(_, channel)| channel)
    }

    /// The channel list closed without a choice.
    pub fn cancel_pick(&mut self) {
        self.pick = None;
    }

    /// Keeps the tiles `f` keeps (it may change their channel, e.g. when
    /// the data changed); `false` when none is left.
    pub fn retain(&mut self, mut f: impl FnMut(usize, &mut C) -> bool) -> bool {
        if self.pending_pick.take().is_some() {
            self.dialog = None;
        }
        let mut at = 0;
        while at < self.tiles.len() {
            let t = &mut self.tiles[at];
            if f(t.surface, &mut t.channel) {
                at += 1;
            } else {
                self.remove(at);
            }
        }
        !self.tiles.is_empty()
    }

    /// Tile `tile` full screen as a detour: it becomes active and moves
    /// to surface 0, the tile there (if any) taking its surface.
    fn full_screen(&mut self, tile: usize) -> Outcome<C> {
        let Some(surface) = self.tiles.get(tile).map(|t| t.surface) else {
            return Outcome::Stay;
        };
        if surface != 0 {
            if let Some(main) = self.tile_on(0) {
                self.tiles[main].surface = surface;
            }
            self.tiles[tile].surface = 0;
        }
        self.tiles[tile].paused = false;
        self.active = tile;
        self.detour = Some(tile);
        self.hint = false;
        Outcome::FullScreen
    }

    fn leave(&mut self, tile: usize) -> Outcome<C> {
        let t = &self.tiles[tile];
        Outcome::Leave {
            channel: t.channel.clone(),
            surface: t.surface,
        }
    }

    fn remove(&mut self, tile: usize) {
        if tile >= self.tiles.len() {
            return;
        }
        let gone = self.tiles.remove(tile).id;
        // The sound back to the tile heard before, else the full-screen
        // player's, else the first.
        let fallback = self
            .tiles
            .iter()
            .find(|t| t.surface == 0)
            .or(self.tiles.first())
            .map_or(gone, |t| t.id);
        let tiles = &self.tiles;
        self.heard
            .gone(gone, fallback, |id| tiles.iter().any(|t| t.id == id));
        let shift = |i: usize| if i > tile { i - 1 } else { i };
        self.pick = match self.pick {
            Some(Pick::Replace(t)) if t == tile => None,
            Some(Pick::Replace(t)) => Some(Pick::Replace(shift(t))),
            p => p,
        };
        // The enlarged tile gone (or one tile left): the grid again.
        self.enlarged = self
            .enlarged
            .filter(|&e| e != tile && self.tiles.len() >= 2)
            .map(shift);
        if matches!(self.dialog, Some(Dialog::Options { .. })) {
            self.dialog = None;
        }
        if self.active > tile || self.active >= self.tiles.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.detour = self.detour.filter(|&d| d != tile).map(shift);
    }
}
