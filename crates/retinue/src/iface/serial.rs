//! Serial carriers over a host tty: RNS's SerialInterface (HDLC) and KISSInterface (a KISS
//! TNC) (`SerialInterface.py` 63-230; `KISSInterface.py` 84-394).
//!
//! [`attach`] attaches one endpoint interface for the life of the carrier. A port that
//! fails is reopened every 5 s under the same [`InterfaceId`], and packets offered while it
//! is down are dropped, as RNS drops them while offline.

use alloc::vec::Vec;

use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use serial2_tokio::{CharSize, FlowControl, SerialPort, Settings, StopBits};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{Instant, sleep_until};

use super::hdlc;
use super::kiss::{self, KissTnc};
use crate::announce_admission::AnnounceIngressPolicy;
use crate::endpoint::{Endpoint, InterfaceId, InterfaceSink, OutboundPackets};
use crate::ifac::Ifac;

/// The largest frame either framing deframes: the packet MTU plus the largest IFAC
/// (`SerialInterface.py` 85).
pub const HW_MTU: usize = 564;
/// A partial frame is discarded after this long without bytes (`SerialInterface.py` 98, 193).
pub const IDLE_RESET: Duration = Duration::from_millis(100);
/// Delay between attempts to reopen a failed port (`SerialInterface.py` 217-228).
pub const REOPEN: Duration = Duration::from_secs(5);

/// Parity, as RNS's `parity` key: none, even, or odd.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Parity {
    #[default]
    None,
    Even,
    Odd,
}

/// Port settings with RNS's defaults: 9600 baud, 8N1, no flow control (`SerialInterface.py`
/// 76-80, 122-136).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerialConfig {
    pub path: PathBuf,
    pub speed: u32,
    /// 5 to 8.
    pub databits: u8,
    pub parity: Parity,
    /// 1 or 2.
    pub stopbits: u8,
}

impl SerialConfig {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            speed: 9_600,
            databits: 8,
            parity: Parity::None,
            stopbits: 1,
        }
    }

    fn char_size(&self) -> io::Result<CharSize> {
        Ok(match self.databits {
            5 => CharSize::Bits5,
            6 => CharSize::Bits6,
            7 => CharSize::Bits7,
            8 => CharSize::Bits8,
            _ => return Err(invalid("databits must be 5 to 8")),
        })
    }

    fn stop_bits(&self) -> io::Result<StopBits> {
        Ok(match self.stopbits {
            1 => StopBits::One,
            2 => StopBits::Two,
            _ => return Err(invalid("stopbits must be 1 or 2")),
        })
    }

    fn apply(&self, mut settings: Settings) -> io::Result<Settings> {
        settings.set_raw();
        settings.set_baud_rate(self.speed)?;
        settings.set_char_size(self.char_size()?);
        settings.set_stop_bits(self.stop_bits()?);
        settings.set_parity(match self.parity {
            Parity::None => serial2_tokio::Parity::None,
            Parity::Even => serial2_tokio::Parity::Even,
            Parity::Odd => serial2_tokio::Parity::Odd,
        });
        settings.set_flow_control(FlowControl::None);
        Ok(settings)
    }
}

/// How packets are framed on the line.
#[derive(Debug)]
pub enum Framing {
    /// RNS SerialInterface: HDLC, as on TCP.
    Hdlc,
    /// RNS KISSInterface: KISS DATA frames to a TNC.
    Kiss(KissTnc),
}

impl Framing {
    /// Time the device gets after open before traffic (`SerialInterface.py` 144-150;
    /// `KISSInterface.py` 178-180).
    fn settle(&self) -> Duration {
        match self {
            Self::Hdlc => Duration::from_millis(500),
            Self::Kiss(_) => Duration::from_secs(2),
        }
    }

    /// RNS's bitrate for the carrier: the line speed, or the KISS guess.
    fn bitrate(&self, speed: u32) -> u64 {
        match self {
            Self::Hdlc => u64::from(speed),
            Self::Kiss(_) => kiss::BITRATE_GUESS,
        }
    }
}

/// Attach a serial carrier to `endpoint` and return its interface id with the future that
/// runs it. The future ends when the endpoint closes.
///
/// Like every RNS serial-family interface, the carrier is exempt from announce ingress
/// control (`SerialInterface.py` 230) and declares its bitrate.
pub fn attach(
    endpoint: &Endpoint,
    config: SerialConfig,
    framing: Framing,
    ifac: Option<Ifac>,
) -> io::Result<(InterfaceId, impl Future<Output = ()> + Send + 'static)> {
    config.char_size()?;
    config.stop_bits()?;
    let interface = match ifac {
        Some(ifac) => endpoint.attach_interface_with_ifac(HW_MTU, ifac)?,
        None => endpoint.attach_interface_with_frame_limit(HW_MTU)?,
    };
    let id = interface.id();
    endpoint.set_interface_bitrate(id, Some(framing.bitrate(config.speed)));
    let exempt = AnnounceIngressPolicy {
        enabled: false,
        ..AnnounceIngressPolicy::default()
    };
    endpoint.set_interface_ingress_policy(id, Some(exempt));
    let (outbound, sink) = interface.split();
    let carrier = run(move || open(&config), outbound, sink, framing);
    Ok((id, carrier))
}

/// Open a port with best-effort modem lines.
fn open(config: &SerialConfig) -> io::Result<SerialPort> {
    let port = SerialPort::open(&config.path, |settings| config.apply(settings))?;
    // pyserial asserts DTR and RTS but tolerates a line that has neither, as a pty has not.
    for line in [port.set_dtr(true), port.set_rts(true)] {
        line.or_else(|error| match error.raw_os_error() {
            Some(code) if code == ENOTTY || code == EINVAL => Ok(()),
            _ => Err(error),
        })?;
    }
    Ok(port)
}

const EINVAL: i32 = 22;
const ENOTTY: i32 = 25;

/// Open, run until the port fails, wait out [`REOPEN`] dropping offered packets, repeat.
async fn run<T, F>(
    mut open: F,
    mut outbound: OutboundPackets,
    sink: InterfaceSink,
    mut framing: Framing,
) where
    T: AsyncRead + AsyncWrite + Unpin,
    F: FnMut() -> io::Result<T>,
{
    let epoch = Instant::now();
    loop {
        if let Ok(port) = open() {
            let settled = Instant::now() + framing.settle();
            if !idle_until(settled, &mut outbound).await {
                return;
            }
            if run_port(port, &mut outbound, &sink, &mut framing, epoch)
                .await
                .is_ok()
            {
                return;
            }
        }
        if !idle_until(Instant::now() + REOPEN, &mut outbound).await {
            return;
        }
    }
}

/// Drop offered packets until `deadline`. False if the endpoint closed meanwhile.
async fn idle_until(deadline: Instant, outbound: &mut OutboundPackets) -> bool {
    loop {
        tokio::select! {
            _ = sleep_until(deadline) => return true,
            packet = outbound.recv() => if packet.is_none() { return false },
        }
    }
}

/// Move frames until the endpoint closes (`Ok`) or the port fails (`Err`).
async fn run_port<T>(
    mut io: T,
    outbound: &mut OutboundPackets,
    sink: &InterfaceSink,
    framing: &mut Framing,
    epoch: Instant,
) -> io::Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let now_ms = || u64::try_from(epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
    if let Framing::Kiss(tnc) = framing {
        io.write_all(&tnc.startup()).await?;
    }
    let mut deframer = hdlc::Deframer::new();
    let mut last_rx = Instant::now();
    let mut buf = [0_u8; 1024];
    loop {
        let (ready, wake_ms) = match framing {
            Framing::Hdlc => (true, None),
            Framing::Kiss(tnc) => (tnc.is_ready(), tnc.wake_at_ms()),
        };
        let wake = wake_ms.map_or(Instant::now() + Duration::from_secs(3600), |ms| {
            epoch + Duration::from_millis(ms)
        });
        tokio::select! {
            packet = outbound.recv(), if ready => {
                let Some(packet) = packet else { return Ok(()) };
                // A packet IFAC cannot seal is the endpoint's fault, not the port's.
                let Ok(bytes) = outbound.encode(&packet) else { continue };
                let wire = match framing {
                    Framing::Hdlc => hdlc::frame(&bytes),
                    Framing::Kiss(tnc) => tnc.send(&bytes, now_ms()),
                };
                io.write_all(&wire).await?;
                io.flush().await?;
            }
            read = io.read(&mut buf) => {
                let count = read?;
                if count == 0 {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                if last_rx.elapsed() > IDLE_RESET {
                    deframer = hdlc::Deframer::new();
                    if let Framing::Kiss(tnc) = framing {
                        tnc.reset_partial();
                    }
                }
                last_rx = Instant::now();
                let mut frames = Vec::new();
                match framing {
                    Framing::Hdlc => frames = deframer.push(&buf[..count]),
                    Framing::Kiss(tnc) => tnc.receive(&buf[..count], &mut frames),
                }
                for frame in frames {
                    // An undecodable or wrongly sealed frame is dropped; only a closed
                    // endpoint ends the carrier.
                    if let Ok(false) = sink.deliver_frame(&frame) {
                        return Ok(());
                    }
                }
            }
            _ = sleep_until(wake) => {
                if let Framing::Kiss(tnc) = framing
                    && let Some(wire) = tnc.poll(now_ms())
                {
                    io.write_all(&wire).await?;
                    io.flush().await?;
                }
            }
        }
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
#[path = "serial_tests.rs"]
mod tests;
