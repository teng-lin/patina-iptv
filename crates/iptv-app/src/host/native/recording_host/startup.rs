//! User-scoped, opt-in login integration; never install from a background tick.
//! Scheduled recording is retired, so launches now remove what earlier
//! versions registered ([`retire`]).
use std::{
    io,
    path::{Path, PathBuf},
};
#[cfg(any(windows, target_os = "macos", test))]
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
#[cfg(target_os = "linux")]
fn desktop_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('%', "%%")
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$")
    )
}
#[cfg(target_os = "linux")]
fn linux_executable(
    image: Option<&Path>,
    fallback: std::path::PathBuf,
    usable: impl FnOnce(&Path) -> bool,
) -> std::path::PathBuf {
    image
        .filter(|path| path.is_absolute() && usable(path))
        .map(Path::to_owned)
        .unwrap_or(fallback)
}

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
/// Earlier versions' marker that start at login was enabled. Windows
/// re-registers its wake task on each schedule publication while it exists.
const ENABLED: &str = "recording-login.enabled";
#[cfg_attr(not(windows), allow(dead_code))]
const WINDOWS_TASK: &str = "Patina Recorder";

/// Where earlier versions registered Patina to start at login or wake.
#[derive(Debug, Default)]
pub(super) struct Registrations {
    /// The Linux autostart entry.
    autostart: Option<PathBuf>,
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
        Self {
            autostart: config
                .filter(|_| cfg!(target_os = "linux"))
                .map(|config| config.join("autostart/patina-recorder.desktop")),
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
    #[cfg(windows)]
    let _tasks = super::windows::TASKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut result = Ok(());
    for path in [&registrations.autostart, &registrations.launch_agent]
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

pub(super) fn install(directory: &Path, enabled: bool) -> io::Result<()> {
    #[cfg(windows)]
    let _tasks = super::windows::TASKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let executable = std::env::current_exe()?;
    #[cfg(target_os = "linux")]
    let executable = linux_executable(
        std::env::var_os("APPIMAGE").as_deref().map(Path::new),
        executable,
        |path| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        },
    );
    #[cfg(target_os = "linux")]
    {
        let root = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
            })
            .ok_or(io::ErrorKind::NotFound)?;
        let path = root.join("autostart/patina-recorder.desktop");
        if enabled {
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(
                path,
                format!(
                    "[Desktop Entry]\nType=Application\nName=Patina Recorder\nExec={} --background-recordings\nTerminal=false\nNoDisplay=true\n",
                    desktop_quote(&executable.to_string_lossy())
                ),
            )?;
        } else if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").ok_or(io::ErrorKind::NotFound)?;
        let path = std::path::PathBuf::from(home)
            .join("Library/LaunchAgents/com.hantekllc.patina.recorder.plist");
        if enabled {
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(
                path,
                format!(
                    "<?xml version=\"1.0\"?>\
                    <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\
                    <plist version=\"1.0\"><dict><key>Label</key><string>com.hantekllc.patina.recorder</string>\
                    <key>ProgramArguments</key><array><string>{}</string><string>--background-recordings</string></array>\
                    <key>RunAtLoad</key><true/></dict></plist>",
                    xml(&executable.to_string_lossy())
                ),
            )?;
        } else if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    #[cfg(windows)]
    {
        let task = WINDOWS_TASK;
        if enabled {
            let file = directory.join("recording-task.xml");
            std::fs::write(&file, task_xml(&executable.to_string_lossy(), None))?;
            let result = std::process::Command::new("schtasks.exe")
                .args(["/Create", "/F", "/TN", task, "/XML"])
                .arg(&file)
                .output()?;
            let _ = std::fs::remove_file(file);
            if !result.status.success() {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
        } else {
            remove_windows_task(task)?;
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = executable;
        return Err(io::ErrorKind::Unsupported.into());
    }
    let marker = directory.join("recording-login.enabled");
    if enabled {
        std::fs::write(marker, b"1")?;
    } else if marker.exists() {
        std::fs::remove_file(marker)?;
    }
    Ok(())
}
#[cfg(any(windows, test))]
pub(super) fn task_xml(executable: &str, next: Option<&str>) -> String {
    let time = next.map_or(String::new(), |at| {
        format!(
            "<TimeTrigger><StartBoundary>{}</StartBoundary><Enabled>true</Enabled></TimeTrigger>",
            xml(at)
        )
    });
    format!(
        "<?xml version=\"1.0\"?><Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\"><Triggers>\
                    <LogonTrigger><Enabled>true</Enabled></LogonTrigger>{time}</Triggers><Principals><Principal id=\"Author\">\
                    <LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals><Settings>\
                    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\
                    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\
                    <StartWhenAvailable>true</StartWhenAvailable><WakeToRun>true</WakeToRun>\
                    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit></Settings><Actions Context=\"Author\"><Exec><Command>{}</Command>\
                    <Arguments>--background-recordings</Arguments></Exec></Actions></Task>",
        xml(executable)
    )
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
    #[cfg(target_os = "linux")]
    #[test]
    fn image_startup_uses_a_stable_absolute_executable_and_escapes_exec_fields() {
        let mounted = std::path::PathBuf::from("/tmp/.mount_generated/usr/bin/patina");
        let image = Path::new("/home/generated/Patina 100%.AppImage");
        assert_eq!(
            linux_executable(Some(image), mounted.clone(), |path| path == image),
            image
        );
        assert_eq!(
            linux_executable(
                Some(Path::new("relative.AppImage")),
                mounted.clone(),
                |_| true
            ),
            mounted
        );
        assert_eq!(
            linux_executable(Some(image), mounted.clone(), |_| false),
            mounted
        );
        assert_eq!(linux_executable(None, mounted.clone(), |_| true), mounted);
        assert_eq!(
            desktop_quote("/home/generated/Patina 100%.AppImage"),
            "\"/home/generated/Patina 100%%.AppImage\""
        );
        assert_eq!(
            desktop_quote("/home/generated/$literal`name\".AppImage"),
            "\"/home/generated/\\$literal\\`name\\\".AppImage\""
        );
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

    #[test]
    fn startup_task_has_a_bounded_identity_and_escaped_executable() {
        let task = task_xml(
            "C:\\Generated & Test\\patina.exe",
            Some("2030-01-01T12:00:00Z"),
        );
        assert!(task.contains("Generated &amp; Test"));
        assert!(task.contains("<WakeToRun>true</WakeToRun>"));
        assert!(task.contains("<StartBoundary>2030-01-01T12:00:00Z</StartBoundary>"));
        assert!(task.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
    }
}
