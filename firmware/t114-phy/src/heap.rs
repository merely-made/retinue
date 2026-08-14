//! The board's heap.
//!
//! Until N3 this firmware had no allocator at all, and the N0 receipt could record a heap
//! high-water mark of zero by construction. Linking `retinue` ends that: its sans-io core is
//! `no_std + alloc`, so packet payloads and reassembly buffers are heap-allocated. The
//! heltec doc's done condition asks for a heap high-water figure precisely because this
//! moment was expected.
//!
//! What replaces "no heap" as the guarantee:
//!
//! - **The heap is a fixed array**, sized here and never grown. It cannot take memory from
//!   anything else, and the linker accounts for it as ordinary static storage.
//! - **Every table above it is already bounded** by N1's capacity work, so the number of
//!   live allocations has a ceiling rather than depending on what a peer sends.
//! - **Usage is measurable** through [`used`], [`free`], and [`high_water`], so a receipt can
//!   state the real peak rather than mistaking a post-flood live allocation for one.
//!
//! A linked-list-first-fit allocator is chosen over TLSF because this workload is a small
//! number of short-lived buffers rather than a churn of many sizes, and LLFF is the simpler
//! thing to reason about when it goes wrong.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

use embedded_alloc::LlffHeap;

#[global_allocator]
static HEAP: TrackingHeap = TrackingHeap::empty();

/// The firmware cannot reconstruct a peak from a current allocation after a packet buffer
/// has been released. Keep the peak beside the fixed allocator instead, updating it only
/// after a successful allocation. This is a measurement aid, not a second allocator.
struct TrackingHeap {
    inner: LlffHeap,
    high_water: AtomicUsize,
}

impl TrackingHeap {
    const fn empty() -> Self {
        Self {
            inner: LlffHeap::empty(),
            high_water: AtomicUsize::new(0),
        }
    }

    unsafe fn init(&self, start_addr: usize, size: usize) {
        // SAFETY: forwarded from this module's one-shot `init` contract.
        unsafe { self.inner.init(start_addr, size) };
    }

    fn note_high_water(&self) {
        let used = self.inner.used();
        let mut known = self.high_water.load(Ordering::Relaxed);
        while used > known {
            match self.high_water.compare_exchange_weak(
                known,
                used,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(current) => known = current,
            }
        }
    }
}

unsafe impl GlobalAlloc for TrackingHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `GlobalAlloc` receives a valid layout from the allocation runtime; the
        // wrapped allocator owns the fixed region initialised before any allocation.
        let allocation = unsafe { self.inner.alloc(layout) };
        if !allocation.is_null() {
            self.note_high_water();
        }
        allocation
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: allocations passed back by the runtime came from `alloc` above.
        unsafe { self.inner.dealloc(ptr, layout) };
    }
}

/// Bytes reserved for the heap.
///
/// Sized against the bounded tables above it: one resource reassembly at
/// `MAX_RESOURCE_PARTS` parts is roughly 14 KB, link and packet buffers are a few KB more,
/// and the rest is headroom so fragmentation has somewhere to go. The board has 232 KB of
/// RAM and used 11 KB of static before this, so the ceiling is generous on purpose: an
/// allocator that fails is far worse than one that is larger than it needs to be.
pub const HEAP_SIZE: usize = 48 * 1024;

static mut HEAP_MEMORY: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

/// Hand the heap its memory. Call once, before anything allocates.
///
/// # Safety
///
/// Must be called exactly once, and before any allocation. Both hold because the only
/// caller is the first statement of `main`.
pub unsafe fn init() {
    let start = &raw const HEAP_MEMORY as usize;
    unsafe { HEAP.init(start, HEAP_SIZE) }
}

/// Bytes currently allocated. The figure a receipt reports.
pub fn used() -> usize {
    HEAP.inner.used()
}

/// Bytes still available.
pub fn free() -> usize {
    HEAP.inner.free()
}

/// Largest live allocation observed since boot.
///
/// Unlike [`used`], this survives packet-buffer release and is the figure a sustained flood
/// receipt needs. It resets only on reboot, alongside every other board counter.
pub fn high_water() -> usize {
    HEAP.high_water.load(Ordering::Relaxed)
}
