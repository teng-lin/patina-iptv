use super::*;

#[test]
fn ok_shows_the_card_with_its_action_cards_and_no_card_focused() {
    let mut p = Panels {
        quick: Some(3),
        ..Panels::default()
    };
    p.show_card(0);
    assert_eq!(p.shown, Shown::Card);
    assert_eq!(
        p.quick_focused(),
        None,
        "OK again hides; Left/Right reach the row"
    );
    assert!(p.captures_input() && p.card_shown());
    assert_eq!(p.hide_at_ms(), Some(7_000));
}

#[test]
fn left_right_reach_the_action_cards_and_ok_presses_one() {
    let mut p = Panels::default();
    p.show_card(0);
    p.move_focus(1, 1);
    assert_eq!(
        p.quick_focused(),
        Some(QuickAction::Guide),
        "Right: the first card"
    );
    p.move_focus(1, 2);
    p.move_focus(1, 3);
    assert_eq!(p.quick_focused(), Some(QuickAction::Channels));
    assert_eq!(p.select(4), Pick::Quick(QuickAction::Channels));
    assert_eq!(p.shown, Shown::Card, "the panels stay while the card acts");
    p.move_focus(99, 5);
    assert_eq!(p.quick_focused(), Some(QuickAction::Controls), "clamped");
    p.move_focus(-99, 6);
    assert_eq!(p.quick_focused(), Some(QuickAction::Guide));
    // Left on the bare card also lands on the first card.
    p.hide();
    p.show_card(7);
    p.move_focus(-1, 8);
    assert_eq!(p.quick_focused(), Some(QuickAction::Guide));
}

#[test]
fn ok_with_no_card_focused_hides_the_card() {
    let mut p = Panels::default();
    p.show_card(0);
    assert_eq!(p.select(1), Pick::Hidden);
    assert_eq!((p.shown, p.hide_at_ms()), (Shown::Hidden, None));
    assert_eq!(p.select(2), Pick::Nothing, "nothing shown takes OK");
}

#[test]
fn long_ok_and_menu_show_the_control_strip() {
    let mut p = Panels::default();
    p.show_controls(0);
    assert_eq!((p.shown, p.focused()), (Shown::Controls, Control::Search));
    assert!(p.captures_input() && !p.card_shown());
    p.move_focus(3, 1);
    assert_eq!(p.select(2), Pick::Control(Control::Multiview));
    p.show_card(3);
    assert_eq!(p.shown, Shown::Controls, "open buttons stay as they are");
    // Menu toggles it; from the card it replaces the card.
    p.toggle_controls(4);
    assert_eq!(p.shown, Shown::Hidden);
    p.show_card(5);
    p.toggle_controls(6);
    assert_eq!((p.shown, p.focused()), (Shown::Controls, Control::Search));
}

#[test]
fn one_time_line_at_most_and_a_replay_shows_its_seek_bar() {
    let mut p = Panels::default();
    assert_eq!(p.line(false), None);
    p.show_banner(0);
    assert_eq!(p.line(false), Some(Line::Progress));
    p.show_info(1);
    assert_eq!(p.line(false), Some(Line::Progress));
    assert_eq!(p.line(true), Some(Line::Seek), "the seek bar replaces it");
    p.hide();
    p.show_card(2);
    assert_eq!(p.line(false), Some(Line::Progress));
    assert_eq!(p.line(true), Some(Line::Seek));
    p.show_controls(3);
    assert_eq!(p.line(false), None, "the strip replaces the card");
    assert_eq!(p.line(true), None);
}

#[test]
fn the_zap_banner_takes_no_input_and_hides_after_three_seconds() {
    let mut p = Panels::default();
    p.show_banner(1_000);
    assert_eq!(p.shown, Shown::Banner);
    assert!(!p.captures_input() && !p.card_shown());
    assert_eq!(p.hide_at_ms(), Some(4_000));
    // Another zap restarts it.
    p.show_banner(2_000);
    assert!(!p.tick(4_999));
    assert!(p.tick(5_000));
    assert_eq!(p.shown, Shown::Hidden);
    // Over open buttons it keeps them (they show the new channel).
    p.show_card(6_000);
    p.move_focus(1, 6_100);
    p.show_banner(6_200);
    assert_eq!(
        (p.shown, p.quick_focused()),
        (Shown::Card, Some(QuickAction::Guide))
    );
    p.show_controls(6_300);
    p.show_banner(6_400);
    assert_eq!(p.shown, Shown::Controls);
    // The card alone gives way to it; OK on the banner brings the card.
    let mut p = Panels::default();
    p.show_info(0);
    p.show_banner(10);
    assert_eq!(p.shown, Shown::Banner);
    p.show_card(20);
    assert_eq!(p.shown, Shown::Card);
}

#[test]
fn info_takes_no_input_and_keeps_open_buttons() {
    let mut p = Panels::default();
    p.show_info(100);
    assert_eq!(p.shown, Shown::Info);
    assert!(!p.captures_input() && p.card_shown());
    assert_eq!(p.hide_at_ms(), Some(4_100));
    p.move_focus(1, 150);
    assert_eq!(p.quick, None, "the card alone has no buttons");
    p.show_controls(200);
    p.move_focus(2, 300);
    p.show_info(400);
    assert_eq!((p.shown, p.focus), (Shown::Controls, 2));
    // The banner gives way to the card alone (a seek after a zap).
    let mut p = Panels::default();
    p.show_banner(0);
    p.show_info(10);
    assert_eq!(p.shown, Shown::Info);
}

#[test]
fn panels_hide_after_the_timeout_and_input_postpones_it() {
    let mut p = Panels::default();
    p.show_controls(0);
    assert!(!p.tick(6_999));
    p.move_focus(1, 5_000);
    assert!(!p.tick(7_000), "input restarted the timer");
    assert!(p.tick(12_000));
    assert_eq!((p.shown, p.hide_at_ms()), (Shown::Hidden, None));
    assert!(!p.tick(20_000));
}

#[test]
fn the_mouse_shows_the_card_and_it_hides_soon_after_the_mouse_rests() {
    let mut p = Panels::default();
    p.pointer_moved(0);
    assert_eq!((p.shown, p.hide_at_ms()), (Shown::Card, Some(3_000)));
    p.pointer_moved(2_000);
    assert!(!p.tick(4_999));
    assert!(p.tick(5_000));
    // Keys after the mouse: the usual timeout again.
    p.pointer_moved(10_000);
    p.move_focus(1, 11_000);
    assert_eq!(p.hide_at_ms(), Some(18_000));
    // The mouse over open controls keeps them.
    p.show_controls(12_000);
    p.pointer_moved(13_000);
    assert_eq!(p.shown, Shown::Controls);
}

#[test]
fn reopening_starts_at_the_first_control_and_no_card() {
    let mut p = Panels::default();
    p.show_controls(0);
    p.move_focus(-1, 1);
    assert_eq!(p.focus, 0);
    p.move_focus(99, 2);
    assert_eq!(p.focused(), Control::Record);
    p.show_controls(3);
    assert_eq!(p.focused(), Control::Record, "still open: focus stays");
    p.hide();
    p.show_controls(4);
    assert_eq!(p.focused(), Control::Search);
    p.hide();
    p.show_card(5);
    p.move_focus(2, 6);
    p.hide();
    p.show_card(7);
    assert_eq!(p.quick_focused(), None, "a new session starts unfocused");
}

#[test]
fn controls_the_host_never_offers_are_left_out_of_the_strip_and_its_focus() {
    let never = |c| {
        !matches!(
            c,
            Control::Recordings | Control::Multiview | Control::Record
        )
    };
    let mut p = Panels::default();
    p.offer(never);
    assert_eq!(
        p.controls().collect::<Vec<_>>(),
        [
            Control::Search,
            Control::ChannelList,
            Control::PictureInPicture,
            Control::VideoTracks,
            Control::AudioTracks,
            Control::AudioOffset,
            Control::Captions,
            Control::Scaling,
            Control::SleepTimer,
            Control::Favorite,
        ]
    );
    p.show_controls(0);
    p.move_focus(2, 1);
    assert_eq!(p.select(2), Pick::Control(Control::PictureInPicture));
    p.move_focus(-1, 3);
    assert_eq!(p.focused(), Control::ChannelList, "steps over the omitted");
    p.move_focus(99, 4);
    assert_eq!(p.focused(), Control::Favorite, "the last shown, not Record");
    p.focus_control(Control::Record);
    assert_eq!(
        p.focused(),
        Control::Favorite,
        "an omitted control takes no focus"
    );

    // Losing the focused control passes the focus to its neighbor, and a
    // strip without its first control opens on the first one shown.
    p.focus_control(Control::Captions);
    p.offer(|c| c != Control::Captions && c != Control::Search);
    assert_eq!(p.focused(), Control::AudioOffset);
    p.hide();
    p.show_controls(5);
    assert_eq!(p.focused(), Control::ChannelList);
}

#[test]
fn a_held_key_keeps_the_card_until_its_press_is_decided() {
    let mut p = Panels::default();
    p.timeouts.controls_ms = 1_000;
    p.show_card(0);
    // A key held at 500 decides tap or hold at 2_500 (a 2 s threshold).
    p.touch(500);
    p.keep_until(2_500);
    assert!(!p.tick(1_600), "the card outlives its own timeout");
    assert!(p.tick(2_500));
    // A later deadline of its own is kept; hidden panels stay hidden.
    p.show_card(3_000);
    p.keep_until(3_100);
    assert_eq!(p.hide_at_ms(), Some(4_000));
    p.hide();
    p.keep_until(9_000);
    assert_eq!(p.hide_at_ms(), None);
}

#[test]
fn input_during_a_hold_does_not_pull_its_deadline_in() {
    let mut p = Panels::default();
    p.timeouts.controls_ms = 1_000;
    p.show_card(0);
    p.keep_until(2_050);
    // A zap commits mid-hold: the card stays (it owns input) and its own
    // timer restarts, but the held key's deadline stands.
    p.show_banner(250);
    assert_eq!(p.shown, Shown::Card);
    assert_eq!(p.hide_at_ms(), Some(2_050));
    assert!(!p.tick(1_500));
    // Decided: the card's own timer alone.
    p.release_hold();
    assert!(!p.holding());
    assert_eq!(p.hide_at_ms(), Some(1_250));
    // A hold over the card does not keep the strip that replaced it.
    p.show_card(3_000);
    p.keep_until(9_000);
    p.show_controls(3_100);
    assert_eq!(p.hide_at_ms(), Some(4_100));
}
