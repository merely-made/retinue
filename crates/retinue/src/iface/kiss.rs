//! KISS framing and a sans-io host for a KISS TNC (`KISSInterface.py` 38-394).
//!
//! [`KissTnc`] owns no port and no clock: write [`KissTnc::startup`] once the port settles,
//! feed read bytes to [`KissTnc::receive`], ask [`KissTnc::is_ready`] before encoding the next
//! packet, and call [`KissTnc::poll`] at [`KissTnc::wake_at_ms`] for the flow-control unlock
//! and the station ID beacon. Holding packets back while not ready leaves them in the
//! endpoint's own queues, so the TNC never needs a queue of its own.

use alloc::vec::Vec;

use super::beacon::Beacon;

pub const FEND: u8 = 0xC0;
pub const FESC: u8 = 0xDB;
pub const TFEND: u8 = 0xDC;
pub const TFESC: u8 = 0xDD;

/// KISS command bytes (`KISSInterface.py` 43-52).
pub mod cmd {
    pub const DATA: u8 = 0x00;
    pub const TXDELAY: u8 = 0x01;
    pub const P: u8 = 0x02;
    pub const SLOTTIME: u8 = 0x03;
    pub const TXTAIL: u8 = 0x04;
    pub const READY: u8 = 0x0F;
}

/// The largest frame a KISS carrier deframes: the packet MTU plus the largest IFAC
/// (`KISSInterface.py` 102).
pub const HW_MTU: usize = 564;
/// RNS's bitrate assumption for a KISS TNC (`KISSInterface.py` 61).
pub const BITRATE_GUESS: u64 = 1_200;
/// A TNC that never answers READY is unlocked after this (`KISSInterface.py` 128, 353-358).
pub const FLOW_UNLOCK_MS: u64 = 5_000;

/// Encode one KISS frame: `FEND`, the command byte, the escaped payload, `FEND`.
pub fn encode(command: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = alloc::vec![FEND, command];
    for &byte in payload {
        match byte {
            FEND => out.extend([FESC, TFEND]),
            FESC => out.extend([FESC, TFESC]),
            other => out.push(other),
        }
    }
    out.push(FEND);
    out
}

/// TNC parameters with RNS's defaults (`KISSInterface.py` 84-134).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TncConfig {
    pub preamble_ms: u32,
    pub txtail_ms: u32,
    pub persistence: u8,
    pub slottime_ms: u32,
    /// Hold each frame until the TNC answers READY.
    pub flow_control: bool,
}

impl Default for TncConfig {
    fn default() -> Self {
        Self {
            preamble_ms: 350,
            txtail_ms: 20,
            persistence: 64,
            slottime_ms: 20,
            flow_control: false,
        }
    }
}

/// Reassembles KISS frames as (command with its port nibble stripped, payload).
#[derive(Debug, Default)]
struct Deframer {
    command: Option<u8>,
    buf: Vec<u8>,
    escaped: bool,
    poisoned: bool,
    in_frame: bool,
}

impl Deframer {
    /// Feed one byte; a completed frame comes back as `(command, payload)`.
    fn push(&mut self, byte: u8) -> Option<(u8, Vec<u8>)> {
        if byte == FEND {
            let complete = self.in_frame && !self.poisoned;
            let command = self.command.filter(|_| complete);
            let frame = command.map(|c| (c, core::mem::take(&mut self.buf)));
            *self = Self::default();
            self.in_frame = true;
            return frame;
        }
        if !self.in_frame || self.poisoned {
            return None;
        }
        if self.command.is_none() {
            // One port only: the high nibble addresses a TNC port (`KISSInterface.py` 321-325).
            self.command = Some(byte & 0x0F);
            return None;
        }
        let byte = match (core::mem::take(&mut self.escaped), byte) {
            (false, FESC) => {
                self.escaped = true;
                return None;
            }
            (false, other) => other,
            (true, TFEND) => FEND,
            (true, TFESC) => FESC,
            // RNS passes a bad escape through; a frame that broke the framing is dropped here.
            (true, _) => {
                self.poisoned = true;
                return None;
            }
        };
        // Oversize frames are dropped, not truncated as RNS does (`KISSInterface.py` 320).
        if self.buf.len() == HW_MTU {
            self.poisoned = true;
        } else {
            self.buf.push(byte);
        }
        None
    }
}

/// A KISS TNC host.
#[derive(Debug)]
pub struct KissTnc {
    config: TncConfig,
    beacon: Option<Beacon>,
    deframer: Deframer,
    ready: bool,
    locked_at_ms: u64,
}

impl KissTnc {
    pub fn new(config: TncConfig, beacon: Option<Beacon>) -> Self {
        Self {
            config,
            beacon,
            deframer: Deframer::default(),
            ready: false,
            locked_at_ms: 0,
        }
    }

    /// The configuration bytes RNS writes once the port settles: TXDELAY, TXTAIL, P,
    /// SLOTTIME, then READY, which RNS sends whether or not flow control is on
    /// (`KISSInterface.py` 178-253). Marks the TNC ready.
    pub fn startup(&mut self) -> Vec<u8> {
        let tenths = |ms: u32| u8::try_from(ms / 10).unwrap_or(u8::MAX);
        let c = self.config;
        self.ready = true;
        [
            (cmd::TXDELAY, tenths(c.preamble_ms)),
            (cmd::TXTAIL, tenths(c.txtail_ms)),
            (cmd::P, c.persistence),
            (cmd::SLOTTIME, tenths(c.slottime_ms)),
            (cmd::READY, 0x01),
        ]
        .iter()
        .flat_map(|&(command, value)| encode(command, &[value]))
        .collect()
    }

    /// Whether the next frame may go out now.
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// Encode one packet as a DATA frame. Call only while [`Self::is_ready`]; with flow
    /// control on, this locks until the TNC answers READY.
    pub fn send(&mut self, packet: &[u8], now_ms: u64) -> Vec<u8> {
        if self.config.flow_control {
            self.ready = false;
            self.locked_at_ms = now_ms;
        }
        if let Some(beacon) = &mut self.beacon {
            beacon.on_tx(packet, now_ms);
        }
        encode(cmd::DATA, packet)
    }

    /// Feed bytes read from the port, appending every received DATA payload to `packets`.
    pub fn receive(&mut self, bytes: &[u8], packets: &mut Vec<Vec<u8>>) {
        for &byte in bytes {
            match self.deframer.push(byte) {
                Some((cmd::DATA, payload)) if !payload.is_empty() => packets.push(payload),
                Some((cmd::READY, _)) => self.ready = true,
                _ => {}
            }
        }
    }

    /// Discard a partial frame, as RNS does after 100 ms without bytes (`KISSInterface.py`
    /// 342-348).
    pub fn reset_partial(&mut self) {
        self.deframer = Deframer::default();
    }

    /// Run the timers: unlock a TNC that never answered READY, then return the beacon frame
    /// if it is due and may go out.
    pub fn poll(&mut self, now_ms: u64) -> Option<Vec<u8>> {
        if !self.ready && now_ms > self.locked_at_ms.saturating_add(FLOW_UNLOCK_MS) {
            self.ready = true;
        }
        if !self.ready {
            return None;
        }
        let payload = self.beacon.as_mut()?.take_due(now_ms)?;
        Some(self.send(&payload, now_ms))
    }

    /// The next time [`Self::poll`] has work, if any.
    pub fn wake_at_ms(&self) -> Option<u64> {
        if !self.ready {
            return Some(self.locked_at_ms.saturating_add(FLOW_UNLOCK_MS + 1));
        }
        self.beacon.as_ref()?.due_at_ms().map(|due| due + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn flow_controlled(beacon: Option<Beacon>) -> KissTnc {
        let config = TncConfig {
            flow_control: true,
            ..TncConfig::default()
        };
        let mut tnc = KissTnc::new(config, beacon);
        tnc.startup();
        tnc
    }

    #[test]
    fn startup_matches_rns_for_the_defaults() {
        let mut tnc = KissTnc::new(TncConfig::default(), None);
        // TXDELAY 350 ms, TXTAIL 20 ms, P 64, SLOTTIME 20 ms, READY.
        let expected =
            [[1, 35], [4, 2], [2, 64], [3, 2], [0x0F, 1]].map(|[c, v]| [FEND, c, v, FEND]);
        assert_eq!(tnc.startup(), expected.concat());
        assert!(tnc.is_ready());
    }

    #[test]
    fn receive_masks_the_port_and_delivers_data_only() {
        let mut tnc = KissTnc::new(TncConfig::default(), None);
        let mut wire = encode(0x10, &[FEND, 1, FESC]); // DATA on port 1
        wire.extend(encode(cmd::TXDELAY, &[9]));
        wire.extend([FEND, 0x00, 7, FESC, 0x42, FEND]); // a bad escape drops the frame
        let mut got = Vec::new();
        tnc.receive(&wire, &mut got);
        assert_eq!(got, vec![vec![FEND, 1, FESC]]);
    }

    #[test]
    fn oversize_and_partial_frames_are_dropped() {
        let mut tnc = KissTnc::new(TncConfig::default(), None);
        let mut got = Vec::new();
        tnc.receive(&encode(cmd::DATA, &[1; HW_MTU + 1]), &mut got);
        tnc.receive(&[FEND, 0x00, 5, 5], &mut got);
        tnc.reset_partial();
        tnc.receive(&[6, FEND], &mut got);
        assert!(got.is_empty());
        tnc.receive(&encode(cmd::DATA, &[2; HW_MTU]), &mut got);
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn flow_control_holds_one_frame_until_ready_or_timeout() {
        let mut tnc = flow_controlled(None);
        tnc.send(b"one", 1_000);
        assert!(!tnc.is_ready());
        tnc.receive(&encode(cmd::READY, &[1]), &mut Vec::new());
        assert!(tnc.is_ready());
        tnc.send(b"two", 2_000);
        assert_eq!(tnc.wake_at_ms(), Some(7_001));
        assert_eq!((tnc.poll(7_000), tnc.is_ready()), (None, false));
        tnc.poll(7_001);
        assert!(tnc.is_ready(), "unlocked after 5 s");
    }

    #[test]
    fn beacon_waits_for_ready_and_does_not_repeat() {
        let beacon = Beacon::kiss(b"N0CALL", core::time::Duration::from_secs(3));
        let mut tnc = flow_controlled(Some(beacon));
        tnc.send(b"traffic", 0);
        assert_eq!(tnc.poll(3_001), None, "locked: the beacon waits");
        tnc.receive(&encode(cmd::READY, &[1]), &mut Vec::new());
        let id = tnc.poll(3_002).expect("beacon due");
        assert_eq!(id, encode(cmd::DATA, b"N0CALL\0\0\0\0\0\0\0\0\0"));
        tnc.receive(&encode(cmd::READY, &[1]), &mut Vec::new());
        assert_eq!(tnc.wake_at_ms(), None);
        assert_eq!(tnc.poll(60_000), None);
    }
}
