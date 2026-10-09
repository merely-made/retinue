//! The RNode host protocol: a sans-io [`Modem`] over KISS-framed serial commands.
//!
//! Pinned by black-box capture from live hardware (RNode firmware 1.86 on a Heltec T114 and
//! a Heltec V4, driven by RNS 1.3.8; fixtures `rnode_serial_capture.json` and
//! `rnode_rx_capture.json`). The wire, as observed:
//!
//! - Everything is a KISS frame whose first byte is a command.
//! - Init: `DETECT(0x08, 0x73)`, then `FW_VERSION(0x50)`, `PLATFORM(0x48)`, `MCU(0x49)`
//!   probes, then SetHardware: `FREQUENCY(0x01, u32 BE Hz)`, `BANDWIDTH(0x02, u32 BE Hz)`,
//!   `TXPOWER(0x03, dBm)`, `SF(0x04)`, `CR(0x05)`, `RADIO_STATE(0x06, 1)`.
//! - The device echoes each config command as confirmation; a `RADIO_STATE` echo of `01`
//!   means the radio is online, `00` that it refused (e.g. an unverified firmware hash).
//! - TX: `DATA(0x00)` framing the raw packet, KISS-escaped.
//! - RX: per received packet, a triplet: `STAT_RSSI(0x23)` (dBm = raw − 157),
//!   `STAT_SNR(0x24)` (dB = raw as i8 / 4), then `DATA(0x00)` with the packet verbatim.
//! - Unsolicited channel-stat (`0x25`) and battery (`0x27`) frames ride alongside.
//!
//! RNS 1.5.7 semantics on top of the capture (`RNodeInterface.py` 428-500, 619-744,
//! 1076-1209): airtime locks, echo validation before going online, a firmware floor, READY
//! flow control, `ERROR` classification, the online-reset check, and the detach handshake.
//!
//! Sans-io: feed device bytes to [`RNode::on_serial`], write out whatever
//! [`RNode::take_outbound`] returns, drain events via [`Modem::poll`]. The pump owns the
//! serial port and the clock.

use std::collections::VecDeque;

use crate::kiss;
use crate::lora::LoRaParams;
use crate::modem::{Modem, ModemError, ModemEvent};

mod config;

pub use config::{
    ConfigError, DeviceError, FREQ_MAX, FREQ_MIN, FREQ_TOLERANCE_HZ, Fault, HW_MTU, MIN_FIRMWARE,
    Mismatch, RNodeConfig, Reported,
};

/// KISS command bytes (`RNodeInterface.py` 40-82).
pub mod cmd {
    pub const DATA: u8 = 0x00;
    pub const FREQUENCY: u8 = 0x01;
    pub const BANDWIDTH: u8 = 0x02;
    pub const TXPOWER: u8 = 0x03;
    pub const SF: u8 = 0x04;
    pub const CR: u8 = 0x05;
    pub const RADIO_STATE: u8 = 0x06;
    pub const DETECT: u8 = 0x08;
    pub const LEAVE: u8 = 0x0A;
    pub const ST_ALOCK: u8 = 0x0B;
    pub const LT_ALOCK: u8 = 0x0C;
    pub const READY: u8 = 0x0F;
    pub const STAT_RSSI: u8 = 0x23;
    pub const STAT_SNR: u8 = 0x24;
    pub const STAT_CHTM: u8 = 0x25;
    pub const STAT_BAT: u8 = 0x27;
    pub const PLATFORM: u8 = 0x48;
    pub const MCU: u8 = 0x49;
    pub const FW_VERSION: u8 = 0x50;
    pub const RESET: u8 = 0x55;
    pub const ERROR: u8 = 0x90;
}

/// Detect request/response magic bytes.
pub const DETECT_REQ: u8 = 0x73;
pub const DETECT_RESP: u8 = 0x46;
/// The `RESET` payload a device sends after it restarts (`RNodeInterface.py` 1091-1092).
pub const RESET_MARKER: u8 = 0xF8;

/// RSSI on the wire is offset by this: `dBm = raw - 157`.
pub const RSSI_OFFSET: i16 = 157;

/// A sans-io RNode: implements [`Modem`] on top of the captured host protocol.
pub struct RNode {
    config: RNodeConfig,
    deframer: kiss::Deframer,
    outbound: Vec<u8>,
    events: VecDeque<ModemEvent>,
    detected: bool,
    online: bool,
    /// False while a frame awaits the device's READY under flow control.
    ready: bool,
    fw_version: Option<(u8, u8)>,
    reported: Reported,
    /// Stats arriving ahead of their data frame (the RX triplet).
    pending_rssi: Option<i16>,
    pending_snr: Option<f32>,
    /// Last device-reported error command payload, if any.
    last_error: Option<Vec<u8>>,
    fault: Option<Fault>,
}

impl RNode {
    pub fn new(params: LoRaParams) -> Self {
        Self::with_config(RNodeConfig::new(params))
    }

    pub fn with_config(config: RNodeConfig) -> Self {
        RNode {
            config,
            // The command byte plus the largest DATA payload.
            deframer: kiss::Deframer::new(1 + HW_MTU),
            outbound: Vec::new(),
            events: VecDeque::new(),
            detected: false,
            online: false,
            ready: true,
            fw_version: None,
            reported: Reported::default(),
            pending_rssi: None,
            pending_snr: None,
            last_error: None,
            fault: None,
        }
    }

    pub fn config(&self) -> &RNodeConfig {
        &self.config
    }

    fn queue_cmd(&mut self, command: u8, payload: &[u8]) {
        let mut frame = Vec::with_capacity(1 + payload.len());
        frame.push(command);
        frame.extend_from_slice(payload);
        self.outbound.extend_from_slice(&kiss::encode(&frame));
    }

    /// Queue the whole init conversation: detect, probes, SetHardware, radio on. Mirrors the
    /// captured RNS sequence frame for frame, and forgets the echoes of any earlier attempt.
    pub fn start(&mut self) {
        self.detected = false;
        self.online = false;
        self.ready = true;
        self.reported = Reported::default();
        self.queue_cmd(cmd::DETECT, &[DETECT_REQ]);
        self.queue_cmd(cmd::FW_VERSION, &[0x00]);
        self.queue_cmd(cmd::PLATFORM, &[0x00]);
        self.queue_cmd(cmd::MCU, &[0x00]);
        self.queue_config();
    }

    /// Forget everything about the last session, for a reopened port.
    pub fn reopen(&mut self) {
        *self = Self::with_config(self.config);
    }

    fn queue_config(&mut self) {
        let p = self.config.params;
        self.queue_cmd(cmd::FREQUENCY, &p.frequency_hz.to_be_bytes());
        self.queue_cmd(cmd::BANDWIDTH, &p.bandwidth_hz.to_be_bytes());
        self.queue_cmd(cmd::TXPOWER, &[p.tx_power_dbm]);
        self.queue_cmd(cmd::SF, &[p.spreading_factor]);
        self.queue_cmd(cmd::CR, &[coding_rate_wire(&p)]);
        if let Some(limit) = self.config.st_alock {
            self.queue_cmd(cmd::ST_ALOCK, &limit.to_be_bytes());
        }
        if let Some(limit) = self.config.lt_alock {
            self.queue_cmd(cmd::LT_ALOCK, &limit.to_be_bytes());
        }
        self.queue_cmd(cmd::RADIO_STATE, &[0x01]);
    }

    /// Queue the detach handshake: radio off, then LEAVE (`RNodeInterface.py` 496-500,
    /// 1194-1199).
    pub fn leave(&mut self) {
        self.online = false;
        self.queue_cmd(cmd::RADIO_STATE, &[0x00]);
        self.queue_cmd(cmd::LEAVE, &[0xFF]);
    }

    /// Bytes waiting to be written to the serial port. Empties the queue.
    pub fn take_outbound(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.outbound)
    }

    /// Feed bytes read from the serial port.
    pub fn on_serial(&mut self, bytes: &[u8]) {
        let mut frames = Vec::new();
        self.deframer.push(bytes, &mut frames);
        for frame in frames {
            self.on_frame(&frame);
        }
    }

    /// Discard a half-read frame; the pump calls this after 100 ms of silence
    /// (`RNodeInterface.py` 1137-1143).
    pub fn reset_partial(&mut self) {
        self.deframer.reset();
    }

    fn on_frame(&mut self, frame: &[u8]) {
        let Some((&command, payload)) = frame.split_first() else {
            return;
        };
        let byte = payload.first().copied();
        if self.reported.record(command, payload) {
            if command == cmd::RADIO_STATE {
                self.on_radio_state();
            }
            return;
        }
        match command {
            cmd::DETECT => self.detected = byte == Some(DETECT_RESP),
            cmd::FW_VERSION => {
                if let [major, minor, ..] = *payload {
                    self.fw_version = Some((major, minor));
                    if (major, minor) < self.config.min_firmware {
                        self.fault.get_or_insert(Fault::Firmware(major, minor));
                    }
                }
            }
            cmd::READY => self.ready = true,
            cmd::RESET if byte == Some(RESET_MARKER) && self.online => {
                self.online = false;
                self.fault.get_or_insert(Fault::Reset);
            }
            cmd::STAT_RSSI => self.pending_rssi = byte.map(|raw| raw as i16 - RSSI_OFFSET),
            cmd::STAT_SNR => self.pending_snr = byte.map(|raw| raw as i8 as f32 / 4.0),
            cmd::DATA => self.events.push_back(ModemEvent::Received {
                frame: payload.to_vec(),
                rssi_dbm: self.pending_rssi.take().unwrap_or(0),
                snr_db: self.pending_snr.take().unwrap_or(0.0),
            }),
            cmd::ERROR => {
                if let Some(DeviceError::Fatal(code)) = byte.map(DeviceError::classify) {
                    self.online = false;
                    self.fault.get_or_insert(Fault::Device(code));
                }
                self.last_error = Some(payload.to_vec());
            }
            // Channel stats, battery, platform and MCU probes: informational.
            _ => {}
        }
    }

    /// The radio is online only once every echo agrees with the configuration.
    fn on_radio_state(&mut self) {
        let was = self.online;
        self.online = false;
        if self.reported.radio_state != Some(1) {
            return;
        }
        match self.reported.mismatch(&self.config) {
            Some(mismatch) => {
                self.fault.get_or_insert(Fault::Mismatch(mismatch));
            }
            None => {
                self.online = true;
                if !was {
                    self.events.push_back(ModemEvent::ChannelClear);
                }
            }
        }
    }

    /// Whether the device answered the detect probe.
    pub fn is_detected(&self) -> bool {
        self.detected
    }

    /// Whether the radio is on with every setting confirmed.
    pub fn is_online(&self) -> bool {
        self.online
    }

    /// Firmware version as (major, minor), once probed.
    pub fn fw_version(&self) -> Option<(u8, u8)> {
        self.fw_version
    }

    /// The settings the device has echoed since the last [`Self::start`].
    pub fn reported(&self) -> &Reported {
        &self.reported
    }

    /// Take the condition that stops this device carrying traffic until reopened, if any.
    pub fn take_fault(&mut self) -> Option<Fault> {
        self.fault.take()
    }

    /// Whether a sent frame still awaits the device's READY.
    pub fn is_flow_locked(&self) -> bool {
        !self.ready
    }

    /// Release the flow-control lock without a READY, for a device that never answers.
    pub fn release(&mut self) {
        self.ready = true;
    }

    /// The last `ERROR` frame payload the device sent, if any.
    pub fn last_error(&self) -> Option<&[u8]> {
        self.last_error.as_deref()
    }

    /// Take the last `ERROR` frame payload, clearing it.
    ///
    /// A pump calls this to forward the device's own complaints to the host
    /// instead of latching them where nobody looks. Worth surfacing: a device
    /// that silently declines to transmit is indistinguishable from a healthy
    /// one unless something reads this
    /// (`design_docs/2026-07-26_rnode_bulk_frame_loss.md`).
    pub fn take_last_error(&mut self) -> Option<Vec<u8>> {
        self.last_error.take()
    }
}

/// The CR wire value: RNode takes the denominator (5..=8 for 4/5..4/8).
fn coding_rate_wire(p: &LoRaParams) -> u8 {
    use crate::lora::CodingRate::*;
    match p.coding_rate {
        Cr45 => 5,
        Cr46 => 6,
        Cr47 => 7,
        Cr48 => 8,
    }
}

impl Modem for RNode {
    fn params(&self) -> LoRaParams {
        self.config.params
    }

    fn set_params(&mut self, params: LoRaParams) -> Result<(), ModemError> {
        self.config.params = params;
        self.queue_config();
        Ok(())
    }

    fn max_frame_len(&self) -> usize {
        HW_MTU
    }

    fn enqueue(&mut self, frame: &[u8]) -> Result<core::time::Duration, ModemError> {
        if frame.len() > HW_MTU {
            return Err(ModemError::TooLong { max: HW_MTU });
        }
        if !self.online || !self.ready {
            return Err(ModemError::Busy);
        }
        self.ready = !self.config.flow_control;
        self.queue_cmd(cmd::DATA, frame);
        Ok(self.config.params.time_on_air(frame.len()))
    }

    fn poll(&mut self) -> Option<ModemEvent> {
        self.events.pop_front()
    }
}

#[cfg(test)]
mod tests;
