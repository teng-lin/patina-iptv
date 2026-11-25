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

/// Whether a capture is running or due. While one is, `RecordingHost`
/// keeps the sticky foreground service running, which Android restarts if
/// it killed the process, and before Android 12 also arms a 120-second
/// watchdog alarm; otherwise it disarms the alarm and lets the service stop
/// (#275).
pub(crate) fn schedule(active: bool) {
    let Some((vm, class)) = HOST.get() else {
        return;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let class: &JClass = class.as_obj().into();
    let _ = env.call_static_method(class, "schedule", "(Z)V", &[JValue::Bool(active as u8)]);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}

/// Reports an incomplete capture to the Android host without its metadata.
pub(crate) fn notify_failed(reason: crate::runtime::RecordingFailure) {
    let Some((vm, class)) = HOST.get() else {
        return;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let class: &JClass = class.as_obj().into();
    let code = match reason {
        crate::runtime::RecordingFailure::Unexpected => 0,
        crate::runtime::RecordingFailure::TvStandby => 1,
    };
    let _ = env.call_static_method(class, "notifyFailed", "(I)V", &[JValue::Int(code)]);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}

/// Checks TV standby even on a cold restart, before opening any input.
pub(crate) fn standby_blocks_recording() -> bool {
    let Some((vm, class)) = HOST.get() else {
        return false;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return false;
    };
    let class: &JClass = class.as_obj().into();
    let result = env
        .call_static_method(class, "standbyBlocksRecording", "()Z", &[])
        .and_then(|value| value.z());
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    result.unwrap_or(false)
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
        ports.recording_failed = notify_failed;
        ports.recording_standby = Some(standby_blocks_recording);
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
/// Power Standby; 2, Android's background time limit. Returns how many
/// newly started closing.
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
