//! A virtual air for hardware-free RNode gates: N fake RNodes on pty masters.
//!
//! Each device answers the host protocol through `radio_hand::rnode` and forwards every DATA
//! frame to each other online device tuned to the same (frequency, bandwidth, SF, CR),
//! preceded by `STAT_RSSI` and `STAT_SNR`, as a receiving RNode reports it. Every host and
//! device frame is logged as one JSON line, so a gate can assert bytes, not impressions.
//!
//! ```text
//! rnode_air --fd N [--fd N ...] --log PATH [--ready-flow MS]
//!           [--mismatch-txp DEV[,DEV]] [--error-after DEV:COUNT] [--reset-after DEV:COUNT]
//! ```
//!
//! `--fd` names a pty master the harness passed down. `--ready-flow` answers each DATA
//! with READY after MS milliseconds. `--mismatch-txp` echoes TX power one lower on those
//! devices. `--error-after` answers a device's COUNT-th DATA with `ERROR 0x02` (TX failed),
//! and `--reset-after` with `RESET 0xF8`, instead of transmitting it. Airtime limits are
//! echoed as a device that stores them lossily might: one hundredth low, and 100% as 0.

use std::fs::File;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use radio_hand::rnode::{self, Command, Pending, cmd};
use selvage::kiss::Deframer;

/// The received-signal report every forwarded frame carries.
const RSSI_DBM: i16 = -40;
const SNR_DB: i16 = 8;
/// ERROR_TXFAILED (`RNodeInterface.py` 92).
const TX_FAILED: u8 = 0x02;

#[derive(Default)]
struct Flags {
    ready_flow: Option<Duration>,
    mismatch_txp: Vec<usize>,
    error_after: Option<(usize, u32)>,
    reset_after: Option<(usize, u32)>,
}

#[derive(Default)]
struct Radio {
    pending: Pending,
    /// (frequency, bandwidth, SF, CR) while the radio is on.
    tuned: Option<(u32, u32, u8, u8)>,
    data_frames: u32,
}

struct Device {
    out: Mutex<File>,
    radio: Mutex<Radio>,
}

struct Air {
    devices: Vec<Device>,
    log: Mutex<File>,
    epoch: Instant,
    flags: Flags,
}

impl Air {
    fn log(&self, dev: usize, dir: &str, frame: &[u8]) {
        let hex: String = frame.iter().map(|b| format!("{b:02x}")).collect();
        let t = self.epoch.elapsed().as_secs_f64();
        let line = format!("{{\"t\":{t:.4},\"dev\":{dev},\"dir\":\"{dir}\",\"hex\":\"{hex}\"}}\n");
        let mut log = self.log.lock().unwrap();
        let _ = log.write_all(line.as_bytes());
        let _ = log.flush();
    }

    /// Write one device-to-host frame.
    fn reply(&self, dev: usize, command: u8, payload: &[u8]) {
        let mut wire = vec![0_u8; rnode::encoded_max(payload.len())];
        let Some(len) = rnode::encode(command, payload, &mut wire) else {
            return;
        };
        let frame: Vec<u8> = [&[command][..], payload].concat();
        self.log(dev, "d2h", &frame);
        let _ = self.devices[dev]
            .out
            .lock()
            .unwrap()
            .write_all(&wire[..len]);
    }

    fn on_host_frame(self: &Arc<Self>, dev: usize, frame: &[u8]) {
        self.log(dev, "h2d", frame);
        let Some(command) = rnode::decode(frame) else {
            return;
        };
        self.devices[dev]
            .radio
            .lock()
            .unwrap()
            .pending
            .accept(&command);
        match command {
            Command::TxPower(dbm) if self.flags.mismatch_txp.contains(&dev) => {
                self.reply(dev, cmd::TXPOWER, &[dbm.wrapping_sub(1)]);
            }
            Command::RadioState(true) => {
                let mut radio = self.devices[dev].radio.lock().unwrap();
                radio.tuned = radio.pending.profile().map(|p| {
                    let cr = p.coding_rate_denominator;
                    (p.frequency_hz, p.bandwidth_hz, p.spreading_factor, cr)
                });
                let on = radio.tuned.is_some();
                drop(radio);
                self.reply(dev, cmd::RADIO_STATE, &[u8::from(on)]);
            }
            Command::RadioState(false) => {
                self.devices[dev].radio.lock().unwrap().tuned = None;
                self.reply(dev, cmd::RADIO_STATE, &[0]);
            }
            Command::AirtimeLock { long, centi } => {
                let echoed = if centi >= 10_000 {
                    0
                } else {
                    centi.saturating_sub(1)
                };
                let marker = if long { cmd::LT_ALOCK } else { cmd::ST_ALOCK };
                self.reply(dev, marker, &echoed.to_be_bytes());
            }
            Command::Leave => self.devices[dev].radio.lock().unwrap().tuned = None,
            Command::Data(packet) => self.on_data(dev, packet),
            other => {
                if let Some((marker, payload)) = rnode::answer(&other) {
                    self.reply(dev, marker, &payload);
                }
            }
        }
    }

    fn on_data(self: &Arc<Self>, dev: usize, packet: &[u8]) {
        let (tuned, count) = {
            let mut radio = self.devices[dev].radio.lock().unwrap();
            radio.data_frames += 1;
            (radio.tuned, radio.data_frames)
        };
        let Some(channel) = tuned else {
            return;
        };
        if self.flags.error_after == Some((dev, count)) {
            return self.reply(dev, cmd::ERROR, &[TX_FAILED]);
        }
        if self.flags.reset_after == Some((dev, count)) {
            self.devices[dev].radio.lock().unwrap().tuned = None;
            return self.reply(dev, cmd::RESET, &[0xF8]);
        }
        for peer in 0..self.devices.len() {
            if peer != dev && self.devices[peer].radio.lock().unwrap().tuned == Some(channel) {
                self.reply(peer, cmd::STAT_RSSI, &[rnode::rssi_wire(RSSI_DBM)]);
                self.reply(peer, cmd::STAT_SNR, &[rnode::snr_wire(SNR_DB)]);
                self.reply(peer, cmd::DATA, packet);
            }
        }
        if let Some(delay) = self.flags.ready_flow {
            let air = Arc::clone(self);
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                air.reply(dev, cmd::READY, &[0x01]);
            });
        }
    }
}

fn listen(air: Arc<Air>, dev: usize, mut input: File) {
    let mut deframer = Deframer::<{ rnode::DEFRAME_BUF + 64 }>::new();
    let mut buf = [0_u8; 1024];
    while let Ok(count @ 1..) = input.read(&mut buf) {
        for &byte in &buf[..count] {
            if deframer.push(byte) {
                let frame = deframer.frame().to_vec();
                air.on_host_frame(dev, &frame);
            }
        }
    }
}

fn device_count(spec: &str) -> Result<(usize, u32), String> {
    let (dev, count) = spec.split_once(':').ok_or("expected DEV:COUNT")?;
    Ok((
        dev.parse().map_err(|e| format!("{e}"))?,
        count.parse().map_err(|e| format!("{e}"))?,
    ))
}

fn main() -> Result<(), String> {
    let mut fds = Vec::new();
    let mut log = None;
    let mut flags = Flags::default();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--fd" => fds.push(value()?.parse::<u32>().map_err(|e| e.to_string())?),
            "--log" => log = Some(value()?),
            "--ready-flow" => {
                let ms = value()?.parse().map_err(|e| format!("{e}"))?;
                flags.ready_flow = Some(Duration::from_millis(ms));
            }
            "--mismatch-txp" => {
                for dev in value()?.split(',') {
                    flags
                        .mismatch_txp
                        .push(dev.parse().map_err(|e| format!("{e}"))?);
                }
            }
            "--error-after" => flags.error_after = Some(device_count(&value()?)?),
            "--reset-after" => flags.reset_after = Some(device_count(&value()?)?),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    let log = File::create(log.ok_or("--log is required")?).map_err(|e| e.to_string())?;
    let mut inputs = Vec::new();
    let mut devices = Vec::new();
    for fd in fds {
        let port = File::options()
            .read(true)
            .write(true)
            .open(format!("/dev/fd/{fd}"))
            .map_err(|e| format!("fd {fd}: {e}"))?;
        inputs.push(port.try_clone().map_err(|e| e.to_string())?);
        devices.push(Device {
            out: Mutex::new(port),
            radio: Mutex::default(),
        });
    }
    let air = Arc::new(Air {
        devices,
        log: Mutex::new(log),
        epoch: Instant::now(),
        flags,
    });
    println!("AIR_READY {}", air.devices.len());
    let listeners: Vec<_> = inputs
        .into_iter()
        .enumerate()
        .map(|(dev, input)| {
            let air = Arc::clone(&air);
            std::thread::spawn(move || listen(air, dev, input))
        })
        .collect();
    for listener in listeners {
        let _ = listener.join();
    }
    Ok(())
}
