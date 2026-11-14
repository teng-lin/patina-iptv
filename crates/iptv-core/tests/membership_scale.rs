use iptv_core::membership::{Member, Membership};

#[test]
fn reload_large_membership_retains_selection_and_focus_by_identity() {
    let row = |index| Member {
        id: format!("generated-{index}"),
        name: format!("Generated channel {index}"),
    };
    let mut membership = Membership::new((0..100_000).map(row).collect());
    membership.selected = membership.rows.iter().map(|row| row.id.clone()).collect();
    membership.focus = Some("generated-75000".into());
    membership.reload((50_000..150_000).rev().map(row).collect());
    assert_eq!(membership.selected.len(), 50_000);
    assert!(membership.selected.contains("generated-50000"));
    assert!(membership.selected.contains("generated-99999"));
    assert!(!membership.selected.contains("generated-100000"));
    assert_eq!(membership.focus.as_deref(), Some("generated-75000"));
    assert_eq!(membership.visible_len(), 100_000);
}
