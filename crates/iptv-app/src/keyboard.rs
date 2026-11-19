//! Software keyboard dismissal is independent of how much space it occupies.

use nsh_core::prelude::*;
use std::cell::Cell;

#[derive(Clone, Copy)]
struct ImeVisibility(Signal<Option<bool>>);

/// Observe the Android window's IME visibility without changing its insets.
pub(crate) fn bridge() -> impl IntoElement {
    let visibility = signal(None);
    provide_context(ImeVisibility(visibility));
    // Only the floating IME scenarios simulate the Android host observer.
    let enabled = cfg!(target_os = "android")
        || (cfg!(feature = "test-support") && std::env::var_os("IPTV_TEST_FLOATING_IME").is_some());
    when(
        move || enabled,
        move || {
            native("tv.ime-visibility")
                .test_id("ime-visibility")
                .absolute()
                .w(px(0.0))
                .h(px(0.0))
                .pointer_events(PointerEvents::None)
                .accessibility_elements_hidden(true)
                .on_native_event(move |event: &NativeEvent| {
                    if event.name.as_ref() == "visibility"
                        && let Some(visible) = event.data.as_bool()
                    {
                        visibility.set(Some(visible));
                    }
                })
        },
    )
}

/// Leave editing after the system hides the IME, even when it was floating.
pub(crate) fn on_dismissed(editing: impl Fn() -> bool + 'static, dismissed: impl Fn() + 'static) {
    let visibility = use_context::<ImeVisibility>();
    let was_visible = Cell::new(false);
    effect(move || {
        let editing = editing();
        let visible = visibility
            .and_then(|ime| ime.0.get())
            .unwrap_or_else(|| keyboard_height() > px(0.0));
        if was_visible.replace(editing && visible) && editing && !visible {
            dismissed();
        }
    });
}
