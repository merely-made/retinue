//! Exclusive read-only diagnostic session. It sends observation requests only:
//! opening this collector never sends status, configure, TX or UI commands.
//! Cancellation/timeout retires the client; reconnect with a fresh decoder.
use crate::direct_phy::{Decoder, Event};
use selvage::observation::{self as wire, Reply, Request};
use std::{io, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub struct ObservationClient<T> {
    io: T,
    decoder: Decoder,
    timeout: Duration,
    next_id: Option<u32>,
    retired: bool,
    unrelated_events: u64,
}

impl<T: AsyncRead + AsyncWrite + Unpin> ObservationClient<T> {
    pub fn new(io: T, timeout: Duration) -> io::Result<Self> {
        if timeout.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "observation timeout must be nonzero",
            ));
        }
        Ok(Self {
            io,
            decoder: Decoder::new(),
            timeout,
            next_id: Some(1),
            retired: false,
            unrelated_events: 0,
        })
    }
    /// Ordinary direct-PHY events discarded by this metadata-only collector.
    pub fn unrelated_events(&self) -> u64 {
        self.unrelated_events
    }
    pub fn is_retired(&self) -> bool {
        self.retired
    }
    pub fn into_inner(self) -> T {
        self.io
    }

    fn id(&mut self) -> io::Result<u32> {
        if self.retired {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "observation session requires reconnect",
            ));
        }
        let id = self
            .next_id
            .ok_or_else(|| io::Error::other("observation request ids exhausted"))?;
        self.next_id = id.checked_add(1);
        Ok(id)
    }

    pub async fn cursor(
        &mut self,
        boot_id: u64,
        after_sequence: u64,
    ) -> io::Result<wire::CursorReply> {
        let request_id = self.id()?;
        match self
            .exchange(Request::Cursor {
                request_id,
                boot_id,
                after_sequence,
            })
            .await?
        {
            Reply::Cursor(reply) => Ok(reply),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "wrong observation reply kind",
            )),
        }
    }
    pub async fn profile(
        &mut self,
        boot_id: u64,
        profile_id: u8,
    ) -> io::Result<wire::ProfileReply> {
        let request_id = self.id()?;
        match self
            .exchange(Request::Profile {
                request_id,
                boot_id,
                profile_id,
            })
            .await?
        {
            Reply::Profile(reply) => Ok(reply),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "wrong observation reply kind",
            )),
        }
    }
    async fn exchange(&mut self, request: Request) -> io::Result<Reply> {
        let mut command = [0; wire::MAX_OBSERVATION_COMMAND_LEN];
        let len = wire::encode_request(request, &mut command);
        wire::decode_request(&command[..len - 1])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, format!("{error:?}")))?;
        // Set before the first await, so dropping this future cannot allow a new
        // command to run behind a partially written/read predecessor.
        self.retired = true;
        let result =
            tokio::time::timeout(self.timeout, self.exchange_inner(request, &command[..len])).await;
        match result {
            Ok(Ok(reply)) => {
                self.retired = false;
                Ok(reply)
            }
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "observation reply deadline exceeded",
            )),
        }
    }
    async fn exchange_inner(&mut self, request: Request, command: &[u8]) -> io::Result<Reply> {
        self.io.write_all(&[0]).await?;
        self.io.write_all(command).await?;
        self.io.flush().await?;
        let id = match request {
            Request::Cursor { request_id, .. } | Request::Profile { request_id, .. } => request_id,
        };
        let mut bytes = [0; 256];
        loop {
            let len = self.io.read(&mut bytes).await?;
            if len == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "observation carrier closed",
                ));
            }
            let mut events = Vec::new();
            self.decoder.push(&bytes[..len], &mut events);
            let mut found = None;
            for event in events {
                if let Event::Observation(reply) = event {
                    let reply_id = match reply {
                        Reply::Cursor(r) => r.request_id,
                        Reply::Profile(r) => r.request_id,
                    };
                    if reply_id != id {
                        continue;
                    }
                    let matches = matches!(
                        (request, reply),
                        (Request::Cursor { .. }, Reply::Cursor(_))
                            | (Request::Profile { .. }, Reply::Profile(_))
                    );
                    if !matches {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "observation reply kind mismatch",
                        ));
                    }
                    if found.is_some_and(|prior| prior != reply) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "conflicting observation replies",
                        ));
                    }
                    found = Some(reply);
                } else {
                    self.unrelated_events = self.unrelated_events.saturating_add(1);
                }
            }
            if let Some(reply) = found {
                return Ok(reply);
            }
        }
    }
}

impl ObservationClient<serial2_tokio::SerialPort> {
    /// Exclusive port access. The caller supplies connection policy; it must not
    /// open a second collector behind an existing direct-PHY pump.
    pub fn open(
        path: impl AsRef<std::path::Path>,
        baud: u32,
        dtr: bool,
        timeout: Duration,
    ) -> io::Result<Self> {
        let port = serial2_tokio::SerialPort::open(path, baud)?;
        port.set_rts(false)?;
        port.set_dtr(dtr)?;
        Self::new(port, timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response(id: u32) -> Reply {
        Reply::Cursor(wire::CursorReply {
            request_id: id,
            status: wire::Status::Ok,
            boot_id: 9,
            oldest: 0,
            newest: 0,
            next: 0,
            recorded: 0,
            overwritten: 0,
            encode_failed: 0,
            profile_count: 0,
            record_len: 0,
            record: [0; 64],
        })
    }
    #[tokio::test]
    async fn only_observation_commands_and_split_replies() {
        let (host, mut board) = tokio::io::duplex(256);
        let task = tokio::spawn(async move {
            let mut request = [0; wire::MAX_OBSERVATION_COMMAND_LEN + 1];
            board.read_exact(&mut request).await.unwrap();
            assert_eq!(request[0], 0);
            assert_eq!(
                wire::decode_request(&request[1..request.len() - 1]).unwrap(),
                Request::Cursor {
                    request_id: 1,
                    boot_id: 0,
                    after_sequence: 0
                }
            );
            board
                .write_all(&[selvage::EVENT_TX, 0, 1, 0])
                .await
                .unwrap();
            let mut bytes = [0; wire::MAX_OBSERVATION_REPLY_LEN];
            let len = wire::encode_reply(response(1), &mut bytes).unwrap();
            for byte in &bytes[..len] {
                board.write_all(&[*byte]).await.unwrap();
            }
        });
        let mut client = ObservationClient::new(host, Duration::from_secs(1)).unwrap();
        assert_eq!(client.cursor(0, 0).await.unwrap().boot_id, 9);
        assert_eq!(client.unrelated_events(), 1);
        assert!(!client.is_retired());
        task.await.unwrap();
    }
    #[tokio::test]
    async fn timeout_and_external_cancellation_retire_the_stream() {
        for cancel in [false, true] {
            let (host, _unread_board) = tokio::io::duplex(256);
            let deadline = if cancel {
                Duration::from_secs(3600)
            } else {
                Duration::from_millis(20)
            };
            let mut client = ObservationClient::new(host, deadline).unwrap();
            if cancel {
                // Cancel after the exchange actually reaches its pending read.
                // Competing millisecond deadlines race on a loaded host.
                let mut pending = Box::pin(client.cursor(0, 0));
                std::future::poll_fn(|cx| {
                    assert!(std::future::Future::poll(pending.as_mut(), cx).is_pending());
                    std::task::Poll::Ready(())
                })
                .await;
                drop(pending);
            } else {
                assert_eq!(
                    client.cursor(0, 0).await.unwrap_err().kind(),
                    io::ErrorKind::TimedOut
                );
            }
            assert!(client.is_retired());
            assert_eq!(
                client.cursor(0, 0).await.unwrap_err().kind(),
                io::ErrorKind::BrokenPipe
            );
        }
    }
}
