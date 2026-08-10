#![forbid(unsafe_code)]

//! The signal box, as a terminal.
//!
//! ```text
//! signalman PORT [NAME] [BW_KHZ] [phy|rnode]
//! ```
//!
//! One operator, one radio, a prompt. The radio work is [`postilion`]'s; the vocabulary is
//! [`signalman`]'s; what is here is only the glass: reading lines, printing events, and
//! keeping the prompt where a person expects it.

use std::io::{BufRead, Write};
use std::time::Duration;

use postilion::{Radio, Station, StationConfig};
use signalman::{Command, SEND_PATIENCE, describe, parse, render, report};

fn prompt() {
    print!("> ");
    let _ = std::io::stdout().flush();
}

/// Print an out-of-band line without eating the prompt the operator is typing at.
fn interject(line: &str) {
    println!("\n{line}");
    prompt();
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port = args
        .next()
        .ok_or("usage: signalman PORT [NAME] [BW_KHZ] [phy|rnode]")?;
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
    println!("commands: /peers  /to <prefix>  /who  /announce  /quit");

    // Blocking stdin on its own thread, so the runtime keeps driving the radio while a person
    // thinks about what to type.
    let (lines_tx, mut lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if lines_tx.send(line).is_err() {
                return;
            }
        }
    });

    let mut recipient: Option<String> = None;
    prompt();

    loop {
        tokio::select! {
            event = station.next_event() => match event {
                Some(event) => {
                    if let Some(line) = render(&event) {
                        interject(&line);
                    }
                }
                None => break,
            },
            line = lines.recv() => {
                let Some(line) = line else { break };
                match parse(&line) {
                    Command::Blank => {}
                    Command::Quit => return Ok(()),
                    Command::Who => {
                        println!("{}  ({name}) on {port} ({radio:?})", station.address());
                    }
                    Command::Announce => {
                        station.announce();
                        println!("(announced)");
                    }
                    Command::Peers => {
                        let peers = station.peers();
                        if peers.is_empty() {
                            println!("(nobody heard yet)");
                        }
                        for peer in &peers {
                            println!("{}", describe(peer));
                        }
                    }
                    Command::To("") => {
                        println!("who? /to <prefix>");
                    }
                    Command::To(prefix) => {
                        // Remembered whether or not they are in range yet: knowing an address
                        // before hearing its owner is the ordinary case. The send resolves it
                        // when they actually announce.
                        recipient = Some(prefix.to_string());
                        match station.find(prefix) {
                            Some(peer) => println!("talking to {}", peer.destination),
                            None => println!("holding {prefix} until they are heard"),
                        }
                    }
                    Command::Unknown(word) => {
                        println!("no such command: /{word}");
                    }
                    Command::Say(text) => match recipient.clone() {
                        None => println!("pick someone first: /peers then /to <prefix>"),
                        Some(prefix) => {
                            let sent = station
                                .send_text(&prefix, text, SEND_PATIENCE)
                                .await?;
                            println!("{}", report(&sent, &prefix));
                        }
                    },
                }
                prompt();
            }
        }
    }

    // Stdin ending is not the radio ending. A person who closes the terminal, or a script that
    // pipes a few commands and stops, should leave a station that still announces, still
    // receives, and still prints what arrives. Only /quit and Ctrl-C stop it.
    println!("(no more input; still listening, Ctrl-C to stop)");
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            event = station.next_event() => match event {
                Some(event) => {
                    if let Some(line) = render(&event) {
                        println!("{line}");
                    }
                }
                None => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            },
        }
    }
}
