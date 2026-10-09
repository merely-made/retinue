//! Interfaces: how packets get onto a wire.
//!
//! An interface is a byte pipe plus a framing. R1 implements the TCP interface, which is
//! HDLC framing over a TCP stream, in both the client and the server direction.
//!
//! The framing (`hdlc`) is sans-io and needs only the `alloc` feature. The TCP interface
//! (`tcp`) needs a runtime and sits behind the `tokio` feature, which is on by default.
//! The datagram carriers are `udp` (`tokio`) and `auto`, RNS's AutoInterface, which with
//! `netinfo` needs the `auto` feature on a unix host.

#[cfg(all(feature = "auto", unix))]
pub mod auto;
#[cfg(feature = "alloc")]
pub mod beacon;
#[cfg(feature = "alloc")]
pub mod hdlc;
#[cfg(feature = "alloc")]
pub mod kiss;

#[cfg(all(feature = "auto", unix))]
pub mod netinfo;

#[cfg(feature = "serial")]
pub mod serial;

#[cfg(feature = "tokio")]
pub mod tcp;

#[cfg(feature = "tulle-radio")]
pub mod tulle;

#[cfg(feature = "tokio")]
pub mod udp;
