//! Normal-runtime bench for the V4's controller-authenticated `Status` over USB.
//!
//! The result is displayed as `auth=verified-controller`: the board answered a signed
//! outer command from a durable grant holder and journaled its counter first. This bench
//! takes its signer from the environment and its counter from the command line; it never
//! creates, stores, or advances either. A real controller keeps both.

use postilion::control::verified::{ControlClient, UsbControlConfig, UsbControlTransport};
use radio_hand::control::{ControlStatusBootFact, ControlStatusEvidence, NodeId};
use retinue::identity::{IDENTITY_LEN, PrivateIdentity};
use zeroize::Zeroizing;

fn usage() -> &'static str {
    "usage:\n  signalman-control-verified PORT EXPECTED_NODE_HEX COUNTER\n\nEXPECTED_NODE_HEX is the 16-byte opaque V4 node identifier. COUNTER is this controller's next unused outer counter: above the board's last accepted value and no more than 4096 ahead of it. SIGNALMAN_STATION_SECRET_HEX supplies the controller identity whose grant the board holds."
}

fn parse_node(value: &str) -> Result<NodeId, String> {
    let bytes =
        hex::decode(value).map_err(|error| format!("invalid expected node hex: {error}"))?;
    let node: [u8; 16] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| format!("expected node is {} bytes, need 16", bytes.len()))?;
    Ok(NodeId(node))
}

/// This bench path accepts a supplied identity but never creates or persists one.
fn controller_identity_from_env() -> Result<PrivateIdentity, Box<dyn std::error::Error>> {
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
    Ok(PrivateIdentity::from_secret_bytes(&secret))
}

const fn evidence_name(value: ControlStatusEvidence) -> &'static str {
    match value {
        ControlStatusEvidence::Blank => "blank",
        ControlStatusEvidence::Valid => "valid",
        ControlStatusEvidence::Corrupt => "corrupt",
    }
}

const fn boot_name(value: ControlStatusBootFact) -> &'static str {
    match value {
        ControlStatusBootFact::KnownGoodApplied => "known-good-applied",
        ControlStatusBootFact::RecoveredRollback => "recovered-rollback",
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port = args.next().ok_or_else(|| usage().to_owned())?;
    let expected_node = parse_node(&args.next().ok_or_else(|| usage().to_owned())?)?;
    let counter: u64 = args
        .next()
        .ok_or_else(|| usage().to_owned())?
        .parse()
        .map_err(|error| format!("invalid counter: {error}"))?;
    if args.next().is_some() {
        return Err(usage().into());
    }

    let identity = controller_identity_from_env()?;
    let transport = UsbControlTransport::open(port, UsbControlConfig::default())?;
    let mut controller = ControlClient::new(transport, &identity, expected_node);
    let verified = controller.status(counter).await?;
    let status = verified.status;
    println!(
        "auth=verified-controller carrier=usb node={} controller={} counter={} transaction={} control={} pending={} boot={} known-good-generation={} generation-watermark={}",
        hex::encode(status.node().0),
        identity.hash(),
        verified.counter,
        hex::encode(verified.transaction.0),
        evidence_name(status.control()),
        evidence_name(status.pending()),
        boot_name(status.boot()),
        status.known_good_generation().0,
        status.generation_watermark().0,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_node_must_be_exactly_sixteen_bytes() {
        assert_eq!(parse_node("22".repeat(16).as_str()), Ok(NodeId([0x22; 16])));
        assert!(parse_node("22".repeat(15).as_str()).is_err());
        assert!(parse_node("zz").is_err());
    }
}
