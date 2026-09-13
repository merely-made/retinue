use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Measured;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static COUNT: AtomicUsize = AtomicUsize::new(0);
#[global_allocator]
static ALLOCATOR: Measured = Measured;

// This host-only fixture wraps System without logging or allocating in its
// callbacks. It measures requested bytes, excluding allocator bookkeeping.
unsafe impl GlobalAlloc for Measured {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forward the allocator runtime's valid layout unchanged.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(live, Relaxed);
            COUNT.fetch_add(1, Relaxed);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        // SAFETY: every allocation from this wrapper came from System with
        // this same layout; the allocator runtime supplies a live pointer.
        unsafe { System.dealloc(ptr, layout) };
    }
}

fn main() {
    let baseline = LIVE.load(Relaxed);
    PEAK.store(baseline, Relaxed);
    let start_count = COUNT.load(Relaxed);
    let residents = protocol_capacity_probe::workload();
    let retained = LIVE.load(Relaxed) - baseline;
    let peak = PEAK.load(Relaxed) - baseline;
    let allocations = COUNT.load(Relaxed) - start_count;
    drop(residents);
    let remaining = LIVE.load(Relaxed) - baseline;
    assert_eq!(remaining, 0, "workload leaked allocations");
    println!(
        "{{\"host_pointer_bytes\":{},\"retained_requested_bytes\":{retained},\"peak_requested_bytes\":{peak},\"allocation_count\":{allocations},\"after_drop_bytes\":{remaining},\"layout\":{:?}}}",
        size_of::<usize>(),
        protocol_capacity_probe::PROTOCOL_LAYOUT_V1
    );
}
