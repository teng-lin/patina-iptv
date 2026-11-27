//! Playlist-specific custom groups and membership editing.
use crate::membership::Membership;
use iptv_core::membership::Member;
use iptv_library::{ChannelScope, CustomGroup, CustomGroupId, SourceId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    Groups,
    Detail(CustomGroupId),
    Members,
    Destination(bool),
    Delete(CustomGroupId),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Text {
    Create,
    Rename(CustomGroupId),
    Filter,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    pub source: SourceId,
    pub groups: Vec<CustomGroup>,
    pub page: Page,
    pub scope: ChannelScope,
    pub members: Membership,
    /// Toolbar or menu row; None means a channel identity has focus.
    pub control: Option<usize>,
    pub editing: Option<(Text, String)>,
    pub status: String,
}
impl Open {
    pub fn new(source: SourceId) -> Self {
        Self {
            source,
            groups: Vec::new(),
            page: Page::Groups,
            scope: ChannelScope::All,
            members: Membership::default(),
            control: Some(0),
            editing: None,
            status: String::new(),
        }
    }

    pub fn origin(&self) -> Option<CustomGroupId> {
        if let ChannelScope::Custom(group) = self.scope {
            Some(group)
        } else {
            None
        }
    }

    pub fn show_page(&mut self, page: Page, control: usize) {
        self.page = page;
        self.control = Some(control);
    }

    pub fn enter_members(&mut self, scope: ChannelScope, rows: Vec<Member>) {
        self.scope = scope;
        self.members = Membership::new(rows);
        self.show_page(Page::Members, 0);
        self.status.clear();
    }

    pub fn reload_members(&mut self, groups: Vec<CustomGroup>, rows: Vec<Member>) {
        self.reload_groups(groups);
        self.members.reload(rows);
    }

    pub fn finish_transfer(&mut self, status: String) {
        self.members.selected.clear();
        self.show_page(Page::Members, 3);
        self.status = status;
    }

    pub fn back_page(&mut self) {
        match self.page {
            Page::Groups => {}
            Page::Destination(moving) => self.show_page(Page::Members, if moving { 4 } else { 3 }),
            Page::Delete(group) => self.show_page(Page::Detail(group), 3),
            Page::Members => match self.origin() {
                Some(group) => self.show_page(Page::Detail(group), 0),
                None => self.show_page(Page::Groups, 1),
            },
            Page::Detail(group) => {
                let control = self
                    .groups
                    .iter()
                    .position(|r| r.id == group)
                    .map_or(0, |at| at + 2);
                self.show_page(Page::Groups, control);
            }
        }
    }

    pub fn begin_text(&mut self, kind: Text) {
        let value = match kind {
            Text::Filter => self.members.query().to_owned(),
            Text::Rename(group) => self
                .groups
                .iter()
                .find(|r| r.id == group)
                .map(|g| g.name.clone())
                .unwrap_or_default(),
            Text::Create => String::new(),
        };
        self.status.clear();
        self.editing = Some((kind, value));
    }

    pub fn step(&mut self, by: isize, count: usize) {
        if let Some(at) = self.control {
            if by > 0
                && at + 1 >= count
                && self.page == Page::Members
                && self.members.visible_len() > 0
            {
                self.control = None;
            } else {
                self.control = Some(at.saturating_add_signed(by).min(count.saturating_sub(1)));
            }
        } else if by < 0
            && self
                .members
                .visible_at(0)
                .is_some_and(|r| Some(&r.id) == self.members.focus.as_ref())
        {
            self.control = Some(count.saturating_sub(1));
        } else {
            self.members.step(by);
        }
    }

    pub fn toggle_member_row(&mut self, id: &str) {
        self.control = None;
        self.members.focus = Some(id.into());
        self.members.toggle(id);
    }

    pub fn reload_groups(&mut self, groups: Vec<CustomGroup>) {
        let offset = match self.page {
            Page::Groups => Some(2),
            Page::Destination(_) => Some(1),
            _ => None,
        };
        let focused = offset
            .and_then(|offset| self.control.and_then(|at| at.checked_sub(offset)))
            .and_then(|at| {
                if matches!(self.page, Page::Destination(_)) {
                    self.groups
                        .iter()
                        .filter(|g| Some(g.id) != self.origin())
                        .nth(at)
                } else {
                    self.groups.get(at)
                }
            })
            .map(|g| g.id);
        self.groups = groups;
        if let (Some(id), Some(offset)) = (focused, offset) {
            let at = if matches!(self.page, Page::Destination(_)) {
                self.groups
                    .iter()
                    .filter(|g| Some(g.id) != self.origin())
                    .position(|g| g.id == id)
            } else {
                self.groups.iter().position(|g| g.id == id)
            };
            self.control = Some(at.map_or(0, |at| at + offset));
        }
        if matches!(self.page,Page::Detail(g)|Page::Delete(g) if !self.groups.iter().any(|r|r.id==g))
        {
            self.page = Page::Groups;
            self.control = Some(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn group(id: i64) -> CustomGroup {
        CustomGroup {
            id: CustomGroupId(id),
            source_id: SourceId(1),
            name: "Same name".into(),
            channel_count: 0,
        }
    }
    fn screen() -> Open {
        Open {
            source: SourceId(1),
            groups: vec![group(1), group(2), group(3)],
            page: Page::Groups,
            scope: ChannelScope::All,
            members: Membership::default(),
            control: Some(3),
            editing: None,
            status: String::new(),
        }
    }
    #[test]
    fn duplicate_named_group_focus_tracks_identity_through_refresh() {
        let mut s = screen();
        s.reload_groups(vec![group(2), group(3), group(1)]);
        assert_eq!(s.control, Some(2));
        s.reload_groups(vec![group(3)]);
        assert_eq!(s.control, Some(0));
        s.page = Page::Detail(CustomGroupId(3));
        s.reload_groups(vec![]);
        assert_eq!(s.page, Page::Groups);
        assert_eq!(s.control, Some(0));
    }
    #[test]
    fn membership_pages_keep_the_group_and_restore_the_action_on_back() {
        let mut screen = screen();
        let group = CustomGroupId(2);
        screen.show_page(Page::Detail(group), 0);
        screen.enter_members(
            ChannelScope::Custom(group),
            vec![Member {
                id: "synthetic".into(),
                name: "Synthetic channel".into(),
            }],
        );
        screen.toggle_member_row("synthetic");
        assert!(screen.members.selected.contains("synthetic"));
        assert_eq!(screen.control, None);
        screen.step(-1, 6);
        assert_eq!(screen.control, Some(5));

        screen.show_page(Page::Destination(true), 0);
        screen.back_page();
        assert_eq!(screen.control, Some(4));
        screen.show_page(Page::Destination(true), 0);
        screen.finish_transfer("Moved".into());
        assert!(screen.members.selected.is_empty());
        assert_eq!(screen.page, Page::Members);
        assert_eq!(screen.control, Some(3));
        screen.back_page();
        assert_eq!(screen.page, Page::Detail(group));
        screen.back_page();
        assert_eq!(screen.page, Page::Groups);
        assert_eq!(screen.control, Some(3));
    }
}
