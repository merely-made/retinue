//! Emit a deterministic Retinue-built ratchet packet for the stock RNS oracle.

use retinue::destination::DestinationName;
use retinue::identity::{KEY_LEN, PrivateIdentity};
use retinue::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};
use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};

fn main() {
    let recipient = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let destination =
        DestinationName::new("retinue", ["ratchet"]).destination_hash(recipient.public());
    let ratchet_public = XPublicKey::from(&StaticSecret::from([0x71; KEY_LEN]));
    let payload = retinue::token::encrypt_to_ratchet(
        recipient.public(),
        ratchet_public.as_bytes(),
        &[0x73; KEY_LEN],
        &[0x74; retinue::token::IV_LEN],
        b"RETINUE-R9-OUTBOUND",
    );
    let packet = Packet {
        ifac: false,
        header_type: HeaderType::Type1,
        context_flag: false,
        propagation: Propagation::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Data,
        hops: 0,
        transport: None,
        destination,
        context: 0,
        payload,
    };
    println!("{}", hex::encode(packet.encode()));
}
