//! adafruit-nrfutil serial-DFU command construction and progress parsing.

use std::path::Path;

use crate::device::BootloaderObservation;
use crate::executor::ProcessProgress;

pub(crate) fn command(port: &str, payload: &Path) -> Vec<String> {
    vec![
        "dfu".into(),
        "serial".into(),
        "-pkg".into(),
        payload.display().to_string(),
        "-p".into(),
        port.into(),
        "-b".into(),
        "115200".into(),
        "--singlebank".into(),
    ]
}

pub(crate) fn progress(line: &str) -> Option<ProcessProgress> {
    crate::route::parse_progress_line(line).or_else(|| {
        let lower = line.to_ascii_lowercase();
        let percent = lower
            .split_whitespace()
            .find_map(|word| word.strip_suffix('%')?.parse::<u64>().ok())?;
        (percent <= 100).then_some(ProcessProgress {
            written: percent,
            total: 100,
        })
    })
}

pub fn bootloader_facts(output: &str) -> Result<BootloaderObservation, String> {
    let lower = output.to_ascii_lowercase();
    if !lower.contains("nrf52840") && !lower.contains("nrf52") {
        return Err("nRF52840 was not identified".into());
    }
    let flash_size = key_value(output, "flash").and_then(|value| value.parse().ok());
    Ok(BootloaderObservation {
        identifier: output.lines().next().map(str::trim).map(str::to_string),
        descriptor: Some("nRF52 serial DFU".into()),
        processor: Some(crate::package::ProcessorKind::Nrf52840),
        flash_size,
        bootloader: key_value(output, "bootloader").map(str::to_string),
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
