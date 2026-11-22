//! Why a Movies or Shows catalog is empty, and the one thing to do next.
//!
//! The shell describes each enabled playlist, the ones the catalog reads
//! ([`SourceVod`]: its type, whether its movie and series import is on, a
//! refresh running now, how its last import went and whether it has titles); [`VodState::of`] reduces
//! them to one state with at most one action ([`VodState::action`]). Nothing
//! here switches an import on or invents titles: M3U playlists never carry
//! movies or series, so they only ever lead to adding another playlist.

use crate::wizard::FailureKind;

/// What one enabled playlist says about its movies (or shows).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceVod {
    pub source: i64,
    /// Its type can carry movies and series (Xtream Codes, Stalker); an
    /// M3U playlist cannot.
    pub eligible: bool,
    /// Its movie and series import is switched on.
    pub imports_enabled: bool,
    /// A refresh of it is running now.
    pub importing: bool,
    /// Its last import failed, or could not download the titles of the
    /// kind shown, by kind (never the provider's text).
    pub failed: Option<ImportProblem>,
    /// Its catalog has titles of the kind shown.
    pub has_titles: bool,
}

/// Why an import failed, as far as the empty screen tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportProblem {
    /// No answer, or a timeout.
    Network,
    /// The server answered, turning the request down (an HTTP error).
    Refused,
    /// The provider refused the account or the device.
    Access,
    /// The answer was not a playlist.
    Format,
    /// The import was stopped.
    Stopped,
    /// Too many playlists were updating at once.
    Busy,
    Other,
}

impl From<FailureKind> for ImportProblem {
    fn from(kind: FailureKind) -> Self {
        match kind {
            FailureKind::Network => Self::Network,
            FailureKind::Busy => Self::Busy,
            FailureKind::Auth => Self::Access,
            FailureKind::Parse | FailureKind::Unsupported => Self::Format,
            FailureKind::Canceled => Self::Stopped,
            FailureKind::Storage | FailureKind::EmptyPlaylist | FailureKind::Unavailable => {
                Self::Other
            }
        }
    }
}

/// The on-demand state of the playlists on screen, for one kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VodState {
    /// Some playlist has titles of this kind: an empty list is a filter's.
    #[default]
    Available,
    /// No playlist on screen can carry movies or series (only M3U, or none).
    NoEligibleSource,
    /// A playlist that could carry them has its movie and series import off.
    ImportsDisabled { source: i64 },
    /// A refresh that may bring titles is running.
    Importing,
    /// The last import of `source` failed.
    Failed { source: i64, problem: ImportProblem },
    /// `source` imported (or has yet to) and has no titles of this kind.
    Empty { source: i64 },
}

/// The single action an empty Movies or Shows screen offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptyAction {
    /// Open the add-playlist wizard.
    AddPlaylist,
    /// Open this playlist's actions at its movie and series import.
    PlaylistSettings(i64),
    /// Run this playlist's import again.
    Retry(i64),
    /// Refresh this playlist.
    Refresh(i64),
}

impl VodState {
    /// The state of `sources`, in order of what helps most: titles shown,
    /// then a running import, a failed one, an import switched off, an
    /// import that found nothing, and last no playlist that can have any.
    /// Within a state the first playlist in `sources` is the one acted on.
    pub fn of(sources: &[SourceVod]) -> Self {
        let eligible = || sources.iter().filter(|s| s.eligible);
        let enabled = || eligible().filter(|s| s.imports_enabled);
        if eligible().any(|s| s.has_titles) {
            return Self::Available;
        }
        if enabled().any(|s| s.importing) {
            return Self::Importing;
        }
        if let Some((source, problem)) =
            enabled().find_map(|s| s.failed.map(|problem| (s.source, problem)))
        {
            return Self::Failed { source, problem };
        }
        if let Some(s) = eligible().find(|s| !s.imports_enabled) {
            return Self::ImportsDisabled { source: s.source };
        }
        if let Some(s) = enabled().next() {
            return Self::Empty { source: s.source };
        }
        Self::NoEligibleSource
    }

    /// What the empty screen offers: nothing while importing or when titles
    /// exist (an empty list is then a search's or a filter's).
    pub fn action(self) -> Option<EmptyAction> {
        match self {
            Self::Available | Self::Importing => None,
            Self::NoEligibleSource => Some(EmptyAction::AddPlaylist),
            Self::ImportsDisabled { source } => Some(EmptyAction::PlaylistSettings(source)),
            Self::Failed { source, .. } => Some(EmptyAction::Retry(source)),
            Self::Empty { source } => Some(EmptyAction::Refresh(source)),
        }
    }
}

impl EmptyAction {
    /// The playlist it acts on (none for adding one).
    pub fn target(self) -> Option<i64> {
        match self {
            Self::AddPlaylist => None,
            Self::PlaylistSettings(source) | Self::Retry(source) | Self::Refresh(source) => {
                Some(source)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xtream(source: i64) -> SourceVod {
        SourceVod {
            source,
            eligible: true,
            imports_enabled: true,
            ..Default::default()
        }
    }

    fn m3u(source: i64) -> SourceVod {
        SourceVod {
            source,
            ..Default::default()
        }
    }

    #[test]
    fn m3u_alone_or_nothing_offers_adding_a_playlist() {
        for sources in [vec![], vec![m3u(1)], vec![m3u(1), m3u(2)]] {
            let state = VodState::of(&sources);
            assert_eq!(state, VodState::NoEligibleSource);
            assert_eq!(state.action(), Some(EmptyAction::AddPlaylist));
        }
        // An M3U playlist's import switch or failure never matters.
        let mut odd = m3u(1);
        odd.imports_enabled = true;
        odd.failed = Some(ImportProblem::Network);
        odd.importing = true;
        assert_eq!(VodState::of(&[odd]), VodState::NoEligibleSource);
    }

    #[test]
    fn an_import_switched_off_opens_that_playlists_settings() {
        let mut off = xtream(4);
        off.imports_enabled = false;
        // Failures and refreshes of a playlist without the import change nothing.
        off.failed = Some(ImportProblem::Network);
        off.importing = true;
        let state = VodState::of(&[m3u(1), off]);
        assert_eq!(state, VodState::ImportsDisabled { source: 4 });
        assert_eq!(state.action(), Some(EmptyAction::PlaylistSettings(4)));
    }

    #[test]
    fn a_running_import_has_no_action() {
        let mut running = xtream(2);
        running.importing = true;
        running.failed = Some(ImportProblem::Access);
        let state = VodState::of(&[running]);
        assert_eq!(state, VodState::Importing);
        assert_eq!(state.action(), None);
    }

    #[test]
    fn a_failed_import_retries_that_playlist() {
        let mut failed = xtream(3);
        failed.failed = Some(ImportProblem::Format);
        let state = VodState::of(&[xtream(2), failed]);
        assert_eq!(
            state,
            VodState::Failed {
                source: 3,
                problem: ImportProblem::Format
            }
        );
        assert_eq!(state.action(), Some(EmptyAction::Retry(3)));
    }

    #[test]
    fn an_empty_import_refreshes_and_titles_anywhere_are_available() {
        let state = VodState::of(&[m3u(1), xtream(2)]);
        assert_eq!(state, VodState::Empty { source: 2 });
        assert_eq!(state.action(), Some(EmptyAction::Refresh(2)));
        let mut stocked = xtream(5);
        stocked.has_titles = true;
        let mut failed = xtream(6);
        failed.failed = Some(ImportProblem::Network);
        let state = VodState::of(&[failed, stocked]);
        assert_eq!(state, VodState::Available);
        assert_eq!(state.action(), None);
    }

    #[test]
    fn the_most_helpful_state_wins_in_combined_playlists() {
        let mut off = xtream(1);
        off.imports_enabled = false;
        let mut failed = xtream(2);
        failed.failed = Some(ImportProblem::Network);
        let mut running = xtream(3);
        running.importing = true;
        let empty = xtream(4);
        assert!(matches!(
            VodState::of(&[off, failed, running, empty]),
            VodState::Importing
        ));
        assert!(matches!(
            VodState::of(&[off, empty, failed]),
            VodState::Failed { source: 2, .. }
        ));
        assert_eq!(
            VodState::of(&[empty, off]),
            VodState::ImportsDisabled { source: 1 }
        );
    }

    #[test]
    fn session_failures_map_to_what_the_screen_tells() {
        assert_eq!(
            ImportProblem::from(FailureKind::Auth),
            ImportProblem::Access
        );
        assert_eq!(
            ImportProblem::from(FailureKind::Canceled),
            ImportProblem::Stopped
        );
        assert_eq!(ImportProblem::from(FailureKind::Busy), ImportProblem::Busy);
    }
}
