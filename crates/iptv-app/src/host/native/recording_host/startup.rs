//! The login and wake registrations earlier versions made for scheduled
//! recording. Scheduled recording is retired, so launches now remove them
//! ([`retire`]); nothing registers them any more.
use std::{
    io,
    path::{Path, PathBuf},
};

#[cfg(windows)]
fn remove_windows_task(task: &str) -> io::Result<()> {
    // Task Scheduler reports an absent task as HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND).
    // Do not infer success from localized schtasks output or suppress other COM errors.
    const REMOVE: &str = r#"
$ErrorActionPreference = 'Stop'
try {
    $scheduler = New-Object -ComObject 'Schedule.Service'
    $scheduler.Connect()
    $folder = $scheduler.GetFolder('\')
    try {
        $folder.DeleteTask($env:PATINA_RECORDING_TASK, 0)
    } catch {
        $failure = $_.Exception
        while ($null -ne $failure.InnerException) { $failure = $failure.InnerException }
        if ($failure.HResult -ne -2147024894) { throw }
    }
    exit 0
} catch { exit 1 }
"#;
    let result = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            REMOVE,
        ])
        // Pass the name as data, never interpolate it into the PowerShell program.
        .env("PATINA_RECORDING_TASK", task)
        .output()?;
    if result.status.success() {
        Ok(())
    } else {
        Err(io::ErrorKind::PermissionDenied.into())
    }
}

/// Written in the data directory once every registration below is gone.
pub(super) const RETIRED: &str = "recording-registrations.retired";
/// Earlier versions' marker that start at login was enabled; their Windows
/// builds re-registered the wake task on each schedule publication while it
/// existed.
const ENABLED: &str = "recording-login.enabled";
#[cfg_attr(not(windows), allow(dead_code))]
const WINDOWS_TASK: &str = "Patina Recorder";

/// Where earlier versions registered Patina to start at login or wake.
#[derive(Debug, Default)]
pub(super) struct Registrations {
    /// The Linux autostart entry.
    autostart: Option<PathBuf>,
    /// The entry under `~/.config` when `XDG_CONFIG_HOME` names another
    /// directory: an earlier install may have run without it.
    default_autostart: Option<PathBuf>,
    /// The macOS LaunchAgent.
    launch_agent: Option<PathBuf>,
    /// The Windows task, with its logon and time triggers and wake flag.
    windows_task: bool,
}

impl Registrations {
    /// The registrations of the signed-in user.
    #[cfg(not(any(test, feature = "test-support")))]
    pub(super) fn current() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute());
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|config| config.is_absolute())
            .or_else(|| home.as_ref().map(|home| home.join(".config")));
        let entry = |config: PathBuf| config.join("autostart/patina-recorder.desktop");
        let linux = cfg!(target_os = "linux");
        let default_autostart = home
            .as_ref()
            .map(|home| home.join(".config"))
            .filter(|default| linux && config.as_ref() != Some(default))
            .map(entry);
        Self {
            autostart: config.filter(|_| linux).map(entry),
            default_autostart,
            launch_agent: home
                .filter(|_| cfg!(target_os = "macos"))
                .map(|home| home.join("Library/LaunchAgents/com.hantekllc.patina.recorder.plist")),
            windows_task: cfg!(windows),
        }
    }
    /// Test builds never touch the real user's login items or tasks: only
    /// the entries under `IPTV_RS_TEST_REGISTRATIONS`, a scenario's
    /// temporary directory, and no Windows task.
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn current() -> Self {
        std::env::var_os("IPTV_RS_TEST_REGISTRATIONS")
            .map(PathBuf::from)
            .filter(|root| root.is_absolute())
            .map_or_else(Self::default, |root| Self::under(&root))
    }
    /// Both desktop file locations under `root`, whatever this system is.
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn under(root: &Path) -> Self {
        Self {
            autostart: Some(root.join("autostart/patina-recorder.desktop")),
            default_autostart: None,
            launch_agent: Some(root.join("LaunchAgents/com.hantekllc.patina.recorder.plist")),
            windows_task: false,
        }
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Scheduled recording is retired: removes every login and wake
/// registration an earlier version made, then the start-at-login marker,
/// and records that in `directory`. Removing what is absent succeeds. On an
/// error the record is not written, so the next launch tries again.
pub(super) fn retire(directory: &Path, registrations: &Registrations) -> io::Result<()> {
    let mut result = Ok(());
    for path in [
        &registrations.autostart,
        &registrations.default_autostart,
        &registrations.launch_agent,
    ]
    .into_iter()
    .flatten()
    {
        result = result.and(remove_if_present(path));
    }
    if registrations.windows_task {
        #[cfg(windows)]
        {
            result = result.and(remove_windows_task(WINDOWS_TASK));
        }
    }
    // Leftovers of an interrupted registration.
    for name in ["recording-task.xml", "recording-wake-task.xml"] {
        let _ = remove_if_present(&directory.join(name));
    }
    // Without the marker no later publication registers the task again,
    // even while removing it failed.
    result = result.and(remove_if_present(&directory.join(ENABLED)));
    result?;
    std::fs::write(directory.join(RETIRED), b"1")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn removing_an_absent_synthetic_task_is_idempotent() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let task = format!("Patina Synthetic Missing {} {unique}", std::process::id());
        remove_windows_task(&task).unwrap();
        remove_windows_task(&task).unwrap();
    }
    /// Every location lives in a temporary directory: tests never touch the
    /// real user's login items.
    #[test]
    fn retiring_removes_each_registration_and_the_marker_once_and_idempotently() {
        let registrations = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let entries = Registrations::under(registrations.path());
        for path in [&entries.autostart, &entries.launch_agent]
            .into_iter()
            .flatten()
        {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "generated registration").unwrap();
        }
        std::fs::write(data.path().join(ENABLED), b"1").unwrap();
        std::fs::write(data.path().join("recording-wake-task.xml"), "generated").unwrap();
        retire(data.path(), &entries).unwrap();
        assert!(!entries.autostart.as_ref().unwrap().exists());
        assert!(!entries.launch_agent.as_ref().unwrap().exists());
        assert!(!data.path().join(ENABLED).exists());
        assert!(!data.path().join("recording-wake-task.xml").exists());
        assert!(data.path().join(RETIRED).exists());
        // Nothing left to remove is still success.
        retire(data.path(), &entries).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_removal_is_not_recorded_so_the_next_launch_retries() {
        use std::os::unix::fs::PermissionsExt;
        let registrations = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let entries = Registrations::under(registrations.path());
        let autostart = entries.autostart.clone().unwrap();
        let folder = autostart.parent().unwrap();
        std::fs::create_dir_all(folder).unwrap();
        std::fs::write(&autostart, "generated registration").unwrap();
        std::fs::write(data.path().join(ENABLED), b"1").unwrap();
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o500)).unwrap();
        let denied = std::fs::write(folder.join("probe"), b"").is_err();
        let failed = retire(data.path(), &entries);
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        if !denied {
            // Running as a user that ignores permissions: nothing to observe.
            return;
        }
        assert!(failed.is_err());
        assert!(!data.path().join(RETIRED).exists());
        // The wake marker goes regardless, so nothing registers again.
        assert!(!data.path().join(ENABLED).exists());
        retire(data.path(), &entries).unwrap();
        assert!(!autostart.exists());
        assert!(data.path().join(RETIRED).exists());
    }
}
