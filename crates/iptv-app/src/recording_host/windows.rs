//! Activate the running executable before a second owner opens storage.
use std::{ffi::c_void, path::PathBuf};
type Handle = *mut c_void;
#[link(name = "user32")]
unsafe extern "system" {
    fn EnumWindows(callback: unsafe extern "system" fn(Handle, isize) -> i32, data: isize) -> i32;
    fn GetWindowThreadProcessId(window: Handle, process: *mut u32) -> u32;
    fn ShowWindow(window: Handle, command: i32) -> i32;
    fn SetForegroundWindow(window: Handle) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, process: u32) -> Handle;
    fn QueryFullProcessImageNameW(
        process: Handle,
        flags: u32,
        path: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}
struct Search {
    executable: PathBuf,
    found: bool,
}
unsafe extern "system" fn visit(window: Handle, data: isize) -> i32 {
    // SAFETY: EnumWindows invokes synchronously while Search remains live.
    let search = unsafe { &mut *(data as *mut Search) };
    let mut process = 0;
    unsafe { GetWindowThreadProcessId(window, &mut process) };
    if process == std::process::id() {
        return 1;
    }
    let handle = unsafe { OpenProcess(0x1000, 0, process) };
    if handle.is_null() {
        return 1;
    }
    let mut buffer = [0u16; 32768];
    let mut size = buffer.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
    unsafe { CloseHandle(handle) };
    if ok != 0
        && PathBuf::from(String::from_utf16_lossy(&buffer[..size as usize])) == search.executable
    {
        unsafe {
            ShowWindow(window, 9);
            SetForegroundWindow(window)
        };
        search.found = true;
        return 0;
    }
    1
}
pub(crate) fn activate_existing() -> bool {
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    let mut search = Search {
        executable,
        found: false,
    };
    unsafe { EnumWindows(visit, &mut search as *mut Search as isize) };
    search.found
}
pub(super) fn schedule(next: i64) {
    let Ok(directory) = crate::platform_data::resolve_data_dir() else {
        return;
    };
    if !directory.join("recording-login.enabled").exists() {
        return;
    }
    std::thread::spawn(move || {
        let Ok(executable) = std::env::current_exe() else {
            return;
        };
        let at = chrono::DateTime::from_timestamp(next, 0).map(|at| at.to_rfc3339());
        let file = directory.join("recording-wake-task.xml");
        let xml = super::startup::task_xml(
            &executable.to_string_lossy(),
            at.as_deref().filter(|_| next > 0),
        );
        if std::fs::write(&file, xml).is_err() {
            return;
        }
        let _ = std::process::Command::new("schtasks.exe")
            .args(["/Create", "/F", "/TN", "Patina Recorder", "/XML"])
            .arg(&file)
            .output();
        let _ = std::fs::remove_file(file);
    });
}
