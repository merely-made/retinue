//! Normal-runtime bench for the V4's public control-status diagnostic.
//!
//! The result is intentionally displayed as `auth=diagnostic-only`. It is not
//! a controller authentication result and this binary has no first-owner,
//! claim, or signing path.

use postilion::control::status::{
    UsbControlStatusConfig, UsbControlStatusTransport, validate_diagnostic_status,
};
use radio_hand::control::{ControlStatusBootFact, ControlStatusEvidence, NodeId};

fn usage() -> &'static str {
    "usage:\n  signalman-control-status PORT EXPECTED_NODE_HEX\n\nEXPECTED_NODE_HEX is the 16-byte opaque V4 node identifier reported during first-owner inspect."
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
    if args.next().is_some() {
        return Err(usage().into());
    }

    let mut transport = UsbControlStatusTransport::open(port, UsbControlStatusConfig::default())?;
    let status = validate_diagnostic_status(transport.read().await?, expected_node)?;
    println!(
        "auth=diagnostic-only transport=modem-only node={} control={} pending={} boot={} known-good-generation={} generation-watermark={}",
        hex::encode(status.node().0),
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
        assert_eq!(parse_node("11".repeat(16).as_str()), Ok(NodeId([0x11; 16])));
        assert!(parse_node("11".repeat(15).as_str()).is_err());
        assert!(parse_node("zz").is_err());
    }
}
