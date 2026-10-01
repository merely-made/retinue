//! All seven pages, and the TRAFFIC ticker, through the mirror.
//!
//! The reference is `radio_face::render` into an independent buffer, so these
//! tests do not compare the mirror with itself.

use embedded_graphics::{pixelcolor::Rgb888, prelude::*};
use radio_face::{
    DetailPolicy, EventKind, EventSource, HostSnapshot, InputEvent, InputProfile, LocalStatus,
    Page, RxSummary, Screen, Surface, Text, Theme, UiEvent, render,
};
use radio_mirror::{Mirror, input, receipt_theme};

struct Reference(Size, Vec<u8>);

impl OriginDimensions for Reference {
    fn size(&self) -> Size {
        self.0
    }
}

impl DrawTarget for Reference {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb888>>>(
        &mut self,
        pixels: I,
    ) -> Result<(), Self::Error> {
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 && p.x < self.0.width as i32 && p.y < self.0.height as i32 {
                let i = (p.y as usize * self.0.width as usize + p.x as usize) * 4;
                self.1[i..i + 4].copy_from_slice(&[c.r(), c.g(), c.b(), 255]);
            }
        }
        Ok(())
    }
}

fn reference(
    surface: Surface,
    theme: Theme<Rgb888>,
    screen: Screen,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Vec<u8> {
    let size = surface.size();
    let mut canvas = Reference(size, vec![0; (size.width * size.height * 4) as usize]);
    render(&mut canvas, surface, theme, screen, local, host).unwrap();
    canvas.1
}

fn fixtures() -> (LocalStatus, HostSnapshot) {
    (
        input::local_from_json(include_str!("../fixtures/receipts-local.json")).unwrap(),
        input::host_from_json(include_str!("../fixtures/receipts-host.json")).unwrap(),
    )
}

const ALL: [Page; 7] = [
    Page::Status,
    Page::Power,
    Page::Radio,
    Page::Traffic,
    Page::Identity,
    Page::Links,
    Page::Peers,
];

#[test]
fn the_simulators_seven_pages_render_through_the_controller() {
    let (local, host) = fixtures();
    for surface in [Surface::Oled128x64, Surface::Tft240x135] {
        for (attached, pages) in [(Some(host), &ALL[..]), (None, &ALL[..4])] {
            let mut mirror = Mirror::new(surface, InputProfile::TwoButton);
            mirror.set_theme(receipt_theme(surface));
            mirror.set_local(local);
            mirror.set_host(attached);
            let mut seen = Vec::new();
            for page in pages {
                assert_eq!(mirror.screen(), Screen::Page(*page));
                let state = *mirror.local();
                let expected = reference(
                    surface,
                    receipt_theme(surface),
                    Screen::Page(*page),
                    &state,
                    attached.as_ref(),
                );
                let rgba = mirror.render().to_vec();
                assert!(rgba == expected, "{surface:?} {page:?}");
                assert!(!seen.contains(&rgba), "{surface:?} {page:?} repeats a page");
                seen.push(rgba);
                mirror.press(InputEvent::AShort);
            }
            assert_eq!(mirror.screen(), Screen::Page(Page::Status), "pages wrap");
        }
    }
}

fn ticker_top(surface: Surface) -> usize {
    match surface {
        Surface::Oled128x64 => 54,
        Surface::Tft240x135 => 118,
    }
}

#[test]
fn traffic_ticker_shows_each_event_from_node_state() {
    let (mut local, _) = fixtures();
    let kinds = [
        EventKind::Info,
        EventKind::Received,
        EventKind::Transmitted,
        EventKind::Delivered,
        EventKind::Propagated,
        EventKind::Failed,
    ];
    for surface in [Surface::Oled128x64, Surface::Tft240x135] {
        let theme = receipt_theme(surface);
        let split = ticker_top(surface) * surface.size().width as usize * 4;
        let mut tickers = Vec::new();
        let mut body = None;
        for source in [EventSource::Local, EventSource::Host] {
            for (index, kind) in kinds.into_iter().enumerate() {
                // A minimal-detail snapshot still carries events: no names needed.
                let host = HostSnapshot {
                    detail: DetailPolicy::Minimal,
                    event: Some(UiEvent {
                        source,
                        kind,
                        text: Text::from_truncated(
                            [
                                "QUEUED",
                                "RX 2 HOPS",
                                "TX VIA C",
                                "DELIVERED",
                                "PROPAGATED",
                                "NO PATH",
                            ][index],
                        ),
                    }),
                    ..HostSnapshot::default()
                };
                let host = input::through_wire(&host).unwrap();
                let mut mirror = Mirror::new(surface, InputProfile::OneButton);
                mirror.set_theme(theme);
                mirror.set_local(local);
                mirror.set_host(Some(host));
                for _ in 0..3 {
                    mirror.press(InputEvent::AShort);
                }
                assert_eq!(mirror.screen(), Screen::Page(Page::Traffic));
                let state = *mirror.local();
                let rgba = mirror.render().to_vec();
                assert!(
                    rgba == reference(
                        surface,
                        theme,
                        Screen::Page(Page::Traffic),
                        &state,
                        Some(&host)
                    )
                );
                let (above, ticker) = rgba.split_at(split);
                assert!(
                    *body.get_or_insert_with(|| above.to_vec()) == above,
                    "only the ticker changes"
                );
                assert!(
                    !tickers.contains(&ticker.to_vec()),
                    "{source:?} {kind:?} ticker repeats"
                );
                tickers.push(ticker.to_vec());
            }
        }

        // Without a host event, the ticker falls back to local RX, then to none.
        for last_rx in [
            Some(RxSummary {
                frame_len: 61,
                rssi_dbm: -88,
                snr_tenths_db: 41,
            }),
            None,
        ] {
            local.last_rx = last_rx;
            let rgba = radio_mirror::render_rgba(
                surface,
                theme,
                Screen::Page(Page::Traffic),
                &local,
                None,
            );
            assert!(
                rgba.as_rgba()
                    == reference(surface, theme, Screen::Page(Page::Traffic), &local, None)
            );
            let ticker = rgba.as_rgba()[split..].to_vec();
            assert!(!tickers.contains(&ticker));
            tickers.push(ticker);
        }
        assert_eq!(tickers.len(), 14);
    }
}
