//! One person, one radio: LXMF chat over direct PHY.
//!
//! Every other direct-PHY example in this crate drives *two* radios from one process, which
//! proves the protocol and cannot be carried to opposite ends of a park. This is the missing
//! shape: one radio, one identity, one operator, typing at a prompt.
//!
//! ```text
//! park PORT [NAME] [BW_KHZ]
//! ```
//!
//! What it does:
//!
//! - Loads or creates an identity in `park-<NAME>.id` beside the binary, so a person keeps
//!   the same LXMF address across runs. Losing it means becoming a stranger to everyone.
//! - Announces `lxmf.delivery` on a timer, and learns peers from theirs.
//! - Prints every authenticated message that arrives, with who sent it.
//! - Sends what you type. `/peers` lists who is known; `/to <prefix>` selects a recipient by
//!   the leading hex of their address; a bare line goes to the current recipient.
//!
//! Stamps are off by default. A cost-8 stamp takes real seconds of CPU on a phone-class
//! machine and this is a liveness test, not a spam-resistance test; `--stamp N` turns it on.
//!
//! # Why this exists as an example rather than an app
//!
//! It is the first consumer of everything below it, and consumers are what reveal what a
//! shared host library actually has to expose. Guessing that shape before writing one would
//! be designing an interface for an imaginary caller. When a second face wants the same
//! logic, the shared parts move out of here and this becomes thin.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DEFAULT_MAX_MESSAGE_BYTES, DeliveryAnnounce, LxmfPayload, announce_delivery,
    receive_direct_with_stamp_cost, register_delivery, send_direct_stamped,
};
use retinue::endpoint::{Endpoint, PeerAnnounce};
use retinue::identity::PrivateIdentity;
use retinue::iface::tulle::drive;
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};

/// The trunk's LongFast-shaped profile, matching what the boards boot into.
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

/// Load this operator's identity, or mint and save one.
///
/// A file rather than a keyring because the point is that the address is stable and the
/// operator can see where it lives. It is a private key: the file is the account.
fn load_identity(name: &str) -> std::io::Result<PrivateIdentity> {
    let path = PathBuf::from(format!("park-{name}.id"));
    if let Ok(bytes) = std::fs::read(&path)
        && bytes.len() == 64
    {
        let mut seed = [0_u8; 64];
        seed.copy_from_slice(&bytes);
        println!("identity: loaded from {}", path.display());
        return Ok(PrivateIdentity::from_secret_bytes(&seed));
    }
    let mut seed = [0_u8; 64];
    getrandom::fill(&mut seed).expect("system entropy");
    std::fs::write(&path, seed)?;
    println!("identity: created {}", path.display());
    Ok(PrivateIdentity::from_secret_bytes(&seed))
}

/// Someone we have heard announce.
///
/// The whole `PeerAnnounce` is kept because that is what `send_direct_stamped` takes: taking
/// it apart into a destination and an identity would only mean rebuilding it later.
#[derive(Clone)]
struct Peer {
    announce: PeerAnnounce,
    stamp_cost: Option<u8>,
}

/// The first known peer whose address starts with `prefix`.
fn find_peer(peers: &Arc<std::sync::Mutex<Vec<Peer>>>, prefix: &str) -> Option<Peer> {
    peers
        .lock()
        .unwrap()
        .iter()
        .find(|p| p.announce.destination.to_string().starts_with(prefix))
        .cloned()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port = args.next().ok_or("usage: park PORT [NAME] [BW_KHZ]")?;
    let name = args.next().unwrap_or_else(|| "me".into());
    let bandwidth_hz = args
        .next()
        .map(|v| v.parse::<u32>())
        .transpose()?
        .unwrap_or(250)
        * 1_000;

    let identity = load_identity(&name)?;

    let mut radio = DirectPhySerialLink::open(
        &port,
        profile(bandwidth_hz),
        AirtimeBudget::new(60_000, 60_000),
        DirectPhySerialConfig {
            online_timeout: Duration::from_secs(10),
            transmit_timeout: Duration::from_secs(10),
            ..DirectPhySerialConfig::default()
        },
    )?;
    tokio::time::timeout(Duration::from_secs(15), radio.wait_online()).await??;
    println!("radio: {port} online");

    let endpoint = Arc::new(Endpoint::new(identity.clone()));
    endpoint.set_link_mtu(255);
    // Pacing derived from the profile, per `tulle::pacing`: the constants that used to sit
    // here fired retries into the answers they were waiting for.
    let params = tulle::lora::LoRaParams::try_from(profile(bandwidth_hz))?;
    endpoint.set_link_setup_retry(tulle::pacing::link_setup_retry(&params, false));
    let interface = endpoint.attach_interface();
    let driver = tokio::spawn(drive(interface, radio));

    // Register as an LXMF delivery destination and say so on the air.
    let announce = DeliveryAnnounce::named(name.as_bytes().to_vec());
    let me = register_delivery(&endpoint, &announce)?;
    println!("you are {me}  ({name})");
    println!("commands: /peers  /to <prefix>  /quit");

    // Registering makes the destination exist; it does not put it on the air. Announce
    // straight away so somebody already listening finds us, then keep announcing so
    // somebody who arrives later does too. Thirty seconds is chatty for a shared band and
    // right for a handful of people in a park; a real deployment wants far less.
    let _ = announce_delivery(&endpoint, &announce);
    {
        let endpoint = Arc::clone(&endpoint);
        let announce = announce.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let _ = announce_delivery(&endpoint, &announce);
            }
        });
    }

    let peers: Arc<std::sync::Mutex<Vec<Peer>>> = Arc::new(std::sync::Mutex::new(Vec::new()));

    // Learn peers from their announces.
    {
        let endpoint = Arc::clone(&endpoint);
        let peers = Arc::clone(&peers);
        tokio::spawn(async move {
            while let Ok(heard) = endpoint.next_announcement().await {
                let cost = DeliveryAnnounce::decode(&heard.app_data)
                    .ok()
                    .and_then(|a| a.stamp_cost);
                let mut table = peers.lock().unwrap();
                if !table
                    .iter()
                    .any(|p| p.announce.destination == heard.destination)
                {
                    println!("\n[peer] {} appeared", heard.destination);
                    print!("> ");
                    let _ = std::io::stdout().flush();
                    table.push(Peer {
                        announce: heard,
                        stamp_cost: cost,
                    });
                }
            }
        });
    }

    // Print what arrives.
    {
        let endpoint = Arc::clone(&endpoint);
        tokio::spawn(async move {
            loop {
                let Ok(accepted) = endpoint.accept_resource().await else {
                    break;
                };
                match receive_direct_with_stamp_cost(
                    &endpoint,
                    accepted,
                    DEFAULT_MAX_MESSAGE_BYTES,
                    None,
                )
                .await
                {
                    Ok(received) => {
                        let title = String::from_utf8_lossy(&received.message.payload.title);
                        let body = String::from_utf8_lossy(&received.message.payload.content);
                        // The sender's DELIVERY destination, not its identity hash: that is
                        // what `/peers` lists and what a person was told, so printing the
                        // identity hash would leave nobody able to match a message to a
                        // peer they know.
                        let from = outrider::delivery_destination(&received.source_identity);
                        println!("\n[{from}] {title}: {body}");
                        print!("> ");
                        let _ = std::io::stdout().flush();
                    }
                    Err(error) => println!("\n[dropped] {error}"),
                }
            }
        });
    }

    // The operator's own loop. Blocking stdin on a worker so the runtime keeps driving.
    let (lines_tx, mut lines_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines().map_while(Result::ok) {
            if lines_tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut recipient: Option<String> = None;
    print!("> ");
    std::io::stdout().flush()?;
    while let Some(line) = lines_rx.recv().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            print!("> ");
            std::io::stdout().flush()?;
            continue;
        }
        if line == "/quit" {
            driver.abort();
            return Ok(());
        }
        if line == "/peers" {
            let table = peers.lock().unwrap();
            if table.is_empty() {
                println!("(nobody heard yet)");
            }
            for peer in table.iter() {
                println!("  {} cost={:?}", peer.announce.destination, peer.stamp_cost);
            }
            print!("> ");
            std::io::stdout().flush()?;
            continue;
        }
        if let Some(prefix) = line.strip_prefix("/to ") {
            // Remembered whether or not they are in range yet. Knowing an address before
            // hearing its owner is the ordinary case in a park: you were told it, or you
            // met last week. The send resolves it when they actually announce.
            recipient = Some(prefix.trim().to_string());
            match find_peer(&peers, prefix.trim()) {
                Some(peer) => println!("talking to {}", peer.announce.destination),
                None => println!("holding {prefix} until they are heard"),
            }
            print!("> ");
            std::io::stdout().flush()?;
            continue;
        }

        let Some(prefix) = recipient.clone() else {
            println!("pick someone first: /peers then /to <prefix>");
            print!("> ");
            std::io::stdout().flush()?;
            continue;
        };

        // Wait briefly for the recipient to announce, rather than failing on a peer who is
        // simply between announces.
        let waited = std::time::Instant::now();
        let peer = loop {
            if let Some(peer) = find_peer(&peers, &prefix) {
                break Some(peer);
            }
            if waited.elapsed() > Duration::from_secs(75) {
                break None;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        };
        let Some(peer) = peer else {
            println!("nobody matching {prefix} announced in time");
            print!("> ");
            std::io::stdout().flush()?;
            continue;
        };

        let payload = LxmfPayload::text(now_secs(), name.as_bytes(), line.into_bytes());
        let sent = send_direct_stamped(
            &endpoint,
            &identity,
            &peer.announce,
            &payload,
            [0_u8; 32],
            // Stamp work is off: this is a liveness test, and a cost-8 stamp burns seconds
            // of CPU that prove nothing about the radio.
            0,
        )
        .await;
        match sent {
            Ok(receipt) => println!("(sent via {:?})", receipt.mode),
            Err(error) => println!("(send failed: {error})"),
        }
        print!("> ");
        std::io::stdout().flush()?;
    }

    // Stdin ending is not the radio ending. A person who closes the terminal, or a script
    // that pipes a few commands and stops, should leave a node that still announces, still
    // receives, and still prints what arrives. Only `/quit` and Ctrl-C stop it.
    println!("(no more input; still listening, Ctrl-C to stop)");
    let _ = tokio::signal::ctrl_c().await;

    driver.abort();
    Ok(())
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}
