//! Live iPad input and accessibility signals. The zero-size component owns
//! its notification tokens; no observers outlive the mounted bridge.

use std::{ptr::NonNull, rc::Rc};

use block2::RcBlock;
use nsh_platform_ios::{ComponentContext, Components, NativeComponent};
use objc2::{
    MainThreadMarker, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, ProtocolObject},
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSObjectProtocol, NSOperationQueue, NSString,
};
use objc2_ui_kit::{
    UIAccessibilityIsSwitchControlRunning, UIAccessibilitySwitchControlStatusDidChangeNotification,
    UIApplicationDidBecomeActiveNotification, UISceneDidActivateNotification, UIView,
};

// GCKeyboard is available since iOS 14; Patina's minimum is iOS 15.
// Linking the framework also guarantees that its class is registered.
#[link(name = "GameController", kind = "framework")]
unsafe extern "C" {
    static GCKeyboardDidConnectNotification: &'static NSString;
    static GCKeyboardDidDisconnectNotification: &'static NSString;
}

pub(super) fn register(components: &Components) {
    components.register("tv.device", Rc::new(|ctx| Box::new(Device::new(ctx))));
}

fn report(ctx: &ComponentContext) {
    let _mtm = MainThreadMarker::new().expect("iOS device reports run on the main thread");
    let class = AnyClass::get(c"GCKeyboard").expect("GameController provides GCKeyboard on iOS 15");
    // SAFETY: documented GCKeyboard class method. The result is borrowed
    // only for the null check; no keyboard events or ownership are taken.
    let keyboard: *mut AnyObject = unsafe { msg_send![class, coalescedKeyboard] };
    ctx.emit(
        "device",
        serde_json::json!({"keyboard": !keyboard.is_null(), "dpad": false}),
    );
    let switch_access = UIAccessibilityIsSwitchControlRunning();
    ctx.emit(
        "accessibility",
        serde_json::json!({"switch_access": switch_access}),
    );
}

struct Device {
    view: Retained<UIView>,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

impl Device {
    fn new(ctx: ComponentContext) -> Self {
        let mtm = MainThreadMarker::new().expect("iOS native components mount on the main thread");
        let view = UIView::new(mtm);
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: framework notification constants; the frameworks are linked.
        let names = unsafe {
            [
                GCKeyboardDidConnectNotification,
                GCKeyboardDidDisconnectNotification,
                UIAccessibilitySwitchControlStatusDidChangeNotification,
                UIApplicationDidBecomeActiveNotification,
                UISceneDidActivateNotification,
            ]
        };
        let observers = names
            .into_iter()
            .map(|name| {
                let ctx = ctx.clone();
                let block = RcBlock::new(move |_: NonNull<NSNotification>| report(&ctx));
                // SAFETY: reports run on the main queue and Drop removes every
                // observer token before this component's lifetime ends.
                unsafe {
                    center.addObserverForName_object_queue_usingBlock(
                        Some(name),
                        None,
                        Some(&NSOperationQueue::mainQueue()),
                        &block,
                    )
                }
            })
            .collect();
        // Register before reading, so mounting cannot miss a state change.
        report(&ctx);
        Self { view, observers }
    }
}

impl NativeComponent for Device {
    fn view(&self) -> &UIView {
        &self.view
    }
    fn set_prop(&mut self, _: &str, _: Option<&serde_json::Value>) {}
}

impl Drop for Device {
    fn drop(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        for observer in &self.observers {
            // SAFETY: tokens returned by this center, removed once on drop.
            unsafe { center.removeObserver(observer.as_ref()) };
        }
    }
}
