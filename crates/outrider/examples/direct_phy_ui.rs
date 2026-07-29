//! Real Retinue/Outrider host projection across two Tulle direct-PHY radios.
//!
//! The headed receipt first attaches an open Retinue interface, then reopens
//! the same carrier with IFAC. It projects real link admission, authenticated
//! direct delivery, propagation storage/fetch, and a host-side delivery
//! failure to both on-device UIs. Snapshot bytes remain a host-edge concern:
//! Tulle carries them opaquely while its packet driver owns each radio.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use outrider::{
    DEFAULT_MAX_MESSAGE_BYTES, DeliveryAnnounce, LxmfPayload, PROPAGATION_METADATA_NAME,
    PropagationAnnounce, PropagationBatch, PropagationCosts, PropagationStore,
    PropagationStoreLimits, fetch_propagation_with_resource_config, prepare_propagation,
    receive_direct_with_stamp_cost_and_resource_config, receive_submission, register_delivery,
    register_propagation, send_direct_stamped_with_resource_config, serve_fetch,
    submit_propagation_with_resource_config,
};
use radio_face::{
    DetailPolicy, EventKind, EventSource, HostSnapshot, IfacState, NodeSummary, PeerPath,
    PeerSummary, Personality, Text, UiEvent, encode_snapshot,
};
use retinue::Ifac;
use retinue::endpoint::{Endpoint, PayloadMode, ResourceTransferConfig};
use retinue::hash::AddressHash;
use retinue::identity::{Identity, PrivateIdentity};
use retinue::iface::tulle::drive;
use rmpv::Value;
use tokio::task::JoinHandle;
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink, DirectPhyUiControl};

const STAMP_COST: u8 = 8;
const LEFT_SEED: [u8; 64] = [0x31; 64];
const RIGHT_SEED: [u8; 64] = [0x42; 64];
const TIMESTAMP: f64 = 1_753_603_204.5;
static STAGE_SECS: AtomicU64 = AtomicU64::new(0);

fn profile(bandwidth_hz: u32) -> PhyProfile {
    PhyProfile {
        frequency_hz: 906_875_000,
        bandwidth_hz,
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

struct RadioPair {
    left: Arc<Endpoint>,
    right: Arc<Endpoint>,
    left_ui: DirectPhyUiControl,
    right_ui: DirectPhyUiControl,
    left_driver: JoinHandle<io::Result<()>>,
    right_driver: JoinHandle<io::Result<()>>,
}

impl RadioPair {
    async fn open(
        left_port: &str,
        right_port: &str,
        bandwidth_hz: u32,
        left_identity: &PrivateIdentity,
        right_identity: &PrivateIdentity,
        ifac: Option<Ifac>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let radio_config = DirectPhySerialConfig {
            online_timeout: Duration::from_secs(10),
            transmit_timeout: Duration::from_secs(10),
            ..DirectPhySerialConfig::default()
        };
        let mut left_radio = DirectPhySerialLink::open(
            left_port,
            profile(bandwidth_hz),
            AirtimeBudget::new(60_000, 60_000),
            radio_config.clone(),
        )?;
        let mut right_radio = DirectPhySerialLink::open(
            right_port,
            profile(bandwidth_hz),
            AirtimeBudget::new(60_000, 60_000),
            radio_config,
        )?;
        let left_ui = left_radio.ui_control();
        let right_ui = right_radio.ui_control();
        tokio::time::timeout(Duration::from_secs(15), left_radio.wait_online()).await??;
        tokio::time::timeout(Duration::from_secs(15), right_radio.wait_online()).await??;

        let left = Arc::new(Endpoint::new(left_identity.clone()));
        let right = Arc::new(Endpoint::new(right_identity.clone()));
        let logical_mtu = 255 - ifac.as_ref().map_or(0, Ifac::size);
        left.set_link_mtu(logical_mtu as u32);
        right.set_link_mtu(logical_mtu as u32);
        let left_interface = match &ifac {
            Some(ifac) => left.attach_interface_with_ifac(255, ifac.clone())?,
            None => left.attach_interface(),
        };
        let right_interface = match ifac {
            Some(ifac) => right.attach_interface_with_ifac(255, ifac)?,
            None => right.attach_interface(),
        };
        let left_driver = tokio::spawn(drive(left_interface, left_radio));
        let right_driver = tokio::spawn(drive(right_interface, right_radio));
        Ok(Self {
            left,
            right,
            left_ui,
            right_ui,
            left_driver,
            right_driver,
        })
    }

    async fn shutdown(self) -> Result<(), Box<dyn std::error::Error>> {
        let Self {
            left,
            right,
            left_driver,
            right_driver,
            ..
        } = self;
        tokio::join!(
            left.shutdown(Duration::from_secs(3)),
            right.shutdown(Duration::from_secs(3))
        );
        tokio::time::timeout(Duration::from_secs(10), left_driver).await???;
        tokio::time::timeout(Duration::from_secs(10), right_driver).await???;
        Ok(())
    }
}

struct View<'a> {
    endpoint: &'a Endpoint,
    identity: &'a Identity,
    node_name: &'a str,
    peer_name: Option<&'a str>,
    detail: DetailPolicy,
    ifac: IfacState,
    links: u8,
    admitted: u8,
    event_kind: EventKind,
    event_text: &'a str,
    started: Instant,
}

fn host_snapshot(view: &View<'_>) -> HostSnapshot {
    let identity_hash = *view.identity.hash().as_bytes();
    let destination = outrider::delivery_destination(view.identity);
    let mut address_tail = [0_u8; 8];
    address_tail.copy_from_slice(&destination.as_bytes()[8..]);
    let named = view.detail == DetailPolicy::Named;
    HostSnapshot {
        valid_for_secs: radio_face::MAX_VALIDITY_SECS,
        personality: Personality::Retinue,
        detail: view.detail,
        node: named.then_some(NodeSummary {
            name: Text::from_truncated(view.node_name),
            address_tail,
            fingerprint: identity_hash,
            role: Text::from_truncated("OUTRIDER"),
            uptime_secs: view.started.elapsed().as_secs().min(u64::from(u32::MAX)) as u32,
        }),
        link_count: view.links,
        admitted_links: view.admitted,
        queue_depth: view
            .endpoint
            .outbound_queue_depth()
            .min(usize::from(u16::MAX)) as u16,
        ifac: view.ifac,
        peers: [
            view.peer_name.map(|name| PeerSummary {
                name: Text::from_truncated(name),
                path: PeerPath::Direct,
                age_secs: 0,
            }),
            None,
            None,
        ],
        peer_overflow: 0,
        event: Some(UiEvent {
            source: EventSource::Host,
            kind: view.event_kind,
            text: Text::from_truncated(view.event_text),
        }),
    }
}

async fn publish(
    control: &DirectPhyUiControl,
    view: &View<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = host_snapshot(view);
    let mut encoded = [0_u8; radio_face::MAX_SNAPSHOT_LEN];
    let len = encode_snapshot(&snapshot, &mut encoded).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("host snapshot did not encode: {error:?}"),
        )
    })?;
    control.publish(&encoded[..len]).await?;
    let stage_secs = STAGE_SECS.load(Ordering::Relaxed);
    if stage_secs > 0 {
        println!("ui stage: {} for {stage_secs}s", view.event_text);
        tokio::time::sleep(Duration::from_secs(stage_secs)).await;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn view<'a>(
    endpoint: &'a Endpoint,
    identity: &'a Identity,
    node_name: &'a str,
    peer_name: Option<&'a str>,
    started: Instant,
    detail: DetailPolicy,
    ifac: IfacState,
    links: u8,
    admitted: u8,
    event_kind: EventKind,
    event_text: &'a str,
) -> View<'a> {
    View {
        endpoint,
        identity,
        node_name,
        peer_name,
        detail,
        ifac,
        links,
        admitted,
        event_kind,
        event_text,
        started,
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let left_port = args.next().unwrap_or_else(|| "COM6".into());
    let right_port = args.next().unwrap_or_else(|| "COM10".into());
    let bandwidth_hz = args
        .next()
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(250)
        * 1_000;
    let transfer_timeout = Duration::from_secs(
        args.next()
            .map(|value| value.parse::<u64>())
            .transpose()?
            .unwrap_or(180),
    );
    let hold_secs = args
        .next()
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(0);
    let stage_secs = args
        .next()
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(0);
    if args.next().is_some() {
        return Err("unexpected extra argument".into());
    }
    STAGE_SECS.store(stage_secs, Ordering::Relaxed);
    let resource_config = ResourceTransferConfig {
        timeout: transfer_timeout,
        retry_interval: Duration::from_secs(3),
        request_window: 1,
    };
    let operation_timeout = transfer_timeout + Duration::from_secs(60);
    let started = Instant::now();
    let left_identity = PrivateIdentity::from_secret_bytes(&LEFT_SEED);
    let right_identity = PrivateIdentity::from_secret_bytes(&RIGHT_SEED);

    let open = RadioPair::open(
        &left_port,
        &right_port,
        bandwidth_hz,
        &left_identity,
        &right_identity,
        None,
    )
    .await?;
    publish(
        &open.left_ui,
        &view(
            &open.left,
            left_identity.public(),
            "RF LEFT",
            None,
            started,
            DetailPolicy::Minimal,
            IfacState::Off,
            0,
            0,
            EventKind::Info,
            "INTERFACE OPEN",
        ),
    )
    .await?;
    publish(
        &open.right_ui,
        &view(
            &open.right,
            right_identity.public(),
            "RF RIGHT",
            None,
            started,
            DetailPolicy::Minimal,
            IfacState::Off,
            0,
            0,
            EventKind::Info,
            "INTERFACE OPEN",
        ),
    )
    .await?;
    println!("ui: minimal open-interface projection accepted on both radios");
    open.shutdown().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let ifac = Ifac::new(Some("retinue-u4"), Some("headed-host-projection"), 8)?;
    let pair = RadioPair::open(
        &left_port,
        &right_port,
        bandwidth_hz,
        &left_identity,
        &right_identity,
        Some(ifac),
    )
    .await?;
    println!("radios online: {left_port}=left, {right_port}=right; interface=IFAC authenticated");
    publish(
        &pair.right_ui,
        &view(
            &pair.right,
            right_identity.public(),
            "RF RIGHT",
            None,
            started,
            DetailPolicy::Named,
            IfacState::On,
            0,
            0,
            EventKind::Info,
            "IFAC ENABLED",
        ),
    )
    .await?;

    register_delivery(
        &pair.left,
        &DeliveryAnnounce {
            display_name: Some(b"Outrider RF Left".to_vec()),
            stamp_cost: Some(STAMP_COST),
        },
    )?;
    let left_announce =
        tokio::time::timeout(Duration::from_secs(20), pair.right.next_announcement()).await??;
    tokio::time::sleep(Duration::from_secs(2)).await;
    register_delivery(
        &pair.right,
        &DeliveryAnnounce {
            display_name: Some(b"Outrider RF Right".to_vec()),
            stamp_cost: Some(STAMP_COST),
        },
    )?;
    let right_announce =
        tokio::time::timeout(Duration::from_secs(20), pair.left.next_announcement()).await??;
    if right_announce.identity != *right_identity.public()
        || left_announce.identity != *left_identity.public()
    {
        return Err("delivery discovery resolved the wrong identity".into());
    }
    publish(
        &pair.right_ui,
        &view(
            &pair.right,
            right_identity.public(),
            "RF RIGHT",
            Some("RF LEFT"),
            started,
            DetailPolicy::Named,
            IfacState::On,
            0,
            0,
            EventKind::Received,
            "PEER DISCOVERED",
        ),
    )
    .await?;
    println!("discovery: IFAC-authenticated delivery announces crossed RF");

    let failed = pair
        .right
        .send_single(AddressHash::from_bytes([0xee; 16]), b"unreachable");
    match failed {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        _ => return Err("unannounced delivery did not fail with NotFound".into()),
    }
    publish(
        &pair.right_ui,
        &view(
            &pair.right,
            right_identity.public(),
            "RF RIGHT",
            Some("RF LEFT"),
            started,
            DetailPolicy::Named,
            IfacState::On,
            0,
            0,
            EventKind::Failed,
            "DELIVERY FAILED",
        ),
    )
    .await?;
    println!("host failure: unannounced destination rejected without a local radio fault");

    let expected_title = b"U4 DIRECT".to_vec();
    let expected_content = b"real Retinue delivery".to_vec();
    let receive_direct = async {
        let accepted =
            tokio::time::timeout(operation_timeout, pair.right.accept_resource()).await??;
        publish(
            &pair.right_ui,
            &view(
                &pair.right,
                right_identity.public(),
                "RF RIGHT",
                Some("RF LEFT"),
                started,
                DetailPolicy::Named,
                IfacState::On,
                1,
                1,
                EventKind::Received,
                "LINK ADMITTED",
            ),
        )
        .await?;
        let received = receive_direct_with_stamp_cost_and_resource_config(
            &pair.right,
            accepted,
            DEFAULT_MAX_MESSAGE_BYTES,
            Some(STAMP_COST),
            resource_config,
        )
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(received)
    };
    let send_direct = async {
        let payload =
            LxmfPayload::text(TIMESTAMP, expected_title.clone(), expected_content.clone());
        let receipt = tokio::time::timeout(
            operation_timeout,
            send_direct_stamped_with_resource_config(
                &pair.left,
                &left_identity,
                &right_announce,
                &payload,
                [0x10; 32],
                100_000,
                resource_config,
            ),
        )
        .await??;
        Ok::<_, Box<dyn std::error::Error>>(receipt)
    };
    let (received, sent) = tokio::join!(receive_direct, send_direct);
    let received = received?;
    let sent = sent?;
    if received.mode != PayloadMode::Data
        || sent.mode != PayloadMode::Data
        || received.message.message_id != sent.message_id
        || received.source_identity != *left_identity.public()
        || received.message.payload.title != expected_title
        || received.message.payload.content != expected_content
    {
        return Err("direct delivery was not byte-exact and authenticated".into());
    }
    publish(
        &pair.right_ui,
        &view(
            &pair.right,
            right_identity.public(),
            "RF RIGHT",
            Some("RF LEFT"),
            started,
            DetailPolicy::Named,
            IfacState::On,
            0,
            0,
            EventKind::Delivered,
            "DIRECT DELIVERED",
        ),
    )
    .await?;
    println!("direct delivery: authenticated Data message passed and UI receipt published");
    pair.shutdown().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let ifac = Ifac::new(Some("retinue-u4"), Some("headed-host-projection"), 8)?;
    let pair = RadioPair::open(
        &left_port,
        &right_port,
        bandwidth_hz,
        &left_identity,
        &right_identity,
        Some(ifac),
    )
    .await?;
    publish(
        &pair.right_ui,
        &view(
            &pair.right,
            right_identity.public(),
            "RF RIGHT",
            None,
            started,
            DetailPolicy::Named,
            IfacState::On,
            0,
            0,
            EventKind::Info,
            "PROP INTERFACE",
        ),
    )
    .await?;
    register_delivery(
        &pair.left,
        &DeliveryAnnounce {
            display_name: Some(b"Outrider RF Left".to_vec()),
            stamp_cost: Some(STAMP_COST),
        },
    )?;
    let left_announce =
        tokio::time::timeout(Duration::from_secs(20), pair.right.next_announcement()).await??;
    tokio::time::sleep(Duration::from_secs(2)).await;
    register_delivery(
        &pair.right,
        &DeliveryAnnounce {
            display_name: Some(b"Outrider RF Right".to_vec()),
            stamp_cost: Some(STAMP_COST),
        },
    )?;
    let right_announce =
        tokio::time::timeout(Duration::from_secs(20), pair.left.next_announcement()).await??;
    if right_announce.identity != *right_identity.public()
        || left_announce.identity != *left_identity.public()
    {
        return Err("propagation phase resolved the wrong delivery identity".into());
    }
    println!("propagation phase: fresh IFAC pair and delivery identities ready");

    let propagation_announce = PropagationAnnounce {
        legacy: false,
        unix_time: TIMESTAMP as u64,
        active: true,
        transfer_limit_kib: 256,
        sync_limit_kib: 10_240,
        costs: PropagationCosts {
            propagation: STAMP_COST,
            flexibility: 3,
            peering: 8,
        },
        metadata: vec![(
            Value::from(PROPAGATION_METADATA_NAME),
            Value::Binary(b"RF Right Propagation".to_vec()),
        )],
    };
    register_propagation(&pair.right, &propagation_announce)?;
    let propagation_node =
        tokio::time::timeout(Duration::from_secs(20), pair.left.next_announcement()).await??;
    println!("discovery: propagation-node announce crossed RF");

    let propagation_content = b"stored then fetched over RF".to_vec();
    let prepared = prepare_propagation(
        &right_identity,
        left_identity.public(),
        &LxmfPayload::text(TIMESTAMP, b"U4 PROPAGATION", propagation_content.clone()),
        &[0x31; 32],
        &[0x41; 16],
        [0x20; 32],
        u16::from(STAMP_COST),
        100_000,
    )?;
    let batch = PropagationBatch {
        transfer_time: TIMESTAMP + 0.5,
        entries: vec![prepared.entry],
    };
    println!("propagation batch: {} packed bytes", batch.encode()?.len());
    let receive_submission = async {
        let mut accepted =
            tokio::time::timeout(operation_timeout, pair.right.accept_resource()).await??;
        println!(
            "propagation submit: link admitted on interface {}",
            accepted.interface
        );
        accepted.session.set_config(resource_config);
        let received =
            receive_submission(&pair.right, accepted, u16::from(STAMP_COST), 4_096, 1).await?;
        println!(
            "propagation submit: payload received as {:?}",
            received.mode
        );
        publish(
            &pair.right_ui,
            &view(
                &pair.right,
                right_identity.public(),
                "RF RIGHT",
                Some("RF LEFT"),
                started,
                DetailPolicy::Named,
                IfacState::On,
                1,
                1,
                EventKind::Received,
                "LINK ADMITTED",
            ),
        )
        .await?;
        println!("propagation submit: UI admission receipt accepted");
        Ok::<_, Box<dyn std::error::Error>>(received)
    };
    let submit = async {
        println!("propagation submit: sender starting");
        let receipt = tokio::time::timeout(
            operation_timeout,
            submit_propagation_with_resource_config(
                &pair.left,
                &propagation_node,
                &batch,
                resource_config,
            ),
        )
        .await??;
        println!("propagation submit: sender completed as {:?}", receipt.mode);
        Ok::<_, Box<dyn std::error::Error>>(receipt)
    };
    let (received_batch, submitted) = tokio::join!(receive_submission, submit);
    let received_batch = received_batch?;
    let submitted = submitted?;
    if submitted.transient_ids != vec![prepared.transient_id]
        || received_batch.batch.entries[0].transient_id() != prepared.transient_id
    {
        return Err("propagation submission changed the transient id".into());
    }
    let mut store = PropagationStore::new(PropagationStoreLimits {
        max_entries: 4,
        max_bytes: 16 * 1024,
        max_message_bytes: 4_096,
        max_age: Duration::from_secs(60),
        max_per_fetch: 1,
    });
    let stored = store.ingest(&received_batch.batch, TIMESTAMP + 1.0);
    if stored.inserted != 1 || store.len() != 1 {
        return Err("propagation node did not store exactly one message".into());
    }
    publish(
        &pair.right_ui,
        &view(
            &pair.right,
            right_identity.public(),
            "RF RIGHT",
            Some("RF LEFT"),
            started,
            DetailPolicy::Named,
            IfacState::On,
            0,
            0,
            EventKind::Propagated,
            "PROP STORED",
        ),
    )
    .await?;
    println!(
        "propagation storage: inserted={} entries={} bytes={}",
        stored.inserted,
        store.len(),
        store.bytes()
    );
    tokio::time::sleep(Duration::from_secs(2)).await;

    let serve = async {
        let mut accepted =
            tokio::time::timeout(operation_timeout, pair.right.accept_resource()).await??;
        accepted.session.set_config(resource_config);
        publish(
            &pair.right_ui,
            &view(
                &pair.right,
                right_identity.public(),
                "RF RIGHT",
                Some("RF LEFT"),
                started,
                DetailPolicy::Named,
                IfacState::On,
                1,
                1,
                EventKind::Received,
                "LINK ADMITTED",
            ),
        )
        .await?;
        let served = serve_fetch(&pair.right, &mut accepted, &mut store, TIMESTAMP + 2.0).await?;
        Ok::<_, Box<dyn std::error::Error>>(served)
    };
    let fetch = async {
        let receipt = tokio::time::timeout(
            operation_timeout,
            fetch_propagation_with_resource_config(
                &pair.left,
                &left_identity,
                &propagation_node,
                &[],
                1,
                TIMESTAMP + 2.0,
                4_096,
                DEFAULT_MAX_MESSAGE_BYTES,
                resource_config,
            ),
        )
        .await??;
        Ok::<_, Box<dyn std::error::Error>>(receipt)
    };
    let (served, fetched) = tokio::join!(serve, fetch);
    let served = served?;
    let fetched = fetched?;
    if served.offered.len() != 1
        || served.served.len() != 1
        || fetched.offered.len() != 1
        || fetched.messages.len() != 1
        || fetched.messages[0].transient_id != prepared.transient_id
        || fetched.messages[0].source_identity != *right_identity.public()
        || fetched.messages[0].message.payload.title != b"U4 PROPAGATION"
        || fetched.messages[0].message.payload.content != propagation_content
    {
        return Err("propagation fetch was not byte-exact and authenticated".into());
    }
    let final_view = view(
        &pair.right,
        right_identity.public(),
        "RF RIGHT",
        Some("RF LEFT"),
        started,
        DetailPolicy::Named,
        IfacState::On,
        0,
        0,
        EventKind::Propagated,
        "PROP FETCHED",
    );
    publish(&pair.right_ui, &final_view).await?;
    println!("propagation fetch: offered=1 served=1 authenticated; UI receipt published");

    let mut held = 0_u64;
    while held < hold_secs {
        let interval = (hold_secs - held).min(60);
        tokio::time::sleep(Duration::from_secs(interval)).await;
        held += interval;
        publish(&pair.right_ui, &final_view).await?;
    }

    pair.shutdown().await?;
    println!("OUTRIDER DIRECT-PHY UI HEADED PASSED");
    Ok(())
}
