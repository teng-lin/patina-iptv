//! Commands: entries of the settings tree that act instead of storing a
//! value (back up, clear, restore defaults). They share the key and label
//! space with settings but are never persisted.

use crate::schema::Section;

// Message marker for descriptions translated by the consuming shell.
const fn key(message: &'static str) -> &'static str {
    message
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub key: &'static str,
    pub section: Section,
    pub subsection: Option<&'static str>,
    /// Ask before running; the confirmation states the consequence.
    pub confirm: bool,
    pub requires_restart: bool,
    /// Settings the command resets to their defaults when it runs.
    pub resets: &'static [&'static str],
    pub summary: &'static str,
}

impl Command {
    const fn new(
        section: Section,
        subsection: &'static str,
        key: &'static str,
        summary: &'static str,
    ) -> Command {
        Command {
            key,
            section,
            subsection: if subsection.is_empty() {
                None
            } else {
                Some(subsection)
            },
            confirm: false,
            requires_restart: false,
            resets: &[],
            summary,
        }
    }

    const fn confirm(mut self) -> Command {
        self.confirm = true;
        self
    }

    const fn restart(mut self) -> Command {
        self.requires_restart = true;
        self
    }

    const fn resets(mut self, keys: &'static [&'static str]) -> Command {
        self.resets = keys;
        self
    }

    pub fn label_key(&self) -> String {
        format!("settings.{}", self.key)
    }

    pub fn help_key(&self) -> String {
        format!("settings.{}.help", self.key)
    }
}

use Section as S;

pub static COMMANDS: &[Command] = &[
    Command::new(
        S::Recordings,
        "",
        "recordings.quit_host",
        key("Stop background recording and quit; pending schedules remain saved."),
    )
    .confirm(),
    Command::new(
        S::Recordings,
        "",
        "recordings.background_status",
        key("Background execution and wake capabilities."),
    ),
    Command::new(
        S::Recordings,
        "",
        "recordings.enable_startup",
        key("Start the recording host at login."),
    ),
    Command::new(
        S::Recordings,
        "",
        "recordings.disable_startup",
        key("Remove the recording host login hook."),
    ),
    Command::new(
        S::Recordings,
        "",
        "recordings.alarm_access",
        key("Allow precise recording wakeups in Android settings."),
    ),
    Command::new(
        S::General,
        "",
        "general.backup",
        "Write settings and customisations to a backup file.",
    ),
    Command::new(
        S::General,
        "",
        "general.restore",
        "Replace everything with a backup's contents.",
    )
    .confirm()
    .restart(),
    Command::new(
        S::General,
        "",
        "general.reset_watch_time",
        "Forget per-channel viewing time.",
    )
    .confirm(),
    Command::new(S::General, "", "general.restart", "Restart the app now.").confirm(),
    Command::new(
        S::General,
        "privacy",
        "general.send_logs",
        "Send a diagnostic log, redacted of credentials.",
    ),
    Command::new(
        S::General,
        "updates",
        "general.check_updates",
        "Look for a newer version now.",
    ),
    Command::new(
        S::General,
        "",
        "general.about",
        "Version, licences and policies.",
    ),
    Command::new(
        S::Playlists,
        "",
        "playlists.manage",
        "Open the playlists screen: add, choose, refresh, rename, change the guide address, delete.",
    ),
    Command::new(
        S::Playlists,
        "",
        "playlists.add",
        "Open the add-playlist wizard.",
    ),
    Command::new(
        S::Playlists,
        "",
        "playlists.reorder",
        "Arrange playlists by hand.",
    ),
    Command::new(
        S::Playlists,
        "updates",
        "playlists.update_all",
        "Refresh every playlist now.",
    ),
    Command::new(
        S::Channels,
        "",
        "channels.manage_groups",
        "Show, hide, rename and arrange groups.",
    ),
    Command::new(S::Epg, "", "epg.add_source", "Add an XMLTV guide source."),
    Command::new(
        S::Epg,
        "",
        "epg.manage_sources",
        "Assign, prioritise and remove guide sources.",
    ),
    Command::new(
        S::Epg,
        "updates",
        "epg.update_now",
        "Download guide data now.",
    ),
    Command::new(
        S::Epg,
        "storage",
        "epg.clear",
        "Delete stored guide data until the next update.",
    )
    .confirm(),
    Command::new(
        S::Logos,
        "",
        "logos.clear_cache",
        "Drop downloaded logo images.",
    ),
    Command::new(
        S::History,
        "history",
        "history.clear",
        "Forget watch history.",
    )
    .confirm(),
    Command::new(
        S::Search,
        "",
        "search.clear_history",
        "Forget earlier search queries.",
    )
    .confirm(),
    Command::new(
        S::Parental,
        "",
        "parental.change_pin",
        "Set or change the parental PIN.",
    ),
    Command::new(
        S::Remote,
        "mapping",
        "remote.customize",
        "Rebind keys per screen.",
    ),
    Command::new(
        S::Remote,
        "mapping",
        "remote.restore_defaults",
        "Drop every key customisation.",
    )
    .confirm()
    .resets(&["remote.key_overrides"]),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The table is built at compile time; this runs the builders to check
    /// that a new command just runs, and that each builder adds only its
    /// own part.
    #[test]
    fn a_new_command_runs_at_once_until_the_builders_add_to_it() {
        let plain = Command::new(S::Epg, "", "epg.x", "S.");
        assert_eq!(
            plain,
            Command {
                key: "epg.x",
                section: S::Epg,
                subsection: None,
                confirm: false,
                requires_restart: false,
                resets: &[],
                summary: "S.",
            }
        );
        assert_eq!(
            Command::new(S::Epg, "storage", "epg.x", "S.").subsection,
            Some("storage")
        );
        let confirm = plain.confirm();
        assert!(confirm.confirm && !confirm.requires_restart && confirm.resets.is_empty());
        let restart = plain.restart();
        assert!(restart.requires_restart && !restart.confirm);
        let resets = plain.resets(&["epg.keep_past"]);
        assert_eq!(resets.resets, ["epg.keep_past"]);
        assert!(!resets.confirm && !resets.requires_restart);
    }
}
