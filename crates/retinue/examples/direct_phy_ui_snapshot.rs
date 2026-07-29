//! Headed injector for the optional direct-PHY on-device host projection.
//!
//! The fixture proves U3 framing, policy, and expiry. It deliberately does not
//! claim real Retinue delivery or propagation; those belong to U4.

use std::time::Duration;

use radio_face::{
    DetailPolicy, EventKind, EventSource, HostSnapshot, IfacState, NodeSummary, PeerPath,
    PeerSummary, Personality, Text, UiEvent, encode_snapshot,
};
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink, UiSnapshotError};

fn profile() -> PhyProfile {
    PhyProfile {
        frequency_hz: 906_875_000,
        bandwidth_hz: 250_000,
        spreading_factor: 8,
        coding_rate_denominator: 5,
        preamble_symbols: 16,
        sync_word: 0x12,
        explicit_header: true,
        crc: true,
        invert_iq: false,
        tx_power_dbm: 17,
    }
}

fn minimal(valid_for_secs: u16) -> HostSnapshot {
    HostSnapshot {
        valid_for_secs,
        personality: Personality::Retinue,
        detail: DetailPolicy::Minimal,
        link_count: 1,
        admitted_links: 1,
        queue_depth: 0,
        ifac: IfacState::On,
        ..HostSnapshot::default()
    }
}

fn named(valid_for_secs: u16) -> HostSnapshot {
    HostSnapshot {
        valid_for_secs,
        personality: Personality::Retinue,
        detail: DetailPolicy::Named,
        node: Some(NodeSummary {
            name: Text::from_truncated("HERALD"),
            address_tail: [0x4c, 0x9f, 0x03, 0xaa, 0x77, 0xe2, 0xbd, 0x08],
            fingerprint: [
                0x4c, 0x9f, 0x03, 0xaa, 0x77, 0xe2, 0x1b, 0x0d, 0x92, 0xc4, 0xe8, 0xf1, 0x5a, 0x36,
                0xbd, 0x08,
            ],
            role: Text::from_truncated("NODE"),
            uptime_secs: 13_700,
        }),
        link_count: 2,
        admitted_links: 1,
        queue_depth: 3,
        ifac: IfacState::On,
        peers: [
            Some(PeerSummary {
                name: Text::from_truncated("ESQUIRE"),
                path: PeerPath::Direct,
                age_secs: 120,
            }),
            Some(PeerSummary {
                name: Text::from_truncated("OUTRIDER"),
                path: PeerPath::Via,
                age_secs: 720,
            }),
            None,
        ],
        peer_overflow: 0,
        event: Some(UiEvent {
            source: EventSource::Host,
            kind: EventKind::Info,
            text: Text::from_truncated("HOST SNAPSHOT"),
        }),
    }
}

fn encoded(snapshot: &HostSnapshot) -> Vec<u8> {
    let mut bytes = [0_u8; radio_face::MAX_SNAPSHOT_LEN];
    let len = encode_snapshot(snapshot, &mut bytes).expect("fixture must encode");
    bytes[..len].to_vec()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port = args.next().unwrap_or_else(|| "COM10".into());
    let mode = args.next().unwrap_or_else(|| "named".into());
    let hold_secs = args
        .next()
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(15);

    let config = DirectPhySerialConfig {
        online_timeout: Duration::from_secs(10),
        transmit_timeout: Duration::from_secs(5),
        ..DirectPhySerialConfig::default()
    };
    let mut radio =
        DirectPhySerialLink::open(&port, profile(), AirtimeBudget::new(60_000, 60_000), config)?;
    radio.wait_online().await?;

    let mut payload = match mode.as_str() {
        "minimal" => encoded(&minimal(30)),
        "named" => encoded(&named(300)),
        "expire" => encoded(&named(5)),
        "future" | "truncated" => encoded(&named(30)),
        other => return Err(format!("unknown mode {other:?}").into()),
    };

    let expected_rejection = match mode.as_str() {
        "future" => {
            payload[0] = payload[0].saturating_add(1);
            Some(tulle::direct_phy::UI_SNAPSHOT_UNSUPPORTED_VERSION)
        }
        "truncated" => {
            payload.pop();
            Some(tulle::direct_phy::UI_SNAPSHOT_MALFORMED)
        }
        _ => None,
    };

    match (
        radio.publish_ui_snapshot(&payload).await,
        expected_rejection,
    ) {
        (Ok(()), None) => println!("UI SNAPSHOT {mode} ACCEPTED"),
        (Err(UiSnapshotError::Rejected { result }), Some(expected)) if result == expected => {
            println!("UI SNAPSHOT {mode} REJECTED result={result}");
            radio.publish_ui_snapshot(&encoded(&minimal(30))).await?;
            println!("UI SNAPSHOT RECOVERY ACCEPTED");
        }
        (outcome, expected) => {
            return Err(
                format!("unexpected snapshot outcome {outcome:?}, expected {expected:?}").into(),
            );
        }
    }

    if hold_secs > 0 {
        tokio::time::sleep(Duration::from_secs(hold_secs)).await;
    }
    radio.shutdown().await?;
    println!("RETINUE DIRECT-PHY UI SNAPSHOT HEADED PASSED");
    Ok(())
}
