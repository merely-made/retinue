//! Pump settings and lifecycle status.

use std::time::Duration;

/// Runtime policy for the serial pump. Durations are settings because USB devices and
/// half-duplex radios need different settle and turnaround margins.
#[derive(Clone, Debug)]
pub struct SerialPumpConfig {
    /// Serial line rate used by RNode firmware.
    pub baud_rate: u32,
    /// Time allowed for a board reset triggered by opening its USB serial port.
    pub open_settle: Duration,
    /// How often to repeat the RNode detect/configuration conversation until it is online.
    pub init_retry: Duration,
    /// Receive window left after each packet's calculated airtime.
    pub turnaround: Duration,
    /// Retry delay when the modem reports its half-duplex queue busy.
    pub busy_retry: Duration,
    /// Maximum number of frames waiting for the radio.
    pub tx_queue: usize,
    /// Maximum number of received frames waiting for the protocol consumer.
    pub rx_queue: usize,
    /// A half-read frame is discarded after this long without bytes (`RNodeInterface.py`
    /// 1137-1143).
    pub idle_reset: Duration,
    /// A flow-control lock the device never releases is lifted after this.
    pub flow_unlock: Duration,
    /// Delay before a supervised link reopens a failed port (`RNodeInterface.py` 1175-1187).
    pub reconnect: Duration,
}

impl Default for SerialPumpConfig {
    fn default() -> Self {
        Self {
            baud_rate: 115_200,
            open_settle: Duration::from_secs(3),
            init_retry: Duration::from_secs(6),
            turnaround: Duration::from_millis(180),
            busy_retry: Duration::from_millis(50),
            tx_queue: 32,
            rx_queue: 32,
            idle_reset: Duration::from_millis(100),
            flow_unlock: Duration::from_secs(5),
            reconnect: Duration::from_secs(5),
        }
    }
}

/// Observable lifecycle of the pump.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PumpStatus {
    Settling,
    Initializing,
    Online { firmware: Option<(u8, u8)> },
    Fault(String),
    Stopped,
}
