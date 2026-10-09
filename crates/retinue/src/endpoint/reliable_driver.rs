//! The reliable stream driver: RNS Channel/Buffer over a link, with proof acks and
//! retransmission.

use alloc::vec::Vec;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::identity::Identity;
use crate::link::{self, CTX_CHANNEL, CTX_LINKCLOSE, CTX_LINKIDENTIFY, Inbound, Link};
use crate::link_liveness::Liveness;
use crate::packet::{Packet, PacketType};
use crate::reliable::ReliableChannel;

use super::entropy::next_iv;
use super::facts::{LinkDirection, LinkRemoteFact};
use super::interface::InterfaceId;
use super::runtime::track_drainable;
use super::shared::{LinkEntry, LinkKind, Shared};
use super::stream::{DUPLEX_BUF, LINK_QUEUE, LinkStream, StreamFault, WRITE_CHUNK};

/// The reliable driver's clock period. Each period advances the channel's millisecond clock
/// by this much, which drives retransmission of unproven channel packets once their
/// RTT-derived timeout passes. One timer per active reliable link; a production build would
/// pause it when the link is fully idle.
const RELIABLE_TICK_MS: u64 = 50;

/// How many times an initiator sends its IDENTIFY over the opening retransmit ticks. RNS
/// sends it once; on a lossy medium a single drop leaves the responder unable to validate our
/// proofs of the data it sends us, stalling that direction with no way to recover. The wire
/// protocol has no IDENTIFY ack, so we simply re-send it a bounded few times, which survives
/// realistic early loss without ever spinning.
const IDENTIFY_MAX_SENDS: u32 = 4;

/// Build a **reliable** [`LinkStream`] for a live link: the RNS Channel/Buffer path with
/// link-proof acks (see [`crate::reliable`]). A single driver task owns the
/// [`ReliableChannel`] and pumps it — app writes in, ordered bytes out, a proof per
/// delivered packet, an inbound proof releasing its sequence, and retransmits on a clock —
/// so the stream stays honest over a lossy interface. `peer` is the remote identity: `Some`
/// for an initiator (the destination's identity from its announce), `None` for a responder,
/// which learns the initiator's identity from the IDENTIFY the initiator sends. Proofs are
/// validated against the link's own peer key either way. An initiator sends its IDENTIFY so
/// the responder learns who it is.
///
/// The channel times its retransmits by the RTT `liveness` measured at setup (RNS times its
/// channel by the link's RTT), with the configured estimate as a floor; a responder takes
/// the RTT from the initiator's RTT packet once it arrives.
pub(super) fn register_reliable_stream(
    shared: &Arc<Shared>,
    link: Link,
    iface: InterfaceId,
    liveness: Liveness,
    peer: Option<Identity>,
    direction: LinkDirection,
    remote: LinkRemoteFact,
) -> Option<LinkStream> {
    let (mine, theirs) = tokio::io::duplex(DUPLEX_BUF);
    let (mut read_half, mut write_half) = tokio::io::split(theirs);
    let (pkt_tx, mut pkt_rx) = mpsc::channel::<Packet>(LINK_QUEUE);
    let link_id = link.id();
    let lost = Arc::new(AtomicBool::new(false));
    // The configured RTT is a floor under the one measured at setup: a responder has no
    // measurement yet, and a slow radio's data turnaround can exceed its setup round trip.
    let initial_rtt_ms = shared
        .reliable_initial_rtt_ms
        .load(Ordering::Relaxed)
        .max(liveness.rtt().unwrap_or(0));

    shared.write_diagnostic(|| {
        shared.links.lock().unwrap().insert(
            link_id,
            LinkEntry {
                link: link.clone(),
                kind: LinkKind::Reliable { packets: pkt_tx },
                iface,
                direction,
                remote,
                liveness,
                lost: Arc::clone(&lost),
            },
        );
        ((), true)
    });

    // An initiator (known peer) identifies itself so the responder learns who it is.
    // Each send is sealed under a fresh IV, so a re-send is a new packet with a new hash and
    // the responder's duplicate window does not count it (Ruling 72).
    let identify_link = peer.is_some().then(|| link.clone());
    let close_link = link.clone();
    let max_window = shared.reliable_max_window.load(Ordering::Relaxed);
    let decoded_frame_limit = shared.reliable_decoded_frame_limit.load(Ordering::Relaxed);
    let receive_error = Arc::new(Mutex::new(None));
    // App bytes read but not yet accepted by the bounded send queue, and whether the eof
    // frame still needs queueing. Holding these is what keeps backpressure from silently
    // becoming data loss.
    let mut pending: Vec<u8> = Vec::new();
    let mut finish_pending = false;
    let mut rc: ReliableChannel = match peer {
        Some(p) => ReliableChannel::new_with_initial_rtt_and_max_window(
            link,
            shared.identity.clone(),
            p,
            initial_rtt_ms,
            max_window,
        ),
        None => ReliableChannel::accepting_with_initial_rtt_and_max_window(
            link,
            shared.identity.clone(),
            initial_rtt_ms,
            max_window,
        ),
    };
    rc.set_decoded_frame_limit(decoded_frame_limit)
        .expect("endpoint validates the decoded frame limit");
    let driver_receive_error = Arc::clone(&receive_error);
    let drv = Arc::clone(shared);
    // A responder's proof has just gone out; the initiator's RTT packet answers it.
    let registered_at = Instant::now();
    let driver_started = track_drainable(shared, async move {
        // Identify to the responder so it learns who we are. RNS sends this once; we
        // re-send it over the first few ticks (in the clock arm below) so a dropped one still
        // lands on a lossy medium.
        if let Some(id_link) = &identify_link {
            drv.send_on(iface, id_link.identify_packet(&drv.identity, &next_iv()));
        }
        let mut identify_sends: u32 = 1;
        let mut buf = [0u8; WRITE_CHUNK];
        // The reliable channel measures RTT and sizes its retransmit timeout in this clock's
        // unit, and its RTT tiers are calibrated in milliseconds, so advance the clock by the
        // real tick period (below) rather than by 1 — otherwise the timeout is off by a factor
        // of RELIABLE_TICK_MS and either storms or stalls the medium.
        let mut clock: u64 = 0;
        let mut writer_open = true; // the app's write side is still open
        let mut peer_done = false; // the peer signalled end-of-stream (its eof frame)
        let mut interval = tokio::time::interval(Duration::from_millis(RELIABLE_TICK_MS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        'driver: loop {
            tokio::select! {
                // Raw inbound packets from the router: channel data (prove + deliver), an
                // ack (release its sequence), or the peer's link close.
                maybe = pkt_rx.recv() => {
                    let Some(pkt) = maybe else { break }; // router dropped the link
                    if pkt.packet_type == PacketType::Proof {
                        rc.on_proof(&pkt, clock);
                    } else if pkt.context == CTX_CHANNEL {
                        if let Some(proof) = rc.on_data_packet(&pkt) {
                            drv.send_on(iface, proof);
                        }
                        // One channel frame can expand beyond the Buffer read bound.
                        // Drain every available chunk before considering the peer's eof;
                        // another packet need not arrive to wake this task again.
                        loop {
                            let bytes = rc.read();
                            if bytes.is_empty() {
                                break;
                            }
                            if write_half.write_all(&bytes).await.is_err() {
                                break 'driver;
                            }
                        }
                        if let Some(error) = rc.receive_error() {
                            *driver_receive_error.lock().unwrap() = Some(error.into());
                            drv.send_on(iface, close_link.close_packet(&next_iv()));
                            break 'driver;
                        }
                        if !peer_done && rc.recv_finished() {
                            // The peer's stream ended: close our read side so the app's
                            // reader sees EOF. We keep running to finish our own sending.
                            let _ = write_half.shutdown().await;
                            peer_done = true;
                        }
                    } else if pkt.context == CTX_LINKIDENTIFY {
                        // The peer (an initiator) identified itself: learn its identity, which
                        // also validates proofs from older retinue initiators.
                        if rc.on_identify(&pkt)
                            && let Some(identity) = rc.peer().copied()
                        {
                            let present = drv.write_diagnostic(|| {
                                let mut links = drv.links.lock().unwrap();
                                let Some(entry) = links.get_mut(&link_id) else {
                                    return (false, false);
                                };
                                let changed = if entry.remote.identity == Some(identity) {
                                    false
                                } else {
                                    entry.remote.identity = Some(identity);
                                    true
                                };
                                (true, changed)
                            });
                            if !present {
                                break;
                            }
                        }
                    } else if pkt.context == link::CTX_LRRTT {
                        // Time the channel by the link's RTT, as RNS does, not a guess.
                        let measured = registered_at.elapsed().as_millis();
                        rc.on_rtt_packet(&pkt, u64::try_from(measured).unwrap_or(u64::MAX));
                    } else if pkt.context == CTX_LINKCLOSE
                        && close_link.receive(&pkt) == Some(Inbound::Close)
                    {
                        // Only a close that decrypts to the link id is the peer's: anyone
                        // can put this context on a packet addressed to the link.
                        let _ = write_half.shutdown().await;
                        break;
                    }
                }
                // App writes -> the reliable send queue. Disabled once the writer closes, so
                // we do not spin on end-of-stream.
                // Only read when the last write was fully accepted; otherwise we would pull
                // more from the app than the bounded queue can hold.
                res = read_half.read(&mut buf), if writer_open && pending.is_empty() => {
                    match res {
                        Ok(0) | Err(_) => {
                            finish_pending = true; // retried below until the queue takes it
                            writer_open = false;
                        }
                        Ok(n) => {
                            let accepted = rc.write(&buf[..n]);
                            if accepted < n {
                                pending.extend_from_slice(&buf[accepted..n]);
                            }
                        }
                    }
                }
                // The retransmit clock, in milliseconds (one tick = RELIABLE_TICK_MS real time).
                _ = interval.tick() => {
                    clock += RELIABLE_TICK_MS;
                    // Re-send IDENTIFY over the first few ticks so a dropped one still reaches
                    // the responder on a lossy medium (bounded; there is no ack to wait on).
                    if let Some(id_link) = &identify_link
                        && identify_sends < IDENTIFY_MAX_SENDS
                    {
                        drv.send_on(iface, id_link.identify_packet(&drv.identity, &next_iv()));
                        identify_sends += 1;
                    }
                }
            }

            // The send queue is bounded, so an earlier write or eof may have been refused.
            // Retry before transmitting, so anything accepted now goes out on this pass.
            if !pending.is_empty() {
                let accepted = rc.write(&pending);
                pending.drain(..accepted);
            }
            if finish_pending && pending.is_empty() && rc.finish() {
                finish_pending = false;
            }

            // After any event, put ready channel packets on the wire: new data within the
            // window, plus retransmits past their timeout.
            for pkt in rc.poll_transmit(clock, next_iv) {
                drv.send_on(iface, pkt);
            }
            // A packet went unproved through every try: the peer is gone. Fail the stream
            // and close the link, as RNS tears its link down when the channel times out.
            if rc.channel_error().is_some() {
                *driver_receive_error.lock().unwrap() = Some(StreamFault::Unacknowledged);
                drv.send_on(iface, close_link.close_packet(&next_iv()));
                break;
            }

            // The stream is fully done only when our side finished sending (write closed and
            // everything, including our eof frame, sent and proven) AND the peer finished
            // sending (its eof arrived). This preserves half-close: after our write closes we
            // keep delivering the peer's reply until it, too, ends. Then close the link.
            // `finish_pending` and `pending` must be clear too: the writer closing is not the
            // same as the queue having accepted everything, now that it can refuse.
            if !writer_open && pending.is_empty() && !finish_pending && peer_done && rc.send_idle()
            {
                drv.send_on(iface, close_link.close_packet(&next_iv()));
                break;
            }
        }
        drv.remove_link(link_id);
    });

    if !driver_started {
        shared.remove_link(link_id);
        return None;
    }

    Some(LinkStream {
        inner: mine,
        receive_error: Some(receive_error),
        lost,
        link_id,
        iface,
    })
}
