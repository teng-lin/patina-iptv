use jni::{
    JavaVM,
    objects::{GlobalRef, JClass, JString, JValue},
    sys::jlong,
};
use std::sync::OnceLock;
static HOST: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hantekllc_iptvrs_host_RecordingHost_initialize(
    env: jni::JNIEnv,
    class: JClass,
) {
    if let (Ok(vm), Ok(class)) = (env.get_java_vm(), env.new_global_ref(class)) {
        let _ = HOST.set((vm, class));
    }
}
pub(crate) fn set_language(tag: &str) {
    let Some((vm, class)) = HOST.get() else {
        return;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let Ok(tag) = env.new_string(tag) else {
        return;
    };
    let class: &JClass = class.as_obj().into();
    let _ = env.call_static_method(
        class,
        "setLanguage",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&tag)],
    );
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}

pub(crate) fn schedule(next: i64, active: bool) -> iptv_core::recording::WakeCapability {
    use iptv_core::recording::WakeCapability;
    let Some((vm, class)) = HOST.get() else {
        return WakeCapability::Denied;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return WakeCapability::Denied;
    };
    let class: &JClass = class.as_obj().into();
    // Scheduled recording is retired: the alarm is only the 120-second
    // watchdog that restarts an active capture's service if Android killed
    // it (`RecordingHost.schedule`). With no capture running, nothing is
    // armed, whatever a carried-over job asks for (#275).
    let next = if active { next } else { 0 };
    let result = env
        .call_static_method(
            class,
            "schedule",
            "(JZ)I",
            &[JValue::Long(next), JValue::Bool(active as u8)],
        )
        .and_then(|value| value.i());
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
        return WakeCapability::Denied;
    }
    match result {
        Ok(1) => WakeCapability::Exact,
        Ok(2) => WakeCapability::Inexact,
        _ => WakeCapability::Denied,
    }
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hantekllc_iptvrs_host_RecordingHost_start(
    mut env: jni::JNIEnv,
    _class: JClass,
    path: JString,
) -> jlong {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let directory = std::path::PathBuf::from(String::from(env.get_string(&path).ok()?));
        if !directory.is_absolute() {
            return None;
        }
        let mut ports = crate::runtime::Ports::new(
            directory,
            crate::host::native::credential_key::database_key,
        );
        ports.background_recordings = true;
        ports.recording_schedule = schedule;
        ports.exclude_from_backup = crate::host::native::platform_data::exclude_from_backup;
        crate::runtime::recordings::host::start_background_host(ports).ok()?;
        Some(1)
    }));
    result.ok().flatten().unwrap_or(0)
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hantekllc_iptvrs_host_RecordingHost_stop(
    mut env: jni::JNIEnv,
    _class: JClass,
    path: JString,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Ok(path) = env.get_string(&path) {
            crate::runtime::recordings::host::stop_background_host(std::path::Path::new(
                &String::from(path),
            ));
        }
    }));
}

/// Stops every running capture early (#275), each kept as a partial
/// recording with the reason: `why` 1, the TV was turned off under Low
/// Power Standby; 2, Android's background time limit. Returns how many were
/// running.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hantekllc_iptvrs_host_RecordingHost_stopCaptures(
    mut env: jni::JNIEnv,
    _class: JClass,
    path: JString,
    why: jni::sys::jint,
) -> jni::sys::jint {
    use crate::runtime::recordings::host::{EarlyStop, stop_captures};
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let why = match why {
            1 => EarlyStop::TvOff,
            2 => EarlyStop::BackgroundLimit,
            _ => return 0,
        };
        let Ok(path) = env.get_string(&path) else {
            return 0;
        };
        stop_captures(std::path::Path::new(&String::from(path)), why) as jni::sys::jint
    }))
    .unwrap_or(0)
}

pub(super) fn request_alarm_access() {
    let Some((vm, class)) = HOST.get() else {
        return;
    };
    if let Ok(mut env) = vm.attach_current_thread() {
        let class: &JClass = class.as_obj().into();
        let _ = env.call_static_method(class, "requestAlarmAccess", "()V", &[]);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hantekllc_iptvrs_host_RecordingHost_suspendViewing(
    mut env: jni::JNIEnv,
    _class: JClass,
    path: JString,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Ok(path) = env.get_string(&path) {
            crate::runtime::recordings::host::suspend_viewing(std::path::Path::new(&String::from(
                path,
            )));
        }
    }));
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hantekllc_iptvrs_host_RecordingHost_resumeViewing(
    mut env: jni::JNIEnv,
    _class: JClass,
    path: JString,
) -> jni::sys::jboolean {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(path) = env.get_string(&path) else {
            return false;
        };
        crate::runtime::recordings::host::resume_viewing(std::path::Path::new(&String::from(path)))
    }))
    .unwrap_or(false) as u8
}
