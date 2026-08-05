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

use embassy_futures::select::{Either3, select3};
use embassy_time::Duration;
use lora_phy::DelayNs;
use lora_phy::mod_traits::RadioKind;

use crate::executive::{Executive, Heartbeat};
use crate::link::{Flow, HostLink};

#[cfg(feature = "node")]
pub mod face;
pub mod modem;
#[cfg(feature = "node")]
pub mod node;
pub mod rnode;

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

    /// Whether this channel keeps working with no host attached.
    ///
    /// The modem does not: every decision it makes is the host's, so with nobody attached
    /// there is nothing to decide and a receiving radio would only cost power. The node
    /// does — it does not stop being a node because nobody is watching, and its announces
    /// and timers have to keep running.
    ///
    /// The default is `false`, matching the personality that came first. A channel that
    /// holds protocol state almost certainly wants `true`.
    fn without_host(&self) -> bool {
        false
    }

    /// Whether the firmware's text banner may be written to this host.
    ///
    /// The board introduces itself in plain text on every attach — what it is, its region,
    /// its carrier, the reset reason — which is right for a person on a terminal and wrong
    /// for a host that opens with a binary frame and expects a binary frame back. A channel
    /// speaking somebody else's protocol from the first byte says so here.
    ///
    /// `true` by default: the banner predates channels, and a personality that wants it kept
    /// should not have to ask.
    fn banner(&self) -> bool {
        true
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

/// Every channel this image ships, as one type.
///
/// The selector structural decision 4 called for, and the reason it waited until a second
/// channel existed. An enum rather than a `dyn Channel` because the serve loop is then
/// compiled once for all personalities instead of once per personality, which is what keeps
/// "every shipped channel is flash-resident always" an affordable cost rather than a
/// multiplying one.
// Exactly one personality is constructed at boot and lives until the reset. There is no
// array of these and none is ever moved, so the larger variant's slack costs a few hundred
// bytes of static storage once — cheaper than the heap indirection boxing would add to every
// event dispatch, and simpler than making the selector allocate.
#[allow(clippy::large_enum_variant)]
#[cfg(feature = "node")]
pub enum Personality<D, const PEERS: usize = 32, const ACTIONS: usize = 8, const LINKS: usize = 4> {
    /// The host-driven modem, and the recovery personality a board falls back to.
    Modem(modem::ModemChannel<D>),
    /// The native Retinue node.
    Node(node::NodeChannel<PEERS, ACTIONS, LINKS>),
    /// The board as an RNode, which is what stock Reticulum software knows how to drive.
    Rnode(rnode::RNodeChannel),
}

#[cfg(feature = "node")]
impl<D, const PEERS: usize, const ACTIONS: usize, const LINKS: usize> ChannelInfo
    for Personality<D, PEERS, ACTIONS, LINKS>
{
    fn at_boundary(&self) -> bool {
        match self {
            Personality::Modem(c) => c.at_boundary(),
            Personality::Node(c) => c.at_boundary(),
            Personality::Rnode(c) => c.at_boundary(),
        }
    }

    fn heartbeat(&self) -> Option<Duration> {
        match self {
            Personality::Modem(c) => c.heartbeat(),
            Personality::Node(c) => c.heartbeat(),
            Personality::Rnode(c) => c.heartbeat(),
        }
    }

    fn without_host(&self) -> bool {
        match self {
            Personality::Modem(c) => c.without_host(),
            Personality::Node(c) => c.without_host(),
            Personality::Rnode(c) => c.without_host(),
        }
    }

    fn banner(&self) -> bool {
        match self {
            Personality::Modem(c) => c.banner(),
            Personality::Node(c) => c.banner(),
            Personality::Rnode(c) => c.banner(),
        }
    }
}

#[cfg(feature = "node")]
impl<L, RK, DLY, D, const PEERS: usize, const ACTIONS: usize, const LINKS: usize>
    Channel<L, RK, DLY> for Personality<D, PEERS, ACTIONS, LINKS>
where
    L: HostLink,
    RK: RadioKind,
    DLY: DelayNs,
    D: crate::executive::ChipDiagnostics<RK, DLY>,
{
    async fn start(&mut self, exec: &mut Executive<'_, RK, DLY>, link: &mut L) -> Flow {
        match self {
            Personality::Modem(c) => c.start(exec, link).await,
            Personality::Node(c) => c.start(exec, link).await,
            Personality::Rnode(c) => c.start(exec, link).await,
        }
    }

    async fn serve(
        &mut self,
        exec: &mut Executive<'_, RK, DLY>,
        link: &mut L,
        event: Event<'_>,
    ) -> Flow {
        match self {
            Personality::Modem(c) => c.serve(exec, link, event).await,
            Personality::Node(c) => c.serve(exec, link, event).await,
            Personality::Rnode(c) => c.serve(exec, link, event).await,
        }
    }

    async fn stop(&mut self, exec: &mut Executive<'_, RK, DLY>, link: &mut L) {
        match self {
            Personality::Modem(c) => c.stop(exec, link).await,
            Personality::Node(c) => c.stop(exec, link).await,
            Personality::Rnode(c) => c.stop(exec, link).await,
        }
    }
}

/// Wait for a host, serving the radio and the channel's clock while waiting.
///
/// The modem simply blocks: with nobody attached there is nothing for it to decide, and a
/// receiving radio would only cost power. The node cannot, and the difference is not a
/// nicety. Running a node's heartbeat inside the host session meant the board announced only
/// while a cable was plugged in, which was found on hardware: a fresh boot reported `tx=0`
/// until something attached, and the first version of that loop reported `tx=1` only because
/// the probe reading the counter was itself the host that started the clock.
///
/// Board-agnostic, so it lives here rather than in a firmware: every board that ships a
/// channel with its own timers needs exactly this.
pub async fn await_host<C, L, RK, DLY>(
    channel: &mut C,
    exec: &mut Executive<'_, RK, DLY>,
    host: &mut L,
    heartbeat: &mut Heartbeat,
) where
    C: Channel<L, RK, DLY>,
    L: HostLink,
    RK: RadioKind,
    DLY: DelayNs,
{
    if !channel.without_host() {
        host.attached().await;
        return;
    }

    loop {
        let _ = exec.ensure_rx().await;
        let mut frame = [0_u8; selvage::MAX_RADIO_FRAME_LEN];
        let woken = select3(host.attached(), exec.receive(&mut frame), heartbeat.next()).await;
        match woken {
            Either3::First(()) => return,
            Either3::Second(Ok(received)) => {
                exec.note_wait(true);
                channel
                    .serve(
                        exec,
                        host,
                        Event::RadioFrame {
                            frame: &frame[..received.len],
                            rssi: received.rssi,
                            snr: received.snr,
                        },
                    )
                    .await;
            }
            // A radio fault with no host to tell. `receive` has already asked for a
            // re-prepare, so the next turn of this loop repairs it.
            Either3::Second(Err(_)) => {}
            Either3::Third(()) => {
                exec.note_wait(false);
                channel.serve(exec, host, Event::Beat).await;
            }
        }
    }
}
