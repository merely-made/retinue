//! Retinue's side of `oracle/interop_serial_hdlc.py` and `oracle/interop_kiss_tnc.py`.
//!
//! ```text
//! serial_peer PORT [--kiss] [--speed N] [--ifac BITS] [--flow-control]
//!                  [--id CALLSIGN:SECS] [--burst N] [--publish LEN]
//! ```
//!
//! Without `--burst` it runs the shared serve script; with it, it announces N destinations
//! back to back and idles, for the fake-TNC checks. It exits when stdin closes.

mod script;

use std::time::Duration;

use retinue::iface::beacon::Beacon;
use retinue::iface::kiss::{KissTnc, TncConfig};
use retinue::iface::serial::{self, CarrierStatus, Framing, SerialConfig};

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let port = args.next().ok_or("usage: serial_peer PORT [flags]")?;
    let mut config = SerialConfig::new(port);
    config.speed = 115_200;
    let (mut kiss, mut tnc) = (false, TncConfig::default());
    let (mut ifac_bits, mut beacon, mut burst, mut publish) = (None, None, None, None);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        let number = |v: String| v.parse::<usize>().map_err(|e| e.to_string());
        match flag.as_str() {
            "--kiss" => kiss = true,
            "--flow-control" => tnc.flow_control = true,
            "--speed" => config.speed = value()?.parse().map_err(|e| format!("{e}"))?,
            "--ifac" => ifac_bits = Some(number(value()?)?),
            "--burst" => burst = Some(number(value()?)?),
            "--publish" => publish = Some(number(value()?)?),
            "--id" => {
                let spec = value()?;
                let (call, secs) = spec.split_once(':').ok_or("--id CALLSIGN:SECS")?;
                let interval = Duration::from_secs(secs.parse().map_err(|e| format!("{e}"))?);
                beacon = Some(Beacon::kiss(call.as_bytes(), interval));
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    let framing = match kiss {
        true => Framing::Kiss(KissTnc::new(tnc, beacon)),
        false => Framing::Hdlc,
    };

    let endpoint = script::endpoint();
    let carrier = serial::attach(&endpoint, config, framing, script::ifac(ifac_bits)?)
        .map_err(|e| format!("attach: {e}"))?;
    println!("ATTACHED {}", carrier.id);
    let mut online = carrier.status.clone();
    let mut status = carrier.status;
    tokio::spawn(async move {
        loop {
            println!("STATUS {:?}", status.borrow_and_update().clone());
            if status.changed().await.is_err() {
                break;
            }
        }
    });
    tokio::spawn(carrier.run);
    match burst {
        Some(count) => {
            // Packets offered before the line is up are dropped, as RNS drops them.
            let _ = online.wait_for(|s| *s == CarrierStatus::Online).await;
            script::burst(endpoint.clone(), count).await
        }
        None => script::serve(endpoint.clone(), publish).await,
    }
    endpoint.shutdown(Duration::from_secs(2)).await;
    println!("DONE");
    Ok(())
}
