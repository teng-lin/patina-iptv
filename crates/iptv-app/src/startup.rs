//! Cold-start phase marks: one `iptv startup: <phase> +<ms> ms` line per
//! phase in the log, in milliseconds since the process started (where the
//! host knows the process's start, `crate::host::process_age`; elsewhere
//! since the first mark),
//! with what it took or counted. Each phase is logged once per process:
//! later imports, guide checks and zaps stay quiet. `tools/perf-android.sh`
//! reads them for its phase table.
//!
//! The phases, in the order a stored playlist's cold start reaches them:
//! `rust root` (the app's root component runs), `settings loaded`,
//! `fonts registered` (all three registered), `store opened` (the
//! startup source read), `last channel resolved` (its stream known from
//! the stored catalog), `channels read`, `first screen read` and `guide
//! window read` (on the import thread; the window is read once `first
//! screen shown`, the first screen's schedule on screen, which a combined
//! guide of several playlists does not mark), `guide shown`
//! (the schedule swapped in on the UI thread, at a cold start the first
//! screen's), `guide frame` (the next display frame after it), `live input
//! ready` (a shared live stream's first packets from the provider, with how
//! long they took), `player opened` (the stream handed to the player),
//! `video size` (reported dimensions;
//! the shared `size` event does not distinguish metadata from a renderer callback),
//! and `video rendered` (a current-source native renderer submission where supported,
//! never display presentation/scanout).

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The phases marked so far.
static MARKED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
/// Where [`since_start`] counts from where the process start is unknown.
static FALLBACK: Mutex<Option<Instant>> = Mutex::new(None);

/// Logs `phase` (the first time only).
pub fn mark(phase: &'static str) {
    mark_with(phase, "");
}

/// Logs `phase` with `detail` (the first time only).
pub fn mark_with(phase: &'static str, detail: impl std::fmt::Display) {
    if !first(phase) {
        return;
    }
    let detail = detail.to_string();
    let sep = if detail.is_empty() { "" } else { ", " };
    log::info!(
        "iptv startup: {phase} +{} ms{sep}{detail}",
        since_start().as_millis()
    );
}

/// `data.rs` put a schedule on screen; `steps` say how long after it
/// started each step had ended. Marks `guide shown`, and `guide frame` at
/// the next display frame (the first one showing it).
pub fn shown(steps: &[(&str, Duration)]) {
    if marked("guide shown") {
        return;
    }
    let steps: Vec<String> = steps
        .iter()
        .map(|(step, at)| format!("{step} {} ms", at.as_millis()))
        .collect();
    mark_with("guide shown", steps.join(", "));
    drop(nsh_core::prelude::request_animation_frame(|_| {
        mark("guide frame")
    }));
}

/// Whether `phase` was marked already (without marking it).
pub fn marked(phase: &'static str) -> bool {
    MARKED.lock().is_ok_and(|m| m.contains(&phase))
}

/// Records `phase`; false when it was recorded before.
fn first(phase: &'static str) -> bool {
    let Ok(mut marked) = MARKED.lock() else {
        return false;
    };
    if marked.contains(&phase) {
        return false;
    }
    marked.push(phase);
    true
}

/// Time since the process started.
pub fn since_start() -> Duration {
    if let Some(d) = crate::host::process_age() {
        return d;
    }
    let mut origin = FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    origin.get_or_insert_with(Instant::now).elapsed()
}

/// Original Patina artwork, embedded so startup needs no file or network access.
pub(crate) fn splash() -> impl nsh_core::prelude::IntoElement {
    use base64::Engine;
    use nsh_core::prelude::*;
    let artwork = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD
            .encode(include_bytes!("../../../art/icon/banner.png")),
    );
    view()
        .test_id("startup-splash")
        .absolute()
        .inset_0()
        .bg(crate::tint(
            crate::theme::Theme::default().background.window,
            0xff,
        ))
        .child(
            image(artwork)
                .accessible(true)
                .label("Patina")
                .w_full()
                .h_full()
                .object_fit(ResizeMode::Contain),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phase_is_marked_once() {
        assert!(!marked("test phase"));
        assert!(first("test phase"));
        assert!(!first("test phase"));
        assert!(marked("test phase"));
    }
}
