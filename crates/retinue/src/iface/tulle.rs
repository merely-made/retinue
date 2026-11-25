//! Bridge between an endpoint raw interface and a Tulle packet radio, and RNS's
//! RNodeInterface on top of it ([`attach_rnode`]).

use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;

use std::future::Future;
use std::io;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::{Instant, sleep_until};
use tulle::airtime::AirtimeBudget;
use tulle::lora::LoRaParams;
use tulle::radio_io::PacketRadio;
use tulle::rnode::{HW_MTU, RNodeConfig, Telemetry};
use tulle::serial::TransmitError;
use tulle::serial::{PumpError, PumpStatus, RNodePort, RNodeSerialLink, SerialPumpConfig};

use super::beacon::Beacon;
use crate::announce_admission::AnnounceIngressPolicy;
use crate::endpoint::{Endpoint, Interface, InterfaceId};
use crate::ifac::Ifac;
use crate::packet::PacketType;

/// How often a station ID that a reopening radio dropped is offered again.
const BEACON_RETRY_MS: u64 = 1_000;

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
/// while offline; they do not end the driver. The station ID is not lost: it stays due and
/// goes out once the radio is back.
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
                    // The ID timer runs from traffic that reached the radio, as RNS arms it
                    // on write (`RNodeInterface.py` 711-722), not from frames queued
                    // while the radio was still coming up.
                    let id_check = beacon.is_some().then(|| bytes.clone());
                    let announce = packet.packet_type == PacketType::Announce;
                    if transmit(&radio, bytes, announce).await?
                        && let (Some(beacon), Some(frame)) = (&mut beacon, id_check)
                    {
                        beacon.on_tx(&frame, now_ms());
                    }
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
                        && !transmit(&radio, id, false).await?
                    {
                        beacon.retry_at(now_ms() + BEACON_RETRY_MS);
                    }
                }
            }
        }
    }
}

/// Send one frame. `Ok(false)` if a reopening radio dropped it.
async fn transmit<R: PacketRadio>(radio: &R, frame: Vec<u8>, announce: bool) -> io::Result<bool> {
    let sent = if announce {
        radio.send_announcement(frame).await
    } else {
        radio.send_frame(frame).await
    };
    match sent {
        // A disabled announce policy is deliberate carrier policy, not a radio fault. Keep
        // the interface alive for the other packet classes it still carries.
        Err(TransmitError::AnnouncementDisabled) if announce => Ok(true),
        Ok(_) => Ok(true),
        Err(TransmitError::Offline) => Ok(false),
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
    // Floored, as retinue has always set it; RNS keeps the float.
    tulle::rnode::bitrate(params.spreading_factor, cr, params.bandwidth_hz) as u64
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

/// An RNode attached to an endpoint. Spawn `run`; it ends when the endpoint closes or a
/// frame cannot be carried.
pub struct RNodeCarrier<F> {
    pub id: InterfaceId,
    pub status: watch::Receiver<PumpStatus>,
    pub telemetry: watch::Receiver<Telemetry>,
    pub run: F,
}

/// Attach the RNode at `port`, RNS's `port` value (a device path or `tcp://host`), as one
/// supervised interface that reopens after any failure (`RNodeInterface.py` 170-193,
/// 1175-1187). It takes RNS's RNode policy ([`apply_rnode_policy`]); each time the radio
/// comes online the bitrate follows the device's echoed SF, CR and bandwidth (698-704).
///
/// # Panics
///
/// Outside a Tokio runtime: the port's supervisor is spawned at once.
pub fn attach_rnode(
    endpoint: &Arc<Endpoint>,
    port: &RNodePort,
    rnode: RNodeConfig,
    budget: AirtimeBudget,
    pump: SerialPumpConfig,
    ifac: Option<Ifac>,
    beacon: Option<Beacon>,
) -> io::Result<RNodeCarrier<impl Future<Output = io::Result<()>> + Send + 'static>> {
    let radio =
        RNodeSerialLink::supervise_port(port, rnode, budget, pump).map_err(|e| match e {
            PumpError::Io(error) => error,
            other => io::Error::other(other.to_string()),
        })?;
    let interface = match ifac {
        Some(ifac) => endpoint.attach_interface_with_ifac(HW_MTU, ifac)?,
        None => endpoint.attach_interface_with_frame_limit(HW_MTU)?,
    };
    let id = interface.id();
    apply_rnode_policy(endpoint, id, &rnode.params);
    let (status, telemetry) = (radio.watch_status(), radio.watch_telemetry());
    let weak = Arc::downgrade(endpoint);
    let set = move |bps| {
        weak.upgrade()
            .is_some_and(|ep| ep.set_interface_bitrate(id, Some(bps)))
    };
    let echoed = follow_echoed_bitrate(status.clone(), telemetry.clone(), set);
    let drive = drive_with_beacon(interface, radio, beacon);
    let run = async move {
        tokio::pin!(drive);
        tokio::select! {
            result = &mut drive => result,
            () = echoed => drive.await,
        }
    };
    Ok(RNodeCarrier {
        id,
        status,
        telemetry,
        run,
    })
}

/// `set` the echoed bitrate whenever the radio comes online, until `set` fails or the radio
/// stops.
async fn follow_echoed_bitrate(
    mut status: watch::Receiver<PumpStatus>,
    telemetry: watch::Receiver<Telemetry>,
    mut set: impl FnMut(u64) -> bool,
) {
    loop {
        let online = matches!(*status.borrow_and_update(), PumpStatus::Online { .. });
        let bitrate = telemetry.borrow().bitrate;
        if let (true, Some(bps)) = (online, bitrate)
            && !set(bps as u64)
        {
            return;
        }
        if status.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destination::DestinationName;
    use crate::identity::PrivateIdentity;
    use std::sync::Mutex;
    use tokio::sync::mpsc;
    use tulle::link::Received;
    use tulle::lora::CodingRate;

    struct Recorder {
        sent: mpsc::UnboundedSender<Vec<u8>>,
        _peer: mpsc::UnboundedSender<Received>,
        inbound: mpsc::UnboundedReceiver<Received>,
        /// Station IDs to refuse as a reopening radio would.
        offline_ids: Mutex<usize>,
    }

    fn recorder(offline_ids: usize) -> (Recorder, mpsc::UnboundedReceiver<Vec<u8>>) {
        let (sent, frames) = mpsc::unbounded_channel();
        let (_peer, inbound) = mpsc::unbounded_channel();
        let offline_ids = Mutex::new(offline_ids);
        let radio = Recorder {
            sent,
            _peer,
            inbound,
            offline_ids,
        };
        (radio, frames)
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
            let offline = frame == b"N0CALL" && {
                let mut left = self.offline_ids.lock().unwrap();
                let drop = *left > 0;
                *left = left.saturating_sub(1);
                drop
            };
            async move {
                if offline {
                    return Err(TransmitError::Offline);
                }
                sent.send(frame).map_err(|_| TransmitError::Stopped)?;
                Ok(Duration::ZERO)
            }
        }

        fn recv_frame(&mut self) -> impl Future<Output = Option<Received>> + Send {
            self.inbound.recv()
        }
    }

    /// Frames sent within `wait` of one announce, with the radio dropping `offline_ids` IDs.
    async fn beacon_run(offline_ids: usize, wait: Duration) -> Vec<Vec<u8>> {
        let ep = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[0x61; 64]),
            crate::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let (radio, mut frames) = recorder(offline_ids);
        let beacon = Beacon::rnode(b"N0CALL", Duration::from_millis(300));
        tokio::spawn(drive_with_beacon(ep.attach_interface(), radio, beacon));
        ep.announce(&DestinationName::new("retinue", ["beacon"]), b"");
        tokio::time::sleep(wait).await;
        let mut got = Vec::new();
        while let Ok(frame) = frames.try_recv() {
            got.push(frame);
        }
        got
    }

    #[tokio::test]
    async fn the_beacon_follows_traffic_once() {
        let got = beacon_run(0, Duration::from_millis(1_200)).await;
        assert_eq!(got.len(), 2, "one announce, one beacon");
        assert_eq!(got[1], b"N0CALL");
    }

    #[tokio::test]
    async fn a_beacon_dropped_while_offline_goes_out_later() {
        let got = beacon_run(2, Duration::from_millis(3_000)).await;
        assert_eq!(
            got.len(),
            2,
            "one announce, then the beacon once back online"
        );
        assert_eq!(got[1], b"N0CALL");
    }

    #[tokio::test]
    async fn the_echoed_bitrate_is_set_each_time_the_radio_comes_online() {
        let (status_tx, status) = watch::channel(PumpStatus::Initializing);
        let (telemetry_tx, telemetry) = watch::channel(Telemetry::default());
        let set = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&set);
        let follow = tokio::spawn(follow_echoed_bitrate(status, telemetry, move |bps| {
            seen.lock().unwrap().push(bps);
            true
        }));
        let online = PumpStatus::Online { firmware: None };
        telemetry_tx.send_modify(|t| t.bitrate = Some(tulle::rnode::bitrate(12, 8, 125_000)));
        tokio::task::yield_now().await;
        status_tx.send(online.clone()).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        status_tx.send(PumpStatus::Fault("gone".into())).unwrap();
        telemetry_tx.send_modify(|t| t.bitrate = Some(21_875.0));
        status_tx.send(online).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(status_tx);
        follow.await.unwrap();
        assert_eq!(*set.lock().unwrap(), [183, 21_875]);
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
