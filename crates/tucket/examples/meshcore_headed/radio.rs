//! Tucket node pumps over the Tulle direct-PHY link.

use std::io;
use std::time::Duration;

use tokio::time::{Instant, timeout_at};
use tucket::node::{Event, Node};
use tulle::PhyProfile;
use tulle::direct_phy_serial::DirectPhySerialLink;

pub(super) fn radio_params(frequency_hz: u32) -> PhyProfile {
    PhyProfile::meshcore(frequency_hz, 250_000, 10, 5)
}

pub(super) async fn receive_ack_and_route(
    link: &mut DirectPhySerialLink,
    node: &mut Node,
    peer_hash: u8,
    expected_ack: [u8; 4],
) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut acknowledged = false;
    loop {
        let received = timeout_at(deadline, link.recv())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "MeshCore ACK timed out"))?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "radio stopped"))?;
        let (events, outgoing) = node.on_frame(&received.frame);
        for event in events {
            if matches!(event, Event::Ack(ack) if ack == expected_ack) {
                acknowledged = true;
            }
        }
        for frame in outgoing {
            link.send(frame)
                .await
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        if acknowledged && node.route_to(peer_hash).is_some() {
            return Ok(());
        }
    }
}

pub(super) async fn receive_text(
    link: &mut DirectPhySerialLink,
    node: &mut Node,
    expected: &str,
) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let received = timeout_at(deadline, link.recv())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "MeshCore text timed out"))?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "radio stopped"))?;
        let (events, outgoing) = node.on_frame(&received.frame);
        for frame in outgoing {
            link.send(frame)
                .await
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        for event in events {
            if let Event::Message { from, message, ack } = event {
                let ack = node.ack_frame_to(from, ack);
                link.send(ack)
                    .await
                    .map_err(|error| io::Error::other(error.to_string()))?;
                if message.text == expected {
                    return Ok(());
                }
            }
        }
    }
}

pub(super) async fn receive_route(
    link: &mut DirectPhySerialLink,
    node: &mut Node,
    peer_hash: u8,
) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(45);
    while node.route_to(peer_hash).is_none() {
        let received = timeout_at(deadline, link.recv())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "MeshCore path timed out"))?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "radio stopped"))?;
        let (_, outgoing) = node.on_frame(&received.frame);
        for frame in outgoing {
            link.send(frame)
                .await
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
    }
    Ok(())
}
