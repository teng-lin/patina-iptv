use super::*;

const DESKTOP: Limits = Limits {
    decoders: 4,
    connections: None,
};
const BOX: Limits = Limits {
    decoders: 2,
    connections: None,
};
/// A 2560×1600 tablet.
const TABLET: f32 = 1.6;

fn ok(_: usize) -> bool {
    false
}

/// Adds `channel` as the menu's Add (or the toolbar's) does.
fn add(mv: &mut Multiview<u32>, channel: u32) {
    assert_eq!(mv.add(), Outcome::PickChannel);
    assert_eq!(mv.pick(), Some(Pick::Add));
    assert_eq!(mv.picked(channel), Outcome::Stay);
}

fn surfaces(mv: &Multiview<u32>) -> Vec<(usize, u32)> {
    mv.tiles().iter().map(|t| (t.surface, t.channel)).collect()
}

fn items(mv: &Multiview<u32>) -> Vec<TileAction> {
    match mv.dialog() {
        Some(Dialog::Options { items, .. }) => items.clone(),
        _ => vec![],
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// `r` in px of a `w`×`h` area, rounded.
fn px(r: Rect, w: f32, h: f32) -> (i32, i32, i32, i32) {
    let p = |v: f32| v.round() as i32;
    (p(r.x * w), p(r.y * h), p(r.w * w), p(r.h * h))
}

#[test]
fn the_grid_holds_just_the_tiles() {
    let grid = |n: usize, i: usize| Layout::Grid { tiles: n }.rect(i, TABLET);
    let (w, h) = (2560.0, 1600.0);
    // One: centered at half the width, 16:9.
    assert_eq!(px(grid(1, 0), w, h), (640, 440, 1280, 720));
    // Two: side by side over the width, centered vertically.
    assert_eq!(px(grid(2, 0), w, h), (0, 440, 1280, 720));
    assert_eq!(px(grid(2, 1), w, h), (1280, 440, 1280, 720));
    // Three: two above, one centered below, the size of four's.
    assert_eq!(px(grid(3, 0), w, h), (0, 80, 1280, 720));
    assert_eq!(px(grid(3, 2), w, h), (640, 800, 1280, 720));
    assert_eq!(px(grid(4, 3), w, h), (1280, 800, 1280, 720));
    // On a 16:9 window four fill it exactly.
    let r = Layout::Grid { tiles: 4 }.rect(3, VIDEO);
    assert!(near(r.x, 0.5) && near(r.y, 0.5) && near(r.w, 0.5) && near(r.h, 0.5));
    // A tall window: the tiles stay 16:9, narrower than a column.
    let r = Layout::Grid { tiles: 4 }.rect(0, 1.0);
    assert!(near(r.w, 0.5) && near(r.w / r.h, VIDEO));
    // A wide one: the rows fill the height, the grid centered.
    let r = Layout::Grid { tiles: 4 }.rect(0, 3.0);
    assert!(near(r.h, 0.5) && near(r.w * 3.0 / r.h, VIDEO) && r.x > 0.0);
    // A bogus area falls back to 16:9.
    assert_eq!(
        Layout::Grid { tiles: 1 }.rect(0, 0.0),
        Layout::Grid { tiles: 1 }.rect(0, VIDEO)
    );
}

#[test]
fn enlarged_puts_one_big_over_a_centered_strip() {
    let (w, h) = (2560.0, 1600.0);
    let two = |i| px(Layout::Enlarged { tiles: 2, big: 1 }.rect(i, TABLET), w, h);
    let (bx, by, bw, bh) = two(1);
    // About 86 % of the width, 16:9, centered, at the top.
    assert!((2180..=2220).contains(&bw), "{bw}");
    assert!((bx - (2560 - bw) / 2).abs() <= 1);
    assert!(by <= 2);
    assert!((bh * 16 - bw * 9).abs() <= 16);
    let (sx, sy, sw, sh) = two(0);
    assert!((sy - (by + bh)).abs() <= 1, "the strip under it");
    assert!((sw - 630).abs() <= 10, "{sw}");
    assert!((sx - (2560 - sw) / 2).abs() <= 1, "one small tile centered");
    assert!(sy + sh <= 1600);
    // Four: three small ones side by side, centered, in tile order.
    let four = |i| Layout::Enlarged { tiles: 4, big: 2 }.rect(i, TABLET);
    let small: Vec<Rect> = [0, 1, 3].iter().map(|&i| four(i)).collect();
    assert!(near(small[0].y, small[2].y));
    assert!(near(small[1].x, small[0].x + small[0].w));
    assert!(near(small[0].x, 1.0 - (small[2].x + small[2].w)));
    // A big index out of range is the grid.
    assert_eq!(
        Layout::Enlarged { tiles: 1, big: 0 }.rect(0, TABLET),
        Layout::Grid { tiles: 1 }.rect(0, TABLET)
    );
}

#[test]
fn tiles_come_and_go_without_an_add_cell() {
    let mut mv = Multiview::open(10, DESKTOP);
    assert_eq!(mv.layout(), Layout::Grid { tiles: 1 });
    assert!(mv.can_add());
    add(&mut mv, 11);
    add(&mut mv, 12);
    assert_eq!(mv.layout(), Layout::Grid { tiles: 3 });
    add(&mut mv, 13);
    assert!(!mv.can_add());
    assert_eq!(mv.add(), Outcome::Stay, "full: nothing to add");
    assert!(mv.pick().is_none());
    assert!(mv.remove_tile(1));
    assert_eq!(mv.layout(), Layout::Grid { tiles: 3 });
    assert!(mv.can_add());
}

#[test]
fn the_device_caps_the_tiles() {
    let mut mv = Multiview::open(10, BOX);
    add(&mut mv, 11);
    assert!(!mv.can_add());
    // A stale pick adds nothing either.
    mv.pick = Some(Pick::Add);
    mv.picked(12);
    assert_eq!(mv.tiles().len(), 2);
    mv.key(Key::Ok, ok);
    assert!(!items(&mv).contains(&TileAction::Add), "no Add at the cap");
    assert!(!items(&mv).contains(&TileAction::SearchAdd));
    // A cap of one: the tile alone, nothing to add.
    let mut single = Multiview::open(
        1u32,
        Limits {
            decoders: 0,
            connections: None,
        },
    );
    assert_eq!(single.limits().decoders, 1);
    assert_eq!(single.add(), Outcome::Stay);
}

#[test]
fn the_arrows_move_the_focus_between_tiles() {
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    add(&mut mv, 12);
    // 10 11 / 12 (centered). The added tile became active.
    assert_eq!(mv.active().channel, 12);
    mv.key(Key::Right, ok);
    assert_eq!(mv.active().channel, 12, "nothing right of the third");
    mv.key(Key::Up, ok);
    assert_eq!(mv.active_index(), 0, "up from the center: the left one");
    mv.key(Key::Up, ok);
    assert_eq!(mv.active_index(), 0, "no wrap");
    mv.key(Key::Right, ok);
    assert_eq!(mv.active().channel, 11);
    mv.key(Key::Down, ok);
    assert_eq!(mv.active().channel, 12);
    // Enlarged (12 big): Down from the big one into the strip, and Up
    // back; the enlarged tile stays where it is.
    mv.key(Key::LongOk, ok);
    assert_eq!(mv.enlarged(), Some(2));
    mv.key(Key::Down, ok);
    assert!(mv.active_index() < 2);
    mv.key(Key::Right, ok);
    mv.key(Key::Left, ok);
    mv.key(Key::Up, ok);
    assert_eq!(mv.active_index(), 2);
    assert_eq!(mv.enlarged(), Some(2), "moving keeps the enlarged tile");
}

#[test]
fn taps_make_active_then_open_the_menu() {
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    assert_eq!(mv.tap(0, ok), Outcome::Stay);
    assert_eq!(mv.active().channel, 10);
    assert!(mv.dialog().is_none(), "the first tap only makes it active");
    mv.tap(0, ok);
    assert!(matches!(mv.dialog(), Some(Dialog::Options { tile: 0, .. })));
    // A tap elsewhere closes the dialog first.
    assert_eq!(mv.tap(1, ok), Outcome::Stay);
    assert!(mv.dialog().is_none());
    assert_eq!(mv.active().channel, 10);
    assert_eq!(mv.tap(7, ok), Outcome::Stay, "beside the tiles");
}

#[test]
fn the_menu_offers_what_applies() {
    use TileAction::*;
    let mut mv = Multiview::open(10, DESKTOP);
    mv.key(Key::Ok, ok);
    assert_eq!(
        items(&mv),
        vec![Add, SearchAdd, Replace, Pause, FullScreen],
        "a single tile"
    );
    mv.key(Key::Back, ok);
    assert!(mv.dialog().is_none(), "Back closes the dialog only");
    add(&mut mv, 11);
    mv.key(Key::Menu, ok);
    assert_eq!(
        items(&mv),
        vec![
            Add, SearchAdd, Replace, Pause, Listen, Enlarge, FullScreen, Remove
        ],
        "two tiles: the focused one is not heard"
    );
    mv.key(Key::Back, ok);
    // A failed tile offers Try again first.
    mv.key(Key::Menu, |s| s == 1);
    assert_eq!(
        items(&mv),
        vec![
            Retry, Add, SearchAdd, Replace, Pause, Listen, Enlarge, FullScreen, Remove
        ]
    );
    assert_eq!(mv.key(Key::Ok, ok), Outcome::Retry(1));
    // The enlarged tile offers Reduce; full, no Add or Search and add; a
    // paused one Play.
    mv.toggle_enlarge(1);
    add(&mut mv, 12);
    add(&mut mv, 13);
    mv.set_paused(1, true);
    mv.tap(1, ok);
    mv.tap(1, ok);
    assert_eq!(
        items(&mv),
        vec![Replace, Play, Listen, Reduce, FullScreen, Remove]
    );
    // Add from the menu picks a channel.
    let mut two = Multiview::open(1u32, DESKTOP);
    two.key(Key::Ok, ok);
    assert_eq!(two.choose(0), Outcome::PickChannel);
    assert_eq!(two.pick(), Some(Pick::Add));
}

#[test]
fn enlarge_moves_and_reduce_restores() {
    let mut mv = Multiview::open(10, DESKTOP);
    mv.toggle_enlarge(0);
    assert_eq!(mv.enlarged(), None, "nothing to enlarge with one tile");
    add(&mut mv, 11);
    add(&mut mv, 12);
    // From the menu.
    mv.tap(1, ok);
    mv.tap(1, ok);
    let enlarge = items(&mv).iter().position(|a| *a == TileAction::Enlarge);
    mv.choose(enlarge.unwrap());
    assert_eq!(mv.enlarged(), Some(1));
    assert_eq!(mv.layout(), Layout::Enlarged { tiles: 3, big: 1 });
    // Another one takes the big place (a long press).
    mv.toggle_enlarge(2);
    assert_eq!(mv.enlarged(), Some(2));
    assert_eq!(mv.active_index(), 2);
    // Long OK on it again reduces.
    mv.key(Key::LongOk, ok);
    assert_eq!(mv.layout(), Layout::Grid { tiles: 3 });
    // Surfaces never change for it.
    assert_eq!(surfaces(&mv), vec![(0, 10), (1, 11), (2, 12)]);
}

#[test]
fn removing_keeps_the_rest_consistent() {
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    add(&mut mv, 12);
    add(&mut mv, 13);
    mv.toggle_enlarge(2);
    // Removing one before the enlarged tile shifts it.
    assert!(mv.remove_tile(0));
    assert_eq!(mv.enlarged(), Some(1));
    assert_eq!(mv.active().channel, 12, "the focus stays with its tile");
    assert_eq!(surfaces(&mv), vec![(1, 11), (2, 12), (3, 13)]);
    // Removing the enlarged tile: the grid again, the focus on a neighbor.
    assert!(mv.remove_tile(1));
    assert_eq!(mv.enlarged(), None);
    assert_eq!(mv.layout(), Layout::Grid { tiles: 2 });
    assert_eq!(mv.active().channel, 13);
    // Enlarged with two: one left is the grid.
    mv.toggle_enlarge(0);
    assert!(mv.remove_tile(1));
    assert_eq!(mv.enlarged(), None);
    assert_eq!(mv.active().channel, 11);
    assert!(!mv.remove_tile(0), "never the last tile");
    assert_eq!(mv.tiles().len(), 1);
    // From the menu, at once, with no confirmation.
    add(&mut mv, 20);
    mv.key(Key::Ok, ok);
    let remove = items(&mv).len() - 1;
    assert_eq!(mv.choose(remove), Outcome::Stay);
    assert_eq!(surfaces(&mv), vec![(1, 11)]);
    assert!(mv.dialog().is_none());
}

#[test]
fn replace_and_picks_keep_the_other_surfaces() {
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    add(&mut mv, 12);
    mv.remove_tile(0);
    // Surface 0 is free again for the next one.
    add(&mut mv, 13);
    assert_eq!(surfaces(&mv), vec![(1, 11), (2, 12), (0, 13)]);
    assert_eq!(mv.assignments(), [Some(13), Some(11), Some(12), None]);
    // Replace the second tile's channel: same surface.
    mv.tap(1, ok);
    mv.tap(1, ok);
    let replace = items(&mv).iter().position(|a| *a == TileAction::Replace);
    assert_eq!(mv.choose(replace.unwrap()), Outcome::PickChannel);
    assert_eq!(mv.pick(), Some(Pick::Replace(1)));
    mv.picked(20);
    assert_eq!(surfaces(&mv), vec![(1, 11), (2, 20), (0, 13)]);
    assert_eq!(mv.active().channel, 20);
    // Picking a channel already on a tile only makes it active.
    mv.pick = Some(Pick::Replace(1));
    mv.picked(11);
    assert_eq!(surfaces(&mv), vec![(1, 11), (2, 20), (0, 13)]);
    assert_eq!(mv.active().channel, 11);
    // A canceled pick changes nothing.
    assert_eq!(mv.add(), Outcome::PickChannel);
    mv.cancel_pick();
    assert_eq!(mv.picked(30), Outcome::Stay);
    assert_eq!(mv.tiles().len(), 3);
    // Removing the tile being replaced drops the pick.
    mv.pick = Some(Pick::Replace(2));
    mv.remove_tile(2);
    assert_eq!(mv.pick(), None);
}

#[test]
fn back_and_the_toolbar_leave_with_the_active_tile() {
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    assert_eq!(
        mv.key(Key::Back, ok),
        Outcome::Leave {
            channel: 11,
            surface: 1
        }
    );
    mv.tap(0, ok);
    assert_eq!(
        mv.exit(),
        Outcome::Leave {
            channel: 10,
            surface: 0
        }
    );
}

#[test]
fn full_screen_is_a_detour_back_to_the_same_grid() {
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    add(&mut mv, 12);
    mv.toggle_enlarge(1);
    let grid = |mv: &Multiview<u32>| {
        let mut ch: Vec<u32> = mv.tiles().iter().map(|t| t.channel).collect();
        ch.push(mv.enlarged().map_or(99, |e| e as u32));
        ch
    };
    let before = grid(&mv);
    // Watch tile 2 (surface 2) full screen: it moves to surface 0, the
    // tile there takes surface 2; nothing else changes.
    mv.tap(2, ok);
    mv.tap(2, ok);
    let full = items(&mv).iter().position(|a| *a == TileAction::FullScreen);
    assert_eq!(mv.choose(full.unwrap()), Outcome::FullScreen);
    assert_eq!(mv.detour(), Some(2));
    assert_eq!(mv.active().channel, 12);
    assert_eq!(surfaces(&mv), vec![(2, 10), (1, 11), (0, 12)]);
    assert_eq!(grid(&mv), before);
    // The grid's keys and taps do nothing meanwhile.
    assert_eq!(mv.key(Key::Back, ok), Outcome::Stay);
    assert_eq!(mv.tap(0, ok), Outcome::Stay);
    assert_eq!(mv.detour(), Some(2));
    // Back to the grid as it was; no surface changes on the way back.
    mv.resume();
    assert_eq!(mv.detour(), None);
    assert_eq!(grid(&mv), before);
    assert_eq!(mv.active_index(), 2);
    assert_eq!(surfaces(&mv), vec![(2, 10), (1, 11), (0, 12)]);
    // A tile already on surface 0 keeps it; Back from the grid still
    // leaves.
    mv.key(Key::Ok, ok);
    let full = items(&mv).iter().position(|a| *a == TileAction::FullScreen);
    mv.choose(full.unwrap());
    assert_eq!(surfaces(&mv), vec![(2, 10), (1, 11), (0, 12)]);
    mv.resume();
    assert_eq!(
        mv.key(Key::Back, ok),
        Outcome::Leave {
            channel: 12,
            surface: 0
        }
    );
    // A single tile: full screen too; its tile goes if its data does.
    let mut one = Multiview::open(5u32, DESKTOP);
    one.key(Key::Ok, ok);
    let full = items(&one)
        .iter()
        .position(|a| *a == TileAction::FullScreen);
    assert_eq!(one.choose(full.unwrap()), Outcome::FullScreen);
    assert!(!one.retain(|_, _| false));
    assert_eq!(one.detour(), None);
}

#[test]
fn the_hint_goes_with_a_key() {
    let mut mv = Multiview::open(10, DESKTOP);
    assert!(mv.hint());
    mv.dismiss_hint();
    assert!(!mv.hint());
    let mut mv = Multiview::open(10, DESKTOP);
    mv.key(Key::Left, ok);
    assert!(!mv.hint());
}

#[test]
fn the_connection_limit_warns_before_adding() {
    let limits = Limits {
        decoders: 4,
        connections: Some(2),
    };
    let mut mv = Multiview::open(10, limits);
    assert_eq!(mv.over_connections(), None);
    add(&mut mv, 11);
    assert_eq!(mv.over_connections(), Some(2));
    // The third stream: a warning, Cancel focused.
    assert_eq!(mv.add(), Outcome::Stay);
    assert_eq!(
        mv.dialog(),
        Some(&Dialog::Warning {
            allowed: 2,
            focus: 1,
            search: false,
        })
    );
    assert_eq!(mv.key(Key::Ok, ok), Outcome::Stay, "Cancel");
    assert_eq!(mv.tiles().len(), 2);
    assert!(mv.pick().is_none());
    // Again from the menu's Add, and insist.
    mv.key(Key::Ok, ok);
    assert_eq!(mv.choose(0), Outcome::Stay);
    assert!(matches!(mv.dialog(), Some(Dialog::Warning { .. })));
    mv.key(Key::Left, ok);
    assert_eq!(mv.key(Key::Ok, ok), Outcome::PickChannel);
    mv.picked(12);
    assert_eq!(mv.tiles().len(), 3);
    // Insisting once is enough for this multiview.
    assert_eq!(mv.add(), Outcome::PickChannel);
    // Replacing opens no new stream: no warning.
    mv.cancel_pick();
    let mut at_limit = Multiview::open(
        1u32,
        Limits {
            decoders: 4,
            connections: Some(1),
        },
    );
    at_limit.key(Key::Ok, ok);
    let replace = items(&at_limit)
        .iter()
        .position(|a| *a == TileAction::Replace);
    assert_eq!(at_limit.choose(replace.unwrap()), Outcome::PickChannel);
    assert_eq!(at_limit.pick(), Some(Pick::Replace(0)));
}

#[test]
fn data_changes_keep_or_drop_tiles() {
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    add(&mut mv, 12);
    mv.toggle_enlarge(2);
    assert!(mv.retain(|_, ch| {
        *ch += 100;
        *ch != 110
    }));
    assert_eq!(surfaces(&mv), vec![(1, 111), (2, 112)]);
    assert_eq!(mv.enlarged(), Some(1), "the enlarged tile follows");
    assert!(!mv.retain(|_, _| false));
    assert!(mv.tiles().is_empty());
}

#[test]
fn search_and_add_picks_a_new_tile_from_search() {
    let mut mv = Multiview::open(10, DESKTOP);
    mv.key(Key::Ok, ok);
    let search = items(&mv).iter().position(|a| *a == TileAction::SearchAdd);
    assert_eq!(mv.choose(search.unwrap()), Outcome::SearchChannel);
    assert_eq!(mv.pick(), Some(Pick::Add), "it picks as the list does");
    // The search screen's choice adds a tile, active.
    assert_eq!(mv.picked(11), Outcome::Stay);
    assert_eq!(surfaces(&mv), vec![(0, 10), (1, 11)]);
    assert_eq!(mv.active().channel, 11);
    // Back from the search screen: nothing added.
    assert_eq!(mv.search_add(), Outcome::SearchChannel);
    mv.cancel_pick();
    assert_eq!(mv.picked(12), Outcome::Stay);
    assert_eq!(mv.tiles().len(), 2);
    // Past the account's limit it warns first, then opens the search.
    let mut limited = Multiview::open(
        1u32,
        Limits {
            decoders: 4,
            connections: Some(1),
        },
    );
    assert_eq!(limited.search_add(), Outcome::Stay);
    assert_eq!(
        limited.dialog(),
        Some(&Dialog::Warning {
            allowed: 1,
            focus: 1,
            search: true,
        })
    );
    assert_eq!(limited.choose(0), Outcome::SearchChannel);
    assert_eq!(limited.pick(), Some(Pick::Add));
    // At the device's cap: nothing to search for.
    let mut full = Multiview::open(1u32, BOX);
    add(&mut full, 2);
    assert_eq!(full.search_add(), Outcome::Stay);
    assert!(full.pick().is_none());
}

#[test]
fn pause_holds_one_tile_only() {
    use TileAction::*;
    let mut mv = Multiview::open(10, DESKTOP);
    add(&mut mv, 11);
    add(&mut mv, 12);
    let choose = |mv: &mut Multiview<u32>, tile: usize, action: TileAction| {
        mv.tap(tile, ok);
        mv.tap(tile, ok);
        let at = items(mv).iter().position(|a| *a == action);
        mv.choose(at.unwrap())
    };
    let paused = |mv: &Multiview<u32>| mv.tiles().iter().map(|t| t.paused).collect::<Vec<_>>();
    assert_eq!(choose(&mut mv, 1, Pause), Outcome::Stay);
    assert_eq!(paused(&mv), vec![false, true, false]);
    assert!(mv.dialog().is_none());
    // Its menu now offers Play; the others still Pause.
    mv.key(Key::Ok, ok);
    assert!(items(&mv).contains(&Play) && !items(&mv).contains(&Pause));
    mv.key(Key::Back, ok);
    mv.tap(0, ok);
    mv.tap(0, ok);
    assert!(items(&mv).contains(&Pause));
    mv.key(Key::Back, ok);
    // Pausing moves nothing: the focus stays where it was.
    assert_eq!(mv.active_index(), 0);
    assert_eq!(choose(&mut mv, 1, Play), Outcome::Stay);
    assert_eq!(paused(&mv), vec![false, false, false]);
    // A new channel on a paused tile plays; so does a tile watched full
    // screen.
    mv.set_paused(2, true);
    assert_eq!(choose(&mut mv, 2, Replace), Outcome::PickChannel);
    mv.picked(20);
    assert_eq!(paused(&mv), vec![false, false, false]);
    mv.set_paused(1, true);
    assert_eq!(choose(&mut mv, 1, FullScreen), Outcome::FullScreen);
    assert!(!mv.tiles()[1].paused);
    mv.resume();
    // Removing a paused tile keeps the others' state.
    mv.set_paused(0, true);
    assert!(mv.remove_tile(1));
    assert_eq!(paused(&mv), vec![true, false]);
    mv.set_paused(9, true);
}

#[test]
fn source_specific_warning_waits_for_confirmation_and_cancels_safely() {
    let mut mv = Multiview::open(10, DESKTOP);
    mv.add();
    mv.picked_with_warning(20, Some(1));
    assert_eq!(surfaces(&mv), vec![(0, 10)]);
    assert!(matches!(
        mv.dialog(),
        Some(Dialog::Warning {
            allowed: 1,
            focus: 1,
            ..
        })
    ));
    assert_eq!(mv.pick(), None);
    mv.choose(1); // Cancel
    assert_eq!(surfaces(&mv), vec![(0, 10)]);
    mv.add();
    mv.picked_with_warning(20, Some(1));
    mv.choose(0);
    assert_eq!(surfaces(&mv), vec![(0, 10), (1, 20)]);
    mv.add();
    mv.picked_with_warning(30, Some(1));
    mv.key(Key::Back, ok);
    assert!(mv.dialog().is_none());
    mv.add();
    mv.picked_with_warning(30, Some(1));
    // A data replacement cancels any pending old row identity.
    mv.retain(|_, _| true);
    mv.choose(0);
    assert_eq!(surfaces(&mv), vec![(0, 10), (1, 20)]);
    // Selecting a tile already present never consumes another connection.
    mv.add();
    mv.picked_with_warning(10, Some(1));
    assert!(mv.dialog().is_none());
    assert_eq!(mv.active().channel, 10);
}
