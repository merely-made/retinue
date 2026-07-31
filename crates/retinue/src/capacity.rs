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
