//! Bridge between an endpoint raw interface and a Tulle packet radio.

use alloc::format;
use alloc::string::ToString;

use std::future::Future;
use std::io;

use tulle::radio_io::PacketRadio;

use crate::endpoint::Interface;

/// Drive one endpoint interface over a running Tulle radio until either side
/// closes or an outbound packet cannot be transmitted.
///
/// The radio's complete-frame limit is installed synchronously when this
/// function is called, before the returned future is polled. Endpoint queues
/// therefore refuse oversized packets before issuing a receipt. The transmit
/// check remains as a backstop for traffic queued before a driver was
/// constructed. Malformed RF packets are dropped at the boundary.
pub fn drive<R>(interface: Interface, mut radio: R) -> impl Future<Output = io::Result<()>>
where
    R: PacketRadio,
{
    let max_frame_len = radio.max_frame_len();
    interface.constrain_frame_limit(max_frame_len);

    async move {
        let (mut outbound, sink) = interface.split();
        loop {
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
                    radio
                        .send_frame(bytes)
                        .await
                        .map_err(|error| io::Error::other(error.to_string()))?;
                }
                received = radio.recv_frame() => {
                    let Some(received) = received else {
                        return Ok(());
                    };
                    match sink.deliver_frame(&received.frame) {
                        Ok(true) => {}
                        Ok(false) => return Ok(()),
                        Err(_) => continue,
                    }
                }
            }
        }
    }
}
