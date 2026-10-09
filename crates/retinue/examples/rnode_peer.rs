//! Retinue's side of `oracle/interop_rnode_air.py`: one supervised RNode on a port.
//!
//! ```text
//! rnode_peer PORT [--freq HZ] [--bw HZ] [--sf N] [--cr N] [--txp DBM] [--ifac BITS]
//!                 [--flow-control] [--alock-st CENTI] [--alock-lt CENTI]
//!                 [--id CALLSIGN:SECS] [--publish LEN]
//! ```
//!
//! Runs the shared serve script and prints each pump status change. When stdin closes it
//! shuts down, which turns the radio off and sends LEAVE.

#[path = "serial_peer/script.rs"]
mod script;

use std::time::Duration;

use retinue::iface::beacon::Beacon;
use retinue::iface::tulle::{apply_rnode_policy, drive_with_beacon};
use tulle::airtime::AirtimeBudget;
use tulle::lora::{CodingRate, LoRaParams};
use tulle::rnode::{HW_MTU, RNodeConfig};
use tulle::serial::{RNodeSerialLink, SerialPumpConfig};

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let port = args.next().ok_or("usage: rnode_peer PORT [flags]")?;
    let mut params = LoRaParams {
        spreading_factor: 7,
        bandwidth_hz: 500_000,
        coding_rate: CodingRate::Cr45,
        frequency_hz: 867_200_000,
        tx_power_dbm: 14,
        preamble_syms: 8,
        explicit_header: true,
        crc: true,
    };
    let mut rnode = RNodeConfig::new(params);
    let (mut ifac_bits, mut beacon, mut publish) = (None, None, None);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        let mut number = || -> Result<u32, String> { value()?.parse().map_err(|e| format!("{e}")) };
        match flag.as_str() {
            "--freq" => params.frequency_hz = number()?,
            "--bw" => params.bandwidth_hz = number()?,
            "--sf" => params.spreading_factor = number()? as u8,
            "--txp" => params.tx_power_dbm = number()? as u8,
            "--cr" => {
                params.coding_rate = match number()? {
                    5 => CodingRate::Cr45,
                    6 => CodingRate::Cr46,
                    7 => CodingRate::Cr47,
                    _ => CodingRate::Cr48,
                }
            }
            "--ifac" => ifac_bits = Some(number()? as usize),
            "--publish" => publish = Some(number()? as usize),
            "--flow-control" => rnode.flow_control = true,
            "--alock-st" => rnode.st_alock = Some(number()? as u16),
            "--alock-lt" => rnode.lt_alock = Some(number()? as u16),
            "--id" => {
                let spec = value()?;
                let (call, secs) = spec.split_once(':').ok_or("--id CALLSIGN:SECS")?;
                let interval = Duration::from_secs(secs.parse().map_err(|e| format!("{e}"))?);
                beacon = Some(Beacon::rnode(call.as_bytes(), interval).ok_or("callsign too long")?);
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    rnode.params = params;

    // RNS waits 2 s after opening before it detects (`RNodeInterface.py` 430).
    let pump = SerialPumpConfig {
        open_settle: Duration::from_secs(2),
        ..SerialPumpConfig::default()
    };
    let radio = RNodeSerialLink::supervise(&port, rnode, AirtimeBudget::new(60_000, 60_000), pump)
        .map_err(|e| format!("open: {e}"))?;
    let mut status = radio.watch_status();
    tokio::spawn(async move {
        loop {
            println!("STATUS {:?}", status.borrow_and_update().clone());
            if status.changed().await.is_err() {
                break;
            }
        }
    });

    let endpoint = script::endpoint();
    let interface = match script::ifac(ifac_bits)? {
        Some(ifac) => endpoint.attach_interface_with_ifac(HW_MTU, ifac),
        None => endpoint.attach_interface_with_frame_limit(HW_MTU),
    }
    .map_err(|e| format!("attach: {e}"))?;
    apply_rnode_policy(&endpoint, interface.id(), &params);
    let driver = tokio::spawn(drive_with_beacon(interface, radio, beacon));
    script::serve(endpoint.clone(), publish).await;
    endpoint.shutdown(Duration::from_secs(2)).await;
    if let Ok(Err(error)) = driver.await {
        println!("DRIVE_ERR {error}");
    }
    // The dropped radio's pump sends RADIO_STATE 0 and LEAVE; let it finish.
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("DONE");
    Ok(())
}
