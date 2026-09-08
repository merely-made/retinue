//! Radio-owner helpers for bounded observation emission.
//!
//! This module knows nothing about a driver, clocks, storage, or transport.  An
//! owner supplies its monotonic uptime at a confirmed hardware edge.  A failed
//! record or exhausted profile/work registry disables observation only; it never
//! changes the radio operation that was being observed.

use super::{
    ObservationKind, RefusalReason, RequestKind, StopReason, TxOutcome,
    recorder::{ObservationRecorder, RecordError},
};
use selvage::PhyProfile;

/// The fixed number of source events retained by the first board-side owner.
pub const OWNER_RECORD_CAPACITY: usize = 32;
/// The fixed number of exact PHY definitions available during one boot.
pub const OWNER_PROFILE_CAPACITY: usize = 16;
/// Local reason bytes for [`ObservationKind::ContinuityLost`].
pub const CONTINUITY_RETUNE: u8 = 1;
pub const CONTINUITY_TRANSMIT: u8 = 2;
pub const CONTINUITY_SETTINGS: u8 = 3;
pub const CONTINUITY_CAD: u8 = 4;

/// Why this owner stopped recording.  The radio remains usable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerObservationFault {
    Record(RecordError),
    ProfileRegistryFull,
    WorkIdExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveInterval {
    Listening,
    Transmitting,
}

/// Per-boot observation state owned beside a radio owner.
pub struct OwnerObservations {
    recorder: ObservationRecorder<OWNER_RECORD_CAPACITY>,
    profiles: [Option<PhyProfile>; OWNER_PROFILE_CAPACITY],
    profile_count: u8,
    next_work: Option<u32>,
    active: Option<ActiveInterval>,
    disabled: Option<OwnerObservationFault>,
}

impl OwnerObservations {
    pub const fn new(boot_id: u64) -> Result<Self, super::recorder::RecorderInitError> {
        let recorder = match ObservationRecorder::new(boot_id) {
            Ok(recorder) => recorder,
            Err(error) => return Err(error),
        };
        Ok(Self {
            recorder,
            profiles: [None; OWNER_PROFILE_CAPACITY],
            profile_count: 0,
            next_work: Some(1),
            active: None,
            disabled: None,
        })
    }

    /// The read-only RAM recorder.  Drains must remain short-lived at the caller.
    pub const fn recorder(&self) -> &ObservationRecorder<OWNER_RECORD_CAPACITY> {
        &self.recorder
    }

    /// The exact PHY definition assigned this boot-local id.
    pub fn profile(&self, id: u8) -> Option<PhyProfile> {
        id.checked_sub(1)
            .and_then(|index| self.profiles.get(usize::from(index)))
            .copied()
            .flatten()
    }

    pub const fn profile_count(&self) -> u8 {
        self.profile_count
    }
    pub const fn disabled(&self) -> bool {
        self.disabled.is_some()
    }
    pub const fn fault(&self) -> Option<OwnerObservationFault> {
        self.disabled
    }

    /// Allocate a correlation id for one bounded request.  IDs never wrap or
    /// reuse within a boot.
    pub fn next_work(&mut self) -> Option<u32> {
        if self.disabled() {
            return None;
        }
        let work = match self.next_work {
            Some(work) => work,
            None => {
                self.disable(OwnerObservationFault::WorkIdExhausted);
                return None;
            }
        };
        self.next_work = work.checked_add(1);
        Some(work)
    }

    pub fn listening_started(&mut self, uptime_ms: u64, assignment: u16, profile: PhyProfile) {
        if self.active.is_some() || self.disabled() {
            return;
        }
        let Some(profile) = self.profile_id(profile) else {
            return;
        };
        self.record(
            uptime_ms,
            ObservationKind::ListeningStarted {
                assignment,
                profile,
            },
        );
        if !self.disabled() {
            self.active = Some(ActiveInterval::Listening);
        }
    }

    /// Record a successful driver standby edge that closes the listening
    /// interval. Callers without that witness must invalidate instead.
    pub fn listening_stopped(&mut self, uptime_ms: u64, assignment: u16) {
        if self.active != Some(ActiveInterval::Listening) || self.disabled() {
            return;
        }
        self.record(
            uptime_ms,
            ObservationKind::ListeningStopped {
                assignment,
                reason: StopReason::Completed,
            },
        );
        if !self.disabled() {
            self.active = None;
        }
    }

    pub fn rx_captured(
        &mut self,
        uptime_ms: u64,
        profile: PhyProfile,
        length: usize,
        rssi_dbm: i16,
        snr_db: i16,
    ) {
        let Some(profile) = self.profile_id(profile) else {
            return;
        };
        self.record(
            uptime_ms,
            ObservationKind::RxCaptured {
                profile,
                length: length.min(u16::MAX as usize) as u16,
                rssi_dbm,
                snr_tenths_db: snr_db.saturating_mul(10),
                capture_tag: 0,
            },
        );
    }

    pub fn rx_damaged(&mut self, uptime_ms: u64, profile: PhyProfile) {
        let Some(profile) = self.profile_id(profile) else {
            return;
        };
        self.record(uptime_ms, ObservationKind::RxDamaged { profile });
    }

    pub fn tx_started(&mut self, uptime_ms: u64, profile: PhyProfile, length: usize, work: u32) {
        let Some(profile) = self.profile_id(profile) else {
            return;
        };
        self.record(
            uptime_ms,
            ObservationKind::TxStarted {
                profile,
                length: length.min(u16::MAX as usize) as u16,
                work,
            },
        );
        if !self.disabled() {
            self.active = Some(ActiveInterval::Transmitting);
        }
    }

    pub fn tx_finished(&mut self, uptime_ms: u64, work: u32) {
        if self.active != Some(ActiveInterval::Transmitting) {
            return;
        }
        self.record(
            uptime_ms,
            ObservationKind::TxFinished {
                work,
                outcome: TxOutcome::Sent,
            },
        );
        if !self.disabled() {
            self.active = None;
        }
    }

    pub fn refused(
        &mut self,
        uptime_ms: u64,
        request: RequestKind,
        reason: RefusalReason,
        work: u32,
    ) {
        self.record(
            uptime_ms,
            ObservationKind::WorkRefused {
                request,
                reason,
                work,
            },
        );
    }

    /// Mark an active interval uncertain when a following operation might have
    /// stopped or retuned hardware without a successful stop witness. `reason`
    /// is bounded local diagnostic context, not a claimed transition.
    pub fn invalidate_hardware(&mut self, uptime_ms: u64, reason: u8) {
        if self.active.is_none() || self.disabled() {
            return;
        }
        self.record(uptime_ms, ObservationKind::ContinuityLost { cause: reason });
        self.active = None;
    }

    fn profile_id(&mut self, profile: PhyProfile) -> Option<u8> {
        if self.disabled() {
            return None;
        }
        for index in 0..usize::from(self.profile_count) {
            if self.profiles[index] == Some(profile) {
                return Some(index as u8 + 1);
            }
        }
        if usize::from(self.profile_count) == OWNER_PROFILE_CAPACITY {
            self.disable(OwnerObservationFault::ProfileRegistryFull);
            return None;
        }
        let index = usize::from(self.profile_count);
        self.profiles[index] = Some(profile);
        self.profile_count += 1;
        Some(index as u8 + 1)
    }

    fn record(&mut self, uptime_ms: u64, kind: ObservationKind) {
        if self.disabled() {
            return;
        }
        if let Err(error) = self.recorder.record(uptime_ms, kind) {
            self.disable(OwnerObservationFault::Record(error));
        }
    }

    fn disable(&mut self, fault: OwnerObservationFault) {
        self.disabled = Some(fault);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(frequency_hz: u32) -> PhyProfile {
        PhyProfile::meshtastic_long_fast(frequency_hz)
    }

    #[test]
    fn profiles_are_exact_deduplicated_and_boot_local() {
        let mut owner = OwnerObservations::new(7).unwrap();
        owner.listening_started(1, 0, profile(915_000_000));
        owner.rx_damaged(2, profile(915_000_000));
        assert_eq!(owner.profile_count(), 1);
        assert_eq!(owner.profile(1), Some(profile(915_000_000)));
        assert_eq!(owner.profile(0), None);
    }

    #[test]
    fn registry_overflow_disables_without_reusing_ids() {
        let mut owner = OwnerObservations::new(1).unwrap();
        for index in 0..OWNER_PROFILE_CAPACITY {
            owner.rx_damaged(index as u64, profile(900_000_000 + index as u32));
        }
        owner.rx_damaged(20, profile(950_000_000));
        assert_eq!(
            owner.fault(),
            Some(OwnerObservationFault::ProfileRegistryFull)
        );
        assert_eq!(owner.profile_count(), OWNER_PROFILE_CAPACITY as u8);
    }

    #[test]
    fn confirmed_start_is_not_repeated_and_invalidation_is_unknown() {
        let mut owner = OwnerObservations::new(1).unwrap();
        owner.listening_started(1, 0, profile(915_000_000));
        owner.listening_started(2, 0, profile(915_000_000));
        owner.invalidate_hardware(3, 9);
        let mut drain = owner
            .recorder()
            .drain(super::super::recorder::CursorRequest {
                boot_id: 1,
                after_sequence: 0,
                max_records: 4,
            })
            .unwrap();
        assert!(matches!(
            drain.next(),
            Some(super::super::ObservationRecord::Event(_))
        ));
        assert!(matches!(
            drain.next(),
            Some(super::super::ObservationRecord::Event(
                super::super::ObservationEvent {
                    kind: ObservationKind::ContinuityLost { .. },
                    ..
                }
            ))
        ));
        assert_eq!(drain.next(), None);
    }

    #[test]
    fn confirmed_standby_closes_a_matching_listening_interval() {
        let mut owner = OwnerObservations::new(1).unwrap();
        owner.listening_started(1, 0, profile(915_000_000));
        owner.listening_stopped(2, 0);
        let mut drain = owner
            .recorder()
            .drain(super::super::recorder::CursorRequest {
                boot_id: 1,
                after_sequence: 0,
                max_records: 4,
            })
            .unwrap();
        assert!(matches!(
            drain.next(),
            Some(super::super::ObservationRecord::Event(
                super::super::ObservationEvent {
                    kind: ObservationKind::ListeningStarted { assignment: 0, .. },
                    ..
                }
            ))
        ));
        assert!(matches!(
            drain.next(),
            Some(super::super::ObservationRecord::Event(
                super::super::ObservationEvent {
                    kind: ObservationKind::ListeningStopped {
                        assignment: 0,
                        reason: StopReason::Completed,
                    },
                    ..
                }
            ))
        ));
        assert_eq!(drain.next(), None);
    }

    #[test]
    fn transmit_timeout_leaves_a_continuity_break_without_a_false_finish() {
        let mut owner = OwnerObservations::new(1).unwrap();
        owner.tx_started(1, profile(915_000_000), 4, 1);
        owner.invalidate_hardware(2, CONTINUITY_TRANSMIT);
        let mut drain = owner
            .recorder()
            .drain(super::super::recorder::CursorRequest {
                boot_id: 1,
                after_sequence: 0,
                max_records: 4,
            })
            .unwrap();
        assert!(matches!(
            drain.next(),
            Some(super::super::ObservationRecord::Event(
                super::super::ObservationEvent {
                    kind: ObservationKind::TxStarted { .. },
                    ..
                }
            ))
        ));
        assert!(matches!(
            drain.next(),
            Some(super::super::ObservationRecord::Event(
                super::super::ObservationEvent {
                    kind: ObservationKind::ContinuityLost { .. },
                    ..
                }
            ))
        ));
        assert_eq!(drain.next(), None);
    }

    #[test]
    fn finished_transmit_is_closed_and_later_invalidation_is_silent() {
        let mut owner = OwnerObservations::new(1).unwrap();
        owner.tx_started(1, profile(915_000_000), 4, 1);
        owner.tx_finished(2, 1);
        owner.invalidate_hardware(3, CONTINUITY_TRANSMIT);
        assert_eq!(owner.recorder().bounds().newest_available, Some(2));
    }

    #[test]
    fn work_ids_start_nonzero_and_exhaust_without_wrapping() {
        let mut owner = OwnerObservations::new(1).unwrap();
        assert_eq!(owner.next_work(), Some(1));
        owner.next_work = Some(u32::MAX);
        assert_eq!(owner.next_work(), Some(u32::MAX));
        assert_eq!(owner.next_work(), None);
        assert_eq!(owner.fault(), Some(OwnerObservationFault::WorkIdExhausted));
    }
}
