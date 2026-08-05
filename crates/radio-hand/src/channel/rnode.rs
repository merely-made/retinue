//! The RNode channel: the board as a radio stock Reticulum drives.
//!
//! Shaped like the modem channel and for the same reason — it holds no protocol state of its
//! own, every decision is the host's — but speaking the protocol the rest of the world
//! already has software for. That is the whole of its value: Sideband, MeshChat and NomadNet
//! drive an RNode, so with this selected they drive this board, unmodified.
//!
//! The wire is [`crate::rnode`], pinned by black-box capture. What is here is the part that
//! needs a radio: when settings are committed, what a transmit is allowed to do, and what a
//! received frame looks like on the way back up.
//!
//! # Two things it does not do
//!
//! **No text banner.** A real RNode says nothing until it is asked, and the host opens with
//! binary. [`ChannelInfo::banner`] is how this channel declines the firmware's greeting.
//!
//! **No unsolicited stats.** A real device emits channel-utilisation and battery frames
//! continuously; `tulle` ignores them and so, presumably, can a host that only needs the
//! link. They are a known omission rather than an oversight, and cost airtime nothing since
//! they never leave the cable.

use core::fmt::Write as _;

use crate::channel::{Channel, ChannelInfo, Event};
use crate::executive::Executive;
use crate::link::{Flow, HostLink};
use crate::rnode::{self, Command, cmd};

/// Bytes needed to encode the largest frame this channel sends.
///
/// A received air frame, escaped worst-case, plus its delimiters. Sized from the *air* limit
/// rather than the protocol's 500, because this is the receive path and 255 is what the radio
/// can hand up.
const OUT_BUF: usize = rnode::encoded_max(rnode::MAX_AIR_FRAME);

/// What the host has asked for and what has come of it.
///
/// Split out from the channel so the frame handler can borrow it while the deframer still
/// holds the frame being handled.
struct State {
    pending: rnode::Pending,
    /// Whether the host has turned the radio on. Nothing is transmitted or forwarded until
    /// it has: a host that has not committed a profile has not chosen a channel, and putting
    /// its packets on whatever carrier the board booted with would be worse than refusing.
    radio_on: bool,
    /// Frames the host asked to send that were longer than the radio carries. The 255/500
    /// fork, counted rather than truncated.
    over_air_mtu: u16,
    /// Transmits the executive refused, for any reason: no region, spent duty, radio fault.
    refused: u16,
    /// Commands this device does not implement.
    unhandled: u16,
    /// Received frames dropped because the host had not turned the radio on.
    dropped: u16,
}

/// The board as an RNode.
pub struct RNodeChannel {
    deframer: rnode::Deframer,
    state: State,
    out: [u8; OUT_BUF],
}

impl Default for RNodeChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl RNodeChannel {
    pub const fn new() -> Self {
        Self {
            deframer: rnode::Deframer::new(),
            state: State {
                pending: rnode::Pending::new(),
                radio_on: false,
                over_air_mtu: 0,
                refused: 0,
                unhandled: 0,
                dropped: 0,
            },
            out: [0; OUT_BUF],
        }
    }

    /// Answer this channel's own text probe.
    ///
    /// Channel-local, following the node channel's `node` and `face` probes: the firmware's
    /// probe table has no way to reach a channel's counters, and these are the ones that
    /// explain the failure a host cannot see. "The app connects but nothing sends" is
    /// `overmtu`, `refused` or `dropped`, and each names a different fix.
    async fn on_line<L: HostLink>(&mut self, line: &[u8], link: &mut L) -> Flow {
        let line = line
            .strip_suffix(b"\r\n")
            .or_else(|| line.strip_suffix(b"\n"))
            .unwrap_or(line);
        if line != b"rnode" {
            return Flow::Continue;
        }
        let mut out = radio_face::Text::<128>::empty();
        let _ = write!(
            &mut out,
            "rnode radio={} overmtu={} refused={} unhandled={} dropped={} airmtu={}\r\n",
            if self.state.radio_on { "on" } else { "off" },
            self.state.over_air_mtu,
            self.state.refused,
            self.state.unhandled,
            self.state.dropped,
            rnode::MAX_AIR_FRAME,
        );
        Flow::from(link.write_all(out.as_str().as_bytes()).await)
    }
}

impl ChannelInfo for RNodeChannel {
    /// Only where no KISS frame is half-read, so a `status` line inside a transmit payload is
    /// carried rather than obeyed.
    fn at_boundary(&self) -> bool {
        self.deframer.is_idle()
    }

    /// A real RNode greets nobody. The host opens with `DETECT` and expects the first bytes
    /// back to be a KISS frame.
    fn banner(&self) -> bool {
        false
    }
}

/// Send one device-to-host frame.
async fn reply<L: HostLink>(link: &mut L, out: &mut [u8], command: u8, payload: &[u8]) -> Flow {
    match rnode::encode(command, payload, out) {
        Some(len) => Flow::from(link.write_all(&out[..len]).await),
        // Unreachable with `out` sized from the protocol, and silent rather than fatal if it
        // ever is not: a reply that will not fit is a bug in this file, not a dead host.
        None => Flow::Continue,
    }
}

/// Handle one deframed command.
///
/// A free function rather than a method so the deframer can keep lending out the frame while
/// the state it updates is borrowed separately.
async fn handle<L, RK, DLY>(
    frame: &[u8],
    state: &mut State,
    out: &mut [u8],
    exec: &mut Executive<'_, RK, DLY>,
    link: &mut L,
) -> Flow
where
    L: HostLink,
    RK: lora_phy::mod_traits::RadioKind,
    DLY: lora_phy::DelayNs,
{
    let Some(command) = rnode::decode(frame) else {
        return Flow::Continue;
    };

    // The settings the host is accumulating, and the answers that need no radio: the probes,
    // and each setting echoed back. Both live in the sans-io half, where the oracle capture
    // can be replayed against them on a desk.
    state.pending.accept(&command);
    if let Some((marker, payload)) = rnode::answer(&command) {
        return reply(link, out, marker, &payload).await;
    }

    match command {
        // The commit. Everything the host set lands on the radio here, through the executive,
        // so the regulatory floor rules this channel exactly as it rules the others: a
        // frequency outside the region is refused whole and power is clamped to the minimum
        // of request, region, and hardware. The host is told plainly whether the radio came
        // up, because `RADIO_STATE` echoing 0 is the protocol's own way of saying it did not.
        Command::RadioState(true) => {
            let applied = match state.pending.profile() {
                Some(profile) => exec.apply_profile(&profile).await == selvage::CONFIG_ACCEPTED,
                None => false,
            };
            state.radio_on = applied;
            if applied {
                exec.request_rx();
            }
            reply(link, out, cmd::RADIO_STATE, &[u8::from(applied)]).await
        }
        Command::RadioState(false) => {
            state.radio_on = false;
            reply(link, out, cmd::RADIO_STATE, &[0]).await
        }

        Command::Data(packet) => {
            if !state.radio_on {
                state.refused = state.refused.saturating_add(1);
                return reply(link, out, cmd::ERROR, &[selvage::TX_RADIO_FAULT]).await;
            }
            if packet.len() > rnode::MAX_AIR_FRAME {
                // The 255/500 fork, told rather than hidden. Truncating would put a corrupt
                // packet on the air and report success, which is the worse of the two.
                state.over_air_mtu = state.over_air_mtu.saturating_add(1);
                return reply(link, out, cmd::ERROR, &[selvage::TX_TOO_LONG]).await;
            }
            let code = exec.transmit(packet).await;
            if code == selvage::TX_ACCEPTED {
                return Flow::Continue;
            }
            state.refused = state.refused.saturating_add(1);
            reply(link, out, cmd::ERROR, &[code]).await
        }

        Command::Unhandled(_) => {
            state.unhandled = state.unhandled.saturating_add(1);
            Flow::Continue
        }

        // Answered above. Listed rather than caught by a wildcard so a command added to the
        // decoder cannot silently fall through to nothing here.
        Command::Detect(_)
        | Command::FirmwareVersion
        | Command::Platform
        | Command::Mcu
        | Command::Frequency(_)
        | Command::Bandwidth(_)
        | Command::TxPower(_)
        | Command::SpreadingFactor(_)
        | Command::CodingRate(_) => Flow::Continue,
    }
}

impl<L, RK, DLY> Channel<L, RK, DLY> for RNodeChannel
where
    L: HostLink,
    RK: lora_phy::mod_traits::RadioKind,
    DLY: lora_phy::DelayNs,
{
    /// Nothing is written and the radio is left alone. A session begins with the host
    /// detecting the device and ends with it choosing a channel; anything this wrote first
    /// would arrive ahead of the `DETECT` answer the host is waiting for.
    async fn start(&mut self, exec: &mut Executive<'_, RK, DLY>, link: &mut L) -> Flow {
        let _ = (exec, link);
        // A new session starts at a frame boundary, and on a radio the new host has not yet
        // configured: its predecessor's channel is not this one's to keep serving.
        self.deframer.reset();
        self.state.pending = rnode::Pending::new();
        self.state.radio_on = false;
        Flow::Continue
    }

    async fn serve(
        &mut self,
        exec: &mut Executive<'_, RK, DLY>,
        link: &mut L,
        event: Event<'_>,
    ) -> Flow {
        match event {
            Event::HostBytes(bytes) => {
                // At a boundary, everything before the first delimiter is not frame data: it
                // is noise, or a person at a terminal. Feeding it to the deframer would leave
                // this channel looking permanently mid-frame, and the probe that switches the
                // board back to the modem is only recognised at a boundary — so a mistyped
                // line would strand the board in this channel until it was reflashed.
                let framed = if self.deframer.is_idle() {
                    let start = bytes.iter().position(|&byte| byte == selvage::kiss::FEND);
                    let text = &bytes[..start.unwrap_or(bytes.len())];
                    if !text.is_empty() && self.on_line(text, link).await == Flow::Detach {
                        return Flow::Detach;
                    }
                    &bytes[start.unwrap_or(bytes.len())..]
                } else {
                    bytes
                };

                for &byte in framed {
                    if self.deframer.push(byte)
                        && handle(
                            self.deframer.frame(),
                            &mut self.state,
                            &mut self.out,
                            exec,
                            link,
                        )
                        .await
                            == Flow::Detach
                    {
                        return Flow::Detach;
                    }
                }
                Flow::Continue
            }

            // The receive triplet, in the captured order: signal first, then the packet. A
            // host reads the stats as belonging to the frame that follows them, so emitting
            // them afterwards would misattribute every report by one frame.
            Event::RadioFrame { frame, rssi, snr } => {
                if !self.state.radio_on {
                    self.state.dropped = self.state.dropped.saturating_add(1);
                    return Flow::Continue;
                }
                let stats = [
                    (cmd::STAT_RSSI, rnode::rssi_wire(rssi)),
                    (cmd::STAT_SNR, rnode::snr_wire(snr)),
                ];
                for (marker, value) in stats {
                    if reply(link, &mut self.out, marker, &[value]).await == Flow::Detach {
                        return Flow::Detach;
                    }
                }
                reply(link, &mut self.out, cmd::DATA, frame).await
            }

            // Never delivered: this channel asks for no heartbeat.
            Event::Beat => Flow::Continue,
        }
    }
}
