//! Serial settings and the wake sequence for sleeping host links.

use std::time::Duration;

/// How to rouse firmware whose host link sleeps, before sending it a command.
///
/// A UART wake may lose the bytes behind the one that triggered it, and a truncated
/// command desynchronises the parser. The host sends a run of
/// [`WAKE_BYTE`](selvage::WAKE_BYTE), lets the link settle, then writes the command;
/// firmware discards wake bytes at a frame boundary. USB builds leave it `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WakeSequence {
    /// Bytes sent to rouse the link. Long enough that losing the first few does not matter.
    pub preamble: Vec<u8>,
    /// How long to wait after the preamble before writing the command.
    pub settle: Duration,
}

impl Default for WakeSequence {
    fn default() -> Self {
        Self {
            preamble: vec![crate::WAKE_BYTE; 8],
            settle: Duration::from_millis(10),
        }
    }
}

/// Runtime settings for a direct-PHY serial link.
#[derive(Clone, Debug)]
pub struct DirectPhySerialConfig {
    pub baud_rate: u32,
    /// Whether to assert DTR while the serial handle is open.
    ///
    /// nRF USB CDC requires this. ESP32-S3 native USB does not, and keeping it
    /// deasserted avoids a control-line transition when the handle closes.
    pub dtr: bool,
    /// Whether to assert RTS while the serial handle is open.
    ///
    /// This remains false by default because RTS enters reset/boot on ESP32 boards.
    pub rts: bool,
    pub open_settle: Duration,
    pub online_timeout: Duration,
    pub transmit_timeout: Duration,
    pub tx_queue: usize,
    pub rx_queue: usize,
    /// Wake handling for a sleeping host link. `None` (the default) writes commands directly,
    /// which is correct for the USB personality and for any link that stays awake.
    pub wake: Option<WakeSequence>,
}

impl Default for DirectPhySerialConfig {
    fn default() -> Self {
        Self {
            baud_rate: 115_200,
            dtr: true,
            rts: false,
            open_settle: Duration::from_millis(800),
            online_timeout: Duration::from_secs(3),
            transmit_timeout: Duration::from_secs(5),
            tx_queue: 32,
            rx_queue: 32,
            wake: None,
        }
    }
}

impl DirectPhySerialConfig {
    /// Settings for firmware built with the low-power UART personality, whose host link
    /// sleeps between commands.
    pub fn low_power_uart() -> Self {
        Self {
            wake: Some(WakeSequence::default()),
            ..Self::default()
        }
    }
}
