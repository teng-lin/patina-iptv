//! Keeps a desktop from idle sleep while it records (#275), without root:
//!
//! - macOS: an IOKit power assertion (`PreventUserIdleSystemSleep`), the
//!   one `caffeinate -i` takes.
//! - Linux: a logind `sleep:idle` block inhibitor (its file descriptor
//!   holds it), or, where logind is not reachable (a Flatpak sandbox), the
//!   XDG desktop portal's Inhibit request for suspend and idle.
//! - Windows: `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`
//!   on a thread of its own, which holds it until told to end.
//!
//! The recorder's schedule publication ([`super::recording_host::schedule`])
//! holds it while a capture runs and releases it when none does; the
//! process ending releases it too. None of these keeps the display on or
//! stops a laptop's lid from sleeping it; logind's lock may make a suspend
//! chosen from the menu ask first.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The last attempt to hold it failed: a capture runs while the computer
/// may sleep (`capture_limits`).
static FAILED: AtomicBool = AtomicBool::new(false);
static HELD: Mutex<Option<Hold>> = Mutex::new(None);

/// Holds the computer awake while `active` (a capture runs), and releases
/// it when not. Blocking: called on the recorder's publication worker.
pub(crate) fn set(active: bool) {
    let mut held = HELD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !active {
        if held.take().is_some() {
            log::info!("iptv recordings: the computer may sleep again");
        }
        FAILED.store(false, Ordering::SeqCst);
        return;
    }
    if held.is_some() {
        return;
    }
    match Hold::acquire() {
        Some(hold) => {
            log::info!("iptv recordings: keeping the computer awake while recording");
            *held = Some(hold);
            FAILED.store(false, Ordering::SeqCst);
        }
        None => {
            log::warn!("iptv recordings: the computer could not be kept awake while recording");
            FAILED.store(true, Ordering::SeqCst);
        }
    }
}

/// Whether a capture runs while the computer could not be kept awake.
pub(crate) fn failed() -> bool {
    FAILED.load(Ordering::SeqCst)
}

/// Headless scenarios never hold the test machine awake: a hold is
/// simulated, and `IPTV_TEST_KEEP_AWAKE=fail` makes it fail.
#[cfg(feature = "test-support")]
struct Hold;
#[cfg(feature = "test-support")]
impl Hold {
    fn acquire() -> Option<Self> {
        (std::env::var_os("IPTV_TEST_KEEP_AWAKE").as_deref() != Some("fail".as_ref()))
            .then_some(Hold)
    }
}

#[cfg(all(not(feature = "test-support"), target_os = "linux"))]
use linux::Hold;
#[cfg(all(not(feature = "test-support"), target_os = "linux"))]
mod linux {
    use std::time::Duration;

    use dbus::blocking::Connection;

    const TIMEOUT: Duration = Duration::from_secs(5);
    const WHY: &str = "Recording a TV program";

    pub(super) enum Hold {
        /// logind keeps the inhibitor while its descriptor is open.
        Logind(#[allow(dead_code)] dbus::arg::OwnedFd),
        /// The portal keeps it until its request is closed (or the
        /// connection that made it is).
        Portal(Connection, dbus::Path<'static>),
    }

    impl Hold {
        pub(super) fn acquire() -> Option<Self> {
            logind().or_else(portal)
        }
    }

    fn logind() -> Option<Hold> {
        let connection = Connection::new_system().ok()?;
        let manager =
            connection.with_proxy("org.freedesktop.login1", "/org/freedesktop/login1", TIMEOUT);
        let (fd,): (dbus::arg::OwnedFd,) = manager
            .method_call(
                "org.freedesktop.login1.Manager",
                "Inhibit",
                ("sleep:idle", "Patina", WHY, "block"),
            )
            .ok()?;
        Some(Hold::Logind(fd))
    }

    fn portal() -> Option<Hold> {
        use dbus::arg::{PropMap, Variant};
        let connection = Connection::new_session().ok()?;
        let desktop = connection.with_proxy(
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            TIMEOUT,
        );
        // Suspend (4) and idle (8).
        let mut options = PropMap::new();
        options.insert("reason".into(), Variant(Box::new(WHY.to_owned())));
        let (handle,): (dbus::Path<'static>,) = desktop
            .method_call(
                "org.freedesktop.portal.Inhibit",
                "Inhibit",
                ("", 12u32, options),
            )
            .ok()?;
        Some(Hold::Portal(connection, handle))
    }

    impl Drop for Hold {
        fn drop(&mut self) {
            if let Hold::Portal(connection, handle) = self {
                let request = connection.with_proxy(
                    "org.freedesktop.portal.Desktop",
                    handle.clone(),
                    TIMEOUT,
                );
                let _: Result<(), _> =
                    request.method_call("org.freedesktop.portal.Request", "Close", ());
            }
        }
    }
}

#[cfg(all(not(feature = "test-support"), target_os = "macos"))]
use macos::Hold;
#[cfg(all(not(feature = "test-support"), target_os = "macos"))]
mod macos {
    use std::ffi::{c_char, c_void};

    type CFStringRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    /// `kIOPMAssertionLevelOn`.
    const ON: u32 = 255;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            text: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(object: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            kind: CFStringRef,
            level: u32,
            name: CFStringRef,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    pub(super) struct Hold(u32);

    fn string(text: &std::ffi::CStr) -> Option<CFStringRef> {
        // SAFETY: `text` is NUL-terminated; a null result is checked.
        let made = unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) };
        (!made.is_null()).then_some(made)
    }

    impl Hold {
        pub(super) fn acquire() -> Option<Self> {
            let kind = string(c"PreventUserIdleSystemSleep")?;
            let Some(name) = string(c"Patina is recording a TV program") else {
                // SAFETY: `kind` was created above and is released once.
                unsafe { CFRelease(kind) };
                return None;
            };
            let mut id = 0u32;
            // SAFETY: both strings are valid CFStrings; `id` is written only
            // on success. The strings are released after the call, which
            // keeps its own references.
            let result = unsafe { IOPMAssertionCreateWithName(kind, ON, name, &mut id) };
            // SAFETY: both strings were created above, are owned here and
            // are released once; the assertion keeps its own references.
            unsafe {
                CFRelease(name);
                CFRelease(kind);
            }
            (result == 0).then_some(Hold(id))
        }
    }

    impl Drop for Hold {
        fn drop(&mut self) {
            // SAFETY: the assertion was created by `acquire` and is released once.
            unsafe { IOPMAssertionRelease(self.0) };
        }
    }
}

#[cfg(all(not(feature = "test-support"), windows))]
use windows::Hold;
#[cfg(all(not(feature = "test-support"), windows))]
mod windows {
    use std::sync::mpsc;

    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }

    /// The thread holding the state: it ends, and the state with it, when
    /// its sender drops.
    pub(super) struct Hold(#[allow(dead_code)] mpsc::Sender<()>);

    impl Hold {
        pub(super) fn acquire() -> Option<Self> {
            let (release, released) = mpsc::channel::<()>();
            let (held, answer) = mpsc::channel();
            std::thread::Builder::new()
                .name("keep-awake".into())
                .spawn(move || {
                    // SAFETY: a plain Win32 call with constant flags.
                    let ok =
                        unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) } != 0;
                    let _ = held.send(ok);
                    if ok {
                        // Until the sender drops.
                        let _ = released.recv();
                        // SAFETY: as above; clears this thread's requirement.
                        unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
                    }
                })
                .ok()?;
            answer.recv().ok()?.then_some(Hold(release))
        }
    }
}

/// Other native hosts (the iPad, Android) never call [`set`] with a
/// capture; nothing to hold.
#[cfg(all(
    not(feature = "test-support"),
    not(any(target_os = "linux", target_os = "macos", windows))
))]
struct Hold;
#[cfg(all(
    not(feature = "test-support"),
    not(any(target_os = "linux", target_os = "macos", windows))
))]
impl Hold {
    fn acquire() -> Option<Self> {
        None
    }
}
