//! Allocation regressions share a link, with a fresh process for each case.
//! Native only: each case runs in a child process, which the browser lacks.

#[cfg(not(target_family = "wasm"))]
mod allocations;
#[cfg(not(target_family = "wasm"))]
mod catalog_stream_memory;
#[cfg(not(target_family = "wasm"))]
mod epg_memory;
#[cfg(not(target_family = "wasm"))]
mod epg_stream_memory;

#[cfg(not(target_family = "wasm"))]
#[path = "../common/mod.rs"]
mod common;
#[cfg(not(target_family = "wasm"))]
#[path = "../epg_synth/mod.rs"]
mod epg_synth;
#[cfg(not(target_family = "wasm"))]
#[path = "../initialization_support/mod.rs"]
mod initialization_support;
#[cfg(not(target_family = "wasm"))]
#[path = "../staged/mod.rs"]
mod staged;
#[cfg(not(target_family = "wasm"))]
#[path = "../stream_support/mod.rs"]
mod stream_support;

#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static ALLOC: allocations::Counting = allocations::Counting;

// Bound total fixture memory as before, when Cargo ran separate binaries in order.
#[cfg(not(target_family = "wasm"))]
static MEASUREMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(not(target_family = "wasm"))]
fn isolated(test: &str, measure: impl FnOnce()) {
    const CHILD: &str = "IPTV_STORE_MEMORY_CHILD";
    if std::env::var_os(CHILD).is_some() {
        measure();
        return;
    }
    let output = {
        let _measurement = MEASUREMENT
            .lock()
            .expect("memory measurement lock poisoned");
        initialization_support::child(test, CHILD, std::time::Duration::from_secs(600))
    }
    .expect("memory child could not complete");
    assert!(
        output.status.success(),
        "memory case {test} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
}
