//! Test-only call counters for per-packet cost regressions. Thread-local, so tests running
//! in parallel do not see each other's work.

use core::cell::Cell;

/// A counted operation.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Probe {
    /// One SHA-256 over a packet's hashable part.
    PacketHash,
    /// One Ed25519 verification of an announce signature.
    AnnounceVerify,
    /// One [`crate::link::PendingLink::prove`] that reached its signature check.
    LinkProve,
    /// One AES and HMAC key schedule for a token key pair.
    TokenKeying,
    /// One scan of the reliable channel's sent-hash table.
    ReliableSweep,
}

const PROBES: usize = 5;

std::thread_local! {
    static COUNTS: Cell<[u32; PROBES]> = const { Cell::new([0; PROBES]) };
}

/// Count one `probe`.
pub(crate) fn hit(probe: Probe) {
    COUNTS.with(|counts| {
        let mut all = counts.get();
        all[probe as usize] += 1;
        counts.set(all);
    });
}

/// How many `probe`s since the last take, resetting it to zero.
pub(crate) fn take(probe: Probe) -> u32 {
    COUNTS.with(|counts| {
        let mut all = counts.get();
        let n = core::mem::take(&mut all[probe as usize]);
        counts.set(all);
        n
    })
}
