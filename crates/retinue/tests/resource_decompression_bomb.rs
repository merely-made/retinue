//! A received bz2 bomb is refused without the allocation it asks for.
//!
//! RNS caps a received resource's decompressed size (64 MiB) and cancels past it. retinue
//! recovered compressed resources with an unbounded decoder, so a link peer could send a
//! few kilobytes that inflate to whatever it liked. This binary counts live heap bytes with
//! its own global allocator, so it holds exactly one test: a parallel test would pollute
//! the count.
#![cfg(feature = "compression")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use retinue::Error;
use retinue::resource::{Incoming, advertise, compress, content};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn a_bz2_bomb_is_refused_without_a_large_allocation() {
    // Large enough to dwarf the bound below, small enough that building it in a debug
    // test build stays quick.
    const BOMB: usize = 32 * 1024 * 1024;
    const LIMIT: usize = 64 * 1024;

    // 32 MiB of zeros, fed to one bz2 stream in 1 MiB steps so building the bomb does not
    // itself hold 32 MiB. The smallest block size keeps the decoder's workspace small, so
    // the peak below measures the output buffer rather than bz2's tables.
    let bomb = {
        use std::io::Write;
        let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        let chunk = vec![0_u8; 1024 * 1024];
        for _ in 0..BOMB / chunk.len() {
            encoder.write_all(&chunk).unwrap();
        }
        encoder.finish().unwrap()
    };
    assert!(bomb.len() < 64 * 1024, "a small packet's worth of bomb");

    // An advertisement for a (claimed) small payload that says it is compressed. The
    // receiver never reaches the hash check: the body is refused while inflating.
    let random_hash = [7, 7, 7, 7];
    let claimed = compress(b"small");
    let (advertisement, _) = advertise(b"small", &claimed, random_hash, true);
    let incoming = Incoming::new(&advertisement).unwrap();
    let decrypted = content(&bomb, &random_hash);

    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let refused = incoming.recover_with_limit(&decrypted, LIMIT);
    let peak = PEAK.load(Ordering::Relaxed) - before;

    assert_eq!(refused, Err(Error::DecompressionLimit));
    // The output buffer stops at LIMIT + 1. The bz2 decoder's own block workspace (about
    // 0.4 MB at block size 1) is separate; 32 MiB is what the unbounded decoder would hold.
    assert!(
        peak < 2 * 1024 * 1024,
        "peak heap during recovery was {peak} bytes"
    );
}
