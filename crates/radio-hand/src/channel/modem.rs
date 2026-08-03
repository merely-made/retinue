//! The modem channel: the board as a radio the host drives.
//!
//! The personality that already existed, now a channel beside the others. It holds no
//! protocol state of its own — every decision is the host's, and the board's job is to put
//! bytes on the air and report what comes back.
//!
//! It is also the **recovery personality**. Structural decision 4 makes it the fallback a
//! crash-loop policy falls back *to*, which is only credible because it needs nothing beyond
//! a working radio and a host: no identity, no links, no tables. A board whose node channel
//! cannot start is still a useful radio here.

use selvage::{CommandStream, MAX_COMMAND_LEN};

use crate::channel::{Channel, ChannelInfo, Event};
use crate::dispatch;
use crate::executive::{ChipDiagnostics, Executive};
use crate::link::{Flow, HostLink};

/// The host-driven direct-PHY modem.
///
/// Generic over the board's chip diagnostics, which is the one thing this cannot do
/// generically: `sx126x_diagnostics` lives on the chip kind rather than on `LoRa`. The type
/// is a zero-sized marker on both boards, so carrying it costs nothing.
pub struct ModemChannel<D> {
    diagnostics: D,
    stream: CommandStream,
    command: [u8; MAX_COMMAND_LEN],
}

impl<D> ModemChannel<D> {
    pub fn new(diagnostics: D) -> Self {
        Self {
            diagnostics,
            stream: CommandStream::new(),
            command: [0_u8; MAX_COMMAND_LEN],
        }
    }
}

impl<D> ChannelInfo for ModemChannel<D> {
    /// Only where no framed command is half-read, so a `status` line inside a transmit
    /// payload is carried rather than obeyed.
    fn at_boundary(&self) -> bool {
        self.stream.is_boundary()
    }
}

impl<L, RK, DLY, D> Channel<L, RK, DLY> for ModemChannel<D>
where
    L: HostLink,
    RK: lora_phy::mod_traits::RadioKind,
    DLY: lora_phy::DelayNs,
    D: ChipDiagnostics<RK, DLY>,
{
    /// Nothing is written here: the firmware's own online line already names this
    /// personality, and adding a second introduction would change bytes the direct-PHY
    /// receipt compares.
    async fn start(&mut self, exec: &mut Executive<'_, RK, DLY>, link: &mut L) -> Flow {
        let _ = link;
        // A new session starts at a frame boundary whatever the last one left behind.
        self.stream = CommandStream::new();
        exec.request_rx();
        Flow::Continue
    }

    async fn serve(
        &mut self,
        exec: &mut Executive<'_, RK, DLY>,
        link: &mut L,
        event: Event<'_>,
    ) -> Flow {
        match event {
            Event::HostBytes(bytes) => {
                dispatch::on_host_bytes(
                    link,
                    exec,
                    &mut self.stream,
                    &mut self.command,
                    &self.diagnostics,
                    bytes,
                )
                .await
                .flow
            }
            Event::RadioFrame { frame, rssi, snr } => {
                dispatch::on_radio_frame(link, exec, frame, rssi, snr).await
            }
            // Never delivered: this channel asks for no heartbeat.
            Event::Beat => Flow::Continue,
        }
    }
}
