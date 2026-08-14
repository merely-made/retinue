//! The radio shapes a resident listener schedules.
//!
//! A LoRa CAD observation says that a configured frequency, spreading factor,
//! and bandwidth saw preamble activity. It does not identify a sync word or
//! make one packet configuration able to decode another. This module keeps
//! that distinction in the types the future radio executive will schedule:
//! one [`DetectionProfile`] may be shared, while every [`ReceiveProfile`] gets
//! its own capture dwell.
//!
//! This is deliberately a radio-free model. It can be proven on the host and
//! provides the exact schedule inputs for the firmware executive without
//! pretending a host test is an RF capture receipt.

use heapless::Vec;
use selvage::PhyProfile;

/// Stable identifier for one CAD configuration in a listener registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct DetectionProfileId(pub u8);

/// Stable identifier for one exact packet-capture configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ReceiveProfileId(pub u8);

/// CAD settings that do not form part of a packet capture configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CadParameters {
    /// Number of LoRa symbols the chip observes during CAD.
    pub symbols: u8,
    /// Measured or configured duration consumed by that observation.
    pub dwell_ms: u32,
}

/// A radio configuration able to detect LoRa preamble activity.
///
/// CAD identifies this modulation shape only. In particular it does not carry
/// a sync word: two protocols can share a detection observation and still need
/// separate exact capture slots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DetectionProfile {
    pub id: DetectionProfileId,
    pub frequency_hz: u32,
    pub bandwidth_hz: u32,
    pub spreading_factor: u8,
    pub cad: CadParameters,
}

impl DetectionProfile {
    /// Take the CAD-relevant fields from an otherwise complete PHY profile.
    pub const fn from_phy(id: DetectionProfileId, phy: PhyProfile, cad: CadParameters) -> Self {
        Self {
            id,
            frequency_hz: phy.frequency_hz,
            bandwidth_hz: phy.bandwidth_hz,
            spreading_factor: phy.spreading_factor,
            cad,
        }
    }
}

/// One exact packet configuration a receiver can capture after detection.
///
/// The referenced [`DetectionProfile`] supplies frequency, SF, and bandwidth.
/// Every remaining receive-side packet setting remains here so hardware cannot
/// quietly use one sync word or packet format to claim coverage of another.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceiveProfile {
    pub id: ReceiveProfileId,
    pub detection: DetectionProfileId,
    pub coding_rate_denominator: u8,
    pub preamble_symbols: u16,
    pub sync_word: u8,
    pub explicit_header: bool,
    pub crc: bool,
    pub invert_iq: bool,
    /// The capture window this exact configuration gets after its CAD group.
    pub capture_dwell_ms: u32,
}

impl ReceiveProfile {
    /// Take the exact receive-side packet fields from a complete PHY profile.
    pub const fn from_phy(
        id: ReceiveProfileId,
        detection: DetectionProfileId,
        phy: PhyProfile,
        capture_dwell_ms: u32,
    ) -> Self {
        Self {
            id,
            detection,
            coding_rate_denominator: phy.coding_rate_denominator,
            preamble_symbols: phy.preamble_symbols,
            sync_word: phy.sync_word,
            explicit_header: phy.explicit_header,
            crc: phy.crc,
            invert_iq: phy.invert_iq,
            capture_dwell_ms,
        }
    }
}

/// One non-overlapping operation the listener must run on its single radio.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanStep {
    Detect(DetectionProfile),
    Capture(ReceiveProfile),
}

impl ScanStep {
    /// The time budget the operation consumes from the listener schedule.
    pub const fn dwell_ms(self) -> u32 {
        match self {
            Self::Detect(profile) => profile.cad.dwell_ms,
            Self::Capture(profile) => profile.capture_dwell_ms,
        }
    }
}

/// A malformed or overfull listener registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileError {
    DuplicateDetection(DetectionProfileId),
    DuplicateReceive(ReceiveProfileId),
    UnknownDetection(DetectionProfileId),
    EmptyCad(DetectionProfileId),
    EmptyCapture(ReceiveProfileId),
    DetectionCapacity,
    ReceiveCapacity,
}

/// Round-robin detection and exact-capture schedule for one radio.
///
/// A detection step is emitted once for each detection group. All receive
/// profiles in that group then get separate capture steps, even if their only
/// difference is sync word. A board with no matching capture step cannot claim
/// it was listening for that profile.
pub struct ScanPlan<const DETECTIONS: usize, const RECEIVES: usize> {
    detections: Vec<DetectionProfile, DETECTIONS>,
    receives: Vec<ReceiveProfile, RECEIVES>,
    next_detection: usize,
    capture_after: Option<(usize, usize)>,
}

impl<const DETECTIONS: usize, const RECEIVES: usize> Default for ScanPlan<DETECTIONS, RECEIVES> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const DETECTIONS: usize, const RECEIVES: usize> ScanPlan<DETECTIONS, RECEIVES> {
    pub const fn new() -> Self {
        Self {
            detections: Vec::new(),
            receives: Vec::new(),
            next_detection: 0,
            capture_after: None,
        }
    }

    pub fn register_detection(&mut self, profile: DetectionProfile) -> Result<(), ProfileError> {
        if profile.cad.symbols == 0 || profile.cad.dwell_ms == 0 {
            return Err(ProfileError::EmptyCad(profile.id));
        }
        if self.detections.iter().any(|known| known.id == profile.id) {
            return Err(ProfileError::DuplicateDetection(profile.id));
        }
        self.detections
            .push(profile)
            .map_err(|_| ProfileError::DetectionCapacity)
    }

    pub fn register_receive(&mut self, profile: ReceiveProfile) -> Result<(), ProfileError> {
        if profile.capture_dwell_ms == 0 {
            return Err(ProfileError::EmptyCapture(profile.id));
        }
        if !self
            .detections
            .iter()
            .any(|known| known.id == profile.detection)
        {
            return Err(ProfileError::UnknownDetection(profile.detection));
        }
        if self.receives.iter().any(|known| known.id == profile.id) {
            return Err(ProfileError::DuplicateReceive(profile.id));
        }
        self.receives
            .push(profile)
            .map_err(|_| ProfileError::ReceiveCapacity)
    }

    /// The next CAD or exact capture operation for the single radio.
    ///
    /// An empty registry has no schedule rather than a synthetic idle profile.
    pub fn next_step(&mut self) -> Option<ScanStep> {
        loop {
            if self.detections.is_empty() {
                return None;
            }
            if let Some((detection_index, receive_start)) = self.capture_after {
                let detection = self.detections[detection_index];
                if let Some((receive_index, profile)) = self
                    .receives
                    .iter()
                    .enumerate()
                    .skip(receive_start)
                    .find(|(_, profile)| profile.detection == detection.id)
                {
                    self.capture_after = Some((detection_index, receive_index + 1));
                    return Some(ScanStep::Capture(*profile));
                }
                self.capture_after = None;
                self.next_detection = (detection_index + 1) % self.detections.len();
                continue;
            }

            let detection_index = self.next_detection;
            self.capture_after = Some((detection_index, 0));
            return Some(ScanStep::Detect(self.detections[detection_index]));
        }
    }

    /// Count actual operations in one complete scan cycle, not just CAD groups.
    pub fn cycle_steps(&self) -> usize {
        self.detections.len() + self.receives.len()
    }

    /// Time a complete scan cycle allocates to CAD and exact capture dwell.
    pub fn cycle_dwell_ms(&self) -> u64 {
        self.detections
            .iter()
            .map(|profile| u64::from(profile.cad.dwell_ms))
            .sum::<u64>()
            + self
                .receives
                .iter()
                .map(|profile| u64::from(profile.capture_dwell_ms))
                .sum::<u64>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use selvage::{MESHCORE_SYNC_WORD, MESHTASTIC_SYNC_WORD};

    const DETECT: DetectionProfileId = DetectionProfileId(1);

    fn long_fast(sync_word: u8) -> PhyProfile {
        let mut profile = PhyProfile::meshtastic_long_fast(906_875_000);
        profile.sync_word = sync_word;
        profile
    }

    fn cad() -> CadParameters {
        CadParameters {
            symbols: 8,
            dwell_ms: 12,
        }
    }

    #[test]
    fn shared_detection_still_has_one_capture_slot_per_sync_word() {
        let meshtastic = long_fast(MESHTASTIC_SYNC_WORD);
        let meshcore = long_fast(MESHCORE_SYNC_WORD);
        let mut plan: ScanPlan<2, 4> = ScanPlan::new();
        plan.register_detection(DetectionProfile::from_phy(DETECT, meshtastic, cad()))
            .unwrap();
        plan.register_receive(ReceiveProfile::from_phy(
            ReceiveProfileId(1),
            DETECT,
            meshtastic,
            40,
        ))
        .unwrap();
        plan.register_receive(ReceiveProfile::from_phy(
            ReceiveProfileId(2),
            DETECT,
            meshcore,
            55,
        ))
        .unwrap();

        assert_eq!(
            plan.cycle_steps(),
            3,
            "one CAD observation plus two captures"
        );
        assert_eq!(plan.cycle_dwell_ms(), 107);
        assert!(matches!(
            plan.next_step(),
            Some(ScanStep::Detect(DetectionProfile { id: DETECT, .. }))
        ));
        assert!(matches!(
            plan.next_step(),
            Some(ScanStep::Capture(ReceiveProfile {
                sync_word: MESHTASTIC_SYNC_WORD,
                ..
            }))
        ));
        assert!(matches!(
            plan.next_step(),
            Some(ScanStep::Capture(ReceiveProfile {
                sync_word: MESHCORE_SYNC_WORD,
                ..
            }))
        ));
        assert!(matches!(plan.next_step(), Some(ScanStep::Detect(_))));
    }

    #[test]
    fn registry_refuses_a_capture_without_an_exact_detection_group() {
        let mut plan: ScanPlan<1, 1> = ScanPlan::new();
        let result = plan.register_receive(ReceiveProfile::from_phy(
            ReceiveProfileId(1),
            DETECT,
            long_fast(MESHTASTIC_SYNC_WORD),
            10,
        ));
        assert_eq!(result, Err(ProfileError::UnknownDetection(DETECT)));
    }

    #[test]
    fn separate_detection_groups_do_not_share_a_cad_step() {
        let meshtastic = long_fast(MESHTASTIC_SYNC_WORD);
        let mut other = meshtastic;
        other.spreading_factor = 10;
        let mut plan: ScanPlan<2, 2> = ScanPlan::new();
        plan.register_detection(DetectionProfile::from_phy(DETECT, meshtastic, cad()))
            .unwrap();
        plan.register_detection(DetectionProfile::from_phy(
            DetectionProfileId(2),
            other,
            cad(),
        ))
        .unwrap();
        plan.register_receive(ReceiveProfile::from_phy(
            ReceiveProfileId(1),
            DETECT,
            meshtastic,
            10,
        ))
        .unwrap();
        plan.register_receive(ReceiveProfile::from_phy(
            ReceiveProfileId(2),
            DetectionProfileId(2),
            other,
            10,
        ))
        .unwrap();

        assert!(matches!(
            plan.next_step(),
            Some(ScanStep::Detect(DetectionProfile {
                spreading_factor: 11,
                ..
            }))
        ));
        let _ = plan.next_step();
        assert!(matches!(
            plan.next_step(),
            Some(ScanStep::Detect(DetectionProfile {
                spreading_factor: 10,
                ..
            }))
        ));
    }
}
