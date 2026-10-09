//! Host lines: `node`, `face`, and the `replay` facility.

extern crate alloc;

use core::fmt::Write as _;

use lora_phy::DelayNs;
use lora_phy::mod_traits::RadioKind;
use retinue::announce::{AnnounceBlob, RAND_HASH_LEN};
use retinue::packet::Packet;

use super::{NodeChannel, RADIO};
use crate::executive::Executive;
use crate::link::{Flow, HostLink};
use crate::replay;

/// One whole host line, from `node` or `replay`.
impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize>
    NodeChannel<PEERS, ACTIONS, LINKS>
{
    pub(super) async fn on_line<L, RK, DLY>(
        &mut self,
        exec: &mut Executive<'_, RK, DLY>,
        link: &mut L,
        line: &[u8],
    ) -> Flow
    where
        L: HostLink,
        RK: RadioKind,
        DLY: DelayNs,
    {
        if line == b"node" {
            let status = exec.status();
            let transport = self.node.transport_counters();
            let transport_on = self.node.transport_config().relay_packets;
            let mut out = radio_face::Text::<224>::empty();
            let _ = write!(
                &mut out,
                "node tx={} rx={} peers={} links={} refusedlinks={} refusedpeers={} \
                 refusedoffers={} routes={} transport={} fwdannounce={} fwdpacket={} \
                 routeexpired={} routeevicted={} hopdrop={} noroute={} unsent={} unseeded={} \
                 timebaseexhausted={} timebase={} undecoded={} echoes={} echorefused={}\r\n",
                status.tx_frames,
                status.rx_frames,
                self.node.peers().len(),
                self.node.link_count(),
                self.node.refused_links(),
                self.node.refused_peers(),
                self.node.refused_offers(),
                self.node.route_count(),
                u8::from(transport_on),
                transport.forwarded_announces,
                transport.forwarded_packets,
                transport.expired_routes,
                transport.evicted_routes,
                transport.hop_limit_dropped,
                transport.unroutable_packets,
                self.unsent,
                self.unseeded,
                self.timebase_exhausted,
                self.timebase.reserved_through(),
                self.undecoded,
                self.echoes,
                self.echo_refused,
            );
            return Flow::from(link.write_all(out.as_str().as_bytes()).await);
        }

        // The panels, as text: exactly the snapshot the screen renders, so a bench can
        // assert panel content over the wire while the TFT paints the same struct.
        if line == b"face" {
            let snapshot = self.face_snapshot(Self::now());
            let mut out = radio_face::Text::<224>::empty();
            let _ = write!(
                &mut out,
                "face name={} links={} peers=[",
                snapshot
                    .node
                    .as_ref()
                    .map(|n| n.name.as_str())
                    .unwrap_or("-"),
                snapshot.link_count,
            );
            for (index, peer) in snapshot.peers.iter().flatten().enumerate() {
                let _ = write!(
                    &mut out,
                    "{}{} age={}s",
                    if index > 0 { " " } else { "" },
                    peer.name,
                    peer.age_secs,
                );
            }
            let _ = write!(
                &mut out,
                "] overflow={} event={}\r\n",
                snapshot.peer_overflow,
                snapshot
                    .event
                    .as_ref()
                    .map(|e| e.text.as_str())
                    .unwrap_or("-"),
            );
            return Flow::from(link.write_all(out.as_str().as_bytes()).await);
        }

        // `replay reset` starts a fresh replay node, so a run is not contaminated by the one
        // before it. The desk half compares against a fresh node per fixture.
        if line == b"replay reset" {
            self.replay = None;
            return Flow::from(link.write_all(b"replay reset\r\n").await);
        }

        if let Some(rest) = line.strip_prefix(b"replay poll ") {
            return self.on_replay_poll(link, rest).await;
        }

        if let Some(rest) = line.strip_prefix(b"replay ") {
            return self.on_replay(link, rest).await;
        }

        Flow::Continue
    }

    /// `replay poll <now> <hex-blob>` — advance the replay node's own timers.
    ///
    /// The exact typed blob comes from the host rather than this board's durable generator,
    /// which keeps replay deterministic and prevents a test harness from consuming a live
    /// ordinal lease.
    async fn on_replay_poll<L: HostLink>(&mut self, link: &mut L, rest: &[u8]) -> Flow {
        let Some((now, hex)) = split_once(rest, b' ') else {
            return Flow::from(link.write_all(b"replay malformed\r\n").await);
        };
        let Some(now) = parse_u64(now) else {
            return Flow::from(link.write_all(b"replay bad clock\r\n").await);
        };
        let mut wire = [0_u8; RAND_HASH_LEN];
        if replay::from_hex(hex, &mut wire) != Some(RAND_HASH_LEN) {
            return Flow::from(link.write_all(b"replay bad blob\r\n").await);
        }

        let node = self
            .replay
            .get_or_insert_with(|| alloc::boxed::Box::new(replay::replay_node()));
        let blob = AnnounceBlob::from_wire(wire);
        let encoded = replay::encode_actions(&node.poll(now, RADIO, Some(&blob)));
        self.report_actions(link, &encoded).await
    }

    /// Write one encoded set of actions back as `actions <hex>`.
    async fn report_actions<L: HostLink>(&self, link: &mut L, encoded: &[u8]) -> Flow {
        let mut out = alloc::vec![0_u8; encoded.len() * 2];
        let written = replay::to_hex(encoded, &mut out);
        if link.write_all(b"actions ").await.is_err()
            || link.write_all(&out[..written]).await.is_err()
        {
            return Flow::Detach;
        }
        Flow::from(link.write_all(b"\r\n").await)
    }

    /// `replay <now> <hex-packet>` — feed one packet to the replay node and report what it
    /// decided, in the encoding the desk half asserts.
    async fn on_replay<L: HostLink>(&mut self, link: &mut L, rest: &[u8]) -> Flow {
        let Some((now, hex)) = split_once(rest, b' ') else {
            return Flow::from(link.write_all(b"replay malformed\r\n").await);
        };
        let Some(now) = parse_u64(now) else {
            return Flow::from(link.write_all(b"replay bad clock\r\n").await);
        };

        let mut frame = [0_u8; selvage::MAX_RADIO_FRAME_LEN];
        let Some(len) = replay::from_hex(hex, &mut frame) else {
            return Flow::from(link.write_all(b"replay bad hex\r\n").await);
        };

        let node = self
            .replay
            .get_or_insert_with(|| alloc::boxed::Box::new(replay::replay_node()));
        // A frame that is not a packet produces no actions, which is an answer rather than
        // an error: the desk half expects the same empty set for the same bytes.
        let encoded = match Packet::decode(&frame[..len]) {
            Ok(packet) => replay::encode_actions(&node.ingest(RADIO, &packet, now)),
            Err(_) => replay::encode_nothing(),
        };
        self.report_actions(link, &encoded).await
    }
}

/// Split at the first `sep`, dropping it. `None` if it is not there.
fn split_once(text: &[u8], sep: u8) -> Option<(&[u8], &[u8])> {
    let at = text.iter().position(|b| *b == sep)?;
    Some((&text[..at], &text[at + 1..]))
}

fn parse_u64(text: &[u8]) -> Option<u64> {
    if text.is_empty() {
        return None;
    }
    let mut value: u64 = 0;
    for byte in text {
        let digit = byte.checked_sub(b'0').filter(|d| *d < 10)?;
        value = value.checked_mul(10)?.checked_add(u64::from(digit))?;
    }
    Some(value)
}
