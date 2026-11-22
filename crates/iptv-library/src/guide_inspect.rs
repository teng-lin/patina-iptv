//! Guide inspection screen state. Focus and reply ownership use content identity.
use crate::{SourceId, inspection::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Audit,
    Candidates,
    ProbeCandidates,
    Samples,
    Link,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Search,
    Channel(ChannelRef),
    Candidate(CandidateRef),
    Next,
    NewLink,
    InspectLink,
    ReuseInspected,
    UseCandidate,
    AutomaticRepair,
    NoGuideRepair,
    ReviewRepair,
    RepairHistory,
    Back,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct ParentPage {
    mode: Mode,
    next: Option<Cursor>,
    focus: Row,
    query: String,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Screen {
    pub source: SourceId,
    pub window: TimeWindow,
    pub mode: Mode,
    pub samples_back: Mode,
    pub focus: Row,
    pub request: u64,
    pub busy: bool,
    pub typing: bool,
    pub error: Option<InspectionError>,
    pub query: String,
    pub link: String,
    pub audit: Vec<Coverage>,
    pub candidates: Vec<Candidate>,
    pub samples: Vec<CandidateProgram>,
    pub current: Vec<ProgramInfo>,
    pub subject: Option<ChannelRef>,
    pub selected: Option<CandidateRef>,
    pub pending_repairs: usize,
    pub stage: Option<ProbeSummary>,
    pub retained_stage: Option<ProbeSummary>,
    pub next: Option<Cursor>,
    /// Staged probes (inspecting a link) are offered: false where the host
    /// cannot run them.
    pub probes: bool,
    parents: Vec<ParentPage>,
}
impl Screen {
    pub fn new(source: SourceId, window: TimeWindow) -> Self {
        Self {
            source,
            window,
            mode: Mode::Audit,
            samples_back: Mode::Candidates,
            focus: Row::NewLink,
            request: 0,
            busy: false,
            typing: false,
            error: None,
            query: String::new(),
            link: String::new(),
            audit: Vec::new(),
            candidates: Vec::new(),
            samples: Vec::new(),
            current: Vec::new(),
            subject: None,
            selected: None,
            pending_repairs: 0,
            stage: None,
            retained_stage: None,
            next: None,
            probes: true,
            parents: Vec::new(),
        }
    }

    /// Without staged probes: no link is offered for inspection.
    pub fn without_probes(mut self) -> Self {
        self.probes = false;
        self.focus = Row::Search;
        self
    }
    pub fn enter(&mut self, mode: Mode) {
        self.parents.push(ParentPage {
            mode: self.mode,
            next: self.next.take(),
            focus: self.focus.clone(),
            query: self.query.clone(),
        });
        self.mode = mode;
        self.restore_focus();
    }
    pub fn begin(&mut self) -> u64 {
        self.request += 1;
        self.busy = true;
        self.error = None;
        self.request
    }
    pub fn finish(&mut self, request: u64) -> bool {
        if self.request != request {
            return false;
        }
        self.busy = false;
        true
    }
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = match self.mode {
            Mode::Audit => self
                .audit
                .iter()
                .map(|r| Row::Channel(r.channel.key.clone()))
                .collect(),
            Mode::Candidates | Mode::ProbeCandidates => self
                .candidates
                .iter()
                .map(|c| Row::Candidate(c.key.clone()))
                .collect(),
            Mode::Link => vec![Row::InspectLink],
            Mode::Samples => Vec::new(),
        };
        if matches!(
            self.mode,
            Mode::Audit | Mode::Candidates | Mode::ProbeCandidates
        ) {
            rows.insert(0, Row::Search);
        }
        if self.next.is_some() {
            rows.push(Row::Next);
        }
        if self.probes && matches!(self.mode, Mode::Audit | Mode::Candidates) {
            rows.push(Row::NewLink);
        }
        if self.mode == Mode::Candidates && self.subject.is_some() {
            if self.probes && self.retained_stage.is_some() {
                rows.push(Row::ReuseInspected);
            }
            rows.extend([Row::AutomaticRepair, Row::NoGuideRepair]);
        }
        if self.mode == Mode::Samples && self.selected.is_some() && self.subject.is_some() {
            rows.push(Row::UseCandidate);
        }
        if self.pending_repairs > 0 {
            rows.push(Row::ReviewRepair);
        }
        rows.push(Row::RepairHistory);
        rows.push(Row::Back);
        rows
    }
    pub fn restore_focus(&mut self) {
        let rows = self.rows();
        if !rows.contains(&self.focus) {
            self.focus = rows.first().cloned().unwrap_or(Row::Back);
        }
    }
    pub fn move_focus(&mut self, delta: isize) {
        let rows = self.rows();
        let at = rows.iter().position(|r| *r == self.focus).unwrap_or(0);
        self.focus = rows[at.saturating_add_signed(delta).min(rows.len() - 1)].clone();
    }
    pub fn back(&mut self) -> bool {
        self.request += 1;
        self.busy = false;
        self.error = None;
        if let Some(parent) = self.parents.pop() {
            self.mode = parent.mode;
            self.next = parent.next;
            self.focus = parent.focus;
            self.query = parent.query;
        } else {
            if self.mode == Mode::Audit {
                return false;
            }
            self.mode = Mode::Audit;
            self.next = None;
        }
        self.restore_focus();
        true
    }
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screen")
            .field("source", &self.source)
            .field("window", &self.window)
            .field("mode", &self.mode)
            .field("focus", &self.focus)
            .field("request", &self.request)
            .field("busy", &self.busy)
            .field("link", &"[redacted]")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn without_probes_no_link_is_offered() {
        let window = TimeWindow { from: 1, to: 2 };
        assert!(
            Screen::new(SourceId(1), window)
                .rows()
                .contains(&Row::NewLink)
        );
        let screen = Screen::new(SourceId(1), window).without_probes();
        assert!(!screen.rows().contains(&Row::NewLink));
        assert!(screen.rows().contains(&screen.focus));
    }
    #[test]
    fn inspection_link_entry_is_redacted_in_leaf_and_containing_debug() {
        let mut screen = Screen::new(SourceId(1), TimeWindow { from: 1, to: 2 });
        screen.mode = Mode::Link;
        screen.typing = true;
        screen.link = "http://synthetic-user:synthetic-password@guide.example.test/private-path?token=synthetic-token".into();
        let shown = format!("{screen:?}");
        let containing = format!("{:?}", Some(&screen));
        for secret in [
            "synthetic-user",
            "synthetic-password",
            "private-path",
            "synthetic-token",
        ] {
            assert!(!shown.contains(secret), "{shown}");
            assert!(!containing.contains(secret), "{containing}");
        }
        assert!(shown.contains("Screen"));
        assert!(shown.contains("Link"));
        assert!(
            screen.link.contains("synthetic-token"),
            "wire input must stay intact"
        );
    }

    #[test]
    fn late_replies_and_reordering_cannot_move_identity_focus() {
        let mut screen = Screen::new(SourceId(1), TimeWindow { from: 1, to: 2 });
        let first = screen.begin();
        let second = screen.begin();
        assert!(!screen.finish(first));
        assert!(screen.finish(second));
        screen.mode = Mode::Candidates;
        let key = CandidateRef {
            source: SourceId(1),
            feed: iptv_epg::FeedId(2),
            incarnation: "generated".into(),
            epg_channel: "amber".into(),
        };
        screen.candidates.push(Candidate {
            key: key.clone(),
            name: None,
            names: Vec::new(),
            metadata: Vec::new(),
            assigned: false,
            evidence: Vec::new(),
            binding: None,
        });
        screen.focus = Row::Candidate(key.clone());
        screen.candidates.reverse();
        screen.restore_focus();
        assert_eq!(screen.focus, Row::Candidate(key));
        let pending = screen.begin();
        assert!(screen.back());
        assert!(!screen.finish(pending));
    }
    #[test]
    fn returning_from_samples_restores_each_parent_page_cursor_and_filter() {
        let mut screen = Screen::new(SourceId(1), TimeWindow { from: 1, to: 2 });
        let cursor = |value: &str| Cursor {
            context: value.into(),
            snapshot: value.into(),
            after: vec![value.into()],
        };
        screen.next = Some(cursor("audit"));
        screen.query = "Amber".into();
        screen.enter(Mode::Candidates);
        screen.next = Some(cursor("candidates"));
        screen.query = "East".into();
        screen.enter(Mode::Samples);
        assert!(screen.next.is_none());
        assert!(screen.back());
        assert_eq!(screen.mode, Mode::Candidates);
        assert_eq!(screen.next, Some(cursor("candidates")));
        assert_eq!(screen.query, "East");
        assert!(screen.back());
        assert_eq!(screen.mode, Mode::Audit);
        assert_eq!(screen.next, Some(cursor("audit")));
        assert_eq!(screen.query, "Amber");
    }
}
