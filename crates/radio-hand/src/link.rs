//! The host link: a byte pipe with a session, and nothing else.
//!
//! Every board reaches its host over something different — USB CDC on the T114 and the V4's
//! default personality, a bare UART on its low-power one, BLE and TCP on boards this family
//! has not reached yet. The protocol above them is identical, so the transport is a seam.
//!
//! What this trait deliberately does not carry, because each exclusion keeps a future
//! transport or personality from having to rebuild it:
//!
//! - **No MTU.** Chunking into 64-byte USB packets or BLE ATT payloads happens inside an
//!   implementation, where the number is known.
//! - **No wake.** `selvage`'s `CommandStream` already consumes `WAKE_BYTE` at a frame
//!   boundary, so a link that sleeps is a parser concern rather than a transport one.
//! - **No command vocabulary.** Nothing here names a `selvage` type, so the RNode and
//!   Meshtastic personalities can ride the same seam rather than growing their own.
//! - **No capability negotiation** until BLE lands and actually demands it.
//!
//! See `design_docs/2026-07-31_retinue_small_plan.md`, structural decision 3.

/// Why a link operation could not complete.
///
/// One variant, because one is all the protocol above can act on: either the peer is still
/// there or the session is over. Transports that cannot detect departure simply never
/// report this, and the shared dispatch above never ends their session — which is exactly
/// how the T114's break-on-write and the V4 UART's fire-and-forget both fall out of one
/// implementation rather than being a behavioural fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkFault {
    /// The peer is gone. The caller should abandon the session and wait for the next.
    Detached,
}

/// A host transport carrying the direct-PHY byte protocol.
/// Auto traits are deliberately unbounded: these futures are polled by a
/// single-threaded embassy executor on the board, never sent across threads, so a
/// `Send` bound would buy nothing and would exclude the HAL types that implement this.
#[allow(async_fn_in_trait)]
pub trait HostLink {
    /// Wait until a peer is attached.
    ///
    /// A transport with no attachment concept — a bare UART with nothing on the other end
    /// to notice — returns immediately, so a session simply begins.
    async fn attached(&mut self);

    /// Read whatever bytes are available, returning how many landed in `buf`.
    ///
    /// A short read is normal: the protocol above reassembles commands from an arbitrarily
    /// fragmented stream, so no implementation needs to deliver whole anything.
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, LinkFault>;

    /// Write every byte, chunking internally if the transport has a frame size.
    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), LinkFault>;
}
