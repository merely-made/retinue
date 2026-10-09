//! Receipted link data: `deliver_payload` against a hand-driven responder that proves,
//! answers, or ignores the packet.

use super::*;

use retinue::endpoint::{LinkDelivery, PayloadReceipt};
use retinue::link::{Link, LinkMode, LinkTrailer, accept};
use retinue::packet::PacketType;

enum Reply {
    Prove,
    Answer(&'static [u8]),
    Ignore,
}

/// Run `deliver_payload` for `data` against a responder that accepts the link, reads the one
/// data packet, and replies as told. Returns the receipt and the bytes the responder read.
async fn deliver_against(data: &[u8], reply: Reply) -> (PayloadReceipt, Vec<u8>) {
    let server = PrivateIdentity::from_secret_bytes(&[0x71; 64]);
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x72; 64]));
    let (mut out, sink) = client.attach_interface().split();
    let dest = DestinationName::new("retinue", ["receipt"]).destination_hash(server.public());

    let responder = async {
        let mut link: Option<Link> = None;
        loop {
            let packet = out.recv().await.expect("client stays up");
            if packet.packet_type == PacketType::LinkRequest {
                let offered = LinkTrailer {
                    mode: LinkMode::DEFAULT,
                    mtu: 500,
                };
                let (accepted, proof) = accept(&packet, &server, &[0x73; 64], offered).unwrap();
                link = Some(accepted);
                assert!(sink.deliver(proof));
                continue;
            }
            let Some(link) = &link else { continue };
            if packet.packet_type != PacketType::Data || packet.context != 0 {
                continue;
            }
            let read = link.decrypt(&packet).unwrap();
            match reply {
                Reply::Prove => assert!(sink.deliver(link.prove_packet(&packet))),
                Reply::Answer(answer) => assert!(sink.deliver(link.data_packet(answer, &[9; 16]))),
                Reply::Ignore => {}
            }
            return read;
        }
    };
    let sending =
        client.deliver_payload(dest, *server.public(), data, quick(Duration::from_secs(5)));
    let (receipt, read) = tokio::join!(sending, responder);
    (receipt.unwrap(), read)
}

#[tokio::test]
async fn a_proved_data_packet_concludes_its_receipt() {
    let (receipt, read) = deliver_against(b"receipted", Reply::Prove).await;
    assert_eq!(read, b"receipted");
    assert_eq!(receipt.mode, PayloadMode::Data);
    assert!(matches!(receipt.delivery, LinkDelivery::Proved { .. }));
}

#[tokio::test]
async fn an_answer_instead_of_a_proof_is_returned() {
    let (receipt, _) = deliver_against(b"refuse me", Reply::Answer(&[0x91, 0xcc, 0xf5])).await;
    assert_eq!(
        receipt.delivery,
        LinkDelivery::Answered(vec![0x91, 0xcc, 0xf5])
    );
}

#[tokio::test(start_paused = true)]
async fn an_unproved_data_packet_times_out_on_its_receipt() {
    let (receipt, _) = deliver_against(b"into the void", Reply::Ignore).await;
    assert_eq!(receipt.delivery, LinkDelivery::Unproved);
}

#[tokio::test]
async fn a_resource_is_proved_by_its_own_transfer() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x74; 64]);
    let server = Endpoint::new(server_id.clone());
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x75; 64]));
    connect(&client, &server, LossModel::new(1), LossModel::new(2));
    let name = DestinationName::new("retinue", ["receipt-resource"]);
    let dest = name.destination_hash(server_id.public());
    server.register_resource(name, b"");

    let payload = incompressible(2_000);
    let receiving = async {
        let mut accepted = server.accept_resource().await.unwrap();
        accepted.session.receive().await.unwrap()
    };
    let sending = client.deliver_payload(
        dest,
        *server_id.public(),
        &payload,
        quick(Duration::from_secs(5)),
    );
    let (receipt, received) = tokio::join!(sending, receiving);
    let receipt = receipt.unwrap();
    assert_eq!(receipt.mode, PayloadMode::Resource);
    assert!(matches!(receipt.delivery, LinkDelivery::Proved { .. }));
    assert_eq!(received, ReceivedPayload::Resource(payload));
}
