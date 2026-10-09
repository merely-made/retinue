//! Argument parsing and dispatch for the `linkboy` binary.

mod command;
mod flash;
mod render;

use std::time::Duration;

use linkboy::{BoardFamily, Error};

pub(crate) use command::run_command;

const BOOTLOADER_PATIENCE: Duration = Duration::from_secs(12);

fn usage() -> &'static str {
    "usage:\n  \
     linkboy list\n  \
     linkboy ask PORT LINE...\n  \
     linkboy inspect PACKAGE\n  \
     linkboy catalog INDEX\n  \
     linkboy catalog-auth INDEX TRUST\n  \
     linkboy plan DEVICE PACKAGE [BOARD@REVISION]\n  \
     linkboy flash DEVICE PACKAGE [BOARD@REVISION] [--loader-snapshot PATH] [--receipt PATH]\n  \
     linkboy flash-volume VOLUME PACKAGE BOARD@REVISION [--receipt PATH]\n  \
     linkboy capture-t114-loader VOLUME PATH\n  \
     linkboy make-uf2 BIN UF2 BASE FAMILY\n  \
     linkboy verify-recovery PORT PACKAGE BOARD@REVISION RECOVERY --loader-snapshot PATH [--receipt PATH]\n  \
     linkboy flash-raw PORT IMAGE [t114|v4]\n  \
     linkboy bootloader PORT"
}

fn bad_usage(what: &str) -> Error {
    Error::ToolFailed {
        tool: "linkboy",
        message: format!("{what}\n{}", usage()),
    }
}

fn parse_u32(value: &str, label: &str) -> Result<u32, Error> {
    let parsed = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map(|digits| u32::from_str_radix(digits, 16))
        .unwrap_or_else(|| value.parse::<u32>());
    parsed.map_err(|_| {
        bad_usage(&format!(
            "{label} must be a 32-bit decimal or 0x hexadecimal value"
        ))
    })
}

fn parse_board_selection(value: &str) -> Result<(BoardFamily, String), Error> {
    let (family, revision) = value
        .split_once('@')
        .ok_or_else(|| bad_usage("BOARD must be t114@REVISION or v4@REVISION"))?;
    let family = match family {
        "t114" => BoardFamily::T114,
        "v4" | "heltec-v4" => BoardFamily::HeltecV4,
        other => return Err(bad_usage(&format!("unknown board family {other}"))),
    };
    if revision.trim().is_empty() {
        return Err(bad_usage("BOARD revision cannot be empty"));
    }
    Ok((family, revision.to_string()))
}
