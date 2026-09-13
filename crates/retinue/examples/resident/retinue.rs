//! Peer-side RF evidence for the board's retained Retinue home instance.
use retinue::{
    announce::{Announce, AnnounceBlob},
    hash::AddressHash,
    identity::Identity,
    link::{LinkMode, LinkTrailer, PendingLink},
    packet::{Packet, PacketType},
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::{Instant, timeout};
use tulle::direct_phy_serial::DirectPhySerialLink;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("host entropy: {e}"))?;
    Ok(bytes)
}

/// The caller arms the peer's home RX before sending resident setup. The board's
/// first announce is spontaneous; later returns do not reset its announce timer.
pub async fn prove_home(peer: &mut DirectPhySerialLink) -> Result<Value> {
    timeout(Duration::from_secs(6), async {
        loop {
            let received = peer.recv().await.ok_or("home peer RX stopped")?;
            let Ok(packet) = Packet::decode(&received.frame) else {
                continue;
            };
            if packet.packet_type != PacketType::Announce {
                continue;
            }
            let announce = Announce::decode(&packet)?;
            return Ok(json!({
                "kind": "retinue_home_announce", "signature_verified": true,
                "identity_public": hex::encode(announce.identity.to_public_bytes()),
                "identity_hash": hex::encode(announce.identity.hash().as_slice()),
                "destination": hex::encode(announce.destination.as_slice()),
                "name_hash": hex::encode(announce.name_hash.as_slice()),
                "blob": hex::encode(announce.rand_hash),
                "timebase": AnnounceBlob::from_wire(announce.rand_hash).timebase(),
                "frame_hex": hex::encode(received.frame),
                "rssi_dbm": received.rssi_dbm, "snr_db": received.snr_db,
            }));
        }
    })
    .await
    .map_err(|_| "resident home announce timed out")?
}

/// A fresh ephemeral request and a signature checked against the initially
/// witnessed identity establish that home RX and its retained identity work
/// after return. An encrypted close follows; caller checks the DUT LinkDown
/// report before making another visit.
pub async fn probe_home(peer: &mut DirectPhySerialLink, initial: &Value) -> Result<Value> {
    let public: [u8; 64] = hex::decode(
        initial["identity_public"]
            .as_str()
            .ok_or("missing identity")?,
    )?
    .try_into()
    .map_err(|_| "invalid identity length")?;
    let destination: [u8; 16] = hex::decode(
        initial["destination"]
            .as_str()
            .ok_or("missing destination")?,
    )?
    .try_into()
    .map_err(|_| "invalid destination length")?;
    let (pending, request) = PendingLink::open(
        AddressHash::from_bytes(destination),
        Identity::from_public_bytes(&public)?,
        &random()?,
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 255,
        },
    );
    let started = Instant::now();
    let request_bytes = request.encode();
    peer.send(request_bytes.clone()).await?;
    let received = timeout(Duration::from_secs(5), async {
        loop {
            let received = peer.recv().await.ok_or("home proof RX stopped")?;
            let Ok(packet) = Packet::decode(&received.frame) else {
                continue;
            };
            if packet.packet_type == PacketType::Proof && packet.destination == pending.link_id() {
                return Ok::<_, Box<dyn std::error::Error>>((received, packet));
            }
        }
    })
    .await
    .map_err(|_| {
        format!(
            "resident home proof timed out; request={}",
            hex::encode(&request_bytes)
        )
    })??;
    let link = pending.prove(&received.1)?;
    let close_bytes = link.close_packet(&random()?).encode();
    peer.send(close_bytes.clone()).await?;
    Ok(json!({
        "kind": "retinue_home_link_proof", "signature_verified": true,
        "same_identity": true, "link_id": hex::encode(pending.link_id().as_slice()),
        "request_hex": hex::encode(request_bytes), "proof_hex": hex::encode(received.0.frame),
        "close_hex": hex::encode(close_bytes), "close_tx_ack": true,
        "rssi_dbm": received.0.rssi_dbm, "snr_db": received.0.snr_db,
        "elapsed_ms": started.elapsed().as_millis(),
    }))
}
