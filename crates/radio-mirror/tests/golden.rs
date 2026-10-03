//! The mirror against `radio-face/examples/render_receipts.rs`.
//!
//! `tests/golden/` is that example's unedited output. Regenerate with
//! `cargo run -p radio-face --example render_receipts -- crates/radio-mirror/tests/golden`.
//! The JSON fixtures restate the example's `fixture()`.

use std::{fs, path::PathBuf};

use radio_face::{Fault, MenuItem, Page, Screen, Surface, Text};
use radio_mirror::{input, names, receipt_theme, render_png, render_rgba};

const SCREENS: [(&str, Screen); 12] = [
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

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn decode_rgb(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().unwrap();
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgb);
    buffer.truncate(info.buffer_size());
    (info.width, info.height, buffer)
}

#[test]
fn every_receipt_screen_matches_pixel_for_pixel_and_byte_for_byte() {
    let local = input::local_from_json(include_str!("../fixtures/receipts-local.json")).unwrap();
    let host = input::host_from_json(include_str!("../fixtures/receipts-host.json")).unwrap();
    let mut compared = 0;
    for surface in [Surface::Oled128x64, Surface::Tft240x135] {
        for (name, screen) in SCREENS {
            let mut state = local;
            if screen == Screen::Fault {
                state.fault = Some(Fault {
                    code: 1,
                    message: Text::from_truncated("SX1262 INIT FAILED"),
                });
            }
            let file = format!("{}-{name}.png", names::surface_name(surface));
            let expected = fs::read(golden(&file)).unwrap();
            let (width, height, rgb) = decode_rgb(&expected);

            let frame = render_rgba(surface, receipt_theme(surface), screen, &state, Some(&host));
            assert_eq!((frame.width(), frame.height()), (width, height), "{file}");
            assert!(frame.to_rgb() == rgb, "{file}: pixels differ");
            assert!(
                frame
                    .as_rgba()
                    .chunks_exact(4)
                    .all(|pixel| pixel[3] == u8::MAX),
                "{file}: not opaque"
            );

            let png =
                render_png(surface, receipt_theme(surface), screen, &state, Some(&host)).unwrap();
            assert!(png == expected, "{file}: PNG bytes differ");
            compared += 1;
        }
    }
    assert_eq!(compared, 24);
}

/// Controls: the comparison above must see a palette change and a one-count change.
#[test]
fn the_comparison_detects_small_differences() {
    let mut local =
        input::local_from_json(include_str!("../fixtures/receipts-local.json")).unwrap();
    let host = input::host_from_json(include_str!("../fixtures/receipts-host.json")).unwrap();
    let traffic = Screen::Page(Page::Traffic);

    let (_, _, tft) = decode_rgb(&fs::read(golden("tft-240x135-traffic.png")).unwrap());
    let mono = render_rgba(
        Surface::Tft240x135,
        radio_mirror::mono_theme(),
        traffic,
        &local,
        Some(&host),
    );
    assert!(mono.to_rgb() != tft);

    let (_, _, oled) = decode_rgb(&fs::read(golden("oled-128x64-traffic.png")).unwrap());
    local.tx_frames += 1;
    let bumped = render_rgba(
        Surface::Oled128x64,
        receipt_theme(Surface::Oled128x64),
        traffic,
        &local,
        Some(&host),
    );
    assert!(bumped.to_rgb() != oled);
}
