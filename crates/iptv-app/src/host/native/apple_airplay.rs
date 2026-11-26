//! An NSH native view for the isolated Apple AirPlay validation build.
use std::{
    ffi::{CString, c_char, c_void},
    rc::Rc,
};

#[cfg(target_os = "ios")]
use nsh_platform_ios::{ComponentContext, Components, NativeComponent};
#[cfg(target_os = "macos")]
use nsh_platform_macos::{
    ComponentContext, Components, NativeComponent, objc2::rc::Retained,
    objc2_app_kit::NSView as View,
};
#[cfg(target_os = "ios")]
use objc2::rc::Retained;
#[cfg(target_os = "ios")]
use objc2_ui_kit::UIView as View;
use serde_json::{Value, json};

unsafe extern "C" {
    fn patina_airplay_create(
        context: *const c_void,
        emit: extern "C" fn(*const c_void, bool, u8),
    ) -> *mut c_void;
    fn patina_airplay_source(view: *const c_void, source: *const c_char);
    fn patina_airplay_command(view: *const c_void, action: u8);
    fn patina_airplay_invalidate(view: *const c_void);
}

pub(super) fn register(components: &Components) {
    components.register("tv.airplay", Rc::new(|ctx| Box::new(Picker::new(ctx))));
}

struct Picker {
    view: Retained<View>,
    // Remains at a fixed address until the native callback is disconnected in Drop.
    _context: Box<ComponentContext>,
    last_command: Option<u64>,
}

extern "C" fn emit(context: *const c_void, external: bool, status: u8) {
    let status = match status {
        0 => "idle",
        1 => "waiting",
        2 => "playing",
        3 => "paused",
        4 => "failed",
        5 => "route-lost",
        _ => return,
    };
    // SAFETY: only this picker's main-thread callbacks use the boxed context. Drop
    // clears the callback before freeing its context, including for queued updates.
    let context = unsafe { &*context.cast::<ComponentContext>() };
    context.emit("state", json!({ "external": external, "status": status }));
}

impl Picker {
    fn new(context: ComponentContext) -> Self {
        let context = Box::new(context);
        // SAFETY: NSH makes native components on the main thread. The constructor
        // returns a +1 Apple view of the appropriate platform type.
        let view = unsafe {
            Retained::from_raw(patina_airplay_create((&raw const *context).cast(), emit).cast())
        }
        .expect("allocate AirPlay probe view");
        Self {
            view,
            _context: context,
            last_command: None,
        }
    }
}

impl NativeComponent for Picker {
    #[cfg(target_os = "ios")]
    fn view(&self) -> &View {
        &self.view
    }
    #[cfg(target_os = "macos")]
    fn view(&self) -> Retained<View> {
        self.view.clone()
    }

    fn set_prop(&mut self, name: &str, value: Option<&Value>) {
        match name {
            "source" => {
                let source = value
                    .and_then(Value::as_str)
                    .and_then(|s| CString::new(s).ok());
                // SAFETY: the view is live; native code copies a non-null string in this call.
                unsafe {
                    patina_airplay_source(
                        Retained::as_ptr(&self.view).cast(),
                        source.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                    );
                }
            }
            "command" => {
                let Some(cmd) = value else { return };
                let Some(serial) = cmd["serial"].as_u64() else {
                    return;
                };
                if self.last_command.is_some_and(|last| serial <= last) {
                    return;
                }
                let action = match cmd["action"].as_str() {
                    Some("play") => 0,
                    Some("pause") => 1,
                    Some("stop") => 2,
                    _ => return,
                };
                self.last_command = Some(serial);
                // SAFETY: NSH updates this retained view's properties on the main thread.
                unsafe { patina_airplay_command(Retained::as_ptr(&self.view).cast(), action) };
            }
            _ => {}
        }
    }
}

impl Drop for Picker {
    fn drop(&mut self) {
        // SAFETY: NSH destroys components on the main thread while their view and
        // callback context are still owned by the component.
        unsafe { patina_airplay_invalidate(Retained::as_ptr(&self.view).cast()) };
    }
}
