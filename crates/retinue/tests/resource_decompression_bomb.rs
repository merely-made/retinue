//! A received bz2 bomb is refused at the limit instead of being inflated.
//!
//! RNS caps a received resource's decompressed size (64 MiB) and cancels past it. retinue
//! recovered compressed resources with an unbounded decoder, so a link peer could send a
//! few kilobytes that inflate to whatever it liked. The bounded decoder's output buffer
//! never grows past the limit (its own unit test covers the exact-fit edge); this test
//! proves recovery takes that path. It does not count heap bytes: a counting global
//! allocator needs `unsafe`, which this crate forbids.
#![cfg(feature = "compression")]

use retinue::Error;
use retinue::resource::{Incoming, advertise, compress, content};

#[test]
fn a_bz2_bomb_is_refused_at_the_limit() {
    // Large enough to dwarf the bound below, small enough that building it in a debug
    // test build stays quick.
    const BOMB: usize = 32 * 1024 * 1024;
    const LIMIT: usize = 64 * 1024;

    // 32 MiB of zeros, fed to one bz2 stream in 1 MiB steps so building the bomb does not
    // itself hold 32 MiB.
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

    assert_eq!(
        incoming.recover_with_limit(&decrypted, LIMIT),
        Err(Error::DecompressionLimit)
    );
}
