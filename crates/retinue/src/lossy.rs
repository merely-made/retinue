//! A deterministic packet loss / delay oracle for testing reliability without radio.
//!
//! Reticulum's reliability machinery — link `Channel`/`Buffer`, resource windowing,
//! retries and cancellation, dynamic window sizing, part timeouts — earns its keep
//! only on a medium that drops, reorders, and delays. The interop oracle is Python
//! RNS over TCP loopback, which does none of that, so a green test there proves
//! nothing about the paths that matter: every retry branch is dead code that passes
//! because it never runs.
//!
//! This connects two [`Endpoint`]s through a **seeded, reproducible** loss model, so
//! those paths run at a desk, before any hardware exists. Same seed + same packet
//! sequence yields the same drops and delays, so a failure reproduces exactly.
//!
//! It plugs into the transport-agnostic [`Endpoint::attach_interface`] seam — the
//! same seam TCP and (later) serial/RNode use — and drops/delays whole packets,
//! which is the granularity link reliability and resource transfer actually care
//! about.
//!
//! ```no_run
//! # #[cfg(feature = "tokio")]
//! # async fn demo() {
//! # use retinue::endpoint::Endpoint;
//! # use retinue::identity::PrivateIdentity;
//! # use retinue::lossy::{self, LossModel};
//! let a = Endpoint::new(PrivateIdentity::from_secret_bytes(&[1u8; 64]));
//! let b = Endpoint::new(PrivateIdentity::from_secret_bytes(&[2u8; 64]));
//! // 20% packet loss and up to 40ms jitter each way, both reproducible.
//! lossy::connect(
//!     &a,
//!     &b,
//!     LossModel::new(1).drop_per_mille(200).max_delay_ms(40),
//!     LossModel::new(2).drop_per_mille(200).max_delay_ms(40),
//! );
//! # }
//! ```

#[cfg(feature = "tokio")]
use std::time::Duration;

#[cfg(feature = "tokio")]
use crate::endpoint::{Endpoint, InterfaceSink};

/// A seeded, deterministic loss model: a probability of dropping each packet and a
/// bounded random delay applied to those that survive.
///
/// Default is lossless (a faithful pass-through), so `LossModel::new(seed)` alone
/// turns [`connect`] into an ordinary in-memory link — useful as the control.
#[derive(Clone)]
pub struct LossModel {
    state: u64,
    drop_per_mille: u32,
    max_delay_ms: u64,
}

impl LossModel {
    /// A lossless model with the given seed. Build loss onto it with the setters.
    pub fn new(seed: u64) -> Self {
        // SplitMix64 to derive the initial state: it decorrelates nearby seeds (so
        // seeds 42 and 43 give unrelated streams) and never lands on xorshift's
        // fixed point at 0.
        Self {
            state: splitmix64(seed),
            drop_per_mille: 0,
            max_delay_ms: 0,
        }
    }

    /// Drop this many packets per thousand (clamped to 1000).
    pub fn drop_per_mille(mut self, per_mille: u32) -> Self {
        self.drop_per_mille = per_mille.min(1000);
        self
    }

    /// Delay each *delivered* packet by a reproducible `0..=ms` milliseconds. Delay
    /// also reorders: a delayed packet arrives after later ones that were not.
    pub fn max_delay_ms(mut self, ms: u64) -> Self {
        self.max_delay_ms = ms;
        self
    }

    /// xorshift64 — deterministic, allocation-free, no RNG dependency (retinue's
    /// core is RNG-free; this is a test shell).
    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Whether the next packet is dropped. Advances the model. Exposed so a sans-io
    /// reliability test (virtual clock) can drive the same seeded decisions the
    /// async pump uses.
    pub fn should_drop(&mut self) -> bool {
        self.drop_per_mille != 0 && (self.next() % 1000) < u64::from(self.drop_per_mille)
    }

    /// The next delivery delay in milliseconds (0..=max). Advances the model. A
    /// sans-io test reads this as a tick count.
    pub fn delay_ms(&mut self) -> u64 {
        if self.max_delay_ms == 0 {
            0
        } else {
            self.next() % (self.max_delay_ms + 1)
        }
    }

    #[cfg(feature = "tokio")]
    fn delay(&mut self) -> Duration {
        Duration::from_millis(self.delay_ms())
    }
}

/// Mix an arbitrary seed into a well-distributed, non-zero xorshift state.
fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    let z = z ^ (z >> 31);
    // xorshift's only fixed point is 0; splitmix maps exactly one seed there.
    if z == 0 { 0x9E37_79B9_7F4A_7C15 } else { z }
}

/// Connect two endpoints through a deterministic lossy link, one [`LossModel`] per
/// direction. Returns immediately; two pump tasks run in the background until either
/// endpoint is dropped.
#[cfg(feature = "tokio")]
pub fn connect(a: &Endpoint, b: &Endpoint, a_to_b: LossModel, b_to_a: LossModel) {
    let (a_out, a_sink) = a.attach_interface().split();
    let (b_out, b_sink) = b.attach_interface().split();
    tokio::spawn(pump(a_out, b_sink, a_to_b));
    tokio::spawn(pump(b_out, a_sink, b_to_a));
}

/// Move packets from one endpoint's outbound stream to the other's sink, applying
/// the loss model: some are dropped, survivors delivered after a bounded delay.
#[cfg(feature = "tokio")]
async fn pump(
    mut out: crate::endpoint::OutboundPackets,
    sink: InterfaceSink,
    mut model: LossModel,
) {
    while let Some(pkt) = out.recv().await {
        if model.should_drop() {
            continue;
        }
        let delay = model.delay();
        if delay.is_zero() {
            // Only a gone endpoint stops the pump. A full router queue drops the packet and
            // keeps going, which is what this harness is for: modelling a lossy link.
            if !sink.deliver(pkt) {
                break;
            }
        } else {
            // Deliver late on its own task, so a delayed packet does not hold up the
            // ones behind it — that is what produces reordering.
            let sink = sink.clone();
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                sink.deliver(pkt);
            });
        }
    }
}

#[cfg(all(test, feature = "tokio"))]
mod tests {
    // `no_std` crate: the tests take Vec from alloc, not the std prelude.
    use std::time::Duration;

    use tokio::time::{Instant, timeout};

    use super::{LossModel, connect};
    use crate::destination::DestinationName;
    use crate::endpoint::Endpoint;
    use crate::identity::PrivateIdentity;

    /// Drive A's announces until B learns A's destination, or the deadline passes.
    async fn learns(a: &Endpoint, b: &Endpoint, name: &DestinationName) -> bool {
        let dest = name.destination_hash(a.identity());
        a.register(name.clone(), b"cap");
        let deadline = Instant::now() + Duration::from_secs(6);
        while b.resolve(dest).is_none() && Instant::now() < deadline {
            a.announce(name, b"cap");
            let _ = timeout(Duration::from_millis(60), b.next_announcement()).await;
        }
        b.resolve(dest).is_some()
    }

    #[tokio::test]
    async fn zero_loss_link_is_faithful() {
        // The control: a lossless model is an ordinary in-memory link. Discovery
        // over the attach_interface seam works exactly like TCP.
        let a = Endpoint::new(PrivateIdentity::from_secret_bytes(&[1u8; 64]));
        let b = Endpoint::new(PrivateIdentity::from_secret_bytes(&[2u8; 64]));
        connect(&a, &b, LossModel::new(1), LossModel::new(2));

        let name = DestinationName::new("test", ["cap"]);
        assert!(
            learns(&a, &b, &name).await,
            "B learns A over a lossless lossy-link (seam is a faithful transport)"
        );
    }

    #[tokio::test]
    async fn announces_survive_moderate_drop() {
        // Loss is really injected, and repetition-based discovery survives it: at
        // 40% packet loss each way, a re-announced destination still gets through.
        let a = Endpoint::new(PrivateIdentity::from_secret_bytes(&[3u8; 64]));
        let b = Endpoint::new(PrivateIdentity::from_secret_bytes(&[4u8; 64]));
        connect(
            &a,
            &b,
            LossModel::new(7).drop_per_mille(400).max_delay_ms(15),
            LossModel::new(8).drop_per_mille(400).max_delay_ms(15),
        );

        let name = DestinationName::new("test", ["cap"]);
        assert!(
            learns(&a, &b, &name).await,
            "repeated announces survive 40% drop + jitter (loss is injected, retry-by-repeat works)"
        );
    }
}

#[cfg(test)]
mod model_tests {
    // `no_std` crate: Vec comes from alloc, not the std prelude.
    use super::LossModel;
    use crate::channel::{Channel, Envelope, STREAM_MSGTYPE, WINDOW_MAX};
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn drop_model_is_deterministic() {
        // Same seed + same draw count => identical drop decisions. This is what
        // makes a reliability-layer failure reproduce exactly.
        let run = |seed: u64| {
            let mut m = LossModel::new(seed).drop_per_mille(500);
            (0..64).map(|_| m.should_drop()).collect::<Vec<_>>()
        };
        assert_eq!(run(42), run(42), "same seed is reproducible");
        assert_ne!(run(42), run(43), "different seeds diverge");
    }

    /// A lost envelope must hold the send window, not just its count (review #5).
    ///
    /// An RNS receiver drops anything more than `WINDOW_MAX` past its next expected
    /// sequence, after the link has already proved it. Here the first three transmissions
    /// of sequence 0 are lost while its successors cross a seeded lossy pipe and are proved,
    /// so the count of unproved envelopes stays small. The sender must still never emit a
    /// sequence `WINDOW_MAX` or more past the oldest unproved one. Once sequence 0 gets
    /// through, everything is delivered in order.
    #[test]
    fn a_lost_envelope_bounds_the_send_span() {
        let messages = 200u16;
        let mut tx: Channel = Channel::with_initial_rtt(STREAM_MSGTYPE, 1);
        let mut rx: Channel = Channel::with_initial_rtt(STREAM_MSGTYPE, 1);
        for i in 0..messages {
            tx.send(vec![i as u8]).expect("the send queue has room");
        }
        let mut fwd = LossModel::new(5).drop_per_mille(150).max_delay_ms(3);
        let mut bwd = LossModel::new(6).drop_per_mille(150).max_delay_ms(3);
        let mut to_rx: Vec<(u64, Envelope)> = Vec::new();
        let mut to_tx: Vec<(u64, u16)> = Vec::new();
        let mut unproved: Vec<u16> = Vec::new();
        let mut got: Vec<u8> = Vec::new();
        let mut highest = 0u16;
        let mut first_sends = 0u32;

        for now in 0..200_000u64 {
            for e in tx.poll_transmit(now) {
                // The oldest unproved sequence is 0 until it is delivered; sequences never
                // wrap in this run, so a plain comparison is exact.
                let oldest = unproved.iter().copied().min().unwrap_or(e.sequence);
                assert!(
                    u32::from(e.sequence - oldest) < WINDOW_MAX,
                    "sent sequence {} with {oldest} still unproved",
                    e.sequence
                );
                if !unproved.contains(&e.sequence) {
                    unproved.push(e.sequence);
                }
                let lost = e.sequence == 0 && first_sends < 3;
                if e.sequence == 0 {
                    first_sends += 1;
                    if first_sends == 4 {
                        assert_eq!(
                            u32::from(highest),
                            WINDOW_MAX - 1,
                            "the sender ran up to the span bound and stopped there"
                        );
                    }
                }
                highest = highest.max(e.sequence);
                if !lost && !fwd.should_drop() {
                    to_rx.push((now + 1 + fwd.delay_ms(), e));
                }
            }
            let mut still = Vec::new();
            for (t, e) in core::mem::take(&mut to_rx) {
                if t <= now {
                    let seq = e.sequence;
                    if rx.handle(e) && !bwd.should_drop() {
                        to_tx.push((now + 1 + bwd.delay_ms(), seq));
                    }
                } else {
                    still.push((t, e));
                }
            }
            to_rx = still;
            to_tx.retain(|&(t, seq)| {
                if t <= now {
                    tx.on_proof(seq, now);
                    unproved.retain(|&s| s != seq);
                    false
                } else {
                    true
                }
            });
            while let Some(m) = rx.recv() {
                got.push(m[0]);
            }
            if got.len() == usize::from(messages) && tx.send_idle() {
                break;
            }
        }
        assert!(
            first_sends >= 4,
            "sequence 0 was retransmitted past its losses"
        );
        assert_eq!(
            got,
            (0..messages).map(|i| i as u8).collect::<Vec<_>>(),
            "everything delivers in order once the lost envelope gets through"
        );
    }

    /// A dead link fails the channel; it does not retransmit forever (review #15).
    ///
    /// With every packet lost, RNS sends each envelope at most five times, backing off
    /// between tries, then gives up and tears the link down. The same holds here on a
    /// virtual clock: no sequence goes out more than `DEFAULT_MAX_TRIES` times, and the
    /// channel ends in a terminal error rather than spinning.
    #[test]
    fn total_loss_gives_up_after_five_tries_per_sequence() {
        use crate::channel::{ChannelError, DEFAULT_MAX_TRIES};
        use alloc::collections::BTreeMap;

        let mut tx: Channel = Channel::with_initial_rtt(STREAM_MSGTYPE, 50);
        for i in 0..20u8 {
            tx.send(vec![i]).expect("the send queue has room");
        }
        let mut wire = LossModel::new(9).drop_per_mille(1000);
        let mut sends: BTreeMap<u16, u8> = BTreeMap::new();
        let mut failed_at = None;
        for now in 0..10_000_000u64 {
            for e in tx.poll_transmit(now) {
                *sends.entry(e.sequence).or_default() += 1;
                assert!(wire.should_drop(), "total loss drops everything");
            }
            if tx.error().is_some() {
                failed_at = Some(now);
                break;
            }
        }
        assert!(failed_at.is_some(), "the channel must give up");
        assert!(
            matches!(
                tx.error(),
                Some(ChannelError::RetriesExhausted { sequence }) if sequence < 2
            ),
            "the first window's envelopes run out of tries: {:?}",
            tx.error()
        );
        assert!(
            sends.values().all(|&n| n <= DEFAULT_MAX_TRIES),
            "no sequence went out more than five times: {sends:?}"
        );
        assert_eq!(sends.values().max(), Some(&DEFAULT_MAX_TRIES));
        assert!(tx.poll_transmit(u64::MAX).is_empty(), "and it stays quiet");
    }
}
