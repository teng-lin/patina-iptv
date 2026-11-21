//! iptv-rs's settings catalog: every setting and command, grouped by
//! section, plus lookups and the UI tree.
//!
//! The capability set and its structure follow the reference player's
//! settings screens, section by section, as they were observed;
//! keys, choice families, defaults and wording are iptv-rs's own. Each
//! row's comment records where its default comes from.
//!
//! Not settings: per-playlist provider data (URL, credentials, name, EPG
//! sources and assignments), the parental PIN (a secret), and customization
//! overlays (names, favorites, hidden, blocked, order) live in their own
//! stores.

/// Marks app-owned help text for translation by the consuming interface.
/// The app catalog lint checks every literal passed here.
const fn key(message: &'static str) -> &'static str {
    message
}

pub mod choices;
mod commands;
mod general;
mod guide;
mod library;
mod player;

pub use commands::{COMMANDS, Command};

use crate::schema::{Section, Setting};

/// The settings of one section, in UI order.
pub fn section(section: Section) -> &'static [Setting] {
    match section {
        Section::General => general::GENERAL,
        Section::Playlists => general::PLAYLISTS,
        Section::Channels => general::CHANNELS,
        Section::Epg => guide::EPG,
        Section::Appearance => guide::APPEARANCE,
        Section::Guide => guide::GUIDE,
        Section::Logos => guide::LOGOS,
        Section::Player => player::PLAYER,
        Section::Playback => player::PLAYBACK,
        Section::Catchup => player::CATCHUP,
        Section::History => library::HISTORY,
        Section::Search => library::SEARCH,
        Section::Recordings => library::RECORDINGS,
        Section::Parental => library::PARENTAL,
        Section::Remote => library::REMOTE,
        Section::Vod => library::VOD,
    }
}

/// Every setting, section by section.
pub fn settings() -> impl Iterator<Item = &'static Setting> {
    Section::ALL.into_iter().flat_map(section)
}

pub fn setting(key: &str) -> Option<&'static Setting> {
    settings().find(|s| s.key == key)
}

pub fn commands() -> impl Iterator<Item = &'static Command> {
    COMMANDS.iter()
}

pub fn command(key: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.key == key)
}

/// One section of the settings tree.
#[derive(Debug)]
pub struct SectionNode {
    pub section: Section,
    /// Non-empty groups; the one without a subsection comes first.
    pub groups: Vec<GroupNode>,
}

/// The entries of one subsection: settings in catalog order, then
/// commands.
#[derive(Debug)]
pub struct GroupNode {
    pub subsection: Option<&'static str>,
    pub settings: Vec<&'static Setting>,
    pub commands: Vec<&'static Command>,
}

/// The settings tree for the UI. Advanced settings are left out unless
/// `advanced` is set; visibility rules that depend on values are applied
/// by [`Settings::is_visible`](crate::Settings::is_visible).
pub fn tree(advanced: bool) -> Vec<SectionNode> {
    Section::ALL
        .into_iter()
        .map(|sec| {
            let subs = core::iter::once(None).chain(sec.subsections().iter().copied().map(Some));
            let groups = subs
                .map(|sub| GroupNode {
                    subsection: sub,
                    settings: section(sec)
                        .iter()
                        .filter(|s| s.subsection == sub && (advanced || !s.flags.advanced))
                        .collect(),
                    commands: COMMANDS
                        .iter()
                        .filter(|c| c.section == sec && c.subsection == sub)
                        .collect(),
                })
                .filter(|g| !g.settings.is_empty() || !g.commands.is_empty())
                .collect();
            SectionNode {
                section: sec,
                groups,
            }
        })
        .collect()
}
