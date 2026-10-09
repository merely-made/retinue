//! Black-box ticket oracle: stock LXMF hands Outrider a ticket, Outrider replies with it
//! instead of proof of work and hands one back, and stock's next message spends it.
//!
//! Driven by `oracle/interop_tickets.py`, which judges stock's side of each step.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use outrider::{DeliveredCache, DeliveryAnnounce, LxmfPayload, TicketBook, check_stamp};
use retinue::endpoint::{Endpoint, PeerAnnounce};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;

const SEED: [u8; 64] = [0x68; 64];
const COST: u8 = 8;

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_secs_f64()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let identity = PrivateIdentity::from_secret_bytes(&SEED);
    let endpoint = Arc::new(Endpoint::new(identity.clone()));
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    let announce = DeliveryAnnounce {
        display_name: Some(b"Outrider Tickets".to_vec()),
        stamp_cost: Some(COST),
    };
    let destination = outrider::register_delivery(&endpoint, &announce)?;
    println!("LISTENING {}", address.port());
    println!("DESTINATION {destination}");

    let announcer = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        async move {
            loop {
                outrider::announce_delivery(&endpoint, &announce).expect("announce encodes");
                tokio::time::sleep(Duration::from_millis(600)).await;
            }
        }
    });
    let stock: Arc<Mutex<Option<PeerAnnounce>>> = Arc::default();
    let listener = tokio::spawn({
        let (endpoint, stock) = (Arc::clone(&endpoint), Arc::clone(&stock));
        async move {
            while let Ok(peer) = endpoint.next_announcement().await {
                if peer.destination == outrider::delivery_destination(&peer.identity) {
                    stock.lock().unwrap().get_or_insert(peer);
                }
            }
        }
    });

    let book = Mutex::new(TicketBook::new());
    let delivered = DeliveredCache::default();
    let inbound = |source: &AddressHash| book.lock().unwrap().inbound(source, now());

    // 1. Stock's first message pays proof of work and carries its ticket.
    let accepted = tokio::time::timeout(Duration::from_secs(60), endpoint.accept_resource())
        .await
        .map_err(|_| "timed out waiting for stock's first message")??;
    let first = outrider::receive_direct_with_tickets(
        &endpoint,
        accepted,
        &delivered,
        outrider::DEFAULT_MAX_MESSAGE_BYTES,
        Some(COST),
        inbound,
        Default::default(),
    )
    .await?;
    let source = first
        .source_identity
        .ok_or("stock's first message is unverified")?;
    let learned = book
        .lock()
        .unwrap()
        .learn(&first.message, &source, now())
        .ok_or("stock's first message carried no ticket")?;
    println!("FIRST_ID {}", hex::encode(first.message.message_id));
    println!("LEARNED {}", hex::encode(learned.ticket));

    // 2. The reply spends that ticket and carries one of ours.
    let peer = loop {
        if let Some(peer) = stock.lock().unwrap().clone() {
            break peer;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let mut reply = LxmfPayload::text(now(), b"TICKET REPLY", b"no work done");
    let mut fresh = [0; outrider::TICKET_LEN];
    getrandom::fill(&mut fresh).map_err(|error| error.to_string())?;
    let issued = {
        let mut book = book.lock().unwrap();
        let issued = book
            .include(&mut reply, peer.destination, now(), fresh)
            .ok_or("no ticket to issue")?;
        if !book.stamp(&mut reply, peer.destination, identity.public(), now())? {
            return Err("no ticket held for the reply".into());
        }
        issued
    };
    let receipt = outrider::send_direct(&endpoint, &identity, &peer, &reply).await?;
    // The issuance interval starts at the proven delivery (`LXMRouter.py` 2765-2768).
    if !receipt.delivered {
        return Err("stock did not prove the reply".into());
    }
    book.lock().unwrap().delivered(peer.destination, now());
    println!("ISSUED {}", hex::encode(issued.ticket));
    println!("REPLY_ID {}", hex::encode(receipt.message_id));

    // 3. Stock's next message spends our ticket in place of proof of work.
    let accepted = tokio::time::timeout(Duration::from_secs(60), endpoint.accept_resource())
        .await
        .map_err(|_| "timed out waiting for stock's ticketed message")??;
    let second = outrider::receive_direct_with_tickets(
        &endpoint,
        accepted,
        &delivered,
        outrider::DEFAULT_MAX_MESSAGE_BYTES,
        Some(COST),
        inbound,
        Default::default(),
    )
    .await?;
    let outcome = check_stamp(
        &second.message.message_id,
        second.message.payload.stamp.as_deref(),
        COST,
        &inbound(&peer.destination),
    );
    println!("SECOND_ID {}", hex::encode(second.message.message_id));
    println!("SECOND_STAMP {outcome:?}");

    announcer.abort();
    listener.abort();
    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
