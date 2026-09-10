//! Explicit finite read-only T114 direct-PHY observation capture to stdout.
use signalman::observation::persistence::{DurableCapture, Retention, store};
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
        .ok_or("usage: signalman-observe PORT [MAX_PAGES] [--output PATH]")?;
    if port == "--help" || port == "-h" {
        println!(
            "usage: signalman-observe PORT [MAX_PAGES] [--output PATH --device-id ID] [--retention-entries N] [--retention-bytes N] [--retention-age-ms N]\nFinite read-only direct-PHY observation capture. Requires exclusive serial access; prints a JSON receipt. Durable output requires an explicit stable local device association and never overwrites an existing file."
        );
        return Ok(());
    }
    let mut remaining: Vec<String> = args.collect();
    let pages = if remaining.first().is_some_and(|arg| !arg.starts_with("--")) {
        remaining.remove(0).parse::<usize>()?
    } else {
        64
    };
    let mut destination = DurableCapture::Disabled;
    let mut device_id = None::<String>;
    let mut retention = Retention {
        max_entries: pages,
        max_payload_bytes: 64 * 1024,
        max_age_ms: None,
    };
    let mut index = 0;
    while index < remaining.len() {
        let option = remaining[index].as_str();
        let value = remaining
            .get(index + 1)
            .ok_or_else(|| format!("{option} requires a value"))?;
        match option {
            "--output" => destination = DurableCapture::CreateNew(value.into()),
            "--device-id" => device_id = Some(value.clone()),
            "--retention-entries" => retention.max_entries = value.parse()?,
            "--retention-bytes" => retention.max_payload_bytes = value.parse()?,
            "--retention-age-ms" => retention.max_age_ms = Some(value.parse()?),
            _ => return Err(format!("unknown option {option}").into()),
        }
        index += 2;
    }
    if matches!(destination, DurableCapture::CreateNew(_)) && device_id.is_none() {
        return Err("--output requires an explicit --device-id association".into());
    }
    let device_association = device_id.as_deref().unwrap_or(&port);
    let mut client = ObservationClient::open(&port, 115_200, true, Duration::from_secs(2))?;
    let result = collect::capture(
        &mut client,
        device_association.as_bytes(),
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
    let stored = store(&destination, &result.bundle, unix_time_ms()?, retention)?;
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
            "receipt_version": 1, "carrier": "local-usb", "device_association": device_association,
            "board_authenticated": false, "boot_id": result.initial.boot_id,
            "initial_newest": result.initial.newest, "reached_target": result.reached_target,
            "last_cursor": result.last.next, "overwritten": result.last.overwritten,
            "missing_records": timeline.summary.missing_records,
            "incomplete_intervals": timeline.summary.incomplete_intervals,
            "unrelated_events_discarded": unrelated_events,
            "durable_capture": format!("{stored:?}"),
            "profiles": profiles, "records": records,
        }))?
    );
    Ok(())
}

fn unix_time_ms() -> Result<u64, Box<dyn Error>> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(u64::try_from(elapsed.as_millis())?)
}
