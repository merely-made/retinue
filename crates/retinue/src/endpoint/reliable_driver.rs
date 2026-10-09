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

/// The reliable driver's clock period: each tick advances the channel's millisecond clock by
/// this much, driving retransmission once a packet's RTT-derived timeout passes.
const RELIABLE_TICK_MS: u64 = 50;

/// How many times an initiator sends its IDENTIFY over the opening ticks. RNS sends it once,
/// but one drop would leave the responder unable to validate our proofs, and IDENTIFY has no
/// ack, so a bounded few copies ride out early loss.
const IDENTIFY_MAX_SENDS: u32 = 4;

/// Build a **reliable** [`LinkStream`] for a live link: RNS Channel/Buffer with link-proof
/// acks (see [`crate::reliable`]). One driver task owns the [`ReliableChannel`]: app writes
/// in, ordered bytes out, proofs both ways, and retransmits on a clock.
///
/// `peer` is `Some` for an initiator, which IDENTIFYs itself, and `None` for a responder,
/// which learns the initiator from that IDENTIFY. Retransmits are timed by the RTT measured
/// at setup (or, for a responder, by the initiator's RTT packet), floored by the configured
/// estimate.
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

    // Each IDENTIFY is sealed under a fresh IV, so the responder's duplicate window does not
    // drop a re-send (Ruling 72).
    let identify_link = peer.is_some().then(|| link.clone());
    let close_link = link.clone();
    let max_window = shared.reliable_max_window.load(Ordering::Relaxed);
    let decoded_frame_limit = shared.reliable_decoded_frame_limit.load(Ordering::Relaxed);
    let receive_error = Arc::new(Mutex::new(None));
    // App bytes read but not yet accepted by the bounded send queue, and whether the eof
    // frame still needs queueing, so backpressure never becomes data loss.
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
        if let Some(id_link) = &identify_link {
            drv.send_on(iface, id_link.identify_packet(&drv.identity, &next_iv()));
        }
        let mut identify_sends: u32 = 1;
        let mut buf = [0u8; WRITE_CHUNK];
        // In milliseconds, the unit the channel's RTT tiers are calibrated in: advancing by 1
        // per tick would skew its timeout by RELIABLE_TICK_MS.
        let mut clock: u64 = 0;
        let mut writer_open = true; // the app's write side is still open
        let mut peer_done = false; // the peer signalled end-of-stream (its eof frame)
        let mut interval = tokio::time::interval(Duration::from_millis(RELIABLE_TICK_MS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        'driver: loop {
            tokio::select! {
                maybe = pkt_rx.recv() => {
                    let Some(pkt) = maybe else { break }; // router dropped the link
                    if pkt.packet_type == PacketType::Proof {
                        rc.on_proof(&pkt, clock);
                    } else if pkt.context == CTX_CHANNEL {
                        if let Some(proof) = rc.on_data_packet(&pkt) {
                            drv.send_on(iface, proof);
                        }
                        // One frame can expand beyond the Buffer read bound: drain it all
                        // now, since no further packet need arrive to wake this task.
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
                            // EOF for the app's reader; keep running to finish sending.
                            let _ = write_half.shutdown().await;
                            peer_done = true;
                        }
                    } else if pkt.context == CTX_LINKIDENTIFY {
                        // The initiator's identity, which also validates proofs from older
                        // retinue initiators.
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
                // App writes, read only while the writer is open and the last write was
                // fully accepted by the bounded queue.
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
                _ = interval.tick() => {
                    clock += RELIABLE_TICK_MS;
                    if let Some(id_link) = &identify_link
                        && identify_sends < IDENTIFY_MAX_SENDS
                    {
                        drv.send_on(iface, id_link.identify_packet(&drv.identity, &next_iv()));
                        identify_sends += 1;
                    }
                }
            }

            // Retry a write or eof the bounded queue refused, before transmitting.
            if !pending.is_empty() {
                let accepted = rc.write(&pending);
                pending.drain(..accepted);
            }
            if finish_pending && pending.is_empty() && rc.finish() {
                finish_pending = false;
            }

            for pkt in rc.poll_transmit(clock, next_iv) {
                drv.send_on(iface, pkt);
            }
            // A packet went unproved through every try: close, as RNS does on channel timeout.
            if rc.channel_error().is_some() {
                *driver_receive_error.lock().unwrap() = Some(StreamFault::Unacknowledged);
                drv.send_on(iface, close_link.close_packet(&next_iv()));
                break;
            }

            // Done only when both sides finished sending (our eof queued, sent and proven, and
            // the peer's eof received), which preserves half-close.
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
