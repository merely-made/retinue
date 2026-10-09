//! The MeshCore companion USB protocol, used only for node management.

use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::time::Duration;

use serial2_tokio::SerialPort;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{Instant, timeout, timeout_at};

pub(super) const CMD_APP_START: u8 = 1;
const CMD_SEND_TXT_MSG: u8 = 2;
pub(super) const CMD_SET_DEVICE_TIME: u8 = 6;
pub(super) const CMD_SEND_SELF_ADVERT: u8 = 7;
pub(super) const CMD_SET_ADVERT_NAME: u8 = 8;
const CMD_ADD_UPDATE_CONTACT: u8 = 9;
const CMD_IMPORT_CONTACT: u8 = 18;
const CMD_GET_CONTACT_BY_KEY: u8 = 30;
const CMD_SYNC_NEXT_MESSAGE: u8 = 10;
const CMD_SET_RADIO_PARAMS: u8 = 11;
pub(super) const CMD_RESET_PATH: u8 = 13;
pub(super) const CMD_DEVICE_QUERY: u8 = 22;

pub(super) const RESP_OK: u8 = 0;
const RESP_ERR: u8 = 1;
pub(super) const RESP_SELF_INFO: u8 = 5;
pub(super) const RESP_SENT: u8 = 6;
const RESP_CONTACT_MESSAGE_V3: u8 = 16;
pub(super) const RESP_DEVICE_INFO: u8 = 13;
const RESP_CONTACT: u8 = 3;
pub(super) const PUSH_SEND_CONFIRMED: u8 = 0x82;

pub(super) struct Companion {
    port: SerialPort,
    pushes: VecDeque<Vec<u8>>,
}

impl Companion {
    pub(super) fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let port = SerialPort::open(path, 115_200)?;
        // The companion's native USB CDC endpoint begins delivering frames
        // after the host asserts DTR. This does not enter the ESP32 flasher;
        // that is the separate PID 0x1001 endpoint.
        port.set_dtr(true)?;
        port.set_rts(false)?;
        Ok(Self {
            port,
            pushes: VecDeque::new(),
        })
    }

    async fn send_frame(&mut self, payload: &[u8]) -> io::Result<()> {
        let len = u16::try_from(payload.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "companion frame too long"))?;
        self.port.write_all(b"<").await?;
        self.port.write_all(&len.to_le_bytes()).await?;
        self.port.write_all(payload).await?;
        self.port.flush().await
    }

    async fn read_frame(&mut self, wait: Duration) -> io::Result<Vec<u8>> {
        timeout(wait, async {
            loop {
                if self.port.read_u8().await? == b'>' {
                    break;
                }
            }
            let len = self.port.read_u16_le().await? as usize;
            let mut frame = vec![0; len];
            self.port.read_exact(&mut frame).await?;
            Ok(frame)
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "MeshCore response timed out"))?
    }

    async fn command(&mut self, payload: &[u8]) -> io::Result<Vec<u8>> {
        self.send_frame(payload).await?;
        loop {
            let frame = self.read_frame(Duration::from_secs(15)).await?;
            let Some(&code) = frame.first() else {
                continue;
            };
            if code >= 0x80 {
                self.pushes.push_back(frame);
                continue;
            }
            if code == RESP_ERR {
                return Err(io::Error::other(format!(
                    "MeshCore command {} failed with error {}",
                    payload[0],
                    frame.get(1).copied().unwrap_or(0)
                )));
            }
            return Ok(frame);
        }
    }

    pub(super) async fn expect(&mut self, payload: &[u8], expected: u8) -> io::Result<Vec<u8>> {
        let frame = self.command(payload).await?;
        if frame.first() != Some(&expected) {
            return Err(io::Error::other(format!(
                "MeshCore command {} returned {:?}, expected {expected}",
                payload[0],
                frame.first()
            )));
        }
        Ok(frame)
    }

    pub(super) async fn wait_push(&mut self, expected: u8, wait: Duration) -> io::Result<Vec<u8>> {
        if let Some(index) = self
            .pushes
            .iter()
            .position(|frame| frame.first() == Some(&expected))
        {
            return Ok(self.pushes.remove(index).expect("index just found"));
        }
        let deadline = Instant::now() + wait;
        loop {
            let frame = timeout_at(deadline, self.read_frame(wait))
                .await
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::TimedOut, "MeshCore push timed out")
                })??;
            if frame.first() == Some(&expected) {
                return Ok(frame);
            }
            if frame.first().is_some_and(|code| *code >= 0x80) {
                self.pushes.push_back(frame);
            }
        }
    }

    pub(super) async fn set_contact_route(
        &mut self,
        public_key: &[u8; 32],
        path: &[u8],
        hash_size: u8,
    ) -> io::Result<()> {
        if !(1..=3).contains(&hash_size)
            || path.len() > 64
            || !path.len().is_multiple_of(hash_size as usize)
            || path.len() / hash_size as usize > 63
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "MeshCore V1 route exceeds 63 hops",
            ));
        }
        let mut get = vec![CMD_GET_CONTACT_BY_KEY];
        get.extend_from_slice(public_key);
        let mut contact = self.expect(&get, RESP_CONTACT).await?;
        const PATH_LEN_AT: usize = 35;
        const PATH_AT: usize = 36;
        const PATH_CAPACITY: usize = 64;
        if contact.len() < PATH_AT + PATH_CAPACITY {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated MeshCore contact",
            ));
        }
        contact[0] = CMD_ADD_UPDATE_CONTACT;
        contact[PATH_LEN_AT] = ((hash_size - 1) << 6) | (path.len() / hash_size as usize) as u8;
        contact[PATH_AT..PATH_AT + PATH_CAPACITY].fill(0);
        contact[PATH_AT..PATH_AT + path.len()].copy_from_slice(path);
        self.expect(&contact, RESP_OK).await?;
        Ok(())
    }
}

pub(super) fn radio_command(frequency_hz: u32) -> Vec<u8> {
    let mut command = vec![CMD_SET_RADIO_PARAMS];
    command.extend_from_slice(&(frequency_hz / 1_000).to_le_bytes());
    command.extend_from_slice(&250_000_u32.to_le_bytes());
    command.extend_from_slice(&[10, 5, 0]);
    command
}

pub(super) fn import_command(advert: &[u8]) -> Vec<u8> {
    let mut command = Vec::with_capacity(1 + advert.len());
    command.push(CMD_IMPORT_CONTACT);
    command.extend_from_slice(advert);
    command
}

pub(super) fn text_command(peer_prefix: &[u8; 6], timestamp: u32, text: &str) -> Vec<u8> {
    let mut command = vec![CMD_SEND_TXT_MSG, 0, 0];
    command.extend_from_slice(&timestamp.to_le_bytes());
    command.extend_from_slice(peer_prefix);
    command.extend_from_slice(text.as_bytes());
    command
}

pub(super) fn contact_text(frame: &[u8]) -> Option<&str> {
    if frame.first() != Some(&RESP_CONTACT_MESSAGE_V3) || frame.len() < 16 {
        return None;
    }
    std::str::from_utf8(&frame[16..]).ok()
}

pub(super) async fn sync_text(companion: &mut Companion, wait: Duration) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + wait;
    loop {
        let frame = companion.command(&[CMD_SYNC_NEXT_MESSAGE]).await?;
        if frame.first() == Some(&RESP_CONTACT_MESSAGE_V3) {
            return Ok(frame);
        }
        if frame.first() != Some(&10) {
            return Err(io::Error::other(format!(
                "unexpected MeshCore message-sync response {:?}",
                frame.first()
            )));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "MeshCore text did not reach its offline queue",
            ));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
