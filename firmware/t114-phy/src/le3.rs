//! The bounded LE3 bench registry.
//!
//! This is a consumer of `radio_hand::profiles`, not a second scheduler model.
//! It fixes one receipt-shaped registry: two CAD groups and three exact capture
//! profiles, including the otherwise-identical MeshCore and Meshtastic sync words.

use radio_hand::profiles::{
    CadParameters, DetectionProfile, DetectionProfileId, ProfileError, ReceiveProfile,
    ReceiveProfileId, ScanPlan,
};
use selvage::{MESHCORE_SYNC_WORD, PhyProfile};

pub const LONG_FAST_DETECTION: DetectionProfileId = DetectionProfileId(1);
pub const FAST_DETECTION: DetectionProfileId = DetectionProfileId(2);
pub const MESHCORE_CAPTURE: ReceiveProfileId = ReceiveProfileId(1);
pub const MESHTASTIC_CAPTURE: ReceiveProfileId = ReceiveProfileId(2);
pub const FAST_CAPTURE: ReceiveProfileId = ReceiveProfileId(3);

pub const CYCLE_BUDGET_MS: u64 = 3_000;

pub type BenchPlan = ScanPlan<2, 3>;

/// Build the one registry exercised by the T114 receipt.
pub fn plan(frequency_hz: u32) -> Result<BenchPlan, ProfileError> {
    let meshtastic = PhyProfile::meshtastic_long_fast(frequency_hz);
    let mut meshcore = meshtastic;
    meshcore.sync_word = MESHCORE_SYNC_WORD;
    let mut fast = meshtastic;
    fast.spreading_factor = 9;

    let mut plan = BenchPlan::new();
    plan.register_detection(DetectionProfile::from_phy(
        LONG_FAST_DETECTION,
        meshtastic,
        CadParameters {
            symbols: 8,
            dwell_ms: 100,
        },
    ))?;
    plan.register_detection(DetectionProfile::from_phy(
        FAST_DETECTION,
        fast,
        CadParameters {
            symbols: 8,
            dwell_ms: 40,
        },
    ))?;
    plan.register_receive(ReceiveProfile::from_phy(
        MESHCORE_CAPTURE,
        LONG_FAST_DETECTION,
        meshcore,
        900,
    ))?;
    plan.register_receive(ReceiveProfile::from_phy(
        MESHTASTIC_CAPTURE,
        LONG_FAST_DETECTION,
        meshtastic,
        900,
    ))?;
    plan.register_receive(ReceiveProfile::from_phy(
        FAST_CAPTURE,
        FAST_DETECTION,
        fast,
        900,
    ))?;
    Ok(plan)
}

/// Complete driver profile for one CAD group.
pub fn detection_phy(plan: &BenchPlan, id: DetectionProfileId) -> Option<PhyProfile> {
    let detection = plan.detection(id)?;
    let mut profile = PhyProfile::meshtastic_long_fast(detection.frequency_hz);
    profile.bandwidth_hz = detection.bandwidth_hz;
    profile.spreading_factor = detection.spreading_factor;
    Some(profile)
}

/// Complete driver profile for one exact receive window.
pub fn receive_phy(plan: &BenchPlan, id: ReceiveProfileId) -> Option<(ReceiveProfile, PhyProfile)> {
    let receive = plan.receive(id)?;
    let phy = plan.receive_phy(receive, 7)?;
    Some((receive, phy))
}

/// This registry fits its declared cycle, while the immediately smaller budget
/// is refused. The second fact is printed by the board so the receipt proves the
/// runtime consumer called the guard rather than relying only on a host unit test.
pub fn budget_facts(plan: &BenchPlan) -> (bool, bool) {
    (
        plan.require_cycle_budget(CYCLE_BUDGET_MS).is_ok(),
        plan.require_cycle_budget(plan.cycle_dwell_ms().saturating_sub(1))
            .is_err(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use radio_hand::profiles::ScanStep;
    use selvage::MESHTASTIC_SYNC_WORD;

    #[test]
    fn registry_has_two_cad_groups_and_separate_sync_word_windows() {
        let mut plan = plan(906_875_000).unwrap();
        assert_eq!(plan.detection_count(), 2);
        assert_eq!(plan.receive_count(), 3);
        assert_eq!(plan.cycle_steps(), 5);
        assert_eq!(plan.cycle_dwell_ms(), 2_840);
        assert_eq!(budget_facts(&plan), (true, true));

        assert!(matches!(plan.next_step(), Some(ScanStep::Detect(_))));
        assert!(matches!(
            plan.next_step(),
            Some(ScanStep::Capture(ReceiveProfile {
                sync_word: MESHCORE_SYNC_WORD,
                ..
            }))
        ));
        assert!(matches!(
            plan.next_step(),
            Some(ScanStep::Capture(ReceiveProfile {
                sync_word: MESHTASTIC_SYNC_WORD,
                ..
            }))
        ));
    }
}
