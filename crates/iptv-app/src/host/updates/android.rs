//! Pass a verified, private APK to the direct distribution's Android activity.
use jni::{
    JavaVM,
    objects::{GlobalRef, JClass, JString, JValue},
};
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};
static HOST: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hantekllc_iptvrs_host_UpdateHost_initialize(
    env: jni::JNIEnv,
    class: JClass,
) {
    if let (Ok(vm), Ok(class)) = (env.get_java_vm(), env.new_global_ref(class)) {
        let _ = HOST.set((vm, class));
    }
}

fn with_host<T>(
    call: impl FnOnce(&mut jni::JNIEnv, &JClass) -> jni::errors::Result<T>,
) -> Option<T> {
    let (vm, class) = HOST.get()?;
    let mut env = vm.attach_current_thread().ok()?;
    let result = call(&mut env, class.as_obj().into());
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    result.ok()
}

pub(super) fn directory() -> Option<PathBuf> {
    let path: String = with_host(|env, class| {
        let path = env
            .call_static_method(class, "cacheDirectory", "()Ljava/lang/String;", &[])?
            .l()?;
        let path = JString::from(path);
        Ok(env.get_string(&path)?.into())
    })?;
    let path = PathBuf::from(path);
    path.is_absolute().then_some(path)
}

pub(super) fn handoff(path: &Path, code: u64) -> bool {
    let Some(path) = path.to_str() else {
        return false;
    };
    let Ok(code) = i64::try_from(code) else {
        return false;
    };
    with_host(|env, class| {
        let path = env.new_string(path)?;
        env.call_static_method(
            class,
            "open",
            "(Ljava/lang/String;J)Z",
            &[JValue::Object(&path), JValue::Long(code)],
        )?
        .z()
    })
    .unwrap_or(false)
}
