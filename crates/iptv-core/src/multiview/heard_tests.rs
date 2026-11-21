//! The heard tile (#135): focus never moves the sound, Listen to this
//! does, and the heard tile is kept by identity through presentation
//! changes and handed back when it goes.

use super::*;

const DESKTOP: Limits = Limits {
    decoders: 4,
    connections: None,
};

fn ok(_: usize) -> bool {
    false
}

fn add(mv: &mut Multiview<u32>, channel: u32) {
    assert_eq!(mv.add(), Outcome::PickChannel);
    assert_eq!(mv.picked(channel), Outcome::Stay);
}

fn heard(mv: &Multiview<u32>) -> u32 {
    mv.tiles()[mv.heard_index()].channel
}

fn items(mv: &Multiview<u32>) -> Vec<TileAction> {
    match mv.dialog() {
        Some(Dialog::Options { items, .. }) => items.clone(),
        _ => vec![],
    }
}

/// Tile `index`'s menu item `action` (a tap focuses it, OK opens its
/// menu).
fn choose(mv: &mut Multiview<u32>, index: usize, action: TileAction) -> Outcome<u32> {
    if mv.active_index() != index {
        mv.tap(index, ok);
    }
    mv.key(Key::Ok, ok);
    let at = items(mv)
        .iter()
        .position(|a| *a == action)
        .unwrap_or_else(|| panic!("{action:?} in {:?}", items(mv)));
    mv.choose(at)
}

#[test]
fn opening_and_adding_keep_the_sound_on_the_playing_channel() {
    let mut mv = Multiview::open(10, DESKTOP);
    assert_eq!(heard(&mv), 10);
    assert_eq!(mv.audible_surface(), 0);
    add(&mut mv, 11);
    add(&mut mv, 12);
    assert_eq!(mv.active().channel, 12, "the new tile has the focus");
    assert_eq!(heard(&mv), 10, "but not the sound");
    // Picking a channel already on a tile only focuses it.
    mv.add();
    mv.picked(11);
    assert_eq!(mv.active().channel, 11);
    assert_eq!(heard(&mv), 10);
}

#[test]
fn focus_never_moves_the_sound_listen_to_this_does() {
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    add(&mut mv, 12);
    add(&mut mv, 13);
    for key in [Key::Left, Key::Up, Key::Right, Key::Down, Key::Left] {
        mv.key(key, ok);
        assert_eq!(heard(&mv), 10, "{key:?} moved no sound");
    }
    mv.tap(2, ok);
    assert_eq!(heard(&mv), 10, "a tap focuses only");
    mv.toggle_enlarge(3);
    assert_eq!(heard(&mv), 10, "enlarging focuses only");
    // The heard tile's menu has no Listen to this; the others' have.
    mv.tap(0, ok);
    mv.tap(0, ok);
    assert!(!items(&mv).contains(&TileAction::Listen));
    mv.key(Key::Back, ok);
    assert_eq!(choose(&mut mv, 2, TileAction::Listen), Outcome::Stay);
    assert_eq!(heard(&mv), 12);
    assert!(mv.dialog().is_none());
    assert_eq!(mv.audible_surface(), 2);
    // Working on another tile keeps it there.
    mv.tap(1, ok);
    mv.tap(1, ok);
    assert!(items(&mv).contains(&TileAction::Listen));
    mv.key(Key::Back, ok);
    assert_eq!(heard(&mv), 12);
    // A single tile offers no Listen to this.
    let mut one = Multiview::open(1u32, DESKTOP);
    one.key(Key::Ok, ok);
    assert!(!items(&one).contains(&TileAction::Listen));
}

#[test]
fn replacing_the_heard_tile_keeps_the_sound_on_that_tile() {
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    mv.listen(1);
    assert_eq!(
        choose(&mut mv, 1, TileAction::Replace),
        Outcome::PickChannel
    );
    mv.picked(20);
    assert_eq!(heard(&mv), 20);
    // Replacing another one does not take it.
    assert_eq!(
        choose(&mut mv, 0, TileAction::Replace),
        Outcome::PickChannel
    );
    mv.picked(30);
    assert_eq!(heard(&mv), 20);
}

#[test]
fn removing_the_heard_tile_hands_the_sound_back() {
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    add(&mut mv, 12);
    add(&mut mv, 13);
    mv.listen(1);
    mv.listen(3);
    // Removing another tile changes nothing.
    assert!(mv.remove_tile(2));
    assert_eq!(heard(&mv), 13);
    // The heard one gone: back to the one heard before it (11).
    assert!(mv.remove_tile(2));
    assert_eq!(heard(&mv), 11);
    // Then to the one heard before that (10, the first).
    assert!(mv.remove_tile(1));
    assert_eq!(heard(&mv), 10);
    // Nothing heard before still there: the full-screen player's tile.
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    add(&mut mv, 12);
    mv.listen(2);
    mv.remove_tile(0);
    add(&mut mv, 13); // Takes the free surface 0.
    assert_eq!(heard(&mv), 12);
    let at = mv.tiles().iter().position(|t| t.channel == 12).unwrap();
    mv.remove_tile(at);
    assert_eq!(heard(&mv), 13, "the tile on surface 0");
    assert_eq!(mv.audible_surface(), 0);
    // Removing from the menu, and the data taking a tile away, too.
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    add(&mut mv, 12);
    mv.listen(2);
    assert_eq!(choose(&mut mv, 2, TileAction::Remove), Outcome::Stay);
    assert_eq!(heard(&mv), 10);
    mv.listen(1);
    assert!(mv.retain(|_, c| *c != 11));
    assert_eq!(heard(&mv), 10);
}

#[test]
fn presentation_changes_keep_the_heard_tile_by_identity() {
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    add(&mut mv, 12);
    mv.listen(1);
    // Enlarging and reducing any tile.
    mv.toggle_enlarge(2);
    mv.toggle_enlarge(1);
    mv.toggle_enlarge(1);
    assert_eq!(heard(&mv), 11);
    assert_eq!(mv.audible_surface(), 1);
    // A detour: tile 2 full screen on surface 0 (trading with tile 0);
    // the full-screen one is heard meanwhile.
    assert_eq!(
        choose(&mut mv, 2, TileAction::FullScreen),
        Outcome::FullScreen
    );
    assert_eq!(mv.audible_surface(), 0);
    assert_eq!(mv.tiles()[2].surface, 0);
    // Back to the grid: the sound on the heard tile again, which never
    // changed its surface.
    mv.resume();
    assert_eq!(heard(&mv), 11);
    assert_eq!(mv.audible_surface(), 1);
    // A detour of the heard tile itself: it is heard on surface 0, and
    // stays heard (now on the surface it traded for) after.
    choose(&mut mv, 1, TileAction::FullScreen);
    assert_eq!(mv.audible_surface(), 0);
    mv.resume();
    assert_eq!(heard(&mv), 11);
    assert_eq!(mv.audible_surface(), 0);
    // Removing a tile before it reflows the grid; the sound stays.
    mv.remove_tile(0);
    assert_eq!(heard(&mv), 11);
}
