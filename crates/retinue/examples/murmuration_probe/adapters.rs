//! Caller-owned Retinue and Sennet frame sources.

use retinue::hash::AddressHash;
use retinue::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};
use sennet::node::Channel;
use sennet::packet_id::PacketIdState;
use sennet::transport::{BROADCAST_DESTINATION, Header};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use crate::Result;

pub(super) struct RetinueAdapter {
    pub(super) prefix: String,
    pub(super) sequence: u32,
}
impl RetinueAdapter {
    pub(super) fn frame_with_length(&mut self, length: usize) -> Result<Vec<u8>> {
        let frame = self.frame("usb-boundary");
        if frame.len() > length {
            return Err("boundary target shorter than packet".into());
        }
        let mut packet = Packet::decode(&frame)?;
        packet
            .payload
            .resize(packet.payload.len() + length - frame.len(), b'x');
        let encoded = packet.encode();
        if encoded.len() != length {
            return Err("boundary packet encoding changed length".into());
        }
        Ok(encoded)
    }
    pub(super) fn frame(&mut self, text: &str) -> Vec<u8> {
        self.sequence += 1;
        Packet {
            ifac: false,
            header_type: HeaderType::Type1,
            context_flag: false,
            propagation: Propagation::Broadcast,
            destination_type: DestinationType::Plain,
            packet_type: PacketType::Data,
            hops: 0,
            transport: None,
            destination: AddressHash::from_bytes([0x4d; 16]),
            context: 0,
            payload: format!("{}:{}:{text}", self.prefix, self.sequence).into_bytes(),
        }
        .encode()
    }
}
pub(super) struct SennetAdapter {
    channel: Channel,
    ids: PacketIdState,
    file: File,
    pub(super) sent: u32,
}
impl SennetAdapter {
    pub(super) fn new(channel: Channel, source: u32, path: &Path) -> Result<Self> {
        Ok(Self {
            channel,
            ids: PacketIdState::new(source, 1),
            file: OpenOptions::new().write(true).create_new(true).open(path)?,
            sent: 0,
        })
    }
    pub(super) fn frame(&mut self, text: &str) -> Result<Vec<u8>> {
        let id = self
            .ids
            .reserve()
            .map_err(|e| format!("packet reservation: {e:?}"))?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&self.ids.encode())?;
        self.file.sync_all()?;
        self.sent += 1;
        self.channel
            .seal_text(
                Header {
                    destination: BROADCAST_DESTINATION,
                    source: id.source,
                    packet_id: id.packet_id,
                    hop_limit: 0,
                    want_ack: false,
                    via_mqtt: false,
                    hop_start: 0,
                    channel_hash: self.channel.hash,
                    next_hop: 0,
                    relay_node: id.source as u8,
                },
                text,
            )
            .map_err(|e| format!("Sennet seal: {e:?}").into())
    }
}
