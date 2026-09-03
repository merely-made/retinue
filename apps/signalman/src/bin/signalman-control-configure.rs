//! Bench for the V4's controller-authenticated configuration lifecycle over USB.
//!
//! Three actions: `apply` stages one PHY profile as the provisional candidate and applies
//! it, printing the board-minted commit token; `commit` confirms that candidate; `revert`
//! abandons it. The signer comes from the environment and the outer counter and mutation
//! sequence from the command line; this bench never creates, stores, or advances either.
//! A candidate left unconfirmed rolls back on the board when its lifetime passes.

use postilion::control::verified::{
    ControlClient, Mutation, ProvisionalReceipt, UsbControlConfig, UsbControlTransport,
};
use radio_hand::control::{
    COMMIT_TOKEN_LEN, ChangeId, ConfigGeneration, ManagementCarrierSet, NodeId,
    PublicConfigurationV1, ReticulumTransportPolicy,
};
use radio_hand::region::Region;
use retinue::identity::{IDENTITY_LEN, PrivateIdentity};
use zeroize::Zeroizing;

fn usage() -> &'static str {
    "usage:\n  signalman-control-configure PORT NODE_HEX COUNTER SEQUENCE EXPECTED_GENERATION apply CHANGE_HEX REGION FREQUENCY_HZ BANDWIDTH_HZ TX_POWER_DBM LIFETIME_MS\n  signalman-control-configure PORT NODE_HEX COUNTER SEQUENCE EXPECTED_GENERATION commit CHANGE_HEX CANDIDATE_GENERATION TOKEN_HEX\n  signalman-control-configure PORT NODE_HEX COUNTER SEQUENCE EXPECTED_GENERATION revert CHANGE_HEX\n\nCOUNTER is this controller's next unused outer counter; SEQUENCE its next mutation sequence; EXPECTED_GENERATION the known-good generation last read. CHANGE_HEX is a 16-byte id the controller chooses and reuses for the commit or revert of the same change. REGION: us915, eu868, eu433, anz915, jp920. SIGNALMAN_STATION_SECRET_HEX supplies the controller identity whose grant the board holds."
}

fn parse_hex16(value: &str, what: &str) -> Result<[u8; 16], String> {
    let bytes = hex::decode(value).map_err(|error| format!("invalid {what} hex: {error}"))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| format!("{what} is {} bytes, need 16", bytes.len()))
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

fn next<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
    what: &str,
) -> Result<T, Box<dyn std::error::Error>>
where
    T::Err: std::fmt::Display,
{
    let value = args.next().ok_or_else(|| usage().to_owned())?;
    value
        .parse::<T>()
        .map_err(|error| format!("invalid {what} {value:?}: {error}").into())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port = args.next().ok_or_else(|| usage().to_owned())?;
    let node = NodeId(parse_hex16(
        &args.next().ok_or_else(|| usage().to_owned())?,
        "node",
    )?);
    let counter: u64 = next(&mut args, "counter")?;
    let mutation = Mutation {
        sequence: next(&mut args, "sequence")?,
        expected_generation: ConfigGeneration(next(&mut args, "expected generation")?),
    };
    let action = args.next().ok_or_else(|| usage().to_owned())?;
    let change = ChangeId(parse_hex16(
        &args.next().ok_or_else(|| usage().to_owned())?,
        "change",
    )?);

    let identity = controller_identity_from_env()?;
    let transport = UsbControlTransport::open(port, UsbControlConfig::default())?;
    let mut controller = ControlClient::new(transport, &identity, node);

    match action.as_str() {
        "apply" => {
            let region = region(&args.next().ok_or_else(|| usage().to_owned())?)?;
            let frequency_hz: u32 = next(&mut args, "frequency")?;
            let bandwidth_hz: u32 = next(&mut args, "bandwidth")?;
            let tx_power_dbm: i8 = next(&mut args, "tx power")?;
            let lifetime_ms: u64 = next(&mut args, "lifetime")?;
            if args.next().is_some() {
                return Err(usage().into());
            }
            let mut phy = postilion::profile(bandwidth_hz);
            phy.frequency_hz = frequency_hz;
            phy.tx_power_dbm = tx_power_dbm;
            let public = PublicConfigurationV1::new(
                region,
                phy,
                ReticulumTransportPolicy::new(false, false, 0)
                    .map_err(|error| format!("transport policy: {error:?}"))?,
                ManagementCarrierSet::from_mask(1)
                    .map_err(|error| format!("carrier set: {error:?}"))?,
            )
            .map_err(|error| format!("public configuration: {error:?}"))?;
            let receipt = controller
                .provisional_apply(counter, mutation, change, public, lifetime_ms)
                .await?;
            println!(
                "auth=verified-controller action=apply node={} counter={} transaction={} change={} candidate-generation={} deadline-ms={} commit-token={}",
                hex::encode(node.0),
                receipt.counter,
                hex::encode(receipt.transaction.0),
                hex::encode(receipt.change.0),
                receipt.candidate_generation.0,
                receipt.deadline_ms,
                hex::encode(receipt.commit_token),
            );
        }
        "commit" => {
            let candidate_generation = ConfigGeneration(next(&mut args, "candidate generation")?);
            let commit_token: [u8; COMMIT_TOKEN_LEN] =
                parse_hex16(&args.next().ok_or_else(|| usage().to_owned())?, "token")?;
            if args.next().is_some() {
                return Err(usage().into());
            }
            let receipt = ProvisionalReceipt {
                transaction: radio_hand::control::TransactionId([0; 16]),
                counter: 0,
                change,
                candidate_generation,
                deadline_ms: 0,
                commit_token,
            };
            let applied = controller.commit(counter, mutation, &receipt).await?;
            println!(
                "auth=verified-controller action=commit node={} counter={} transaction={} change={} known-good-generation={}",
                hex::encode(node.0),
                applied.counter,
                hex::encode(applied.transaction.0),
                hex::encode(change.0),
                applied.known_good_generation.0,
            );
        }
        "revert" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            let applied = controller.revert(counter, mutation, change).await?;
            println!(
                "auth=verified-controller action=revert node={} counter={} transaction={} change={} known-good-generation={}",
                hex::encode(node.0),
                applied.counter,
                hex::encode(applied.transaction.0),
                hex::encode(change.0),
                applied.known_good_generation.0,
            );
        }
        _ => return Err(usage().into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_byte_hex_arguments_are_exact() {
        assert_eq!(parse_hex16(&"ab".repeat(16), "node"), Ok([0xab; 16]));
        assert!(parse_hex16(&"ab".repeat(15), "node").is_err());
        assert!(parse_hex16("zz", "node").is_err());
    }
}
