//! The persistent native route picker and its runtime-owned AVPlayer mailbox.
mod transport;
use std::{
    cell::{Cell, RefCell},
    ffi::{CString, c_char, c_void},
    rc::Rc,
    sync::Arc,
};
pub(super) use transport::Sender;

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
        poll: extern "C" fn(*const c_void),
    ) -> *mut c_void;
    fn patina_airplay_source(view: *const c_void, source: *const c_char);
    fn patina_airplay_command(view: *const c_void, action: u8);
    fn patina_airplay_remote(view: *const c_void, command: u8, value: i16);
    fn patina_airplay_disconnect(view: *const c_void);
    fn patina_airplay_invalidate(view: *const c_void);
    fn patina_airplay_owner(view: *const c_void, owner: u64);
    fn patina_cast_button_create() -> *mut c_void;
    fn patina_cast_button_owner(view: *const c_void, owner: u64);
    fn patina_cast_button_accent(view: *const c_void, rgb: u32);
}

pub(super) fn register(components: &Components) {
    components.register("tv.airplay", Rc::new(|ctx| Box::new(Picker::new(ctx))));
    components.register("tv.cast-picker", Rc::new(|_| Box::new(RouteButton::new())));
}

struct Picker {
    view: Retained<View>,
    // Remains at a fixed address until the native callback is disconnected in Drop.
    _context: Box<Context>,
    last_command: Option<u64>,
}

struct Context {
    component: ComponentContext,
    bridge: RefCell<Option<Arc<transport::Bridge>>>,
    generation: Cell<u64>,
    owner: Cell<Option<u64>>,
    view: Cell<*const c_void>,
}

extern "C" fn poll(context: *const c_void) {
    // SAFETY: the native timer is main-thread-only and invalidated before Drop.
    let context = unsafe { &*context.cast::<Context>() };
    let Some(bridge) = context.bridge.borrow().clone() else {
        return;
    };
    for message in bridge.drain() {
        use iptv_core::cast::Command;
        let view = context.view.get();
        match message {
            transport::Message::Prepare { generation, url } if bridge.accepts(generation) => {
                // SAFETY: the retained view is live on main. Retire the old item
                // before admitting callbacks for the new generation.
                unsafe { patina_airplay_command(view, 2) };
                context.generation.set(generation);
                if let Ok(url) = CString::new(url) {
                    // SAFETY: this retained view and copied string remain live.
                    unsafe {
                        patina_airplay_source(view, url.as_ptr());
                        patina_airplay_command(view, 0);
                    }
                }
            }
            transport::Message::Command {
                generation,
                command,
                reply,
            } if bridge.accepts(generation) => {
                let (action, value) = match command {
                    Command::Play => (0, 0),
                    Command::Pause => (1, 0),
                    Command::Stop => (2, 0),
                    Command::Volume(value) => (3, i16::from(value)),
                    Command::StepVolume(value) => (4, i16::from(value)),
                    Command::ToggleMute => (5, 0),
                };
                // SAFETY: the mailbox is consumed on the native view's main thread.
                unsafe {
                    patina_airplay_remote(view, action, value);
                }
                let _ = reply.send(());
            }
            transport::Message::Stop {
                generation,
                disconnect,
            } if bridge.current(generation) => {
                // SAFETY: only this runtime's player is stopped/disconnected.
                unsafe {
                    if disconnect {
                        patina_airplay_disconnect(view);
                    } else {
                        patina_airplay_command(view, 2);
                    }
                }
            }
            transport::Message::Flushed(reply) => {
                let _ = reply.send(());
            }
            _ => {}
        }
    }
}

extern "C" fn emit(context: *const c_void, external: bool, status: u8) {
    // SAFETY: native callbacks are detached before this boxed context is freed.
    let owner = unsafe { &*context.cast::<Context>() };
    if let Some(name) = match status {
        6 => Some("routes-open"),
        7 => Some("routes-closed"),
        8 => Some("route-selected"),
        9 => Some("local-selected"),
        _ => None,
    } {
        owner.component.emit(name, json!({}));
        return;
    }
    if let Some(bridge) = owner.bridge.borrow().as_ref() {
        bridge.publish(owner.generation.get(), external, status);
    }
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
    let context = &owner.component;
    context.emit("state", json!({ "external": external, "status": status }));
}

impl Picker {
    fn new(context: ComponentContext) -> Self {
        let context = Box::new(Context {
            component: context,
            bridge: RefCell::new(None),
            generation: Cell::new(0),
            owner: Cell::new(None),
            view: Cell::new(std::ptr::null()),
        });
        // SAFETY: NSH makes native components on the main thread. The constructor
        // returns a +1 Apple view of the appropriate platform type.
        let view: Retained<View> = unsafe {
            Retained::from_raw(
                patina_airplay_create((&raw const *context).cast(), emit, poll).cast(),
            )
        }
        .expect("allocate AirPlay view");
        context.view.set(Retained::as_ptr(&view).cast());
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
            "owner" => {
                let id = value.and_then(Value::as_u64);
                if self._context.owner.get() != id {
                    if let Some(bridge) = self._context.bridge.borrow_mut().take() {
                        bridge.detach();
                    }
                    // SAFETY: replacing a runtime must also release the former
                    // owner's native route before attaching the next mailbox.
                    unsafe {
                        patina_airplay_disconnect(Retained::as_ptr(&self.view).cast());
                    }
                    self._context.owner.set(id);
                    self._context.generation.set(0);
                    let bridge = id.and_then(transport::Bridge::attach);
                    let bound_owner = if bridge.is_some() { id.unwrap_or(0) } else { 0 };
                    *self._context.bridge.borrow_mut() = bridge;
                    // SAFETY: only the successfully attached owner may bind
                    // controls to this retained main-thread player host.
                    unsafe {
                        patina_airplay_owner(Retained::as_ptr(&self.view).cast(), bound_owner)
                    };
                }
            }
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
        if let Some(bridge) = self._context.bridge.borrow().as_ref() {
            bridge.detach();
        }
        // SAFETY: NSH destroys components on main while this retained view
        // and callback context are still live; invalidation stops its timer.
        unsafe { patina_airplay_invalidate(Retained::as_ptr(&self.view).cast()) };
    }
}

/// The system device button shares the persistent host's player. Removing a
/// play-bar control never destroys that player or its active relay.
struct RouteButton {
    view: Retained<View>,
}
impl RouteButton {
    fn new() -> Self {
        // SAFETY: NSH creates native controls on main; C returns a +1 platform view.
        let view = unsafe { Retained::from_raw(patina_cast_button_create().cast()) }
            .expect("allocate device picker");
        Self { view }
    }
}
impl NativeComponent for RouteButton {
    #[cfg(target_os = "ios")]
    fn view(&self) -> &View {
        &self.view
    }
    #[cfg(target_os = "macos")]
    fn view(&self) -> Retained<View> {
        self.view.clone()
    }
    fn set_prop(&mut self, name: &str, value: Option<&Value>) {
        let number = value.and_then(Value::as_u64).unwrap_or(0);
        match name {
            "owner" => {
                // SAFETY: retained main-thread view; only an opaque runtime ID crosses FFI.
                unsafe { patina_cast_button_owner(Retained::as_ptr(&self.view).cast(), number) };
            }
            "accent" => {
                // SAFETY: the native setter reads only this view and the supplied RGB value.
                unsafe {
                    patina_cast_button_accent(
                        Retained::as_ptr(&self.view).cast(),
                        u32::try_from(number).unwrap_or(0),
                    )
                };
            }
            _ => {}
        }
    }
}
