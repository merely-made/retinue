//! Table sizes, so one algorithm runs at two scales.
//!
//! The desktop and a board must run the *same* link, channel, and resource code,
//! or the desktop stops being an oracle for the board and becomes a second
//! implementation that merely interoperates. So the algorithms take their table
//! sizes as parameters rather than forking: [`desktop`] instantiates them large,
//! [`small`] instantiates them for a 256 KB part.
//!
//! # Why these are constants and not an associated-const trait
//!
//! The natural shape is one type parameter, `Channel<C: Capacity>`, reading its
//! sizes from `C`'s associated consts. It does not compile on stable: using
//! `C::SENT_HASHES` as a const-generic argument to a `heapless` collection needs
//! `generic_const_exprs`. So each bounded type takes its sizes as const-generic
//! parameters with the [`desktop`] values as defaults, which means existing
//! callers that write the bare type keep compiling and get the desktop profile.
//!
//! Capacities are powers of two. `heapless` 0.9 does not require it, but earlier
//! versions did, and nothing here is worth a surprise on a version bump.

/// Sizes for a host with real memory. These are the defaults every bounded type
/// uses when a caller does not say otherwise.
pub mod desktop {
    /// Channel packets on the wire whose proof has not yet come back.
    ///
    /// Sized for a fast link with a deep window and several retransmit
    /// generations outstanding at once.
    pub const SENT_HASHES: usize = 1024;
}

/// Sizes for the T114 profile: one link, a shallow window, and no room to be
/// generous. See `design_docs/2026-07-31_retinue_small_plan.md`.
pub mod small {
    /// Channel packets on the wire whose proof has not yet come back.
    ///
    /// A board runs one link at a shallow window. Overflow is not a failure
    /// here: an unrecorded packet simply retransmits, so this trades a little
    /// airtime under heavy loss for a hard memory bound.
    pub const SENT_HASHES: usize = 32;
}

/// The bounded types at the [`small`] profile, so a board never writes the positional
/// parameters itself.
///
/// The parameters are positional because associated consts cannot feed const generics on
/// stable, as the module docs explain. Confining the ugliness to these three lines is the
/// price; every board-side caller writes a name instead.
pub mod small_types {
    use super::small;
    use crate::channel::{Buffer, Channel};
    use crate::reliable::ReliableChannel;

    /// In-flight envelopes on a board: a shallow window for a half-duplex radio.
    pub const WINDOW: usize = 8;
    /// Application queue depth in each direction.
    pub const QUEUE: usize = 16;
    /// Held-back future sequences. The desktop keeps 256 as anti-abuse headroom; a board
    /// cannot spend that, and a peer that streams only future sequences is refused sooner.
    pub const REORDER: usize = 16;
    /// Bytes held for a reader that has not read yet.
    pub const READ_BYTES: usize = 8_192;

    /// A [`Channel`] at the small profile.
    pub type SmallChannel = Channel<WINDOW, QUEUE, REORDER>;
    /// A [`Buffer`] at the small profile.
    pub type SmallBuffer = Buffer<WINDOW, QUEUE, REORDER, READ_BYTES>;
    /// A [`ReliableChannel`] at the small profile.
    pub type SmallReliableChannel =
        ReliableChannel<{ small::SENT_HASHES }, WINDOW, QUEUE, REORDER, READ_BYTES>;
}
