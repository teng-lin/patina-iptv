//! Filtered bulk selection by stable channel identity. A filter changes
//! visibility, never the selection; select-filtered adds to the selection.
use std::collections::{BTreeSet, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Membership {
    pub rows: Vec<Member>,
    pub query: String,
    pub selected: BTreeSet<String>,
    pub focus: Option<String>,
    matching: Vec<usize>,
}
impl Membership {
    pub fn new(rows: Vec<Member>) -> Self {
        let focus = rows.first().map(|r| r.id.clone());
        let matching = (0..rows.len()).collect();
        Self {
            rows,
            focus,
            matching,
            ..Self::default()
        }
    }
    pub fn visible(&self) -> Vec<&Member> {
        self.matching.iter().map(|&i| &self.rows[i]).collect()
    }
    pub fn visible_len(&self) -> usize {
        self.matching.len()
    }
    pub fn visible_at(&self, index: usize) -> Option<&Member> {
        self.matching.get(index).map(|&i| &self.rows[i])
    }
    fn match_rows(&mut self) {
        let words: Vec<_> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        self.matching = if words.is_empty() {
            (0..self.rows.len()).collect()
        } else {
            self.rows
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    let name = r.name.to_lowercase();
                    words.iter().all(|w| {
                        name.split(|c: char| !c.is_alphanumeric())
                            .any(|token| token.starts_with(w))
                    })
                })
                .map(|(i, _)| i)
                .collect()
        };
    }
    pub fn filter(&mut self, query: String) {
        self.query = query;
        self.match_rows();
        self.restore();
    }
    fn restore(&mut self) {
        if !self
            .visible()
            .iter()
            .any(|r| Some(&r.id) == self.focus.as_ref())
        {
            self.focus = self.visible().first().map(|r| r.id.clone());
        }
    }
    pub fn reload(&mut self, rows: Vec<Member>) {
        self.rows = rows;
        let present: HashSet<&str> = self.rows.iter().map(|r| r.id.as_str()).collect();
        self.selected.retain(|id| present.contains(id.as_str()));
        self.match_rows();
        self.restore();
    }
    pub fn step(&mut self, by: isize) {
        let rows = self.visible();
        if rows.is_empty() {
            self.focus = None;
            return;
        }
        let at = rows
            .iter()
            .position(|r| Some(&r.id) == self.focus.as_ref())
            .unwrap_or(0);
        self.focus = Some(
            rows[(at as isize + by).clamp(0, rows.len() as isize - 1) as usize]
                .id
                .clone(),
        );
    }
    pub fn toggle(&mut self, id: &str) {
        if self.rows.iter().any(|r| r.id == id) && !self.selected.remove(id) {
            self.selected.insert(id.into());
        }
    }
    pub fn select_filtered(&mut self) {
        self.selected.extend(
            self.visible()
                .iter()
                .map(|r| r.id.clone())
                .collect::<Vec<_>>(),
        );
    }
    /// Selected identities in source-list order, including ones hidden by a filter.
    pub fn selection(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|r| self.selected.contains(&r.id))
            .map(|r| r.id.clone())
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn rows() -> Vec<Member> {
        [
            ("a", "Local News"),
            ("b", "Local Sport"),
            ("c", "Other News"),
        ]
        .into_iter()
        .map(|(id, name)| Member {
            id: id.into(),
            name: name.into(),
        })
        .collect()
    }
    #[test]
    fn filtering_preserves_selection_and_uses_all_case_insensitive_words() {
        let mut m = Membership::new(rows());
        m.filter("LOCAL news".into());
        m.select_filtered();
        assert_eq!(m.selection(), ["a"]);
        m.filter("sport".into());
        m.select_filtered();
        assert_eq!(m.selection(), ["a", "b"]);
        assert_eq!(m.visible().len(), 1);
        m.filter("absent".into());
        m.select_filtered();
        m.step(1);
        assert_eq!(m.focus, None);
        assert_eq!(m.selected.len(), 2);
        m.toggle("unknown");
        assert_eq!(m.selected.len(), 2);
    }
    #[test]
    fn refresh_and_reorder_restore_identity_and_drop_stale_selection() {
        let mut m = Membership::new(rows());
        m.step(1);
        m.toggle("b");
        m.toggle("a");
        let mut rows = rows();
        rows.reverse();
        m.reload(rows);
        assert_eq!(m.focus.as_deref(), Some("b"));
        m.reload(vec![Member {
            id: "a".into(),
            name: "Renamed".into(),
        }]);
        assert_eq!(m.focus.as_deref(), Some("a"));
        assert_eq!(m.selection(), ["a"]);
        m.step(-99);
        m.step(99);
        assert_eq!(m.focus.as_deref(), Some("a"));
        m.toggle("a");
        assert!(m.selection().is_empty());
    }
}
