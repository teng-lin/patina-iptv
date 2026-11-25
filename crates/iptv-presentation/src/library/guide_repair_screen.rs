//! Pure batch repair editor state. Every actionable row carries content identity.
use iptv_library::{
    Choice, SourceId,
    guide_repair::*,
    inspection::{ChannelInfo, ChannelRef, TimeWindow},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Mapping(ChannelRef),
    Remove(ChannelRef),
    Preview,
    Apply,
    MoreMappings,
    MoreIndirect,
    Verify,
    Status,
    Undo,
    Cancel,
    History(String),
    SelectMore,
    Clear,
    Back,
}
/// A repair operation's result, ready to apply to the current editor.
#[derive(Debug)]
pub enum Reply {
    History(Vec<Commit>),
    Preview(Preview),
    Applied(Outcome),
    CompletionUnknown(String),
    Status(Operation),
    Verified(Verification),
    Undone(Outcome),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    pub open: bool,
    pub request: Request,
    pub names: Vec<(ChannelRef, String, Choice)>,
    pub focus: Row,
    pub generation: u64,
    pub busy: bool,
    pub preview: Option<Preview>,
    pub outcome: Option<Outcome>,
    pub verification: Option<Verification>,
    pub operation: Option<Operation>,
    pub uncertain: Option<String>,
    pub history: Vec<Commit>,
    pub error: Option<Error>,
    pub mapping_offset: usize,
    pub indirect_offset: usize,
}
impl Screen {
    pub fn new(source: SourceId, window: TimeWindow) -> Self {
        Self {
            open: true,
            request: Request {
                source,
                mappings: vec![],
                feeds: vec![],
                additions: vec![],
                feed_order: None,
                window,
            },
            names: vec![],
            focus: Row::Preview,
            generation: 0,
            busy: false,
            preview: None,
            outcome: None,
            verification: None,
            operation: None,
            uncertain: None,
            history: vec![],
            error: None,
            mapping_offset: 0,
            indirect_offset: 0,
        }
    }
    pub fn add(&mut self, channel: ChannelInfo, mut target: Target) -> bool {
        let existing = self
            .request
            .mappings
            .iter()
            .position(|r| r.channel == channel.key);
        if channel.key.source != self.request.source
            || self.outcome.is_some()
            || (existing.is_none() && self.request.mappings.len() >= MAX_MAPPING_CHANGES)
        {
            self.report_error(Some(Error::InvalidRequest));
            return false;
        }
        if let Target::Staged {
            evidence, addition, ..
        } = &mut target
            && evidence.feed.is_none()
        {
            let index = self
                .request
                .additions
                .iter()
                .position(|a| a.evidence == *evidence)
                .unwrap_or_else(|| {
                    self.request.additions.push(FeedAddition {
                        evidence: evidence.clone(),
                        name: None,
                    });
                    self.request.additions.len() - 1
                });
            *addition = Some(index);
        }
        let row = MappingRequest {
            channel: channel.key.clone(),
            target,
            replace_explicit: false,
        };
        if let Some(index) = existing {
            self.request.mappings[index] = row;
        } else {
            self.request.mappings.push(row);
        }
        self.names.retain(|(key, _, _)| *key != channel.key);
        self.names
            .push((channel.key.clone(), channel.name, channel.choice));
        self.focus = Row::Mapping(channel.key);
        self.prune_additions();
        self.open = true;
        self.invalidate();
        true
    }
    pub fn remove(&mut self, key: &ChannelRef) {
        self.request.mappings.retain(|row| row.channel != *key);
        self.names.retain(|(id, _, _)| id != key);
        self.prune_additions();
        self.restore_focus();
    }
    fn prune_additions(&mut self) {
        let mut used: Vec<_> = self
            .request
            .mappings
            .iter()
            .filter_map(|row| match &row.target {
                Target::Staged {
                    addition: Some(index),
                    ..
                } => Some(*index),
                _ => None,
            })
            .collect();
        used.sort_unstable();
        used.dedup();
        self.request.additions = used
            .iter()
            .filter_map(|index| self.request.additions.get(*index).cloned())
            .collect();
        for row in &mut self.request.mappings {
            if let Target::Staged {
                addition: Some(index),
                ..
            } = &mut row.target
                && let Ok(next) = used.binary_search(index)
            {
                *index = next;
            }
        }
    }
    /// Close the editor and return the preview the runtime should release.
    pub fn close(&mut self) -> Option<Preview> {
        self.open = false;
        self.invalidate()
    }

    /// Reopen the current draft with its existing focus if possible.
    pub fn reopen(&mut self) {
        self.open = true;
        self.restore_focus();
    }

    /// Replace the draft with a new draft for the same source and window.
    pub fn clear(&mut self) {
        *self = Self::new(self.request.source, self.request.window);
    }

    /// Focus the row selected by the viewer.
    pub fn focus_row(&mut self, row: Row) {
        self.focus = row;
    }

    /// Change consent to replace an explicit mapping on exactly one channel.
    pub fn toggle_replacement(&mut self, channel: &ChannelRef) {
        if let Some(row) = self
            .request
            .mappings
            .iter_mut()
            .find(|r| &r.channel == channel)
        {
            row.replace_explicit = !row.replace_explicit;
        }
    }

    /// Replace the visible preview page and its two independent cursors.
    pub fn set_page(&mut self, preview: Preview, mapping: usize, indirect: usize) {
        self.preview = Some(preview);
        self.mapping_offset = mapping;
        self.indirect_offset = indirect;
        self.restore_focus();
    }

    /// Show the error from a request that runs without the busy transition.
    pub fn report_error(&mut self, error: Option<Error>) {
        self.error = error;
    }

    pub fn invalidate(&mut self) -> Option<Preview> {
        self.generation += 1;
        self.busy = false;
        self.error = None;
        self.verification = None;
        self.mapping_offset = 0;
        self.indirect_offset = 0;
        self.preview.take()
    }
    pub fn begin(&mut self) -> u64 {
        self.generation += 1;
        self.busy = true;
        self.error = None;
        self.generation
    }
    /// Whether a completion belongs to the current open draft.
    pub fn accepts(&self, generation: u64) -> bool {
        self.open && self.generation == generation
    }

    /// Apply the reply only when it belongs to the current draft.
    pub fn finish(&mut self, generation: u64, result: Result<Reply>) -> bool {
        if !self.accepts(generation) {
            return false;
        }
        self.busy = false;
        self.error = match result {
            Ok(reply) => self.apply_reply(reply).err(),
            Err(error) => Some(error),
        };
        self.restore_focus();
        true
    }

    fn apply_reply(&mut self, reply: Reply) -> Result<()> {
        match reply {
            Reply::History(history) => self.history = history,
            Reply::Preview(preview) => {
                self.preview = Some(preview);
                self.mapping_offset = 0;
                self.indirect_offset = 0;
                self.focus = Row::Apply;
            }
            Reply::Applied(outcome) => {
                self.outcome = Some(outcome);
                self.preview = None;
                self.focus = Row::Status;
            }
            Reply::CompletionUnknown(operation) => {
                self.uncertain = Some(operation.clone());
                self.focus = Row::Status;
                return Err(Error::CompletionUnknown { operation });
            }
            Reply::Status(operation) => {
                self.outcome = Some(Outcome {
                    commit: operation.commit.clone(),
                    configuration_committed: true,
                    mapping_saved: true,
                    effective: operation.effective,
                    verification_pending: !operation
                        .verification
                        .as_ref()
                        .is_some_and(|v| v.verified_coverage),
                    operation: operation.id.clone(),
                });
                self.verification = operation.verification.clone();
                self.operation = Some(operation);
                self.uncertain = None;
            }
            Reply::Verified(verification) => self.verification = Some(verification),
            Reply::Undone(outcome) => {
                self.outcome = Some(outcome);
                self.verification = None;
            }
        }
        Ok(())
    }

    pub fn commit_id(&self) -> Option<String> {
        self.outcome
            .as_ref()
            .map(|o| o.commit.id.clone())
            .or_else(|| self.uncertain.clone())
    }
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if self.outcome.is_none() && self.uncertain.is_none() {
            let selected: Vec<_> = self.preview.as_ref().map_or_else(
                || {
                    self.request
                        .mappings
                        .iter()
                        .map(|r| r.channel.clone())
                        .collect()
                },
                |p| p.mappings.iter().map(|r| r.channel.clone()).collect(),
            );
            if !self.request.mappings.is_empty()
                || !self.request.feeds.is_empty()
                || !self.request.additions.is_empty()
            {
                rows.push(Row::Preview);
            }
            if let Some(preview) = &self.preview {
                rows.push(Row::Apply);
                if preview.next_mapping.is_some() {
                    rows.push(Row::MoreMappings);
                }
                if preview.next_indirect.is_some() {
                    rows.push(Row::MoreIndirect);
                }
            }
            for key in selected {
                rows.push(Row::Mapping(key.clone()));
                rows.push(Row::Remove(key));
            }
            rows.push(Row::SelectMore);
            if !self.request.mappings.is_empty() {
                rows.push(Row::Clear);
            }
        } else {
            rows.extend([Row::Status, Row::Verify]);
            if self.outcome.as_ref().is_some_and(|o| !o.commit.undone) {
                rows.push(Row::Undo);
            }
            rows.push(Row::Cancel);
        }
        rows.extend(self.history.iter().map(|c| Row::History(c.id.clone())));
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_library::inspection::ChannelInfo;
    use iptv_providers::ContentId;
    fn channel(key: &str) -> ChannelInfo {
        ChannelInfo {
            key: ChannelRef {
                source: SourceId(1),
                content: ContentId::from_stored(key),
            },
            name: format!("Synthetic {key}"),
            number: None,
            hidden: false,
            blocked: false,
            epg_id: None,
            epg_name: None,
            choice: Choice::Automatic,
        }
    }
    #[test]
    fn draft_accumulates_channels_and_focus_survives_reordering_by_identity() {
        let mut editor = Screen::new(SourceId(1), TimeWindow { from: 1, to: 2 });
        assert!(editor.add(channel("amber"), Target::NoGuide));
        assert!(editor.add(channel("birch"), Target::NoGuide));
        let focus = editor.focus.clone();
        editor.request.mappings.reverse();
        editor.restore_focus();
        assert_eq!(editor.focus, focus);
        assert!(editor.add(channel("amber"), Target::Automatic));
        assert_eq!(editor.request.mappings.len(), 2);
        let pending = editor.begin();
        editor.invalidate();
        assert!(!editor.finish(pending, Err(Error::Canceled)));
    }
    #[test]
    fn removing_or_replacing_last_staged_mapping_drops_its_unneeded_feed_addition() {
        use iptv_library::inspection::{ProbeEvidence, ProbeId};
        let mut editor = Screen::new(SourceId(1), TimeWindow { from: 1, to: 2 });
        let evidence = |name: &str| ProbeEvidence {
            stage: ProbeId(name.into()),
            source: SourceId(1),
            feed: None,
            incarnation: name.into(),
            digest: [0; 32],
            expires_at: 100,
        };
        for (key, index) in [("amber", 0), ("birch", 1)] {
            let evidence = evidence(key);
            editor.request.additions.push(FeedAddition {
                evidence: evidence.clone(),
                name: None,
            });
            editor.add(
                channel(key),
                Target::Staged {
                    evidence,
                    epg_channel: key.into(),
                    addition: Some(index),
                },
            );
        }
        editor.remove(&channel("amber").key);
        assert_eq!(editor.request.additions.len(), 1);
        assert!(matches!(
            &editor.request.mappings[0].target,
            Target::Staged {
                addition: Some(0),
                ..
            }
        ));
        editor.add(channel("birch"), Target::Automatic);
        assert!(editor.request.additions.is_empty());
    }
    #[test]
    fn rejecting_a_staged_mapping_does_not_attach_an_unused_feed() {
        use iptv_library::inspection::{ProbeEvidence, ProbeId};

        let mut editor = Screen::new(SourceId(1), TimeWindow { from: 1, to: 2 });
        for index in 0..MAX_MAPPING_CHANGES {
            assert!(editor.add(channel(&format!("channel-{index}")), Target::Automatic));
        }
        let evidence = ProbeEvidence {
            stage: ProbeId("new-feed".into()),
            source: SourceId(1),
            feed: None,
            incarnation: "generated".into(),
            digest: [0; 32],
            expires_at: 100,
        };
        assert!(!editor.add(
            channel("extra"),
            Target::Staged {
                evidence,
                epg_channel: "extra".into(),
                addition: None,
            }
        ));
        assert!(editor.request.additions.is_empty());
        assert_eq!(editor.error, Some(Error::InvalidRequest));
    }

    #[test]
    fn unknown_apply_result_exposes_recovery_and_close_fences_old_replies() {
        let mut editor = Screen::new(SourceId(1), TimeWindow { from: 1, to: 2 });
        assert!(editor.add(channel("amber"), Target::Automatic));
        let generation = editor.begin();
        assert!(editor.finish(generation, Ok(Reply::CompletionUnknown("synthetic".into()))));
        assert!(matches!(
            editor.error,
            Some(Error::CompletionUnknown { .. })
        ));
        assert_eq!(editor.commit_id().as_deref(), Some("synthetic"));
        assert_eq!(editor.focus, Row::Status);
        editor.close();
        editor.reopen();
        assert!(!editor.finish(generation, Err(Error::PlanMissing)));
        assert_eq!(editor.commit_id().as_deref(), Some("synthetic"));
        editor.clear();
        assert!(editor.request.mappings.is_empty());
        assert!(editor.commit_id().is_none());
    }

    #[test]
    fn replacement_opt_in_is_per_row() {
        let mut editor = Screen::new(SourceId(1), TimeWindow { from: 1, to: 2 });
        editor.add(channel("amber"), Target::NoGuide);
        editor.add(channel("birch"), Target::NoGuide);
        editor.toggle_replacement(&channel("amber").key);
        assert!(editor.request.mappings[0].replace_explicit);
        assert!(!editor.request.mappings[1].replace_explicit);
    }
}
