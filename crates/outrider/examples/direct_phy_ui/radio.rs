//! The two direct-PHY radios, each driving its own Retinue endpoint.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use retinue::Ifac;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::iface::tulle::drive;
use tokio::task::JoinHandle;
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink, DirectPhyUiControl};

fn profile(bandwidth_hz: u32) -> PhyProfile {
    PhyProfile {
        frequency_hz: 906_875_000,
        bandwidth_hz,
        spreading_factor: 8,
        coding_rate_denominator: 5,
        preamble_symbols: 16,
        sync_word: 0x12,
        explicit_header: true,
        crc: true,
        invert_iq: false,
        tx_power_dbm: 17,
    }
}

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
        let mut left_radio = DirectPhySerialLink::open(
            left_port,
            profile(bandwidth_hz),
            AirtimeBudget::new(60_000, 60_000),
            radio_config.clone(),
        )?;
        let mut right_radio = DirectPhySerialLink::open(
            right_port,
            profile(bandwidth_hz),
            AirtimeBudget::new(60_000, 60_000),
            radio_config,
        )?;
        let left_ui = left_radio.ui_control();
        let right_ui = right_radio.ui_control();
        tokio::time::timeout(Duration::from_secs(15), left_radio.wait_online()).await??;
        tokio::time::timeout(Duration::from_secs(15), right_radio.wait_online()).await??;

        let left = Arc::new(Endpoint::new(left_identity.clone()));
        let right = Arc::new(Endpoint::new(right_identity.clone()));
        let logical_mtu = 255 - ifac.as_ref().map_or(0, Ifac::size);
        left.set_link_mtu(logical_mtu as u32);
        right.set_link_mtu(logical_mtu as u32);
        let left_interface = match &ifac {
            Some(ifac) => left.attach_interface_with_ifac(255, ifac.clone())?,
            None => left.attach_interface(),
        };
        let right_interface = match ifac {
            Some(ifac) => right.attach_interface_with_ifac(255, ifac)?,
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
