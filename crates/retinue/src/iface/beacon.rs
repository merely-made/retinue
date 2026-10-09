//! The amateur-radio station ID beacon shared by the KISS TNC and RNode carriers
//! (`KISSInterface.py` 262-366; `RNodeInterface.py` 711-744, 1145-1149).
//!
//! The beacon is due `interval` after the first transmission that was not itself a beacon,
//! and sending it disarms the timer, so an idle channel never repeats it. Unlike RNS's KISS
//! carrier, which compares escaped, unpadded bytes and so re-arms on its own beacon, the
//! comparison here is against the exact payload this beacon puts on the air.

use alloc::vec::Vec;
use core::time::Duration;

/// RNS's KISS carrier zero-pads the callsign to this length (`KISSInterface.py` 361-365).
pub const KISS_MIN_LEN: usize = 15;
/// The longest callsign an RNode carrier accepts (`RNodeInterface.py` 117, 336-347).
pub const CALLSIGN_MAX_LEN: usize = 32;

/// A station ID beacon. The caller owns the clock and passes milliseconds since any epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Beacon {
    payload: Vec<u8>,
    interval_ms: u64,
    due_ms: Option<u64>,
}

impl Beacon {
    /// A KISS TNC beacon: the callsign zero-padded to [`KISS_MIN_LEN`].
    pub fn kiss(callsign: &[u8], interval: Duration) -> Self {
        let mut payload = callsign.to_vec();
        payload.resize(payload.len().max(KISS_MIN_LEN), 0);
        Self::with_payload(payload, interval)
    }

    /// An RNode beacon: the callsign as is. `None` past [`CALLSIGN_MAX_LEN`] bytes, which RNS
    /// refuses as a configuration error.
    pub fn rnode(callsign: &[u8], interval: Duration) -> Option<Self> {
        (callsign.len() <= CALLSIGN_MAX_LEN)
            .then(|| Self::with_payload(callsign.to_vec(), interval))
    }

    fn with_payload(payload: Vec<u8>, interval: Duration) -> Self {
        Self {
            payload,
            interval_ms: u64::try_from(interval.as_millis()).unwrap_or(u64::MAX),
            due_ms: None,
        }
    }

    /// The exact bytes this beacon transmits as one data frame.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Record a transmitted data frame. Anything but the beacon arms the timer if idle.
    pub fn on_tx(&mut self, frame: &[u8], now_ms: u64) {
        if frame == self.payload.as_slice() {
            self.due_ms = None;
        } else {
            self.due_ms
                .get_or_insert(now_ms.saturating_add(self.interval_ms));
        }
    }

    /// When the beacon falls due, if armed.
    pub fn due_at_ms(&self) -> Option<u64> {
        self.due_ms
    }

    /// The payload to send now, disarming the timer, or `None` if not yet due.
    pub fn take_due(&mut self, now_ms: u64) -> Option<Vec<u8>> {
        let due = self.due_ms?;
        (now_ms > due).then(|| {
            self.due_ms = None;
            self.payload.clone()
        })
    }

    /// Re-arm a beacon taken but not sent, as RNS keeps it due while the radio is offline
    /// (`RNodeInterface.py` 1145-1149). Earlier traffic's deadline wins.
    pub fn retry_at(&mut self, at_ms: u64) {
        self.due_ms.get_or_insert(at_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kiss_pads_and_rnode_does_not() {
        let kiss = Beacon::kiss(b"N0CALL", Duration::from_secs(600));
        assert_eq!(kiss.payload(), b"N0CALL\0\0\0\0\0\0\0\0\0");
        let rnode = Beacon::rnode(b"N0CALL", Duration::from_secs(600)).unwrap();
        assert_eq!(rnode.payload(), b"N0CALL");
        assert!(Beacon::rnode(&[b'A'; 33], Duration::from_secs(1)).is_none());
    }

    #[test]
    fn fires_once_after_traffic_and_never_on_an_idle_channel() {
        let mut beacon = Beacon::kiss(b"N0CALL", Duration::from_secs(5));
        assert_eq!(beacon.take_due(1_000_000), None, "idle from the start");
        beacon.on_tx(b"packet", 1_000);
        beacon.on_tx(b"later packet", 3_000);
        assert_eq!(beacon.due_at_ms(), Some(6_000), "the first packet arms it");
        assert_eq!(beacon.take_due(6_000), None);
        let sent = beacon.take_due(6_001).expect("due");
        beacon.on_tx(&sent, 6_001);
        assert_eq!(
            beacon.due_at_ms(),
            None,
            "its own transmission does not re-arm it"
        );
        assert_eq!(beacon.take_due(60_000), None);
    }

    #[test]
    fn a_beacon_the_radio_could_not_send_is_retried() {
        let mut beacon = Beacon::rnode(b"N0CALL", Duration::from_secs(5)).unwrap();
        beacon.on_tx(b"packet", 0);
        assert!(beacon.take_due(5_001).is_some());
        beacon.retry_at(6_000);
        assert_eq!(beacon.take_due(5_500), None);
        assert_eq!(beacon.take_due(6_001).as_deref(), Some(&b"N0CALL"[..]));
    }
}
