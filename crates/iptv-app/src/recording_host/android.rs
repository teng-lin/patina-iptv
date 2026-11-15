use jni::{
    JavaVM,
    objects::{GlobalRef, JClass, JString, JValue},
    sys::jlong,
};
use std::sync::OnceLock;
static HOST: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hantekllc_iptvrs_host_RecordingHost_initialize(
    mut env: jni::JNIEnv,
    class: JClass,
) {
    if let (Ok(vm), Ok(class)) = (env.get_java_vm(), env.new_global_ref(class)) {
        let _ = HOST.set((vm, class));
    }
}
pub(crate) fn schedule(next: i64, active: bool) {
    let Some((vm, class)) = HOST.get() else {
        return;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let class: &JClass = class.as_obj().into();
    let _ = env.call_static_method(
        class,
        "schedule",
        "(JZ)V",
        &[JValue::Long(next), JValue::Bool(active as u8)],
    );
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
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
        let mut ports = crate::runtime::Ports::new(directory, crate::credential_key::database_key);
        ports.background_recordings = true;
        ports.recording_schedule = schedule;
        ports.exclude_from_backup = crate::platform_data::exclude_from_backup;
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
