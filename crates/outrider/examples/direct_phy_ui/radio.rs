//! The two direct-PHY radios, each driving its own Retinue endpoint.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use retinue::Ifac;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::iface::tulle::drive;
use tokio::task::JoinHandle;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink, DirectPhyUiControl};
use tulle::radio_io::PacketRadio;

pub(super) struct RadioPair {
    pub(super) left: Arc<Endpoint>,
    pub(super) right: Arc<Endpoint>,
    pub(super) left_ui: DirectPhyUiControl,
    pub(super) right_ui: DirectPhyUiControl,
    left_driver: JoinHandle<io::Result<()>>,
    right_driver: JoinHandle<io::Result<()>>,
}

impl RadioPair {
    pub(super) async fn open(
        left_port: &str,
        right_port: &str,
        bandwidth_hz: u32,
        left_identity: &PrivateIdentity,
        right_identity: &PrivateIdentity,
        ifac: Option<Ifac>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let radio_config = DirectPhySerialConfig {
            online_timeout: Duration::from_secs(10),
            transmit_timeout: Duration::from_secs(10),
            ..DirectPhySerialConfig::default()
        };
        let mut profile = postilion::profile(bandwidth_hz);
        if let Some(power) = std::env::var_os("SIGNALMAN_TX_POWER_DBM") {
            profile.tx_power_dbm = power.to_str().ok_or("invalid transmit power")?.parse()?;
        }
        let mut left_radio = DirectPhySerialLink::open(
            left_port,
            profile,
            AirtimeBudget::new(60_000, 60_000),
            radio_config.clone(),
        )?;
        let mut right_radio = DirectPhySerialLink::open(
            right_port,
            profile,
            AirtimeBudget::new(60_000, 60_000),
            radio_config,
        )?;
        let left_ui = left_radio.ui_control();
        let right_ui = right_radio.ui_control();
        tokio::time::timeout(Duration::from_secs(15), left_radio.wait_online()).await??;
        tokio::time::timeout(Duration::from_secs(15), right_radio.wait_online()).await??;

        let left = Arc::new(Endpoint::with_network_stamp_policy(
            left_identity.clone(),
            retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        ));
        let right = Arc::new(Endpoint::with_network_stamp_policy(
            right_identity.clone(),
            retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        ));
        let frame_limit = left_radio.max_frame_len().min(right_radio.max_frame_len());
        let logical_mtu = frame_limit - ifac.as_ref().map_or(0, Ifac::size);
        println!("frame limit={frame_limit} logical link MTU={logical_mtu}");
        left.set_link_mtu(logical_mtu as u32);
        right.set_link_mtu(logical_mtu as u32);
        let left_interface = match &ifac {
            Some(ifac) => left.attach_interface_with_ifac(frame_limit, ifac.clone())?,
            None => left.attach_interface(),
        };
        let right_interface = match ifac {
            Some(ifac) => right.attach_interface_with_ifac(frame_limit, ifac)?,
            None => right.attach_interface(),
        };
        let left_driver = tokio::spawn(drive(left_interface, left_radio));
        let right_driver = tokio::spawn(drive(right_interface, right_radio));
        Ok(Self {
            left,
            right,
            left_ui,
            right_ui,
            left_driver,
            right_driver,
        })
    }

    pub(super) async fn shutdown(self) -> Result<(), Box<dyn std::error::Error>> {
        let Self {
            left,
            right,
            left_driver,
            right_driver,
            ..
        } = self;
        tokio::join!(
            left.shutdown(Duration::from_secs(3)),
            right.shutdown(Duration::from_secs(3))
        );
        tokio::time::timeout(Duration::from_secs(10), left_driver).await???;
        tokio::time::timeout(Duration::from_secs(10), right_driver).await???;
        Ok(())
    }
}
