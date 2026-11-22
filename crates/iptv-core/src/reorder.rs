//! Moves in a persisted order that can include absent items.

/// Moves an item to an index among present entries, retaining absent keys.
pub fn move_to_present<T: PartialEq<Q>, Q: ?Sized>(
    items: &mut Vec<T>,
    key: &Q,
    index: usize,
    present: impl Fn(&T) -> bool,
) -> bool {
    let Some(from) = items.iter().position(|item| item == key) else {
        return false;
    };
    let moved = items.remove(from);
    let anchor = items
        .iter()
        .enumerate()
        .filter(|(_, item)| present(item))
        .nth(index)
        .map_or(items.len(), |(at, _)| at);
    items.insert(anchor, moved);
    true
}
