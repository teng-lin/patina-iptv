//! Commands: entries of the settings tree that act instead of storing a
//! value (back up, clear, restore defaults). They share the key and label
//! space with settings but are never persisted.

use super::key;
use crate::schema::Section;

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
        key("Write settings and customizations to a backup file."),
    ),
    Command::new(
        S::General,
        "",
        "general.restore",
        key("Replace everything with a backup's contents."),
    )
    .confirm()
    .restart(),
    Command::new(
        S::General,
        "",
        "general.reset_watch_time",
        key("Forget per-channel viewing time."),
    )
    .confirm(),
    Command::new(
        S::General,
        "",
        "general.restart",
        key("Restart the app now."),
    )
    .confirm(),
    Command::new(
        S::General,
        "privacy",
        "general.send_logs",
        key("Send a diagnostic log, redacted of credentials."),
    ),
    Command::new(
        S::General,
        "updates",
        "general.check_updates",
        key("Look for a newer version now."),
    ),
    Command::new(
        S::General,
        "",
        "general.about",
        key("Version, licenses and policies."),
    ),
    Command::new(
        S::About,
        "",
        "about.privacy_policy",
        key("Open the privacy policy in your browser."),
    ),
    Command::new(
        S::Playlists,
        "",
        "playlists.manage",
        key(
            "Open the playlists screen: add, choose, refresh, rename, change the guide address, delete.",
        ),
    ),
    Command::new(
        S::Playlists,
        "",
        "playlists.add",
        key("Open the add-playlist wizard."),
    ),
    Command::new(
        S::Playlists,
        "",
        "playlists.reorder",
        key("Arrange playlists by hand."),
    ),
    Command::new(
        S::Playlists,
        "updates",
        "playlists.update_all",
        key("Refresh every playlist now."),
    ),
    Command::new(
        S::Channels,
        "",
        "channels.manage_groups",
        key("Show, hide, rename and arrange groups."),
    ),
    Command::new(
        S::Epg,
        "",
        "epg.add_source",
        key("Add an XMLTV guide source."),
    ),
    Command::new(
        S::Epg,
        "",
        "epg.manage_sources",
        key("Assign, prioritize and remove guide sources."),
    ),
    Command::new(
        S::Epg,
        "updates",
        "epg.update_now",
        key("Download guide data now."),
    ),
    Command::new(
        S::Epg,
        "storage",
        "epg.clear",
        key("Delete stored guide data until the next update."),
    )
    .confirm(),
    Command::new(
        S::Logos,
        "",
        "logos.clear_cache",
        key("Drop downloaded logo images."),
    ),
    Command::new(
        S::History,
        "history",
        "history.clear",
        key("Forget watch history."),
    )
    .confirm(),
    Command::new(
        S::Search,
        "",
        "search.clear_history",
        key("Forget earlier search queries."),
    )
    .confirm(),
    Command::new(
        S::Parental,
        "",
        "parental.change_pin",
        key("Set or change the parental PIN."),
    ),
    Command::new(
        S::Remote,
        "mapping",
        "remote.customize",
        key("Rebind keys per screen."),
    ),
    Command::new(
        S::Remote,
        "mapping",
        "remote.restore_defaults",
        key("Drop every key customization."),
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
