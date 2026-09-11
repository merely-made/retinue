#![forbid(unsafe_code)]

//! Tulle: the shared radio interface layer for LoRa mesh stacks.
//!
//! Tulle is the seam between protocol stacks and radio hardware: RNode serial
//! control, direct-PHY USB packets, and medium access shared by every protocol
//! on the same radio. It sits beneath
//! [retinue](https://github.com/merely-made/retinue) and its mesh interop siblings,
//! tucket and sennet.
//!
//! A tulle is a fine net fabric: the material every protocol is woven across.

pub mod airtime;
pub mod direct_phy;
#[cfg(feature = "serial-async")]
pub mod direct_phy_serial;
pub mod kiss;
pub mod link;
pub mod lora;
pub mod modem;
#[cfg(feature = "serial-async")]
pub mod observation_serial;
pub mod pacing;
pub mod personality;
#[cfg(feature = "serial-async")]
pub mod personality_serial;
#[cfg(feature = "serial-async")]
pub mod radio_io;
pub mod rnode;
#[cfg(feature = "serial-async")]
pub mod serial;

pub use selvage::observation as observation_wire;
pub use selvage::{PhyProfile, ProfileError, WAKE_BYTE, decode_config_command};
