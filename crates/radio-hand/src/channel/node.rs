//! The node channel: the board as a Retinue node that answers for itself.
//!
//! The trunk personality. Where the modem channel holds no protocol state and does what the
//! host says, this one holds the board's identity, its address book, and its links, and
//! decides for itself. A host attached to it is an observer, not a driver.
//!
//! # What this channel is
//!
//! A driver for [`retinue::node::Node`], which is executor-neutral by construction: it never
//! acts, it decides, returning [`Action`]s for a shell to perform. This is that shell. The
//! division is what lets the same protocol code run against desktop fixtures and on a board
//! with 256 KB, and it is why gate N3's done condition can ask the two to agree.
//!
//! Everything the node cannot do for itself arrives from the executive: the radio to send
//! by, the entropy an announce needs, and the clock.

extern crate alloc;

use core::fmt::Write as _;

use embassy_time::{Duration, Instant};
use lora_phy::DelayNs;
use lora_phy::mod_traits::RadioKind;
use radio_face::{EventKind, Text, UiEvent};
use retinue::announce::RAND_HASH_LEN;
use retinue::hash::AddressHash;
use retinue::node::{Action, Actions, InterfaceId, Node};
use retinue::packet::Packet;

use crate::channel::{Channel, ChannelInfo, Event};
use crate::executive::Executive;
use crate::link::{Flow, HostLink};
use crate::replay;

/// The radio, as the node numbers its interfaces. One radio, so one number.
const RADIO: InterfaceId = 0;

/// How often the node's own timers are advanced.
///
/// Not the announce cadence — that is the node's, and defaults to ten minutes. This is the
/// granularity at which it gets to notice one is due. Five seconds is loose enough to cost a
/// battery board almost nothing and tight enough for the link timeouts and resource
/// retransmits `poll` will own as the gates land; it wants revisiting when it does.
const BEAT: Duration = Duration::from_secs(5);

/// The longest host line this channel accumulates.
///
/// A replay line is `replay <now> <hex>`, and the hex is a whole radio frame, so this is
/// two characters per byte plus room for the verb and the clock. Anything longer is refused
/// rather than truncated: a half-read packet that decoded anyway would be the worst possible
/// outcome for a facility whose entire job is proving two implementations agree.
const MAX_LINE: usize = 2 * selvage::MAX_RADIO_FRAME_LEN + 40;

/// The longest wait, in beats, between re-attempts of an announce the radio would not
/// carry. At a five-second beat this is about two and a half minutes.
///
/// The node stamps its announce when it *decides* to send one, so a frame the shell could
/// not put on the air would otherwise cost a whole announce interval of invisibility — a
/// ten-second jam making the board unfindable for ten minutes, which is what the hardware
/// showed. The retry backs off rather than counting down to zero: a fixed budget is spent
/// while the channel is still busy and gives up exactly when the air clears, which is the
/// wrong moment. Backing off instead keeps trying forever, at a cost that decays to a CAD
/// check every couple of minutes — cheap enough for a board whose radio is truly dead.
const ANNOUNCE_RETRY_MAX_BEATS: u8 = 32;

/// The board as a Retinue node.
pub struct NodeChannel<const PEERS: usize = 32, const ACTIONS: usize = 8, const LINKS: usize = 4> {
    pub(super) node: Node<PEERS, ACTIONS, LINKS>,
    /// Host bytes accumulated since the last newline. Host reads arrive in 64-byte chunks
    /// and a replay line is several hundred bytes, so a line spans many of them.
    line: heapless::Vec<u8, MAX_LINE>,
    /// Set when a line overran the buffer, so the rest of it is discarded to the next
    /// newline instead of being read as the start of a new command.
    line_lost: bool,
    /// The node a replay runs against: a fixed test identity, never the board's own, built
    /// on demand so a board that is never asked to replay pays nothing for the facility.
    replay: Option<alloc::boxed::Box<Node<PEERS, ACTIONS, LINKS>>>,
    /// Frames the node asked for that never reached the air. Counted rather than queued: a
    /// retransmit is the protocol's decision, not the shell's, and a shell that silently
    /// buffers would be lying to it about what happened.
    pub(super) unsent: u16,
    /// Announces skipped because the board could not produce entropy.
    unseeded: u16,
    /// Frames that arrived but did not decode as a packet. Ordinary weather on a shared
    /// band, where any Meshtastic or MeshCore traffic on the same sync word lands here.
    undecoded: u16,
    /// Resources echoed back on their link by the loopback service.
    echoes: u16,
    /// Echoes refused because the link was gone or a transfer still held it.
    echo_refused: u16,
    /// When each recently-heard peer last announced, most recent last. The address book
    /// holds identity and keys; this holds the one thing it deliberately does not, a clock,
    /// so the Peers panel can show genuine ages instead of a projected guess.
    pub(super) heard: heapless::Vec<(AddressHash, u64), 8>,
    /// The face's event line: the last thing worth telling a passer-by.
    pub(super) last_event: Option<UiEvent>,
    /// Beats remaining before the next announce re-attempt; zero when none is pending.
    announce_retry_in: u8,
    /// The current wait between re-attempts, doubling on each failure up to
    /// [`ANNOUNCE_RETRY_MAX_BEATS`].
    announce_retry_wait: u8,
}

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize>
    NodeChannel<PEERS, ACTIONS, LINKS>
{
    pub fn new(node: Node<PEERS, ACTIONS, LINKS>) -> Self {
        Self {
            node,
            line: heapless::Vec::new(),
            line_lost: false,
            replay: None,
            unsent: 0,
            unseeded: 0,
            undecoded: 0,
            echoes: 0,
            echo_refused: 0,
            heard: heapless::Vec::new(),
            last_event: None,
            announce_retry_in: 0,
            announce_retry_wait: 0,
        }
    }

    /// The node, for a host that wants to report on it. N6's panels read through here.
    pub fn node(&self) -> &Node<PEERS, ACTIONS, LINKS> {
        &self.node
    }

    /// The board's clock, in the unit the node counts in.
    fn now() -> u64 {
        Instant::now().as_millis()
    }

    /// Carry out what the node decided.
    ///
    /// Sends go on the air through the executive, so they pass whatever the executive
    /// enforces. A completed inbound resource is echoed back on its link — the loopback
    /// service, this node's first and so far only application. Everything else is a report:
    /// the node has already recorded it, and a shell that has no face for it yet may simply
    /// let it by.
    async fn perform<L, RK, DLY>(
        &mut self,
        exec: &mut Executive<'_, RK, DLY>,
        link: &mut L,
        actions: Actions<ACTIONS>,
    ) -> Flow
    where
        L: HostLink,
        RK: RadioKind,
        DLY: DelayNs,
    {
        let _ = link;
        // The echo's advertisement, decided while walking the actions and sent after them,
        // so a transfer's own proof always leaves before the reply that answers it.
        let mut echo: Option<Actions<ACTIONS>> = None;
        for action in actions {
            match action {
                Action::Send { packet, .. } => {
                    self.transmit(exec, packet).await;
                }
                // The loopback service: what arrives whole goes back whole, on the same
                // link. This is what N5's byte-exact both-directions receipt drives, and
                // until the panels land it is the one way a peer can make the board speak.
                Action::Resource { link_id, data } => {
                    let mut random_hash = [0_u8; retinue::resource::RANDOM_HASH_LEN];
                    let mut iv = [0_u8; retinue::token::IV_LEN];
                    if exec.random(&mut random_hash).is_err() || exec.random(&mut iv).is_err() {
                        self.unseeded = self.unseeded.saturating_add(1);
                        continue;
                    }
                    let mut label = Text::<24>::empty();
                    let _ = write!(&mut label, "echo {}b", data.len());
                    match self
                        .node
                        .publish(link_id, RADIO, &data, random_hash, &iv, Self::now())
                    {
                        Some(actions) => {
                            self.echoes = self.echoes.saturating_add(1);
                            self.note_event(EventKind::Delivered, label.as_str());
                            echo = Some(actions);
                        }
                        // The link vanished or a transfer is still running on it. Counted,
                        // not retried: the peer that wants the echo will ask again.
                        None => self.echo_refused = self.echo_refused.saturating_add(1),
                    }
                }
                // The face's living panels: peers stamp the recency table, links write the
                // event line. All of it is this node's own state; nothing is projected.
                Action::Learned { destination } => {
                    self.note_heard(destination, Self::now());
                }
                Action::LinkUp { .. } => {
                    self.note_event(EventKind::Info, "link up");
                }
                Action::LinkDown { .. } => {
                    self.note_event(EventKind::Info, "link down");
                }
                Action::Data { .. } => {}
            }
        }
        if let Some(actions) = echo {
            for action in actions {
                if let Action::Send { packet, .. } = action {
                    self.transmit(exec, packet).await;
                }
            }
        }
        Flow::Continue
    }

    /// Put one packet on the air, keeping the face and the counters honest.
    async fn transmit<RK, DLY>(&mut self, exec: &mut Executive<'_, RK, DLY>, packet: Packet)
    where
        RK: RadioKind,
        DLY: DelayNs,
    {
        let bytes = packet.encode();
        if bytes.len() > selvage::MAX_RADIO_FRAME_LEN
            || exec.transmit(&bytes).await != selvage::TX_ACCEPTED
        {
            self.unsent = self.unsent.saturating_add(1);
            return;
        }
        let status = exec.status_mut();
        status.tx_frames = status.tx_frames.saturating_add(1);
        status.last_tx = radio_face::TxResult::Sent {
            frame_len: bytes.len() as u16,
        };
        exec.publish(radio_face::LedSignal::Activity);
    }
}

/// One whole host line, from `node` or `replay`.
impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize>
    NodeChannel<PEERS, ACTIONS, LINKS>
{
    async fn on_line<L, RK, DLY>(
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
                 undecoded={} echoes={} echorefused={}\r\n",
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

    /// `replay poll <now> <hex-seed>` — advance the replay node's own timers.
    ///
    /// The seed comes from the host rather than the board's RNG, which is the only way the
    /// announce it builds can be compared to anything. The protocol layer already refuses to
    /// hold an RNG for exactly this reason; here that decision pays.
    async fn on_replay_poll<L: HostLink>(&mut self, link: &mut L, rest: &[u8]) -> Flow {
        let Some((now, hex)) = split_once(rest, b' ') else {
            return Flow::from(link.write_all(b"replay malformed\r\n").await);
        };
        let Some(now) = parse_u64(now) else {
            return Flow::from(link.write_all(b"replay bad clock\r\n").await);
        };
        let mut seed = [0_u8; RAND_HASH_LEN];
        if replay::from_hex(hex, &mut seed) != Some(RAND_HASH_LEN) {
            return Flow::from(link.write_all(b"replay bad seed\r\n").await);
        }

        let node = self
            .replay
            .get_or_insert_with(|| alloc::boxed::Box::new(replay::replay_node()));
        let encoded = replay::encode_actions(&node.poll(now, RADIO, &seed));
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

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize> ChannelInfo
    for NodeChannel<PEERS, ACTIONS, LINKS>
{
    /// Only where no host line is half-read. A replay line is several hundred bytes and
    /// arrives across many host reads, so a fragment of one must never be mistaken for a
    /// board probe — which is exactly what this trait method exists for.
    fn at_boundary(&self) -> bool {
        self.line.is_empty()
    }

    fn heartbeat(&self) -> Option<Duration> {
        Some(BEAT)
    }

    /// Yes. A node with no host attached is still a node: it announces, it answers links, and
    /// it keeps its own timers. Gating any of that on a USB cable would make the board a
    /// peripheral of a computer, which is the opposite of what this personality is for.
    fn without_host(&self) -> bool {
        true
    }
}

impl<L, RK, DLY, const PEERS: usize, const ACTIONS: usize, const LINKS: usize> Channel<L, RK, DLY>
    for NodeChannel<PEERS, ACTIONS, LINKS>
where
    L: HostLink,
    RK: RadioKind,
    DLY: DelayNs,
{
    /// Names itself and its address, so a host can tell at a glance which personality
    /// answered without having to ask.
    async fn start(&mut self, exec: &mut Executive<'_, RK, DLY>, link: &mut L) -> Flow {
        exec.request_rx();
        // The panels are live from the first moment of a session rather than a beat later.
        exec.publish_host(self.face_snapshot(Self::now()));
        let mut line = [0_u8; 32];
        let text = b"channel=node dest=";
        line[..text.len()].copy_from_slice(text);
        let mut at = text.len();
        for byte in &self.node.destination().as_slice()[..4] {
            line[at] = hex_digit(byte >> 4);
            line[at + 1] = hex_digit(byte & 0x0f);
            at += 2;
        }
        line[at] = b'\r';
        line[at + 1] = b'\n';
        Flow::from(link.write_all(&line[..at + 2]).await)
    }

    async fn serve(
        &mut self,
        exec: &mut Executive<'_, RK, DLY>,
        link: &mut L,
        event: Event<'_>,
    ) -> Flow {
        match event {
            Event::RadioFrame { frame, rssi, snr } => {
                let Ok(packet) = Packet::decode(frame) else {
                    self.undecoded = self.undecoded.saturating_add(1);
                    return Flow::Continue;
                };
                let status = exec.status_mut();
                status.rx_frames = status.rx_frames.saturating_add(1);
                status.last_rx = Some(radio_face::RxSummary {
                    frame_len: frame.len() as u16,
                    rssi_dbm: rssi,
                    snr_tenths_db: snr.saturating_mul(10),
                });
                status.last_wake = radio_face::WakeSource::Radio;
                exec.publish(radio_face::LedSignal::Activity);

                let actions = self.node.ingest(RADIO, &packet, Self::now());
                self.perform(exec, link, actions).await
            }
            Event::Beat => {
                // The face first: every beat republishes the four panels from local state,
                // which is what keeps them alive and fresh with no host attached. The
                // snapshot's 15 s validity spans three beats of slack.
                exec.publish_host(self.face_snapshot(Self::now()));

                let mut rand_hash = [0_u8; RAND_HASH_LEN];
                if exec.random(&mut rand_hash).is_err() {
                    // No entropy, so no announce. The node's timer is untouched, so the next
                    // beat tries again rather than the board going quiet forever.
                    self.unseeded = self.unseeded.saturating_add(1);
                    return Flow::Continue;
                }
                // A pending re-attempt comes due before the poll that would carry it.
                if self.announce_retry_in > 0 {
                    self.announce_retry_in -= 1;
                    if self.announce_retry_in == 0 {
                        self.node.retry_announce();
                    }
                }

                let actions = self.node.poll(Self::now(), RADIO, &rand_hash);
                let unsent_before = self.unsent;
                let flow = self.perform(exec, link, actions).await;

                // Something the node's own timers asked for did not reach the air. Resource
                // retransmits carry their own stamps and will come round again; the
                // announce is the one whose stamp would otherwise swallow the failure, so
                // it is the one scheduled to try again.
                if self.unsent != unsent_before {
                    self.announce_retry_wait = self
                        .announce_retry_wait
                        .max(1)
                        .saturating_mul(2)
                        .min(ANNOUNCE_RETRY_MAX_BEATS);
                    self.announce_retry_in = self.announce_retry_wait;
                } else if self.announce_retry_in == 0 {
                    // Nothing failed and nothing is waiting: the backoff has done its work.
                    self.announce_retry_wait = 0;
                }
                flow
            }
            // A host is an observer of this channel: it may ask what the node has seen and
            // done (`node`, `face`), and it may drive a replay. The panels need none of
            // this — they publish from local state on the beat.
            Event::HostBytes(bytes) => {
                for &byte in bytes {
                    if byte != b'\n' {
                        if self.line.push(byte).is_err() {
                            self.line_lost = true;
                            self.line.clear();
                        }
                        continue;
                    }
                    // Taken out of `self` so the handler may borrow the rest of it.
                    let overran = core::mem::take(&mut self.line_lost);
                    let mut line = core::mem::take(&mut self.line);
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    let flow = if overran {
                        Flow::from(link.write_all(b"line too long\r\n").await)
                    } else {
                        self.on_line(exec, link, &line).await
                    };
                    if flow == Flow::Detach {
                        return flow;
                    }
                }
                Flow::Continue
            }
        }
    }
}

fn hex_digit(nibble: u8) -> u8 {
    match nibble {
        0..=9 => b'0' + nibble,
        _ => b'a' + (nibble - 10),
    }
}
