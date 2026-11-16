//! Preserve both process-wide and per-thread allocation measurements.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

pub(super) static LIVE: AtomicIsize = AtomicIsize::new(0);
pub(super) static PEAK: AtomicIsize = AtomicIsize::new(0);

fn add(bytes: isize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
    thread::add(bytes);
}

pub(super) mod thread {
    use std::cell::Cell;

    thread_local! {
        pub(crate) static LIVE: Cell<isize> = const { Cell::new(0) };
        pub(crate) static PEAK: Cell<isize> = const { Cell::new(0) };
    }

    pub(super) fn add(bytes: isize) {
        // Allocations during thread teardown cannot access destroyed TLS counters.
        let _ = LIVE.try_with(|live| {
            let value = live.get() + bytes;
            live.set(value);
            let _ = PEAK.try_with(|peak| peak.set(peak.get().max(value)));
        });
    }
}

pub(super) struct Counting;

// SAFETY: every allocation operation delegates unchanged to System.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        add(layout.size() as isize);
        // SAFETY: the caller's allocation contract is passed unchanged to System.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        add(layout.size() as isize);
        // SAFETY: the caller's allocation contract is passed unchanged to System.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        add(-(layout.size() as isize));
        // SAFETY: System allocated the pointer with the caller's matching layout.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        add(size as isize - layout.size() as isize);
        // SAFETY: the caller's pointer, layout and new size are passed unchanged.
        unsafe { System.realloc(pointer, layout, size) }
    }
}
