//! Bounded heap accounting for the optional resident-protocols image.
//!
//! Requested bytes are tracked separately from `LlffHeap::used()`, which includes allocator
//! metadata and alignment effects. Failed `alloc` and `realloc` calls are counted.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};
use embedded_alloc::LlffHeap;

pub const HEAP_BYTES: usize = 64 * 1024;

#[cfg_attr(not(test), global_allocator)]
static HEAP: TrackingHeap = TrackingHeap::empty();

#[repr(C, align(16))]
struct HeapStorage([u8; HEAP_BYTES]);
static mut HEAP_STORAGE: HeapStorage = HeapStorage([0; HEAP_BYTES]);

struct TrackingHeap {
    inner: LlffHeap,
    requested: AtomicUsize,
    requested_peak: AtomicUsize,
    allocator_used_peak: AtomicUsize,
    allocation_failures: AtomicUsize,
}

impl TrackingHeap {
    const fn empty() -> Self {
        Self {
            inner: LlffHeap::empty(),
            requested: AtomicUsize::new(0),
            requested_peak: AtomicUsize::new(0),
            allocator_used_peak: AtomicUsize::new(0),
            allocation_failures: AtomicUsize::new(0),
        }
    }

    fn record_peak(&self, value: usize) {
        let mut peak = self.requested_peak.load(Ordering::Relaxed);
        while value > peak {
            match self.requested_peak.compare_exchange_weak(
                peak,
                value,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => peak = observed,
            }
        }
    }

    fn add_requested(&self, bytes: usize, allocator_used: usize) {
        let value = self
            .requested
            .fetch_add(bytes, Ordering::Relaxed)
            .saturating_add(bytes);
        self.record_peak(value);
        self.record_allocator_peak(allocator_used);
    }

    fn record_allocator_peak(&self, value: usize) {
        let mut peak = self.allocator_used_peak.load(Ordering::Relaxed);
        while value > peak {
            match self.allocator_used_peak.compare_exchange_weak(
                peak,
                value,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => peak = observed,
            }
        }
    }
}

unsafe impl GlobalAlloc for TrackingHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: startup initializes the wrapped allocator before any allocation; the runtime supplies a valid layout.
        critical_section::with(|_| {
            let ptr = unsafe { self.inner.alloc(layout) };
            if ptr.is_null() {
                self.allocation_failures.fetch_add(1, Ordering::Relaxed);
            } else {
                self.add_requested(layout.size(), self.inner.used());
            }
            ptr
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the pointer and layout came from this allocator.
        critical_section::with(|_| {
            unsafe { self.inner.dealloc(ptr, layout) };
            self.requested.fetch_sub(layout.size(), Ordering::Relaxed);
        });
    }
}

/// Initialize the fixed heap exactly once before resident state or tasks are constructed.
pub unsafe fn init() {
    // SAFETY: this region is exclusively owned by HEAP and this is the sole startup caller.
    unsafe {
        HEAP.inner
            .init(core::ptr::addr_of_mut!(HEAP_STORAGE) as usize, HEAP_BYTES);
    }
}

pub fn capacity() -> usize {
    HEAP_BYTES
}
pub fn allocator_used() -> usize {
    HEAP.inner.used()
}
pub fn requested() -> usize {
    HEAP.requested.load(Ordering::Relaxed)
}
pub fn requested_peak() -> usize {
    HEAP.requested_peak.load(Ordering::Relaxed)
}
pub fn allocator_used_peak() -> usize {
    HEAP.allocator_used_peak.load(Ordering::Relaxed)
}
pub fn allocation_failures() -> usize {
    HEAP.allocation_failures.load(Ordering::Relaxed)
}

/// Current stack pointer, used with the linker-provided CPU0 main-task bounds below.
pub fn sampled_stack_pointer() -> usize {
    #[cfg(target_arch = "xtensa")]
    {
        let stack_pointer: usize;
        // SAFETY: reading a1 has no memory effects and does not modify machine state.
        unsafe {
            core::arch::asm!("mov {0}, a1", out(reg) stack_pointer, options(nomem, nostack));
        }
        stack_pointer
    }
    #[cfg(not(target_arch = "xtensa"))]
    {
        0
    }
}

/// Sample the default CPU0 main-task stack against the linker-provided bounds. This is a
/// point-in-time estimate, not a high-water mark: startup, interrupts, and other task stacks
/// are not covered, and no live stack memory is painted or read.
pub fn sampled_stack_usage() -> (usize, usize, bool) {
    unsafe extern "C" {
        static _stack_start_cpu0: u8;
        static _stack_end_cpu0: u8;
    }
    let top = core::ptr::addr_of!(_stack_start_cpu0) as usize;
    let bottom = core::ptr::addr_of!(_stack_end_cpu0) as usize;
    let stack_pointer = sampled_stack_pointer();
    if bottom <= stack_pointer && stack_pointer <= top {
        (
            top.saturating_sub(stack_pointer),
            top.saturating_sub(bottom),
            true,
        )
    } else {
        (0, top.saturating_sub(bottom), false)
    }
}

#[cfg(test)]
mod tests {
    use super::{HEAP_BYTES, TrackingHeap};
    use core::alloc::{GlobalAlloc, Layout};

    #[repr(align(16))]
    struct Backing([u8; HEAP_BYTES]);

    #[test]
    fn real_realloc_preserves_bytes_and_records_overlap() {
        let heap = TrackingHeap::empty();
        let mut backing = Backing([0; HEAP_BYTES]);
        unsafe {
            heap.inner.init(backing.0.as_mut_ptr() as usize, HEAP_BYTES);
        }
        let layout = Layout::from_size_align(32, 8).unwrap();
        let ptr = unsafe { heap.alloc(layout) };
        assert!(!ptr.is_null());
        unsafe {
            core::ptr::write_bytes(ptr, 0x5a, 32);
        }
        let resized = unsafe { heap.realloc(ptr, layout, 4096) };
        assert!(!resized.is_null());
        assert!(
            unsafe { core::slice::from_raw_parts(resized, 32) }
                .iter()
                .all(|b| *b == 0x5a)
        );
        assert_eq!(
            heap.requested.load(core::sync::atomic::Ordering::Relaxed),
            4096
        );
        assert!(
            heap.requested_peak
                .load(core::sync::atomic::Ordering::Relaxed)
                >= 4128
        );
        assert!(
            heap.allocator_used_peak
                .load(core::sync::atomic::Ordering::Relaxed)
                > heap.inner.used()
        );
        unsafe {
            heap.dealloc(resized, Layout::from_size_align(4096, 8).unwrap());
        }
    }

    #[test]
    fn failed_realloc_is_counted_and_retains_old_block() {
        let heap = TrackingHeap::empty();
        let mut backing = Backing([0; HEAP_BYTES]);
        unsafe {
            heap.inner.init(backing.0.as_mut_ptr() as usize, HEAP_BYTES);
        }
        let layout = Layout::from_size_align(32, 8).unwrap();
        let ptr = unsafe { heap.alloc(layout) };
        let failed = unsafe { heap.realloc(ptr, layout, HEAP_BYTES) };
        assert!(failed.is_null());
        assert_eq!(
            heap.allocation_failures
                .load(core::sync::atomic::Ordering::Relaxed),
            1
        );
        unsafe {
            heap.dealloc(ptr, layout);
        }
        assert_eq!(
            heap.requested.load(core::sync::atomic::Ordering::Relaxed),
            0
        );
    }
}
