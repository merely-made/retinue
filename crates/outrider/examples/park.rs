//! One person, one radio: LXMF chat over either board personality.
//!
//! ```text
//! park PORT [NAME] [BW_KHZ] [phy|rnode]
//! ```
//!
//! This was the first consumer of everything below it, and consumers are what reveal what a
//! shared host library has to expose. That job is done: what it revealed now lives in
//! [`postilion`], and the operator's side of it in `signalman`. What remains here is the
//! example it always should have been — the shortest honest demonstration that outrider's
//! delivery works over a real radio — and the bench harness that drives it by name.
//!
//! For the operator-facing tool, run `signalman` instead. It takes the same arguments.

use std::io::{BufRead, Write};
use std::time::Duration;

use postilion::{Event, Radio, Sent, Station, StationConfig};

/// How long a send waits for a recipient who has not announced yet.
const PATIENCE: Duration = Duration::from_secs(75);

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port = args
        .next()
        .ok_or("usage: park PORT [NAME] [BW_KHZ] [phy|rnode]")?;
    let name = args.next().unwrap_or_else(|| "me".into());
    let bandwidth_hz = args
        .next()
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(250)
        * 1_000;
    let radio = match args.next().as_deref() {
        None => Radio::default(),
        Some(mode) => Radio::parse(mode)
            .ok_or_else(|| format!("unknown radio mode {mode}, want phy or rnode"))?,
    };

    let mut station = Station::open(StationConfig {
        identity_path: StationConfig::identity_for(&name),
        port: port.clone(),
        name: name.clone(),
        bandwidth_hz,
        radio,
        ..StationConfig::default()
    })
    .await?;

    println!("radio: {port} online ({radio:?})");
    println!("you are {}  ({name})", station.address());
    println!("commands: /peers  /to <prefix>  /quit");

    let (lines_tx, mut lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if lines_tx.send(line).is_err() {
                return;
            }
        }
    });

    let mut recipient: Option<String> = None;
    print!("> ");
    std::io::stdout().flush()?;

    loop {
        tokio::select! {
            event = station.next_event() => {
                let Some(event) = event else { break };
                match event {
                    Event::PeerAppeared(peer) => println!("\n[peer] {} appeared", peer.destination),
                    Event::Message { from, title, body } => println!(
                        "\n[{from}] {}: {}",
                        String::from_utf8_lossy(&title),
                        String::from_utf8_lossy(&body),
                    ),
                    Event::Dropped(reason) => println!("\n[dropped] {reason}"),
                }
                print!("> ");
                std::io::stdout().flush()?;
            }
            line = lines.recv() => {
                let Some(line) = line else { break };
                let line = line.trim().to_string();
                if line == "/quit" {
                    return Ok(());
                } else if line == "/peers" {
                    let peers = station.peers();
                    if peers.is_empty() {
                        println!("(nobody heard yet)");
                    }
                    for peer in &peers {
                        println!("  {} cost={:?}", peer.destination, peer.stamp_cost);
                    }
                } else if let Some(prefix) = line.strip_prefix("/to ") {
                    let prefix = prefix.trim().to_string();
                    match station.find(&prefix) {
                        Some(peer) => println!("talking to {}", peer.destination),
                        None => println!("holding {prefix} until they are heard"),
                    }
                    recipient = Some(prefix);
                } else if !line.is_empty() {
                    match recipient.clone() {
                        None => println!("pick someone first: /peers then /to <prefix>"),
                        Some(prefix) => match station.send_text(&prefix, &line, PATIENCE).await? {
                            Sent::Delivered { mode } => println!("(sent via {mode:?})"),
                            Sent::NoSuchPeer => {
                                println!("nobody matching {prefix} announced in time")
                            }
                        },
                    }
                }
                print!("> ");
                std::io::stdout().flush()?;
            }
        }
    }

    // Stdin ending is not the radio ending: a piped script should leave a node still
    // announcing and still receiving. Only /quit and Ctrl-C stop it.
    println!("(no more input; still listening, Ctrl-C to stop)");
    let _ = tokio::signal::ctrl_c().await;
    Ok(())
}
