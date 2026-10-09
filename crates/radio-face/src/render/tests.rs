use super::*;
use crate::controller::{MenuItem, Page};
use crate::status::{
    DetailPolicy, EventKind, EventSource, Fault, HostState, IfacState, NodeSummary, PeerPath,
    PeerSummary, PowerSource, RadioProfile, RadioState, RxSummary, SleepState, Text, TxResult,
    UiEvent, WakeSource,
};
use embedded_graphics::pixelcolor::Rgb888;
use std::vec;
use std::vec::Vec;

#[cfg(feature = "text")]
mod projection;

struct Canvas {
    size: Size,
    pixels: Vec<Rgb888>,
    out_of_bounds: usize,
}

impl Canvas {
    fn new(size: Size) -> Self {
        Self {
            size,
            pixels: vec![Rgb888::BLACK; (size.width * size.height) as usize],
            out_of_bounds: 0,
        }
    }

    fn lit_pixels(&self) -> usize {
        self.pixels
            .iter()
            .filter(|pixel| **pixel != Rgb888::BLACK)
            .count()
    }

    fn digest(&self) -> u64 {
        self.pixels
            .iter()
            .fold(0xcbf29ce484222325, |mut hash, pixel| {
                for byte in [pixel.r(), pixel.g(), pixel.b()] {
                    hash ^= u64::from(byte);
                    hash = hash.wrapping_mul(0x100000001b3);
                }
                hash
            })
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
            if point.x < 0
                || point.y < 0
                || point.x >= self.size.width as i32
                || point.y >= self.size.height as i32
            {
                self.out_of_bounds += 1;
                continue;
            }
            let index = point.y as usize * self.size.width as usize + point.x as usize;
            self.pixels[index] = color;
        }
        Ok(())
    }
}

fn theme() -> Theme<Rgb888> {
    Theme::new(
        Rgb888::BLACK,
        Rgb888::WHITE,
        Rgb888::new(128, 128, 128),
        Rgb888::new(255, 176, 0),
    )
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
        gnss: crate::status::GnssState::Absent,
    };
    let host = HostSnapshot {
        valid_for_secs: 15,
        personality: crate::status::Personality::Retinue,
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

#[test]
fn every_face_stays_inside_both_real_display_bounds() {
    let (mut local, host) = fixture();
    let screens = [
        Screen::Boot,
        Screen::Page(Page::Status),
        Screen::Page(Page::Power),
        Screen::Page(Page::Radio),
        Screen::Page(Page::Traffic),
        Screen::Page(Page::Identity),
        Screen::Page(Page::Links),
        Screen::Page(Page::Peers),
        Screen::Menu {
            selected: MenuItem::Verify,
            selected_index: 2,
        },
        Screen::Verify,
        Screen::DisplayOff,
    ];
    for surface in [Surface::Oled128x64, Surface::Tft240x135] {
        for screen in screens {
            let mut canvas = Canvas::new(surface.size());
            render(&mut canvas, surface, theme(), screen, &local, Some(&host)).unwrap();
            assert_eq!(
                canvas.out_of_bounds, 0,
                "{surface:?} {screen:?} drew outside the panel"
            );
            assert!(
                canvas.lit_pixels() > 20,
                "{surface:?} {screen:?} rendered blank"
            );
        }

        local.fault = Some(Fault {
            code: 1,
            message: Text::from_truncated("SX1262 INIT FAILED"),
        });
        let mut canvas = Canvas::new(surface.size());
        render(
            &mut canvas,
            surface,
            theme(),
            Screen::Fault,
            &local,
            Some(&host),
        )
        .unwrap();
        assert_eq!(canvas.out_of_bounds, 0);
        assert!(canvas.lit_pixels() > 20);
        local.fault = None;
    }
}

#[test]
fn worst_case_bounded_values_still_clip_at_glyph_boundaries() {
    let (mut local, mut host) = fixture();
    local.board = Text::from_truncated("ABCDEFGHIJKLMNOP");
    local.firmware = Text::from_truncated("ABCDEFGHIJKL");
    local.uptime_secs = u32::MAX;
    local.tx_frames = u32::MAX;
    local.rx_frames = u32::MAX;
    local.profile.frequency_hz = Some(u32::MAX);
    local.profile.bandwidth_hz = Some(u32::MAX);
    local.profile.name = Text::from_truncated("ABCDEFGHIJKLMNOP");
    host.queue_depth = u16::MAX;
    host.peer_overflow = u8::MAX;

    for surface in [Surface::Oled128x64, Surface::Tft240x135] {
        for page in [
            Page::Status,
            Page::Power,
            Page::Radio,
            Page::Traffic,
            Page::Identity,
            Page::Links,
            Page::Peers,
        ] {
            let mut canvas = Canvas::new(surface.size());
            render(
                &mut canvas,
                surface,
                theme(),
                Screen::Page(page),
                &local,
                Some(&host),
            )
            .unwrap();
            assert_eq!(canvas.out_of_bounds, 0, "{surface:?} {page:?}");
        }
    }
}

#[test]
fn selected_faces_have_stable_pixel_goldens() {
    let (local, host) = fixture();
    let mut actual = [0_u64; 4];
    for (index, (surface, screen)) in [
        (Surface::Oled128x64, Screen::Page(Page::Status)),
        (
            Surface::Oled128x64,
            Screen::Menu {
                selected: MenuItem::Verify,
                selected_index: 2,
            },
        ),
        (Surface::Tft240x135, Screen::Page(Page::Status)),
        (
            Surface::Tft240x135,
            Screen::Menu {
                selected: MenuItem::Verify,
                selected_index: 2,
            },
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut canvas = Canvas::new(surface.size());
        render(&mut canvas, surface, theme(), screen, &local, Some(&host)).unwrap();
        actual[index] = canvas.digest();
    }

    assert_eq!(
        actual,
        [
            4_785_255_477_419_125_564,
            7_929_273_586_648_350_406,
            15_393_597_127_465_717_303,
            6_764_923_977_634_548_512,
        ]
    );
}
