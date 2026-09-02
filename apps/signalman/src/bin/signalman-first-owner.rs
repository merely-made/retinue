//! Literal USB bench entry for the V4 first-owner carrier.
//!
//! This is intentionally separate from the normal `signalman PORT ...` radio terminal.

use radio_hand::region::Region;
use retinue::identity::{IDENTITY_LEN, PrivateIdentity};
use zeroize::Zeroizing;

use postilion::control::first_owner::{
    ClaimOutcome, FirstOwnerController, ResumeOutcome, UsbFirstOwnerConfig, UsbFirstOwnerTransport,
    v4_usb_claim_plan,
};

fn usage() -> &'static str {
    "usage:\n  signalman-first-owner inspect PORT\n  signalman-first-owner resume PORT\n  signalman-first-owner abandon PORT\n  signalman-first-owner claim PORT REGION FREQUENCY_HZ BANDWIDTH_HZ TX_POWER_DBM\n\nREGION: us915, eu868, eu433, anz915, jp920"
}

fn region(value: &str) -> Result<Region, String> {
    match value.to_ascii_lowercase().as_str() {
        "us915" => Ok(Region::Us915),
        "eu868" => Ok(Region::Eu868),
        "eu433" => Ok(Region::Eu433),
        "anz915" => Ok(Region::Anz915),
        "jp920" => Ok(Region::Jp920),
        _ => Err(format!(
            "unknown region {value}; choose us915, eu868, eu433, anz915, or jp920"
        )),
    }
}

/// This bench path accepts a supplied identity but never creates or persists one.
fn owner_identity_from_env() -> Result<PrivateIdentity, Box<dyn std::error::Error>> {
    let encoded = Zeroizing::new(std::env::var("SIGNALMAN_STATION_SECRET_HEX").map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "SIGNALMAN_STATION_SECRET_HEX is required; this tool does not create or store identities",
        )
    })?);
    let bytes = Zeroizing::new(
        hex::decode(&*encoded)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?,
    );
    let secret: Zeroizing<[u8; IDENTITY_LEN]> =
        Zeroizing::new(bytes.as_slice().try_into().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "SIGNALMAN_STATION_SECRET_HEX decoded to {} bytes, need {IDENTITY_LEN}",
                    bytes.len()
                ),
            )
        })?);
    let identity = PrivateIdentity::from_secret_bytes(&secret);
    Ok(identity)
}

fn node_text(node: radio_hand::control::NodeId) -> String {
    hex::encode(node.0)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(|| usage().to_owned())?;
    let port = args.next().ok_or_else(|| usage().to_owned())?;
    let transport = UsbFirstOwnerTransport::open(port, UsbFirstOwnerConfig::default())?;

    match command.as_str() {
        "inspect" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            let mut controller = FirstOwnerController::new(transport);
            let inspected = controller.inspect().await?;
            println!(
                "node={} status={:?} eligibility={:?} actions=0x{:02x}",
                node_text(inspected.node()),
                inspected.status(),
                inspected.eligibility(),
                inspected.actions().bits(),
            );
        }
        "claim" => {
            let region = region(&args.next().ok_or_else(|| usage().to_owned())?)?;
            let frequency_hz = args
                .next()
                .ok_or_else(|| usage().to_owned())?
                .parse::<u32>()?;
            let bandwidth_hz = args
                .next()
                .ok_or_else(|| usage().to_owned())?
                .parse::<u32>()?;
            let tx_power_dbm = args
                .next()
                .ok_or_else(|| usage().to_owned())?
                .parse::<i8>()?;
            if args.next().is_some() {
                return Err(usage().into());
            }
            let mut phy = postilion::profile(bandwidth_hz);
            phy.frequency_hz = frequency_hz;
            phy.tx_power_dbm = tx_power_dbm;
            let plan = v4_usb_claim_plan(region, phy)?;
            let identity = owner_identity_from_env()?;
            let mut controller = FirstOwnerController::new(transport);
            match controller.claim(&identity, plan).await? {
                ClaimOutcome::Committed => println!("claim outcome=committed"),
                ClaimOutcome::CommittedCleanupPending => {
                    println!("claim outcome=committed-cleanup-pending")
                }
            }
        }
        "resume" | "abandon" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            let mut controller = FirstOwnerController::new(transport);
            if command == "resume" {
                match controller.resume().await? {
                    ResumeOutcome::Committed => println!("resume outcome=committed"),
                    ResumeOutcome::CommittedCleanupPending => {
                        println!("resume outcome=committed-cleanup-pending")
                    }
                }
            } else {
                controller.abandon().await?;
                println!("abandon outcome=abandoned");
            }
        }
        _ => return Err(usage().into()),
    }
    Ok(())
}
