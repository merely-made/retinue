//! Bridge between an endpoint raw interface and a Tulle packet radio.

use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

use std::future::Future;
use std::io;
use std::time::Duration;

use tokio::time::{Instant, sleep_until};
use tulle::lora::LoRaParams;
use tulle::radio_io::PacketRadio;
use tulle::serial::TransmitError;

use super::beacon::Beacon;
use crate::announce_admission::AnnounceIngressPolicy;
use crate::endpoint::{Endpoint, Interface, InterfaceId};
use crate::packet::PacketType;

/// Drive one endpoint interface over a running Tulle radio until either side
/// closes or an outbound packet cannot be transmitted.
///
/// The radio's complete-frame limit is installed synchronously when this
/// function is called, before the returned future is polled. Endpoint queues
/// therefore refuse oversized packets before issuing a receipt. The transmit
/// check remains as a backstop for traffic queued before a driver was
/// constructed. Malformed RF packets are dropped at the boundary.
pub fn drive<R>(interface: Interface, radio: R) -> impl Future<Output = io::Result<()>>
where
    R: PacketRadio,
{
    drive_with_beacon(interface, radio, None)
}

/// [`drive`], also sending a station ID `beacon` once its interval has passed after
/// traffic (`RNodeInterface.py` 711-744, 1145-1149).
///
/// Frames a supervised radio drops while reopening its port are lost, as RNS loses them
/// while offline; they do not end the driver.
pub fn drive_with_beacon<R>(
    interface: Interface,
    mut radio: R,
    mut beacon: Option<Beacon>,
) -> impl Future<Output = io::Result<()>>
where
    R: PacketRadio,
{
    let max_frame_len = radio.max_frame_len();
    interface.constrain_frame_limit(max_frame_len);

    async move {
        let (mut outbound, sink) = interface.split();
        let epoch = Instant::now();
        let now_ms = || u64::try_from(epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        loop {
            let wake = beacon
                .as_ref()
                .and_then(Beacon::due_at_ms)
                .map_or(Instant::now() + Duration::from_secs(3600), |ms| {
                    epoch + Duration::from_millis(ms + 1)
                });
            tokio::select! {
                packet = outbound.recv() => {
                    let Some(packet) = packet else {
                        return Ok(());
                    };
                    let bytes = outbound
                        .encode(&packet)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                    if bytes.len() > max_frame_len {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "retinue {:?} packet (context {}) is {} bytes, radio frame limit is {max_frame_len}",
                                packet.packet_type,
                                packet.context,
                                bytes.len(),
                            ),
                        ));
                    }
                    if let Some(beacon) = &mut beacon {
                        beacon.on_tx(&bytes, now_ms());
                    }
                    let announce = packet.packet_type == PacketType::Announce;
                    transmit(&radio, bytes, announce).await?;
                }
                received = radio.recv_frame() => {
                    let Some(received) = received else {
                        return Ok(());
                    };
                    match sink.deliver_frame(&received.frame) {
                        // `false` now means the endpoint is gone, which is the only
                        // condition that should end a carrier. A router too busy to take
                        // this packet drops it and says so through `sink.dropped()`; the
                        // radio keeps listening, because the next packet is usually fine.
                        Ok(true) => {}
                        Ok(false) => return Ok(()),
                        Err(_) => continue,
                    }
                }
                _ = sleep_until(wake) => {
                    if let Some(beacon) = &mut beacon
                        && let Some(id) = beacon.take_due(now_ms())
                    {
                        beacon.on_tx(&id, now_ms());
                        transmit(&radio, id, false).await?;
                    }
                }
            }
        }
    }
}

async fn transmit<R: PacketRadio>(radio: &R, frame: Vec<u8>, announce: bool) -> io::Result<()> {
    let sent = if announce {
        radio.send_announcement(frame).await
    } else {
        radio.send_frame(frame).await
    };
    match sent {
        // A disabled announce policy is deliberate carrier policy, not a radio fault. Keep
        // the interface alive for the other packet classes it still carries.
        Err(TransmitError::AnnouncementDisabled) if announce => Ok(()),
        Ok(_) | Err(TransmitError::Offline) => Ok(()),
        Err(error) => Err(io::Error::other(error.to_string())),
    }
}

/// The on-air bitrate RNS derives from a LoRa configuration, in bits per second
/// (`RNodeInterface.py` 698-704).
pub fn lora_bitrate(params: &LoRaParams) -> u64 {
    use tulle::lora::CodingRate::*;
    let cr = match params.coding_rate {
        Cr45 => 5,
        Cr46 => 6,
        Cr47 => 7,
        Cr48 => 8,
    };
    let sf = u64::from(params.spreading_factor);
    sf * 4 * u64::from(params.bandwidth_hz) / (cr << sf)
}

/// Apply RNS's RNode interface policy to `id`: the bitrate from `params`, and no announce
/// ingress control (`RNodeInterface.py` 1211-1212). False if `id` is not attached.
pub fn apply_rnode_policy(endpoint: &Endpoint, id: InterfaceId, params: &LoRaParams) -> bool {
    let exempt = AnnounceIngressPolicy {
        enabled: false,
        ..AnnounceIngressPolicy::default()
    };
    endpoint.set_interface_bitrate(id, Some(lora_bitrate(params)))
        && endpoint.set_interface_ingress_policy(id, Some(exempt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destination::DestinationName;
    use crate::identity::PrivateIdentity;
    use tokio::sync::mpsc;
    use tulle::link::Received;
    use tulle::lora::CodingRate;

    struct Recorder {
        sent: mpsc::UnboundedSender<Vec<u8>>,
        _peer: mpsc::UnboundedSender<Received>,
        inbound: mpsc::UnboundedReceiver<Received>,
    }

    #[allow(clippy::manual_async_fn)]
    impl PacketRadio for Recorder {
        fn max_frame_len(&self) -> usize {
            tulle::rnode::HW_MTU
        }

        fn send_frame(
            &self,
            frame: Vec<u8>,
        ) -> impl Future<Output = Result<Duration, TransmitError>> + Send {
            let sent = self.sent.clone();
            async move {
                sent.send(frame).map_err(|_| TransmitError::Stopped)?;
                Ok(Duration::ZERO)
            }
        }

        fn recv_frame(&mut self) -> impl Future<Output = Option<Received>> + Send {
            self.inbound.recv()
        }
    }

    #[tokio::test]
    async fn the_beacon_follows_traffic_once() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x61; 64]));
        let (sent, mut frames) = mpsc::unbounded_channel();
        let (_peer, inbound) = mpsc::unbounded_channel();
        let radio = Recorder {
            sent,
            _peer,
            inbound,
        };
        let beacon = Beacon::rnode(b"N0CALL", Duration::from_millis(300));
        tokio::spawn(drive_with_beacon(ep.attach_interface(), radio, beacon));
        ep.announce(&DestinationName::new("retinue", ["beacon"]), b"");
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        let mut got = Vec::new();
        while let Ok(frame) = frames.try_recv() {
            got.push(frame);
        }
        assert_eq!(got.len(), 2, "one announce, one beacon");
        assert_eq!(got[1], b"N0CALL");
    }

    #[test]
    fn bitrate_follows_rns() {
        let mut params = LoRaParams {
            spreading_factor: 7,
            bandwidth_hz: 500_000,
            coding_rate: CodingRate::Cr45,
            frequency_hz: 867_200_000,
            tx_power_dbm: 14,
            preamble_syms: 8,
            explicit_header: true,
            crc: true,
        };
        assert_eq!(lora_bitrate(&params), 21_875);
        params.spreading_factor = 12;
        params.bandwidth_hz = 125_000;
        params.coding_rate = CodingRate::Cr48;
        assert_eq!(lora_bitrate(&params), 183);
    }
}
