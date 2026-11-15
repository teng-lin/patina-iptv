//! User-scoped, opt-in login integration; never install from a background tick.
use std::{io, path::Path};
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
        let task = "Patina Recorder";
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
            let result = std::process::Command::new("schtasks.exe")
                .args(["/Delete", "/F", "/TN", task])
                .output()?;
            if !result.status.success() {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
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
