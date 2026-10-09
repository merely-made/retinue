//! Headed compatibility acceptance against an unmodified MeshCore companion.
//!
//! COM6 runs Tulle direct-PHY firmware. COM8 runs the official MeshCore
//! companion USB firmware. The companion API is used only for node management;
//! adverts, encrypted text, path replies, and acknowledgements cross the radio.

mod companion;
mod radio;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::{Instant, timeout, timeout_at};
use tucket::advert::AdvertData;
use tucket::companion::DeviceInfo;
use tucket::identity::LocalIdentity;
use tucket::node::{DirectRoute, Event, Node, TextRetryPolicy};
use tucket::packet::Packet;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};

use companion::{
    CMD_APP_START, CMD_DEVICE_QUERY, CMD_RESET_PATH, CMD_SEND_SELF_ADVERT, CMD_SET_ADVERT_NAME,
    CMD_SET_DEVICE_TIME, Companion, PUSH_SEND_CONFIRMED, RESP_DEVICE_INFO, RESP_OK, RESP_SELF_INFO,
    RESP_SENT, contact_text, import_command, radio_command, sync_text, text_command,
};
use radio::{radio_params, receive_ack_and_route, receive_route, receive_text};

fn now() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after Unix epoch")
        .as_secs() as u32
}

fn relay_path(value: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if !value.is_ascii() || !value.len().is_multiple_of(2) || !(2..=6).contains(&value.len()) {
        return Err("relay prefix must contain one, two or three complete hex bytes".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(Into::into))
        .collect()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let tulle_port = args.next().unwrap_or_else(|| "COM6".into());
    if tulle_port == "--probe" {
        let port = args
            .next()
            .ok_or("usage: meshcore_headed --probe <companion port>")?;
        let mut companion = Companion::open(&port)?;
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        let response = companion
            .expect(&[CMD_DEVICE_QUERY, 10], RESP_DEVICE_INFO)
            .await?;
        let info = DeviceInfo::decode(&response).ok_or("invalid MeshCore device info")?;
        println!(
            "{port}: model={}, firmware={}, build={}, companion_api={}",
            info.model, info.version, info.build, info.protocol_version
        );
        return Ok(());
    }
    let meshcore_port = args.next().unwrap_or_else(|| "COM8".into());
    let frequency_hz = args
        .next()
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(915_000_000);
    let relay_path = args.next().map(|value| relay_path(&value)).transpose()?;
    let flood_hash_size = std::env::var("MESHCORE_PATH_HASH_SIZE")
        .unwrap_or_else(|_| "1".into())
        .parse::<u8>()?;
    if !(1..=3).contains(&flood_hash_size) {
        return Err("MESHCORE_PATH_HASH_SIZE must be 1, 2 or 3".into());
    }

    let mut companion = Companion::open(&meshcore_port)?;
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    let device = companion
        .expect(&[CMD_DEVICE_QUERY, 10], RESP_DEVICE_INFO)
        .await?;
    let info = DeviceInfo::decode(&device).ok_or("invalid MeshCore device info")?;
    let expected_version =
        std::env::var("MESHCORE_EXPECTED_VERSION").unwrap_or_else(|_| "1.17.1".into());
    if !info.matches_release(&expected_version) {
        return Err(format!(
            "MeshCore firmware {} does not match expected {}; settings were not changed",
            info.version, expected_version
        )
        .into());
    }
    let firmware_protocol = info.protocol_version;
    println!(
        "MeshCore peer: model={}, firmware={}, build={}, companion_api={}",
        info.model, info.version, info.build, info.protocol_version
    );
    let mut app_start = vec![CMD_APP_START, 0, 0, 0, 0, 0, 0, 0];
    app_start.extend_from_slice(b"tucket-headed");
    let self_info = companion.expect(&app_start, RESP_SELF_INFO).await?;
    let stock_public: [u8; 32] = self_info
        .get(4..36)
        .ok_or("truncated MeshCore self info")?
        .try_into()?;
    companion
        .expect(&radio_command(frequency_hz), RESP_OK)
        .await?;
    companion
        .expect(
            &[
                CMD_SET_ADVERT_NAME,
                b'M',
                b'e',
                b's',
                b'h',
                b'C',
                b'o',
                b'r',
                b'e',
            ],
            RESP_OK,
        )
        .await?;
    let mut set_time = vec![CMD_SET_DEVICE_TIME];
    set_time.extend_from_slice(&now().to_le_bytes());
    companion.expect(&set_time, RESP_OK).await?;

    let mut link = DirectPhySerialLink::open(
        &tulle_port,
        radio_params(frequency_hz),
        AirtimeBudget::new(60_000, 60_000),
        DirectPhySerialConfig::default(),
    )?;
    timeout(Duration::from_secs(10), link.wait_online()).await??;
    println!(
        "radios online: {tulle_port}=Tulle direct PHY, {meshcore_port}=MeshCore companion protocol {firmware_protocol}"
    );

    let identity = LocalIdentity::from_seed([0x54; 32]);
    let tucket_public = identity.identity().pub_key;
    let mut tucket_prefix = [0_u8; 6];
    tucket_prefix.copy_from_slice(&tucket_public[..6]);
    let mut node = Node::new(identity, false);
    assert!(node.set_flood_hash_size(flood_hash_size));
    println!("Tucket outbound flood hash width: {flood_hash_size} bytes");

    let advert_data = AdvertData::chat("Tucket");
    let imported_advert = node
        .advert_frame_data(now(), &advert_data)
        .ok_or("Tucket advert data is too long")?;
    companion
        .expect(&import_command(&imported_advert), RESP_OK)
        .await?;
    link.send(
        node.advert_frame_data(now().wrapping_add(1), &advert_data)
            .ok_or("Tucket advert data is too long")?,
    )
    .await?;
    companion
        .expect(&[CMD_SEND_SELF_ADVERT, 1], RESP_OK)
        .await?;

    let stock_hash = stock_public[0];
    let advert_deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let received = timeout_at(advert_deadline, link.recv())
            .await?
            .ok_or("radio stopped while awaiting MeshCore advert")?;
        let (events, outgoing) = node.on_frame(&received.frame);
        for frame in outgoing {
            link.send(frame).await?;
        }
        if events.iter().any(
            |event| matches!(event, Event::Advert { identity, .. } if identity.pub_key == stock_public),
        ) {
            break;
        }
    }
    println!("authenticated adverts crossed the MeshCore/Tucket boundary");

    if let Some(relay_path) = relay_path {
        let hash_size = relay_path.len() as u8;
        let path_len = ((hash_size - 1) << 6) | 1;
        let route = DirectRoute::new(path_len, &relay_path).ok_or("invalid one-hop relay route")?;
        if !node.set_route(stock_hash, route) {
            return Err("MeshCore contact disappeared before route installation".into());
        }
        companion
            .set_contact_route(&tucket_public, &relay_path, hash_size)
            .await?;
        println!("forced reciprocal one-hop route through relay {relay_path:02x?}");

        let tucket_text = "Tucket source route crossed the relay";
        let (frame, expected_ack) = node
            .text_frame(stock_hash, now().wrapping_add(1), tucket_text)
            .ok_or("MeshCore contact disappeared")?;
        let packet = Packet::decode(&frame).ok_or("Tucket emitted a malformed packet")?;
        if packet.is_flood() || packet.path_len != path_len || packet.path != relay_path {
            return Err("Tucket did not select the forced relay route".into());
        }
        link.send(frame).await?;
        let received = sync_text(&mut companion, Duration::from_secs(45)).await?;
        if contact_text(&received) != Some(tucket_text) {
            return Err(format!(
                "MeshCore decoded unexpected relayed text: {:?}",
                contact_text(&received)
            )
            .into());
        }
        receive_ack_and_route(&mut link, &mut node, stock_hash, expected_ack).await?;
        if node
            .route_to(stock_hash)
            .is_none_or(|route| route.path_len() != path_len || route.path() != relay_path)
        {
            return Err("Tucket relay route changed while receiving the ACK".into());
        }
        println!("Tucket text and MeshCore ACK crossed the relay");

        let stock_text = "MeshCore source route crossed the relay";
        let sent = companion
            .expect(
                &text_command(&tucket_prefix, now().wrapping_add(2), stock_text),
                RESP_SENT,
            )
            .await?;
        if sent.get(1) != Some(&0) {
            return Err(
                "stock MeshCore flooded instead of selecting the forced relay route".into(),
            );
        }
        receive_text(&mut link, &mut node, stock_text).await?;
        companion
            .wait_push(PUSH_SEND_CONFIRMED, Duration::from_secs(45))
            .await?;
        println!("MeshCore text and Tucket ACK crossed the relay");
        println!("TUCKET MESHCORE RELAY HEADED PASSED");
        link.shutdown().await?;
        return Ok(());
    }

    let mut reset_path = vec![CMD_RESET_PATH];
    reset_path.extend_from_slice(&tucket_public);
    companion.expect(&reset_path, RESP_OK).await?;

    let first_text = "Stock MeshCore flood establishes a route";
    let sent = companion
        .expect(&text_command(&tucket_prefix, now(), first_text), RESP_SENT)
        .await?;
    if sent.get(1) != Some(&1) {
        return Err("stock MeshCore did not flood the route-discovery text".into());
    }
    receive_text(&mut link, &mut node, first_text).await?;
    companion
        .wait_push(PUSH_SEND_CONFIRMED, Duration::from_secs(45))
        .await?;
    receive_route(&mut link, &mut node, stock_hash).await?;
    println!("stock flood text, encrypted ACK, and reciprocal route passed");

    let direct_text = "Tucket direct route works";
    let (direct_frame, direct_ack) = node
        .text_frame(stock_hash, now().wrapping_add(1), direct_text)
        .ok_or("MeshCore contact disappeared")?;
    if Packet::decode(&direct_frame).is_none_or(|packet| packet.is_flood()) {
        return Err("second Tucket text did not select the learned direct route".into());
    }
    link.send(direct_frame).await?;
    let direct_received = sync_text(&mut companion, Duration::from_secs(45)).await?;
    if contact_text(&direct_received) != Some(direct_text) {
        return Err(format!(
            "MeshCore decoded unexpected direct text: {:?}",
            contact_text(&direct_received)
        )
        .into());
    }
    receive_ack_and_route(&mut link, &mut node, stock_hash, direct_ack).await?;
    println!("Tucket selected its learned direct route");

    let stock_text = "Stock MeshCore second direct reply";
    let sent = companion
        .expect(
            &text_command(&tucket_prefix, now().wrapping_add(2), stock_text),
            RESP_SENT,
        )
        .await?;
    if sent.get(1) != Some(&0) {
        return Err("stock MeshCore flooded instead of selecting its learned route".into());
    }
    receive_text(&mut link, &mut node, stock_text).await?;
    companion
        .wait_push(PUSH_SEND_CONFIRMED, Duration::from_secs(45))
        .await?;
    println!("stock MeshCore selected the reciprocal route and received Tucket's ACK");

    // The intentionally wrong hop cannot be consumed by the only stock peer.
    // Three actual RF transmissions must remain unacknowledged; the fourth
    // exercises Tucket's own pending-send policy and recovers through flooding.
    let wrong_hop = stock_hash.wrapping_add(1);
    let invalid_route = DirectRoute::new(1, &[wrong_hop]).ok_or("invalid test route")?;
    if !node.set_route(stock_hash, invalid_route) {
        return Err("stock contact disappeared before fallback test".into());
    }
    let fallback_text = "Tucket fourth attempt recovers through flood";
    let mut pending = node
        .begin_text(
            stock_hash,
            now().wrapping_add(3),
            fallback_text,
            TextRetryPolicy::default(),
        )
        .ok_or("could not begin fallback send")?;
    for number in 0..4 {
        let attempt = node
            .next_text_attempt(&mut pending)
            .ok_or("fallback attempt unexpectedly refused")?;
        if attempt.attempt != number || attempt.flooded != (number == 3) {
            return Err("fallback attempt selected the wrong route or number".into());
        }
        link.send(attempt.frame).await?;
        if number == 3 {
            let received = sync_text(&mut companion, Duration::from_secs(45)).await?;
            if contact_text(&received) != Some(fallback_text) {
                return Err("stock peer did not decode fallback text".into());
            }
            receive_ack_and_route(&mut link, &mut node, stock_hash, attempt.ack).await?;
            if !pending.acknowledge(attempt.ack) || !pending.is_complete() {
                return Err("stock ACK did not complete the pending send".into());
            }
        } else {
            let deadline = Instant::now() + Duration::from_secs(2);
            while let Ok(Some(received)) = timeout_at(deadline, link.recv()).await {
                let (events, outgoing) = node.on_frame(&received.frame);
                if events
                    .iter()
                    .any(|event| matches!(event, Event::Ack(ack) if *ack == attempt.ack))
                {
                    return Err("stock peer acknowledged an intentionally unreachable route".into());
                }
                for frame in outgoing {
                    link.send(frame).await?;
                }
            }
        }
        println!("fallback attempt {number}: flooded={}", attempt.flooded);
    }
    println!("three failed direct attempts, fourth flood, stock ACK and route recovery passed");
    println!("TUCKET MESHCORE HEADED PASSED");

    link.shutdown().await?;
    Ok(())
}
