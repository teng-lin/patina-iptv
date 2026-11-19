//! Reordering lists that may contain orphans (ids whose channel is gone).

use std::collections::HashSet;

/// Moves `item` so it lands at `index` among the items in `present`.
/// Orphans keep their place relative to their neighbours, so a channel that
/// comes back later reappears where it was. An `index` past the end moves
/// the item last. Returns `false` if `item` is not in the list.
pub(crate) fn move_to(
    list: &mut Vec<String>,
    present: &HashSet<String>,
    item: &str,
    index: usize,
) -> bool {
    iptv_core::reorder::move_to_present(list, item, index, |key| present.contains(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn moves_among_present_items_and_keeps_orphans_in_place() {
        let present: HashSet<String> = list(&["a", "b", "c", "d"]).into_iter().collect();
        let mut l = list(&["a", "x", "b", "c", "d"]);
        assert!(move_to(&mut l, &present, "d", 0));
        assert_eq!(l, list(&["d", "a", "x", "b", "c"]));
        assert!(move_to(&mut l, &present, "d", 2));
        assert_eq!(l, list(&["a", "x", "b", "d", "c"]));
        assert!(move_to(&mut l, &present, "a", 99));
        assert_eq!(l, list(&["x", "b", "d", "c", "a"]));
        assert!(!move_to(&mut l, &present, "zz", 0));
    }
}
