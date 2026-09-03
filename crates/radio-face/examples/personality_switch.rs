//! One board, four citizenships: the interoperation story as a filmstrip.
//!
//! `render_receipts` proves each *screen* renders. This renders the same two
//! screens across every `Personality` instead, so the sequence shows what
//! actually changes when a node answers a neighbor's request to speak a
//! different mesh: the firmware label, the PHY the radio is tuned to, and the
//! sync word that decides which mesh can hear it at all.
//!
//! This renders frames; it does not drive a radio. But be precise about what
//! is and is not built, because two different switches get conflated:
//!
//! - **The PHY profile is live-configurable today.** `CMD_CONFIG` ->
//!   `dispatch.rs` -> `Executive::apply_profile` -> `service::apply_profile`
//!   retunes frequency, bandwidth, SF, CR, and the sync word on a running
//!   radio, no reboot. Meshtastic's `0x2b` and MeshCore's `0x12`
//!   (`selvage::{MESHTASTIC,MESHCORE}_SYNC_WORD`) are one host command apart.
//! - **The boot channel is not.** `settings::Channel` (Modem / Node / Rnode)
//!   is a persisted byte plus a reboot, per structural decision 4. Note it has
//!   no Sennet or MeshCore variant at all: those are host-side crates driving
//!   the board while it serves `Modem`, which is how the sennet direct-PHY
//!   receipts were produced.
//!
//! CM1 in the murmuration doc (`2026-08-09_channel_murmuration.md`) is the
//! second one — teardown-correct hot switching between resident on-board
//! channels — and is unbuilt. A host-driven retune between meshes is not
//! waiting on it.
//!
//! ```text
//! cargo run -p radio-face --example personality_switch
//! ```
//!
//! PNGs go to `target/radio-face-personalities` unless a directory is passed.

use std::{
    error::Error,
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
};

use embedded_graphics::{pixelcolor::Rgb888, prelude::*};
use radio_face::{
    DetailPolicy, EventKind, EventSource, HostSnapshot, HostState, IfacState, LocalStatus,
    NodeSummary, Page, PeerPath, PeerSummary, Personality, PowerSource, RadioProfile, RadioState,
    RxSummary, Screen, SleepState, Surface, Text, Theme, TxResult, UiEvent, WakeSource, render,
};

/// One personality, and the radio facts that follow from choosing it.
struct Citizenship {
    slug: &'static str,
    personality: Personality,
    /// `LocalStatus::firmware` is a `Text<12>`; keep these short.
    firmware: &'static str,
    profile_name: &'static str,
    bandwidth_hz: u32,
    spreading_factor: u8,
    coding_rate_denominator: u8,
    /// `None` where the value is not ours to state. The RNode serial
    /// personality takes its PHY from the host (`radio-hand/src/rnode.rs`:
    /// the host sends FREQUENCY, BANDWIDTH, SF, CR), and `selvage` defines no
    /// Reticulum sync constant the way it does for the other two. Rendering
    /// nothing is honest; inventing a byte for a radio audience is not.
    sync_word: Option<u8>,
    /// What the host reports while this citizenship is current.
    event: &'static str,
}

/// Sync words are `selvage::MESHTASTIC_SYNC_WORD` (0x2b) and
/// `selvage::MESHCORE_SYNC_WORD` (0x12), quoted rather than imported so this
/// example adds no dependency edge to the crate it demonstrates.
const CITIZENSHIPS: [Citizenship; 4] = [
    Citizenship {
        slug: "1-retinue",
        personality: Personality::Retinue,
        firmware: "RETINUE V10",
        profile_name: "TRUNK",
        bandwidth_hz: 250_000,
        spreading_factor: 11,
        coding_rate_denominator: 5,
        sync_word: None,
        event: "HOME CHANNEL RESIDENT",
    },
    Citizenship {
        slug: "2-rnode",
        personality: Personality::RNode,
        firmware: "RNODE V10",
        profile_name: "HOST SET",
        bandwidth_hz: 250_000,
        spreading_factor: 11,
        coding_rate_denominator: 5,
        sync_word: None,
        event: "STOCK RNS DRIVING",
    },
    Citizenship {
        slug: "3-sennet",
        personality: Personality::Sennet,
        firmware: "SENNET V10",
        profile_name: "LONGFAST",
        bandwidth_hz: 250_000,
        spreading_factor: 11,
        coding_rate_denominator: 5,
        sync_word: Some(0x2b),
        event: "NEIGHBOR ASKS SENNET",
    },
    Citizenship {
        slug: "4-meshcore",
        personality: Personality::MeshCore,
        firmware: "TUCKET V10",
        profile_name: "MESHCORE",
        bandwidth_hz: 250_000,
        spreading_factor: 11,
        coding_rate_denominator: 5,
        sync_word: Some(0x12),
        event: "NEIGHBOR ASKS MESHCORE",
    },
];

fn main() -> Result<(), Box<dyn Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/radio-face-personalities"));
    fs::create_dir_all(&output)?;

    // Three screens, each carrying a different part of the story: Status has
    // the personality as the firmware label, Radio has the PHY and sync word
    // that decide which mesh can hear it, and Traffic is the only one of the
    // three that draws the host event ticker (`render_traffic` ends in
    // `ticker_event`; Status and Radio do not call it), so it is where the
    // neighbor's request is visible at all. Rendering Power or Peers four
    // times would pad the filmstrip with frames that say nothing new.
    let pages = [
        ("status", Page::Status),
        ("radio", Page::Radio),
        ("traffic", Page::Traffic),
    ];

    for surface in [Surface::Oled128x64, Surface::Tft240x135] {
        for citizenship in &CITIZENSHIPS {
            let (local, host) = frame(citizenship);
            for (page_name, page) in pages {
                let mut canvas = Canvas::new(surface.size());
                render(
                    &mut canvas,
                    surface,
                    theme(surface),
                    Screen::Page(page),
                    &local,
                    Some(&host),
                )?;
                let prefix = match surface {
                    Surface::Oled128x64 => "oled-128x64",
                    Surface::Tft240x135 => "tft-240x135",
                };
                let path = output.join(format!("{prefix}-{}-{page_name}.png", citizenship.slug));
                write_png(&path, &canvas)?;
                println!("{}", path.display());
            }
        }
    }
    Ok(())
}

/// The shared fixture, varied by citizenship. Everything not listed in
/// [`Citizenship`] is deliberately held constant so a viewer flipping through
/// the frames sees only what the personality actually changes.
fn frame(citizenship: &Citizenship) -> (LocalStatus, HostSnapshot) {
    let local = LocalStatus {
        board: Text::from_truncated("HELTEC V4"),
        firmware: Text::from_truncated(citizenship.firmware),
        uptime_secs: 14_719,
        radio: RadioState::Online,
        host: HostState::Attached,
        power_source: PowerSource::Usb,
        battery_percent: Some(73),
        millivolts: Some(3_920),
        display_on: true,
        sleep: SleepState::Disabled,
        last_wake: WakeSource::Radio,
        profile: RadioProfile {
            // Held constant across frames: one SX1262 on one band. The mesh
            // changes, the antenna does not.
            frequency_hz: Some(906_875_000),
            bandwidth_hz: Some(citizenship.bandwidth_hz),
            spreading_factor: Some(citizenship.spreading_factor),
            coding_rate_denominator: Some(citizenship.coding_rate_denominator),
            tx_power_dbm: Some(17),
            sync_word: citizenship.sync_word,
            name: Text::from_truncated(citizenship.profile_name),
        },
        tx_frames: 128,
        rx_frames: 342,
        last_rx: Some(RxSummary {
            frame_len: 243,
            rssi_dbm: -97,
            snr_tenths_db: 62,
        }),
        last_tx: TxResult::Sent { frame_len: 247 },
        fault: None,
        gnss: radio_face::GnssState::Absent,
    };
    let host = HostSnapshot {
        valid_for_secs: 15,
        personality: citizenship.personality,
        detail: DetailPolicy::Named,
        node: Some(NodeSummary {
            name: Text::from_truncated("HERALD"),
            address_tail: [0x4c, 0x9f, 0x03, 0xaa, 0x77, 0xe2, 0xbd, 0x08],
            fingerprint: [
                0x4c, 0x9f, 0x03, 0xaa, 0x77, 0xe2, 0x1b, 0x0d, 0x92, 0xc4, 0xe8, 0xf1, 0x5a, 0x36,
                0xbd, 0x08,
            ],
            role: Text::from_truncated("NODE"),
            uptime_secs: 13_700,
        }),
        link_count: 4,
        admitted_links: 2,
        queue_depth: 3,
        ifac: IfacState::On,
        peers: [
            Some(PeerSummary {
                name: Text::from_truncated("ESQUIRE"),
                path: PeerPath::Direct,
                age_secs: 120,
            }),
            Some(PeerSummary {
                name: Text::from_truncated("MARSHAL"),
                path: PeerPath::Direct,
                age_secs: 3_600,
            }),
            Some(PeerSummary {
                name: Text::from_truncated("OUTRIDER"),
                path: PeerPath::Via,
                age_secs: 720,
            }),
        ],
        peer_overflow: 1,
        event: Some(UiEvent {
            // Info, not Delivered: these frames report a state of citizenship,
            // not the outcome of a message.
            source: EventSource::Host,
            kind: EventKind::Info,
            text: Text::from_truncated(citizenship.event),
        }),
    };
    (local, host)
}

struct Canvas {
    size: Size,
    pixels: Vec<Rgb888>,
}

impl Canvas {
    fn new(size: Size) -> Self {
        Self {
            size,
            pixels: vec![Rgb888::BLACK; (size.width * size.height) as usize],
        }
    }

    fn rgb_bytes(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|pixel| [pixel.r(), pixel.g(), pixel.b()])
            .collect()
    }
}

impl OriginDimensions for Canvas {
    fn size(&self) -> Size {
        self.size
    }
}

impl DrawTarget for Canvas {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x >= 0
                && point.y >= 0
                && point.x < self.size.width as i32
                && point.y < self.size.height as i32
            {
                let index = point.y as usize * self.size.width as usize + point.x as usize;
                self.pixels[index] = color;
            }
        }
        Ok(())
    }
}

fn theme(surface: Surface) -> Theme<Rgb888> {
    match surface {
        Surface::Oled128x64 => {
            Theme::new(Rgb888::BLACK, Rgb888::WHITE, Rgb888::WHITE, Rgb888::WHITE)
        }
        Surface::Tft240x135 => Theme::new(
            Rgb888::BLACK,
            Rgb888::new(238, 242, 255),
            Rgb888::new(128, 139, 156),
            Rgb888::new(255, 176, 0),
        ),
    }
}

fn write_png(path: &Path, canvas: &Canvas) -> Result<(), Box<dyn Error>> {
    let output = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(output, canvas.size.width, canvas.size.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&canvas.rgb_bytes())?;
    Ok(())
}
