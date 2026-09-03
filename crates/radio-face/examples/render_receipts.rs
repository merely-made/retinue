use std::{
    error::Error,
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
};

use embedded_graphics::{pixelcolor::Rgb888, prelude::*};
use radio_face::{
    DetailPolicy, EventKind, EventSource, Fault, HostSnapshot, HostState, IfacState, LocalStatus,
    MenuItem, NodeSummary, Page, PeerPath, PeerSummary, Personality, PowerSource, RadioProfile,
    RadioState, RxSummary, Screen, SleepState, Surface, Text, Theme, TxResult, UiEvent, WakeSource,
    render,
};

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

fn main() -> Result<(), Box<dyn Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/radio-face-receipts"));
    fs::create_dir_all(&output)?;

    let (local, host) = fixture();
    let screens = [
        ("boot", Screen::Boot),
        ("status", Screen::Page(Page::Status)),
        ("power", Screen::Page(Page::Power)),
        ("radio", Screen::Page(Page::Radio)),
        ("traffic", Screen::Page(Page::Traffic)),
        ("identity", Screen::Page(Page::Identity)),
        ("links", Screen::Page(Page::Links)),
        ("peers", Screen::Page(Page::Peers)),
        (
            "menu",
            Screen::Menu {
                selected: MenuItem::Verify,
                selected_index: 2,
            },
        ),
        ("verify", Screen::Verify),
        ("display-off", Screen::DisplayOff),
        ("fault", Screen::Fault),
    ];

    for surface in [Surface::Oled128x64, Surface::Tft240x135] {
        for (name, screen) in screens {
            let mut state = local;
            if screen == Screen::Fault {
                state.fault = Some(Fault {
                    code: 1,
                    message: Text::from_truncated("SX1262 INIT FAILED"),
                });
            }
            let mut canvas = Canvas::new(surface.size());
            render(
                &mut canvas,
                surface,
                theme(surface),
                screen,
                &state,
                Some(&host),
            )?;
            let prefix = match surface {
                Surface::Oled128x64 => "oled-128x64",
                Surface::Tft240x135 => "tft-240x135",
            };
            let path = output.join(format!("{prefix}-{name}.png"));
            write_png(&path, &canvas)?;
            println!("{}", path.display());
        }
    }
    Ok(())
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

fn fixture() -> (LocalStatus, HostSnapshot) {
    let local = LocalStatus {
        board: Text::from_truncated("HELTEC V4"),
        firmware: Text::from_truncated("PHY V10"),
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
            frequency_hz: Some(906_875_000),
            bandwidth_hz: Some(250_000),
            spreading_factor: Some(11),
            coding_rate_denominator: Some(5),
            tx_power_dbm: Some(17),
            sync_word: Some(0x2b),
            name: Text::from_truncated("LONGFAST"),
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
        personality: Personality::Retinue,
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
            source: EventSource::Host,
            kind: EventKind::Delivered,
            text: Text::from_truncated("DIRECT DELIVERED"),
        }),
    };
    (local, host)
}
