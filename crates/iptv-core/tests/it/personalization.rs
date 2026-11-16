use iptv_core::{
    favorites::Favorites,
    guide::{Focus, Move, Pane, SyntheticSchedule},
    timeline::Timeline,
};
#[test]
fn every_supported_time_span_keeps_keyboard_focus_in_view_and_pan_at_the_end() {
    for span in (60..=360).step_by(30) {
        let mut f = Focus::at(0);
        f.set_visible(span, &SyntheticSchedule);
        for _ in 0..40 {
            f.apply(Move::Right, &SyntheticSchedule);
            assert_eq!(f.pane, Pane::Guide);
            let p = f.program(&SyntheticSchedule);
            assert!(p.start.max(f.now) < f.window + span);
        }
        f.step_time(99999, &SyntheticSchedule);
        assert!(f.cursor < f.window + span);
        f.step_time(-99999, &SyntheticSchedule);
        assert!(f.cursor >= f.window);
        f.pan_to(99999, &SyntheticSchedule);
        assert!(f.cursor < f.window + span);
        let mut t = Timeline::new(0, (0, 1000));
        t.set_span(span);
        t.set_bounds((0, 1000));
        t.follow(9999);
        assert_eq!(t.window(), 1000 - span);
        t.set_bounds((0, 20));
        assert_eq!(t.window(), 0);
    }
}
#[test]
fn changing_time_span_and_reordering_favorites_preserve_identity() {
    let mut f = Focus::at(100);
    f.cursor = 210;
    f.set_visible(60, &SyntheticSchedule);
    assert_eq!(f.visible, 60);
    assert_eq!(f.cursor, 210);
    f.set_visible(0, &SyntheticSchedule);
    assert_eq!(f.visible, 60);
    f.set_visible(999, &SyntheticSchedule);
    assert_eq!(f.visible, 360);
    let mut fav = Favorites::from_keys(["a", "b", "c"].map(str::to_owned));
    assert!(fav.move_key("c", 0));
    assert_eq!(fav.keys(), ["c", "a", "b"]);
    assert!(fav.move_key("c", 999));
    assert_eq!(fav.keys(), ["a", "b", "c"]);
    assert!(!fav.move_key("absent", 0));
}
