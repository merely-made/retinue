//! Black-box paper-message oracle: `lxm://` URIs between Outrider and stock LXMF.
//!
//! `stock_paper encode <recipient public key hex>` writes a paper message to the recipient.
//! `stock_paper decode <uri> <sender public key hex>` reads one addressed to this identity.
//! Driven by `oracle/interop_paper.py`, which judges stock's side.

use outrider::{LxmfPayload, PropagationMessage, prepare_paper};
use retinue::identity::{Identity, PrivateIdentity};

const SEED: [u8; 64] = [0x6a; 64];
const TIMESTAMP: f64 = 1_753_603_203.5;

fn identity(hex_key: &str) -> Result<Identity, Box<dyn std::error::Error>> {
    let bytes = hex::decode(hex_key)?
        .try_into()
        .map_err(|_| "a public key is 64 bytes")?;
    Ok(Identity::from_public_bytes(&bytes)?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let me = PrivateIdentity::from_secret_bytes(&SEED);
    println!("PUBLIC {}", hex::encode(me.public().to_public_bytes()));
    println!(
        "DESTINATION {}",
        outrider::delivery_destination(me.public())
    );
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["encode", recipient] => {
            let payload = LxmfPayload::text(TIMESTAMP, b"OUTRIDER PAPER", b"written by hand");
            let (mut ephemeral, mut iv) = ([0; 32], [0; 16]);
            getrandom::fill(&mut ephemeral).map_err(|error| error.to_string())?;
            getrandom::fill(&mut iv).map_err(|error| error.to_string())?;
            let paper = prepare_paper(&me, &identity(recipient)?, &payload, &ephemeral, &iv)?;
            println!("MESSAGE_ID {}", hex::encode(paper.message_id));
            println!("URI {}", paper.message.to_uri()?);
        }
        ["decode", uri, sender] => {
            let message = PropagationMessage::from_uri(uri, outrider::PAPER_MDU)?
                .decrypt(&me, outrider::DEFAULT_MAX_MESSAGE_BYTES)?;
            let sender = identity(sender)?;
            let verified = message.source == *outrider::delivery_destination(&sender).as_bytes()
                && message.verify_with(|bytes, signature| sender.verify(bytes, signature));
            println!("MESSAGE_ID {}", hex::encode(message.message_id));
            println!("TITLE {}", hex::encode(&message.payload.title));
            println!("CONTENT {}", hex::encode(&message.payload.content));
            println!("VERIFIED {verified}");
        }
        _ => return Err("usage: stock_paper encode <recipient> | decode <uri> <sender>".into()),
    }
    Ok(())
}
