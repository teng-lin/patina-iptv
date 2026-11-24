//! Activate the running executable before a second owner opens storage.
use std::{
    ffi::c_void,
    path::{Path, PathBuf},
};
type Handle = *mut c_void;
#[link(name = "user32")]
unsafe extern "system" {
    fn EnumWindows(callback: unsafe extern "system" fn(Handle, isize) -> i32, data: isize) -> i32;
    fn GetWindowThreadProcessId(window: Handle, process: *mut u32) -> u32;
    fn GetWindowTextW(window: Handle, text: *mut u16, count: i32) -> i32;
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
    // SAFETY: EnumWindows supplied the HWND; process points to a live u32.
    unsafe { GetWindowThreadProcessId(window, &mut process) };
    if process == std::process::id() {
        return 1;
    }
    let mut title = [0u16; 128];
    // SAFETY: title is writable for the supplied UTF-16 character count. An
    // HWND destroyed during enumeration simply makes this Win32 call fail.
    let title_len = unsafe { GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32) };
    if title_len <= 0 || String::from_utf16_lossy(&title[..title_len as usize]) != "Patina" {
        return 1;
    }

    // SAFETY: query-only access, no inherited handle, and a numeric process ID.
    // Failure returns NULL and is handled before using or closing the handle.
    let handle = unsafe { OpenProcess(0x1000, 0, process) };
    if handle.is_null() {
        return 1;
    }
    let mut buffer = [0u16; 32768];
    let mut size = buffer.len() as u32;
    // SAFETY: handle is owned from OpenProcess; buffer and size remain writable
    // for the call, and size describes the allocated UTF-16 element count.
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
    // SAFETY: this owned handle is closed exactly once after the final query.
    unsafe { CloseHandle(handle) };
    if ok != 0
        && Path::new(&String::from_utf16_lossy(&buffer[..size as usize])) == search.executable
    {
        // SAFETY: both APIs accept the enumerated opaque HWND, retain no Rust
        // pointers, and return failure if the window has since disappeared.
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
    // SAFETY: enumeration and callbacks are synchronous. search outlives every
    // callback, its address fits LPARAM, and visit has the required system ABI.
    unsafe { EnumWindows(visit, &mut search as *mut Search as isize) };
    search.found
}
