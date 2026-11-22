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

/// "Sound follows the selected tile" on (#179): tiles 10 to 13 (the grid
/// 10 11 / 12 13), the focus on 13, the sound still on 10.
fn following() -> Multiview<u32> {
    let mut mv = Multiview::open(10, DESKTOP);
    for channel in 11..=13 {
        add(&mut mv, channel);
    }
    mv.set_sound_follows_focus(true);
    mv
}

#[test]
fn with_the_sound_following_focus_each_focus_move_takes_it() {
    let mut mv = following();
    assert!(mv.sound_follows_focus());
    assert_eq!(mv.active().channel, 13);
    assert_eq!(heard(&mv), 10, "turning it on moves nothing by itself");
    // The arrows; one that moves no focus moves no sound.
    mv.key(Key::Right, ok);
    assert_eq!(heard(&mv), 10);
    for (key, channel) in [
        (Key::Left, 12),
        (Key::Up, 10),
        (Key::Right, 11),
        (Key::Down, 13),
    ] {
        mv.key(key, ok);
        assert_eq!(heard(&mv), channel, "{key:?}");
    }
    assert_eq!(mv.audible_surface(), 3);
    // A tap on another tile; opening the active one's menu and moving in
    // it change nothing.
    mv.tap(1, ok);
    assert_eq!(heard(&mv), 11);
    mv.tap(1, ok);
    assert!(mv.dialog().is_some());
    assert!(!items(&mv).contains(&TileAction::Listen), "it is heard");
    mv.key(Key::Down, ok);
    mv.key(Key::Back, ok);
    assert_eq!(heard(&mv), 11);
    // Listen to this still works where the focus is not.
    mv.listen(0);
    assert_eq!(heard(&mv), 10);
    // The tap to tile 3's menu takes the sound; replacing its channel
    // keeps it there.
    assert_eq!(
        choose(&mut mv, 3, TileAction::Replace),
        Outcome::PickChannel
    );
    mv.picked(20);
    assert_eq!((mv.active().channel, heard(&mv)), (20, 20));
    // Picking a channel already on a tile focuses it, so it is heard.
    mv.tap(0, ok);
    assert!(mv.remove_tile(3));
    assert_eq!(mv.add(), Outcome::PickChannel);
    mv.picked(11);
    assert_eq!((mv.active().channel, heard(&mv)), (11, 11));
    // Adding a tile focuses it, so it is heard.
    add(&mut mv, 14);
    assert_eq!((mv.active().channel, heard(&mv)), (14, 14));
    // Turned off again: focus moves leave the sound where it is.
    mv.set_sound_follows_focus(false);
    mv.tap(0, ok);
    mv.key(Key::Right, ok);
    assert_eq!(heard(&mv), 14);
}

#[test]
fn with_the_sound_following_focus_the_hand_back_rules_hold() {
    let mut mv = following();
    mv.tap(1, ok);
    assert_eq!(heard(&mv), 11);
    // Enlarging and reducing, also another tile (which focuses it), keep
    // the heard tile.
    mv.toggle_enlarge(2);
    assert_eq!(mv.active().channel, 12);
    assert_eq!(heard(&mv), 11, "enlarging keeps the sound");
    mv.toggle_enlarge(2);
    assert_eq!(heard(&mv), 11, "reducing too");
    mv.key(Key::LongOk, ok);
    mv.key(Key::LongOk, ok);
    assert_eq!(heard(&mv), 11);
    // A detour of tile 3 (trading surfaces with tile 0): heard full
    // screen meanwhile; back on the grid the heard tile is heard again,
    // though the detour focused tile 3.
    assert_eq!(mv.full_screen(3), Outcome::FullScreen);
    assert_eq!(mv.audible_surface(), 0);
    assert_eq!(mv.key(Key::Left, ok), Outcome::Stay);
    mv.resume();
    assert_eq!(mv.active().channel, 13);
    assert_eq!(heard(&mv), 11);
    assert_eq!(mv.audible_surface(), 1);
    // Removing the heard tile hands the sound back to the one heard
    // before it (10), wherever the focus lands.
    mv.tap(0, ok);
    mv.tap(1, ok);
    assert_eq!(heard(&mv), 11);
    assert!(mv.remove_tile(1));
    assert_eq!(heard(&mv), 10);
    // Removing another tile keeps it.
    assert!(mv.remove_tile(1));
    assert_eq!(heard(&mv), 10);
}

/// Tile `active`'s menu item Change channel, by OK on the focused tile.
fn change_channel(mv: &mut Multiview<u32>) -> Outcome<u32> {
    mv.key(Key::Ok, ok);
    let at = items(mv)
        .iter()
        .position(|a| *a == TileAction::Replace)
        .unwrap();
    mv.choose(at)
}

/// With the sound following the focus, only a focus that moves takes it:
/// once focus and sound are apart (Listen to this on another tile),
/// changing the focused tile's channel, picking the channel it already
/// plays and an arrow at the grid's edge leave the sound where the viewer
/// put it.
#[test]
fn with_the_sound_following_focus_a_focus_that_stays_moves_no_sound() {
    let mut mv = following();
    assert_eq!(mv.active().channel, 13);
    mv.listen(1);
    assert_eq!(heard(&mv), 11);
    assert_eq!(change_channel(&mut mv), Outcome::PickChannel);
    mv.picked(20);
    assert_eq!(mv.active().channel, 20);
    assert_eq!(heard(&mv), 11, "replacing the focused tile's channel");
    change_channel(&mut mv);
    mv.picked(20);
    assert_eq!(heard(&mv), 11, "re-picking its own channel");
    // Adding the channel the focused tile plays only focuses it again.
    assert!(mv.remove_tile(2));
    assert_eq!(mv.active().channel, 20);
    assert_eq!(mv.add(), Outcome::PickChannel);
    mv.picked(20);
    assert_eq!(heard(&mv), 11, "picking the focused tile's channel");
    // Arrows at the edge (20 is alone on the bottom row) move nothing.
    mv.key(Key::Right, ok);
    mv.key(Key::Down, ok);
    assert_eq!(mv.active().channel, 20);
    assert_eq!(heard(&mv), 11, "arrows that move no focus");
    // A focus that moves takes it.
    mv.key(Key::Up, ok);
    assert_ne!(mv.active().channel, 20);
    assert_eq!(heard(&mv), mv.active().channel);
}
