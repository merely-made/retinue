//! Startup-owned Retinue envelope at the physical radio boundary.
extern crate alloc;

use alloc::vec::Vec;
use retinue::{Packet, ifac::Ifac};

/// Credentials belong to the caller. There is deliberately no live setter.
#[derive(Clone, Debug, Default)]
pub struct RetinueCarrier {
    ifac: Option<Ifac>,
}

impl RetinueCarrier {
    pub fn protected(ifac: Ifac) -> Self {
        Self { ifac: Some(ifac) }
    }

    pub fn is_protected(&self) -> bool {
        self.ifac.is_some()
    }

    /// Install only before an authenticated carrier can inherit any live session.
    pub fn configure_node<const P: usize, const A: usize, const L: usize, const R: usize>(
        &self,
        node: &mut retinue::node::Node<P, A, L, R>,
    ) -> Result<(), retinue::node::LogicalMtuError> {
        if self.is_protected() && node.has_active_sessions() {
            return Err(retinue::node::LogicalMtuError::SessionsActive);
        }
        node.set_logical_mtu(node.logical_mtu().min(self.logical_mtu()))
    }

    pub fn logical_mtu(&self) -> u32 {
        (selvage::MAX_RADIO_FRAME_LEN - self.ifac.as_ref().map_or(0, Ifac::size)) as u32
    }

    /// Authentication completes before any logical packet is decoded.
    pub fn decode(&self, frame: &[u8]) -> retinue::Result<Packet> {
        if frame.len() > selvage::MAX_RADIO_FRAME_LEN {
            return Err(retinue::Error::Oversize);
        }
        match &self.ifac {
            Some(ifac) => Packet::decode(&ifac.open(frame)?),
            None if frame.first().is_some_and(|flags| flags & 0x80 != 0) => {
                Err(retinue::Error::BadIfac)
            }
            None => Packet::decode(frame),
        }
    }

    /// Check the final packet, including any relay header and access code.
    pub fn encode(&self, packet: &Packet) -> retinue::Result<Vec<u8>> {
        if packet.ifac {
            return Err(retinue::Error::BadIfac);
        }
        if packet.encoded_len() > self.logical_mtu() as usize {
            return Err(retinue::Error::Oversize);
        }
        let logical = packet.encode();
        let frame = match &self.ifac {
            Some(ifac) => ifac.seal(&logical)?,
            None => logical,
        };
        if frame.len() > selvage::MAX_RADIO_FRAME_LEN {
            return Err(retinue::Error::Oversize);
        }
        Ok(frame)
    }
}
