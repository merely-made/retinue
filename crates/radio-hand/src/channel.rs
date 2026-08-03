//! Channels: one board personality at a time, behind one trait.
//!
//! Structural decision 4 rules that the shipped image carries its personalities as
//! runtime-selectable channels rather than as separate images swapped over DFU, because the
//! hardware already enforces the constraint — one SX1262, one PHY configuration, one sync
//! word, so the board is physically a citizen of exactly one mesh at a time.
//!
//! # Why `serve` takes an event rather than owning a loop
//!
//! The ruling names three methods, start, serve, stop. The obvious reading gives `serve` the
//! whole loop, and it is the wrong one here: the same decision keeps "the executor, the
//! select between host and radio, and the text probes" in the firmware binary, where the
//! board's own concerns live (the T114's bootloader entry through `GPREGRET`, its display
//! diagnostic). A channel that owned the loop would have to own those too, or they would
//! have to move into every channel that ever ships.
//!
//! So the firmware keeps the select and hands over what it did not claim. `serve` handles
//! one event and answers the only question the loop needs: does this host session survive.
//!
//! # Switching
//!
//! By reboot, per the ruling. A channel is chosen from the persisted settings at boot and
//! never changes while running, which is why [`Channel::stop`] is about releasing a host
//! session rather than about handing the radio to a successor.

use embassy_time::Duration;
use lora_phy::DelayNs;
use lora_phy::mod_traits::RadioKind;

use crate::executive::Executive;
use crate::link::{Flow, HostLink};

pub mod modem;

/// Something a channel is asked to handle.
///
/// The three sources a board has: the host, the air, and time.
pub enum Event<'a> {
    /// Bytes arrived from the host. Arbitrarily fragmented; a channel that parses commands
    /// must keep its parser across calls.
    HostBytes(&'a [u8]),
    /// A frame arrived over the air, already read out of the radio.
    RadioFrame {
        frame: &'a [u8],
        rssi: i16,
        snr: i16,
    },
    /// The channel's own heartbeat came due. Only delivered to channels that asked for one.
    Beat,
}

/// What a channel reports about itself.
///
/// Separate from [`Channel`] because none of it depends on the transport or the radio, and a
/// firmware asking `channel.heartbeat()` should not have to name three generic parameters to
/// say which implementation it means.
pub trait ChannelInfo {
    /// Whether a text probe from the firmware would be unambiguous right now.
    ///
    /// The board's own probes (`status`, `sync`, `ui`, `bootloader`) share one byte stream
    /// with whatever the channel parses, so they may only be recognised where no framed
    /// command is half-read. A channel that parses nothing framed is always at a boundary,
    /// which is why the default says so.
    fn at_boundary(&self) -> bool {
        true
    }

    /// How often this channel wants an [`Event::Beat`], if at all.
    ///
    /// `None` by default, and the default is not a formality: [`crate::executive::Heartbeat`]
    /// turns it into a future that never completes, so a channel with no timer costs no
    /// periodic wake at all.
    fn heartbeat(&self) -> Option<Duration> {
        None
    }
}

/// One board personality.
///
/// Generic over the host transport so a channel works over USB CDC, a bare UART, or BLE
/// without knowing which; generic over the radio kind for the same reason `service` is.
/// Auto traits are deliberately unbounded, matching [`HostLink`]: these futures are polled by
/// a single-threaded embassy executor and never cross a thread.
#[allow(async_fn_in_trait)]
pub trait Channel<L: HostLink, RK: RadioKind, DLY: DelayNs>: ChannelInfo {
    /// Prepare for a host session. Called once, after the host attaches, before any event.
    ///
    /// A channel may introduce itself over `link` here. The firmware writes its own preamble
    /// first, so anything written here follows it.
    async fn start(&mut self, exec: &mut Executive<'_, RK, DLY>, link: &mut L) -> Flow;

    /// Handle one event.
    async fn serve(
        &mut self,
        exec: &mut Executive<'_, RK, DLY>,
        link: &mut L,
        event: Event<'_>,
    ) -> Flow;

    /// The host session ended. Release anything held for it.
    ///
    /// Takes the link it is closing, for the same reason [`Channel::start`] takes the one it
    /// is opening: these two bracket a *session*, not the channel. Switching is by reboot, so
    /// a channel outlives every session it serves and is only ever destroyed by the reset.
    async fn stop(&mut self, exec: &mut Executive<'_, RK, DLY>, link: &mut L) {
        let _ = (exec, link);
    }
}
