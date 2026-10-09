//! The node channel: the board as a Retinue node that answers for itself.
//!
//! The trunk personality: it holds the board's identity, address book, and links, and decides
//! for itself; an attached host observes rather than drives.
//!
//! [`retinue::node::Node`] never acts, it returns [`Action`]s; this is the shell that performs
//! them, which is what lets gate N3 ask desktop fixtures and the board to agree. The radio,
//! announce entropy, and the clock arrive from the executive.

extern crate alloc;

use core::fmt::Write as _;

use embassy_time::{Duration, Instant};
use lora_phy::DelayNs;
use lora_phy::mod_traits::RadioKind;
use radio_face::{EventKind, Text, UiEvent};
use retinue::announce::{ANNOUNCE_NONCE_LEN, AnnounceBlob, TimebaseGenerator};
use retinue::hash::AddressHash;
use retinue::node::{Action, Actions, InterfaceId, Node};
use retinue::packet::{Packet, PacketType};

use crate::channel::{Channel, ChannelInfo, Event};
use crate::executive::Executive;
use crate::link::{Flow, HostLink};

mod host;

/// The radio, as the node numbers its interfaces. One radio, so one number. A firmware
/// configuring the node per interface, such as its first-hop airtime, names this one.
pub const RADIO: InterfaceId = 0;

/// How often the node's own timers are advanced: the granularity at which it notices an
/// announce is due, not the announce cadence itself. Relayed announces, resource watchdogs
/// and link timers are timed finer, by [`ChannelInfo::wake_at`].
const BEAT: Duration = Duration::from_secs(5);

/// The longest host line: `replay <now> <hex>` with a whole radio frame in hex. Longer lines
/// are refused, never truncated, since a half-read packet that decoded anyway would defeat a
/// facility whose job is proving two implementations agree.
const MAX_LINE: usize = 2 * selvage::MAX_RADIO_FRAME_LEN + 40;

/// The longest wait, in beats (about 2.5 minutes), between re-attempts of an announce the
/// radio would not carry.
///
/// The node stamps an announce when it decides to send, so an unsent frame would otherwise
/// cost a whole announce interval of invisibility. The retry backs off forever rather than
/// spending a fixed budget, which would give up exactly when the air clears.
const ANNOUNCE_RETRY_MAX_BEATS: u8 = 32;

/// The face's event line for a link request that expired unanswered (Ruling 46).
pub const LINK_UNANSWERED: &str = "link unanswered";

/// Startup configuration refused before any channel state is retained.
#[derive(Debug)]
pub enum NodeChannelConfigError {
    Mtu(retinue::node::LogicalMtuError),
    Timebase(retinue::announce::TimebaseError),
}

/// The board as a Retinue node.
pub struct NodeChannel<const PEERS: usize = 32, const ACTIONS: usize = 8, const LINKS: usize = 4> {
    pub(super) node: Node<PEERS, ACTIONS, LINKS>,
    carrier: crate::retinue_carrier::RetinueCarrier,
    /// Host bytes accumulated since the last newline. Host reads arrive in 64-byte chunks
    /// and a replay line is several hundred bytes, so a line spans many of them.
    line: heapless::Vec<u8, MAX_LINE>,
    /// Set when a line overran the buffer, so the rest of it is discarded to the next
    /// newline instead of being read as the start of a new command.
    line_lost: bool,
    /// The node a replay runs against: a fixed test identity, never the board's own, built
    /// on demand so a board that is never asked to replay pays nothing for the facility.
    replay: Option<alloc::boxed::Box<Node<PEERS, ACTIONS, LINKS>>>,
    /// Frames the node asked for that never reached the air. Counted, not queued: a
    /// retransmit is the protocol's decision, not the shell's.
    pub(super) unsent: u16,
    /// The board's own announces among [`Self::unsent`], the one failure the shell retries.
    announce_unsent: u16,
    /// Announces skipped because the board could not produce entropy.
    unseeded: u16,
    /// Announces denied because this boot's durable lease is spent. This is a
    /// terminal condition until a reboot successfully reserves another lease;
    /// it must never turn into an uptime or entropy fallback.
    timebase_exhausted: u16,
    /// The checked generator built only from an already verified durable lease.
    timebase: TimebaseGenerator,
    /// Frames that arrived but did not decode as a packet. Ordinary weather on a shared
    /// band, where any Meshtastic or MeshCore traffic on the same sync word lands here.
    undecoded: u16,
    /// Resources echoed back on their link by the loopback service.
    echoes: u16,
    /// Echoes refused because the link was gone or a transfer still held it.
    echo_refused: u16,
    /// When each recently-heard peer last announced, most recent last: the clock the
    /// address book lacks, so the Peers panel shows genuine ages.
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
    pub fn new(
        node: Node<PEERS, ACTIONS, LINKS>,
        lease: crate::announce_reservation::ActiveLease,
    ) -> Result<Self, retinue::announce::TimebaseError> {
        Self::build(node, lease, Default::default())
    }

    /// Credentials and carrier MTU are fixed before this channel starts serving.
    pub fn new_with_carrier(
        mut node: Node<PEERS, ACTIONS, LINKS>,
        lease: crate::announce_reservation::ActiveLease,
        carrier: crate::retinue_carrier::RetinueCarrier,
    ) -> Result<Self, NodeChannelConfigError> {
        carrier
            .configure_node(&mut node)
            .map_err(NodeChannelConfigError::Mtu)?;
        Self::build(node, lease, carrier).map_err(NodeChannelConfigError::Timebase)
    }

    fn build(
        node: Node<PEERS, ACTIONS, LINKS>,
        lease: crate::announce_reservation::ActiveLease,
        carrier: crate::retinue_carrier::RetinueCarrier,
    ) -> Result<Self, retinue::announce::TimebaseError> {
        Ok(Self {
            node,
            carrier,
            line: heapless::Vec::new(),
            line_lost: false,
            replay: None,
            unsent: 0,
            announce_unsent: 0,
            unseeded: 0,
            timebase_exhausted: 0,
            timebase: TimebaseGenerator::firmware_lease(lease.floor(), lease.reserved_through())?,
            undecoded: 0,
            echoes: 0,
            echo_refused: 0,
            heard: heapless::Vec::new(),
            last_event: None,
            announce_retry_in: 0,
            announce_retry_wait: 0,
        })
    }

    /// The node, for a host that wants to report on it. N6's panels read through here.
    pub fn node(&self) -> &Node<PEERS, ACTIONS, LINKS> {
        &self.node
    }

    /// The board's clock, in the unit the node counts in.
    fn now() -> u64 {
        Instant::now().as_millis()
    }

    /// Mint one announce blob from the already durable lease.
    ///
    /// `now` schedules the node, but it is deliberately not the ordinal
    /// source. A hardware uptime or future real clock would otherwise jump
    /// through a whole lease without emits. The timebase is a logical sequence:
    /// each attempted announce advances it exactly once.
    fn next_announce_blob(
        timebase: &mut TimebaseGenerator,
        _scheduled_at: u64,
        nonce: [u8; ANNOUNCE_NONCE_LEN],
    ) -> Result<AnnounceBlob, retinue::announce::TimebaseError> {
        let ordinal = timebase.next(0)?;
        AnnounceBlob::mint(nonce, ordinal)
    }

    /// Only an announce the node has declared due may consume a lease ordinal.
    /// Keeping this pure makes the difference between a timer wake and an
    /// attempted send explicit, including the retry path.
    fn announce_blob_if_due(
        timebase: &mut TimebaseGenerator,
        due: bool,
        scheduled_at: u64,
        nonce: [u8; ANNOUNCE_NONCE_LEN],
    ) -> Result<Option<AnnounceBlob>, retinue::announce::TimebaseError> {
        if !due {
            return Ok(None);
        }
        Self::next_announce_blob(timebase, scheduled_at, nonce).map(Some)
    }

    /// Carry out what the node decided.
    ///
    /// Sends pass through the executive. A completed inbound resource is echoed back on its
    /// link (the loopback service); everything else is a report the node already recorded.
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
                    let own = is_own_announce(self.node.destination(), &packet);
                    if !self.transmit(exec, packet).await && own {
                        self.announce_unsent = self.announce_unsent.saturating_add(1);
                    }
                }
                // The loopback service: what arrives whole goes back whole, metadata and
                // all, on the same link. N5's byte-exact both-directions receipt drives this.
                Action::Resource {
                    link_id,
                    data,
                    metadata,
                } => {
                    let mut random_hash = [0_u8; retinue::resource::RANDOM_HASH_LEN];
                    let mut iv = [0_u8; retinue::token::IV_LEN];
                    if exec.random(&mut random_hash).is_err() || exec.random(&mut iv).is_err() {
                        self.unseeded = self.unseeded.saturating_add(1);
                        continue;
                    }
                    let mut label = Text::<24>::empty();
                    let _ = write!(&mut label, "echo {}b", data.len());
                    let now = Self::now();
                    let started = match &metadata {
                        Some(metadata) => self.node.publish_with_metadata(
                            link_id,
                            RADIO,
                            &data,
                            metadata,
                            random_hash,
                            &iv,
                            now,
                        ),
                        None => self
                            .node
                            .publish(link_id, RADIO, &data, random_hash, &iv, now),
                    };
                    match started {
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
                // A request this node opened got no answer. Not a link going down: none
                // came up. An existing event kind, so the face needs no new state.
                Action::LinkRequestTimedOut { .. } => {
                    self.note_event(EventKind::Failed, LINK_UNANSWERED);
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

    /// Put one packet on the air, keeping the face and the counters honest. Returns whether
    /// it went.
    async fn transmit<RK, DLY>(&mut self, exec: &mut Executive<'_, RK, DLY>, packet: Packet) -> bool
    where
        RK: RadioKind,
        DLY: DelayNs,
    {
        let Ok(bytes) = self.carrier.encode(&packet) else {
            self.unsent = self.unsent.saturating_add(1);
            return false;
        };
        if bytes.len() > selvage::MAX_RADIO_FRAME_LEN
            || exec.transmit(&bytes).await != selvage::TX_ACCEPTED
        {
            self.unsent = self.unsent.saturating_add(1);
            return false;
        }
        let status = exec.status_mut();
        status.tx_frames = status.tx_frames.saturating_add(1);
        status.last_tx = radio_face::TxResult::Sent {
            frame_len: bytes.len() as u16,
        };
        exec.publish(radio_face::LedSignal::Activity);
        true
    }
}

/// Whether `packet` is the board's own announce rather than a relay of someone else's.
fn is_own_announce(own: AddressHash, packet: &Packet) -> bool {
    packet.packet_type == PacketType::Announce && packet.destination == own
}

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize> ChannelInfo
    for NodeChannel<PEERS, ACTIONS, LINKS>
{
    /// Only where no host line is half-read, so a fragment of a replay line is never
    /// mistaken for a board probe.
    fn at_boundary(&self) -> bool {
        self.line.is_empty()
    }

    fn heartbeat(&self) -> Option<Duration> {
        Some(BEAT)
    }

    /// The node's next deadline: a relayed announce, so its jitter and retry keep RNS's timing
    /// rather than the beat's (`Transport.py` 765-829, 2338), a resource watchdog, or a link
    /// timer, whichever comes first.
    fn wake_at(&self) -> Option<Instant> {
        self.node.next_deadline().map(Instant::from_millis)
    }

    /// Yes. A node with no host attached still announces, answers links, and keeps its own
    /// timers.
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
        let mut line = [0_u8; 64];
        let text = b"channel=node state=node-timebase-v1 dest=";
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
                let Ok(packet) = self.carrier.decode(frame) else {
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

                // A pending re-attempt comes due before the poll that would carry it.
                if self.announce_retry_in > 0 {
                    self.announce_retry_in -= 1;
                    if self.announce_retry_in == 0 {
                        self.node.retry_announce();
                    }
                }

                let now = Self::now();
                let due = self.node.announce_due(now);
                let blob = if due {
                    let mut nonce = [0_u8; ANNOUNCE_NONCE_LEN];
                    if exec.random(&mut nonce).is_err() {
                        // No entropy, so no announce. The node's timer is untouched, so the
                        // next beat tries again rather than the board going quiet forever.
                        self.unseeded = self.unseeded.saturating_add(1);
                        None
                    } else {
                        match Self::announce_blob_if_due(&mut self.timebase, due, now, nonce) {
                            Ok(blob) => blob,
                            Err(_) => {
                                self.timebase_exhausted = self.timebase_exhausted.saturating_add(1);
                                self.note_event(EventKind::Failed, "timebase exhausted");
                                None
                            }
                        }
                    }
                } else {
                    None
                };
                let actions = self.node.poll(now, RADIO, blob.as_ref());
                let unsent_before = self.announce_unsent;
                let flow = self.perform(exec, link, actions).await;

                // The board's own announce did not reach the air, and its stamp would swallow
                // the failure, so it is rescheduled. Relays and resource retransmits carry
                // their own retries.
                if self.announce_unsent != unsent_before {
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
            // A relayed announce, resource or link timer came due between beats. Polling without a blob never
            // announces, so the beat keeps the board's own cadence and its retry.
            Event::Wake => {
                let actions = self.node.poll(Self::now(), RADIO, None);
                self.perform(exec, link, actions).await
            }
            // A host only observes (`node`, `face`) or drives a replay; the panels publish
            // from local state on the beat.
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

#[cfg(test)]
mod tests;
