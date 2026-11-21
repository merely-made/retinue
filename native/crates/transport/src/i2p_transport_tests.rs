use super::*;
use crate::i2p::TunnelProfile;
use crate::i2p::fixture::SamFixture;
use identity::UnassociatedKeypair;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use truchman::Key;

const ALPN: &str = "mere/i2p-test/v1";

async fn session(fixture: &SamFixture, publish: bool) -> I2pSession {
    let profile = TunnelProfile::new(2, 2, 2, publish).unwrap();
    I2pSession::create(fixture.bridge(), Key::Transient, profile, Duration::from_secs(2))
        .await
        .unwrap()
}

#[tokio::test]
async fn a_dialled_peer_is_proven_both_ways_and_carries_bytes() {
    let fixture = SamFixture::start().await;
    let server_key = UnassociatedKeypair::generate();
    let client_key = UnassociatedKeypair::generate();
    let server = I2pTransport::new(session(&fixture, true).await, &server_key, vec![Alpn::new(ALPN)]);
    let client = I2pTransport::new(session(&fixture, false).await, &client_key, vec![])
        .with_connect_timeout(Duration::from_secs(5));
    let peer = client.add_peer_ticket(&server.ticket().unwrap()).unwrap();
    assert_eq!(peer, server.local_peer_id());

    let dial = async {
        let mut outbound = client.connect_session(peer, Alpn::new(ALPN)).await.unwrap();
        assert_eq!(outbound.authenticated_initiator, Some(client.local_peer_id()));
        outbound.stream.write_all(b"over i2p").await.unwrap();
        outbound.stream.flush().await.unwrap();
        outbound
    };
    let (outbound, accepted) = tokio::join!(dial, server.accept(Alpn::new(ALPN)));
    let mut accepted = accepted.unwrap();
    assert_eq!(accepted.peer, Some(client.local_peer_id()), "Noise proved the initiator");
    let mut bytes = [0; 8];
    accepted.stream.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"over i2p");
    assert!(matches!(
        server.accept(Alpn::new("mere/other/v1")).await,
        Err(TransportError::AlpnNotRegistered)
    ));
    drop((outbound, accepted, client, server));
    fixture.shutdown().await;
}

#[tokio::test]
async fn a_destination_answering_as_another_peer_is_refused() {
    let fixture = SamFixture::start().await;
    let server = I2pTransport::new(
        session(&fixture, true).await,
        &UnassociatedKeypair::generate(),
        vec![Alpn::new(ALPN)],
    );
    let client = I2pTransport::new(session(&fixture, false).await, &UnassociatedKeypair::generate(), vec![])
        .with_connect_timeout(Duration::from_secs(5));
    // The server's destination, claimed for somebody else's key.
    let impostor = PeerID::from_public_key(UnassociatedKeypair::generate().public_key());
    let (_, address) = parse_ticket(&server.ticket().unwrap()).unwrap();
    let forged = format!("{TICKET_PREFIX}{}.{}", to_hex(&impostor.to_bytes()), address.as_str());
    let claimed = client.add_peer_ticket(&forged).unwrap();
    let (refused, _) = tokio::join!(
        client.connect(claimed, Alpn::new(ALPN)),
        timeout(Duration::from_secs(5), server.accept(Alpn::new(ALPN)))
    );
    let error = refused.err().expect("the impostor ticket is refused").to_string();
    assert!(error.contains("another peer"), "{error}");
    assert!(parse_ticket("mi2p1.zz.nope").is_err());
    drop((client, server));
    fixture.shutdown().await;
}
