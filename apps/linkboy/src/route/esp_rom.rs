//! espflash command construction and progress parsing.

use std::path::Path;

use crate::device::BootloaderObservation;
use crate::executor::{ProcessFailure, ProcessProgress, ProcessRunner};

pub(crate) fn command(port: &str, payload: &Path) -> Vec<String> {
    vec![
        "flash".into(),
        "-p".into(),
        port.into(),
        "--chip".into(),
        "esp32s3".into(),
        payload.display().to_string(),
    ]
}

/// Query the ESP ROM loader and return the board to its application afterward.
pub fn board_info_command(port: &str) -> Vec<String> {
    vec![
        "board-info".into(),
        "-p".into(),
        port.into(),
        "--before".into(),
        "default-reset".into(),
        "--after".into(),
        "hard-reset".into(),
        "--non-interactive".into(),
    ]
}

/// Run the non-writing loader query used to establish V4 hardware facts before planning.
pub fn discover<P: ProcessRunner>(
    process: &mut P,
    port: &str,
) -> Result<BootloaderObservation, ProcessFailure> {
    let output = process.run("espflash", &board_info_command(port), &mut |_| {})?;
    bootloader_facts(&output.diagnostics).map_err(|detail| ProcessFailure::Failed {
        program: "espflash".into(),
        diagnostics: detail,
    })
}

pub(crate) fn progress(line: &str) -> Option<ProcessProgress> {
    crate::route::parse_progress_line(line).or_else(|| {
        let (written, total) = line.split_once('/')?;
        Some(ProcessProgress {
            written: written.trim().parse().ok()?,
            total: total.split_whitespace().next()?.parse().ok()?,
        })
    })
}

pub fn bootloader_facts(output: &str) -> Result<BootloaderObservation, String> {
    let lower = output.to_ascii_lowercase();
    if !lower.contains("esp32-s3") && !lower.contains("esp32s3") {
        return Err("ESP32-S3 was not identified".into());
    }
    let flash_size = key_value(output, "flash")
        .and_then(|value| value.parse::<u32>().ok())
        .or_else(|| {
            lower
                .split_whitespace()
                .find_map(|word| word.strip_suffix("mb")?.parse::<u32>().ok())
                .map(|megabytes| megabytes * 1024 * 1024)
        });
    Ok(BootloaderObservation {
        identifier: output.lines().next().map(str::trim).map(str::to_string),
        descriptor: Some("ESP32-S3 ROM".into()),
        processor: Some(crate::package::ProcessorKind::Esp32S3),
        flash_size,
        bootloader: Some("esp-rom".into()),
        usb_vid: key_value(output, "vid").and_then(parse_number),
        usb_pid: key_value(output, "pid").and_then(parse_number),
    })
}

fn key_value<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output
        .split(|character: char| character.is_whitespace() || character == ';' || character == ',')
        .find_map(|word| word.strip_prefix(&format!("{key}=")))
}

fn parse_number(value: &str) -> Option<u16> {
    value
        .strip_prefix("0x")
        .and_then(|digits| u16::from_str_radix(digits, 16).ok())
        .or_else(|| value.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_info_stays_in_the_rom_loader() {
        assert_eq!(
            board_info_command("COM8"),
            vec![
                "board-info",
                "-p",
                "COM8",
                "--before",
                "default-reset",
                "--after",
                "hard-reset",
                "--non-interactive",
            ]
        );
    }

    #[test]
    fn board_info_text_produces_loader_facts() {
        let facts = bootloader_facts(
            "Chip type: ESP32-S3\nFlash size: 16MB\nUSB VID: 0x303a\nUSB PID: 0x1001",
        )
        .unwrap();
        assert_eq!(
            facts.processor,
            Some(crate::package::ProcessorKind::Esp32S3)
        );
        assert_eq!(facts.flash_size, Some(16 * 1024 * 1024));
    }
}
