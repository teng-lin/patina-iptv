//! Portable chooser state and the values associated with each rendered row.
use crate::i18n::Language;
use crate::play_bar::Control;
use crate::player_options::Chooser;
use crate::tracks::entries_in;
use iptv_core::tracks::Kind as Track;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Picture,
    Cast,
    AirPlay,
    CastEnded,
    More,
    Volume,
    Sleep,
    Track(Track),
    AudioDelay,
    /// The recently watched channels.
    History,
    /// No or Yes before leaving the app.
    Leave,
}

/// The open chooser.
#[derive(Clone, Debug, PartialEq)]
pub struct Open<W> {
    pub kind: Kind,
    pub labels: Vec<String>,
    /// What each entry sets, for tracks (a track id, `auto`, `off`) and
    /// history (a channel key).
    pub values: Vec<String>,
    /// Snapshot behind both the translated labels and their opaque pick values.
    pub tracks: Option<iptv_core::tracks::Tracks>,
    pub receivers: Vec<iptv_core::cast::Receiver>,
    // Preserve action identity for a press and keep the originating desktop window.
    pub more: Option<More<W>>,
    pub list: Chooser,
}

#[derive(Clone, Debug, PartialEq)]
pub struct More<W> {
    pub controls: Vec<Control>,
    pub window: Option<W>,
}

impl<W> Open<W> {
    pub fn for_tracks(kind: Track, tracks: iptv_core::tracks::Tracks, language: &Language) -> Self {
        let labels = entries_in(&tracks, kind, language);
        let values = (0..labels.len())
            .map(|i| tracks.pick(kind, i).unwrap_or_default())
            .collect();
        Self {
            kind: Kind::Track(kind),
            list: Chooser::new(labels.len(), tracks.selected_entry(kind)),
            labels,
            values,
            tracks: Some(tracks),
            receivers: Vec::new(),
            more: None,
        }
    }

    pub fn track_labels_in(&self, language: &Language) -> Vec<String> {
        match (self.kind, self.tracks.as_ref()) {
            (Kind::Track(kind), Some(tracks)) => entries_in(tracks, kind, language),
            _ => Vec::new(),
        }
    }
}
