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
    Back,
}
#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub samples: Vec<CandidateProgramme>,
    pub current: Vec<ProgrammeInfo>,
    pub subject: Option<ChannelRef>,
    pub stage: Option<ProbeSummary>,
    pub next: Option<Cursor>,
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
            stage: None,
            next: None,
        }
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
        if self.mode == Mode::Audit {
            rows.push(Row::NewLink);
        }
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
        self.next = None;
        self.mode = match self.mode {
            Mode::Audit => return false,
            Mode::Samples => self.samples_back,
            _ => Mode::Audit,
        };
        self.restore_focus();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
