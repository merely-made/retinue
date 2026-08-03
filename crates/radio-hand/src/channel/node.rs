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

use core::fmt::Write as _;

use embassy_time::{Duration, Instant};
use lora_phy::DelayNs;
use lora_phy::mod_traits::RadioKind;
use retinue::announce::RAND_HASH_LEN;
use retinue::node::{Action, Actions, InterfaceId, Node};
use retinue::packet::Packet;

use crate::channel::{Channel, ChannelInfo, Event};
use crate::executive::Executive;
use crate::link::{Flow, HostLink};

/// The radio, as the node numbers its interfaces. One radio, so one number.
const RADIO: InterfaceId = 0;

/// How often the node's own timers are advanced.
///
/// Not the announce cadence — that is the node's, and defaults to ten minutes. This is the
/// granularity at which it gets to notice one is due. Five seconds is loose enough to cost a
/// battery board almost nothing and tight enough for the link timeouts and resource
/// retransmits `poll` will own as the gates land; it wants revisiting when it does.
const BEAT: Duration = Duration::from_secs(5);

/// The board as a Retinue node.
pub struct NodeChannel<const PEERS: usize = 32, const ACTIONS: usize = 8, const LINKS: usize = 4> {
    node: Node<PEERS, ACTIONS, LINKS>,
    /// Frames the node asked for that never reached the air. Counted rather than queued: a
    /// retransmit is the protocol's decision, not the shell's, and a shell that silently
    /// buffers would be lying to it about what happened.
    unsent: u16,
    /// Announces skipped because the board could not produce entropy.
    unseeded: u16,
    /// Frames that arrived but did not decode as a packet. Ordinary weather on a shared
    /// band, where any Meshtastic or MeshCore traffic on the same sync word lands here.
    undecoded: u16,
}

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize>
    NodeChannel<PEERS, ACTIONS, LINKS>
{
    pub fn new(node: Node<PEERS, ACTIONS, LINKS>) -> Self {
        Self {
            node,
            unsent: 0,
            unseeded: 0,
            undecoded: 0,
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
    /// enforces. Everything else is a report: the node has already recorded it, and a shell
    /// that has no face for it yet may simply let it by.
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
        for action in actions {
            if let Action::Send { packet, .. } = action {
                let bytes = packet.encode();
                if bytes.len() > selvage::MAX_RADIO_FRAME_LEN
                    || exec.transmit(&bytes).await != selvage::TX_ACCEPTED
                {
                    self.unsent = self.unsent.saturating_add(1);
                    continue;
                }
                let status = exec.status_mut();
                status.tx_frames = status.tx_frames.saturating_add(1);
                status.last_tx = radio_face::TxResult::Sent {
                    frame_len: bytes.len() as u16,
                };
                exec.publish(radio_face::LedSignal::Activity);
            }
        }
        Flow::Continue
    }
}

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize> ChannelInfo
    for NodeChannel<PEERS, ACTIONS, LINKS>
{
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
                let mut rand_hash = [0_u8; RAND_HASH_LEN];
                if exec.random(&mut rand_hash).is_err() {
                    // No entropy, so no announce. The node's timer is untouched, so the next
                    // beat tries again rather than the board going quiet forever.
                    self.unseeded = self.unseeded.saturating_add(1);
                    return Flow::Continue;
                }
                let actions = self.node.poll(Self::now(), RADIO, &rand_hash);
                self.perform(exec, link, actions).await
            }
            // The host protocol for this channel is gate N6, where the panels drive from
            // board-local state. Until then a host is an observer, and `node` is the one
            // thing it may ask: what this node has actually seen and done.
            Event::HostBytes(bytes) => {
                if bytes == b"node\n" || bytes == b"node\r\n" {
                    let status = exec.status();
                    let mut line = radio_face::Text::<160>::empty();
                    let _ = write!(
                        &mut line,
                        "node tx={} rx={} peers={} links={} refused={} \
                         unsent={} unseeded={} undecoded={}\r\n",
                        status.tx_frames,
                        status.rx_frames,
                        self.node.peers().len(),
                        self.node.link_count(),
                        self.node.refused_links(),
                        self.unsent,
                        self.unseeded,
                        self.undecoded,
                    );
                    return Flow::from(link.write_all(line.as_str().as_bytes()).await);
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
