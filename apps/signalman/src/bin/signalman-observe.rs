//! Explicit finite read-only T114 direct-PHY observation capture to stdout.
use signalman::observation::{Admission, BundleEntry, collect, replay};
use std::{
    error::Error,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tulle::observation_serial::ObservationClient;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let port = args
        .next()
        .ok_or("usage: signalman-observe PORT [MAX_PAGES]")?;
    if port == "--help" || port == "-h" {
        println!(
            "usage: signalman-observe PORT [MAX_PAGES]\nFinite read-only direct-PHY observation capture. Requires exclusive serial access; prints a JSON receipt."
        );
        return Ok(());
    }
    let pages = args
        .next()
        .map(|n| n.parse::<usize>())
        .transpose()?
        .unwrap_or(64);
    if args.next().is_some() {
        return Err("usage: signalman-observe PORT [MAX_PAGES]".into());
    }
    let mut client = ObservationClient::open(&port, 115_200, true, Duration::from_secs(2))?;
    let result = collect::capture(
        &mut client,
        port.as_bytes(),
        &port,
        Admission {
            max_frames: pages,
            max_bytes: 64 * 1024,
        },
        pages,
        || {
            let elapsed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(std::io::Error::other)?;
            u64::try_from(elapsed.as_millis()).map_err(std::io::Error::other)
        },
    )
    .await;
    // DTR fall is the board's session retirement/recovery edge, including errors.
    let unrelated_events = client.unrelated_events();
    let serial = client.into_inner();
    let lowered = serial.set_dtr(false);
    drop(serial);
    lowered?;
    let result = result?;
    let timeline = replay(&result.bundle).map_err(|error| format!("{error:?}"))?;
    let records: Vec<_> = result
        .bundle
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            BundleEntry::Record(record) => Some(serde_json::json!({
                "received_unix_ms": record.received_unix_ms, "raw_hex": hex::encode(&record.raw),
            })),
            _ => None,
        })
        .collect();
    let profiles: Vec<_> = result
        .bundle
        .profiles()
        .iter()
        .map(|profile| {
            serde_json::json!({
                "id": profile.id, "version": profile.version, "name": profile.name,
                "definition_hex": hex::encode(&profile.definition),
            })
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "receipt_version": 1, "carrier": "local-usb", "device_association": port,
            "board_authenticated": false, "boot_id": result.initial.boot_id,
            "initial_newest": result.initial.newest, "reached_target": result.reached_target,
            "last_cursor": result.last.next, "overwritten": result.last.overwritten,
            "missing_records": timeline.summary.missing_records,
            "incomplete_intervals": timeline.summary.incomplete_intervals,
            "unrelated_events_discarded": unrelated_events,
            "profiles": profiles, "records": records,
        }))?
    );
    Ok(())
}
