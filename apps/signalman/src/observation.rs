//! Bounded in-memory capture and conservative radio availability replay.
//!
//! A bundle describes one source and one immutable exact-profile registry.
//! Collector-supplied identity and carrier labels do not authenticate a board.
//! Raw bytes, host receipt times and source boot/uptime remain separate.
//! UTC mapping and remote authentication are later consumers. The versioned
//! disk container in [`persistence`] preserves this evidence without decoding
//! or rewriting the source records.

use radio_hand::observation::{
    DecodeError, MAX_RECORD_BYTES, ObservationEvent, ObservationGap, ObservationKind,
    ObservationRecord, QuietCause,
};
use std::collections::{BTreeMap, BTreeSet};
pub mod collect;
pub mod persistence;

pub const BUNDLE_VERSION: u8 = 1;
pub const MAX_DEVICE_ID_BYTES: usize = 128;
pub const MAX_LABEL_BYTES: usize = 128;
pub const MAX_PROFILE_DEFINITION_BYTES: usize = 1024;
pub const MAX_PROFILE_ENTRIES: usize = 256;

/// Collection-route evidence, without a board authentication claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CarrierKind {
    LocalUsb,
    Imported,
    Other(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileEntry {
    pub id: u8,
    pub name: String,
    pub version: u32,
    /// Canonical exact PHY definition, including its format identifier.
    /// Equal numeric ids across different bundles are not an equality proof.
    pub definition: Vec<u8>,
}

/// Caller-selected input payload and entry limits. Entry count also bounds
/// replay allocations; byte accounting is not Rust heap accounting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Admission {
    pub max_frames: usize,
    pub max_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    UnsupportedVersion,
    InvalidDevice,
    MetadataTooLarge,
    InvalidProfile,
    DuplicateProfile(u8),
    UnknownProfile(u8),
    TooManyFrames,
    TooManyBytes,
    InvalidRecord(DecodeError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedRecord {
    pub received_unix_ms: u64,
    pub raw: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleEntry {
    Record(CapturedRecord),
    /// A host observation without a fabricated source timestamp or sequence.
    Disconnected {
        received_unix_ms: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationBundle {
    version: u8,
    device: Vec<u8>,
    carrier: CarrierKind,
    carrier_label: String,
    profiles: Vec<ProfileEntry>,
    admission: Admission,
    payload_bytes: usize,
    entries: Vec<BundleEntry>,
}

impl ObservationBundle {
    /// Bounds metadata before copying it. Identity is a caller-supplied local
    /// association, not an authenticated radio identity.
    pub fn new(
        version: u8,
        device: &[u8],
        carrier: CarrierKind,
        carrier_label: &str,
        profiles: &[ProfileEntry],
        admission: Admission,
    ) -> Result<Self, AdmissionError> {
        if version != BUNDLE_VERSION {
            return Err(AdmissionError::UnsupportedVersion);
        }
        if device.is_empty() || device.len() > MAX_DEVICE_ID_BYTES {
            return Err(AdmissionError::InvalidDevice);
        }
        if carrier_label.len() > MAX_LABEL_BYTES || profiles.len() > MAX_PROFILE_ENTRIES {
            return Err(AdmissionError::MetadataTooLarge);
        }
        let mut bytes = device.len() + carrier_label.len() + 2;
        for (index, profile) in profiles.iter().enumerate() {
            if profile.name.len() > MAX_LABEL_BYTES
                || profile.definition.is_empty()
                || profile.definition.len() > MAX_PROFILE_DEFINITION_BYTES
                || profile.version == 0
            {
                return Err(AdmissionError::InvalidProfile);
            }
            if profiles[..index]
                .iter()
                .any(|previous| previous.id == profile.id)
            {
                return Err(AdmissionError::DuplicateProfile(profile.id));
            }
            bytes += profile.name.len() + profile.definition.len() + 5;
        }
        if bytes > admission.max_bytes {
            return Err(AdmissionError::TooManyBytes);
        }
        Ok(Self {
            version,
            device: device.to_vec(),
            carrier,
            carrier_label: carrier_label.into(),
            profiles: profiles.to_vec(),
            admission,
            payload_bytes: bytes,
            entries: Vec::new(),
        })
    }
    pub fn version(&self) -> u8 {
        self.version
    }
    pub fn device(&self) -> &[u8] {
        &self.device
    }
    pub fn carrier(&self) -> CarrierKind {
        self.carrier
    }
    pub fn carrier_label(&self) -> &str {
        &self.carrier_label
    }
    pub fn profiles(&self) -> &[ProfileEntry] {
        &self.profiles
    }
    pub fn entries(&self) -> &[BundleEntry] {
        &self.entries
    }
    pub fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }
    pub fn admission(&self) -> Admission {
        self.admission
    }

    fn admit_size(&self, additional: usize) -> Result<usize, AdmissionError> {
        if self.entries.len() >= self.admission.max_frames {
            return Err(AdmissionError::TooManyFrames);
        }
        self.payload_bytes
            .checked_add(additional)
            .filter(|total| *total <= self.admission.max_bytes)
            .ok_or(AdmissionError::TooManyBytes)
    }

    /// Bounds and validates borrowed input before allocating a copy.
    /// Host civil time may regress; it does not order source radio facts.
    pub fn admit(&mut self, raw: &[u8], received_unix_ms: u64) -> Result<(), AdmissionError> {
        if raw.len() > MAX_RECORD_BYTES {
            return Err(AdmissionError::InvalidRecord(DecodeError::BadLength));
        }
        let total = self.admit_size(raw.len() + 8)?;
        let record = ObservationRecord::decode(raw).map_err(AdmissionError::InvalidRecord)?;
        if let ObservationRecord::Event(event) = record {
            let profile = match event.kind {
                ObservationKind::ListeningStarted { profile, .. }
                | ObservationKind::RxCaptured { profile, .. }
                | ObservationKind::RxDamaged { profile }
                | ObservationKind::TxStarted { profile, .. } => Some(profile),
                _ => None,
            };
            if let Some(profile) = profile
                && !self.profiles.iter().any(|entry| entry.id == profile)
            {
                return Err(AdmissionError::UnknownProfile(profile));
            }
        }
        self.entries.push(BundleEntry::Record(CapturedRecord {
            received_unix_ms,
            raw: raw.to_vec(),
        }));
        self.payload_bytes = total;
        Ok(())
    }
    pub fn disconnect(&mut self, received_unix_ms: u64) -> Result<(), AdmissionError> {
        let total = self.admit_size(8)?;
        self.entries
            .push(BundleEntry::Disconnected { received_unix_ms });
        self.payload_bytes = total;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayError {
    Decode(DecodeError),
    ConflictingSequence,
    NonMonotonicSequence,
    NonMonotonicTime,
    RevisitedBoot,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Complete,
    Incomplete,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncompleteReason {
    Gap,
    Restart,
    Disconnect,
    RepeatedStart,
    MismatchedStop,
    UnmatchedStop,
    UnknownEvent,
    OwnerUncertain,
    ContradictoryCapture,
    EndOfCapture,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Listening {
        assignment: u16,
        profile: Option<u8>,
    },
    Transmit {
        work: u32,
        profile: Option<u8>,
    },
    Quiet {
        cause: QuietCause,
    },
    Sleep,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval {
    pub boot_id: u64,
    pub activity: Activity,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub edge: Edge,
    pub incomplete_reason: Option<IncompleteReason>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GapEvidence {
    pub gap: ObservationGap,
    pub implicit: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Exact profile ids within this bundle's registry, never across registries.
    pub listening_ms: BTreeMap<u8, u128>,
    pub transmit_ms: u128,
    pub quiet_ms: u128,
    pub quiet_by_cause_ms: Vec<(QuietCause, u128)>,
    pub sleep_ms: u128,
    pub missing_records: u128,
    pub incomplete_intervals: usize,
    pub captures: usize,
    pub damaged: usize,
    pub refusals: usize,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timeline {
    pub intervals: Vec<Interval>,
    /// Accepted source events including future kinds/reasons and point observations.
    pub events: Vec<ObservationEvent>,
    pub gaps: Vec<GapEvidence>,
    pub duplicates: usize,
    pub disconnects: usize,
    pub summary: Summary,
}

fn close(
    out: &mut Timeline,
    open: &mut Option<Interval>,
    end: Option<u64>,
    reason: Option<IncompleteReason>,
) {
    if let Some(mut interval) = open.take() {
        interval.end_ms = end;
        interval.edge = if reason.is_none() {
            Edge::Complete
        } else {
            Edge::Incomplete
        };
        interval.incomplete_reason = reason;
        out.intervals.push(interval);
    }
}
fn matches_stop(start: Activity, stop: Activity) -> bool {
    match (start, stop) {
        (Activity::Listening { assignment: a, .. }, Activity::Listening { assignment: b, .. }) => {
            a == b
        }
        (Activity::Transmit { work: a, .. }, Activity::Transmit { work: b, .. }) => a == b,
        (Activity::Quiet { cause: a }, Activity::Quiet { cause: b }) => a == b,
        (Activity::Sleep, Activity::Sleep) => true,
        _ => false,
    }
}

/// Replays one device in collection order. Exact earlier events are harmless
/// re-reads. Conflicting or unseen backwards events are refused. A gap describes
/// unavailable records in this capture, not radio silence.
pub fn replay(bundle: &ObservationBundle) -> Result<Timeline, ReplayError> {
    let mut out = Timeline::default();
    let mut boot = None;
    let mut seen_boots = BTreeSet::new();
    let mut seen_events: BTreeMap<(u64, u64), &[u8]> = BTreeMap::new();
    let mut sequence = 0_u64;
    let mut uptime = None;
    let mut open = None;
    for entry in bundle.entries() {
        let frame = match entry {
            BundleEntry::Disconnected { .. } => {
                close(
                    &mut out,
                    &mut open,
                    None,
                    Some(IncompleteReason::Disconnect),
                );
                out.disconnects += 1;
                continue;
            }
            BundleEntry::Record(frame) => frame,
        };
        let record = ObservationRecord::decode(&frame.raw).map_err(ReplayError::Decode)?;
        let record_boot = match record {
            ObservationRecord::Event(event) => {
                if let Some(previous) = seen_events.get(&(event.boot_id, event.sequence)) {
                    if *previous != frame.raw.as_slice() {
                        return Err(ReplayError::ConflictingSequence);
                    }
                    out.duplicates += 1;
                    continue;
                }
                event.boot_id
            }
            ObservationRecord::Gap(gap) => gap.boot_id,
        };
        if boot != Some(record_boot) {
            if !seen_boots.insert(record_boot) {
                return Err(ReplayError::RevisitedBoot);
            }
            close(&mut out, &mut open, None, Some(IncompleteReason::Restart));
            boot = Some(record_boot);
            sequence = 0;
            uptime = None;
        }
        match record {
            ObservationRecord::Gap(gap) => {
                let last = gap.first_missing + (gap.count - 1);
                if last <= sequence {
                    out.duplicates += 1;
                    continue;
                }
                // A larger overwrite gap can overlap already retained records.
                // Only the still-missing tail contributes to loss.
                let first = sequence + 1;
                if gap.first_missing > first {
                    out.gaps.push(GapEvidence {
                        gap: ObservationGap {
                            boot_id: record_boot,
                            first_missing: first,
                            count: gap.first_missing - first,
                        },
                        implicit: true,
                    });
                }
                let first = gap.first_missing.max(first);
                out.gaps.push(GapEvidence {
                    gap: ObservationGap {
                        boot_id: record_boot,
                        first_missing: first,
                        count: last - first + 1,
                    },
                    implicit: false,
                });
                sequence = last;
                close(&mut out, &mut open, None, Some(IncompleteReason::Gap));
            }
            ObservationRecord::Event(event) => {
                if event.sequence <= sequence {
                    return Err(ReplayError::NonMonotonicSequence);
                }
                if uptime.is_some_and(|previous| event.uptime_ms < previous) {
                    return Err(ReplayError::NonMonotonicTime);
                }
                if event.sequence - sequence > 1 {
                    out.gaps.push(GapEvidence {
                        gap: ObservationGap {
                            boot_id: record_boot,
                            first_missing: sequence + 1,
                            count: event.sequence - sequence - 1,
                        },
                        implicit: true,
                    });
                    close(&mut out, &mut open, None, Some(IncompleteReason::Gap));
                }
                sequence = event.sequence;
                uptime = Some(event.uptime_ms);
                seen_events.insert((event.boot_id, event.sequence), &frame.raw);
                out.events.push(event);
                let start = match event.kind {
                    ObservationKind::ListeningStarted {
                        assignment,
                        profile,
                    } => Some(Activity::Listening {
                        assignment,
                        profile: Some(profile),
                    }),
                    ObservationKind::TxStarted { work, profile, .. } => Some(Activity::Transmit {
                        work,
                        profile: Some(profile),
                    }),
                    ObservationKind::QuietStarted { cause } => Some(Activity::Quiet { cause }),
                    ObservationKind::SleepStarted => Some(Activity::Sleep),
                    _ => None,
                };
                if let Some(activity) = start {
                    close(
                        &mut out,
                        &mut open,
                        Some(event.uptime_ms),
                        Some(IncompleteReason::RepeatedStart),
                    );
                    open = Some(Interval {
                        boot_id: record_boot,
                        activity,
                        start_ms: Some(event.uptime_ms),
                        end_ms: None,
                        edge: Edge::Incomplete,
                        incomplete_reason: Some(IncompleteReason::EndOfCapture),
                    });
                    continue;
                }
                let stop = match event.kind {
                    ObservationKind::ListeningStopped { assignment, .. } => {
                        Some(Activity::Listening {
                            assignment,
                            profile: None,
                        })
                    }
                    ObservationKind::TxFinished { work, .. } => Some(Activity::Transmit {
                        work,
                        profile: None,
                    }),
                    ObservationKind::QuietStopped { cause } => Some(Activity::Quiet { cause }),
                    ObservationKind::SleepStopped { .. } => Some(Activity::Sleep),
                    _ => None,
                };
                if let Some(activity) = stop {
                    if open
                        .as_ref()
                        .is_some_and(|interval| matches_stop(interval.activity, activity))
                    {
                        close(&mut out, &mut open, Some(event.uptime_ms), None);
                    } else {
                        close(
                            &mut out,
                            &mut open,
                            Some(event.uptime_ms),
                            Some(IncompleteReason::MismatchedStop),
                        );
                        out.intervals.push(Interval {
                            boot_id: record_boot,
                            activity,
                            start_ms: None,
                            end_ms: Some(event.uptime_ms),
                            edge: Edge::Incomplete,
                            incomplete_reason: Some(IncompleteReason::UnmatchedStop),
                        });
                    }
                } else if matches!(event.kind, ObservationKind::ContinuityLost { .. }) {
                    close(
                        &mut out,
                        &mut open,
                        None,
                        Some(IncompleteReason::OwnerUncertain),
                    );
                } else if matches!(event.kind, ObservationKind::Unknown { .. }) {
                    close(
                        &mut out,
                        &mut open,
                        Some(event.uptime_ms),
                        Some(IncompleteReason::UnknownEvent),
                    );
                } else if let ObservationKind::RxCaptured { profile, .. }
                | ObservationKind::RxDamaged { profile } = event.kind
                    && open.as_ref().is_some_and(|interval| {
                        !matches!(
                            interval.activity,
                            Activity::Listening { profile: Some(active), .. } if active == profile
                        )
                    })
                {
                    close(
                        &mut out,
                        &mut open,
                        Some(event.uptime_ms),
                        Some(IncompleteReason::ContradictoryCapture),
                    );
                }
            }
        }
    }
    close(
        &mut out,
        &mut open,
        None,
        Some(IncompleteReason::EndOfCapture),
    );
    for interval in &out.intervals {
        if interval.edge == Edge::Incomplete {
            out.summary.incomplete_intervals += 1;
            continue;
        }
        if let (Some(start), Some(end)) = (interval.start_ms, interval.end_ms) {
            let duration = u128::from(end - start);
            match interval.activity {
                Activity::Listening {
                    profile: Some(profile),
                    ..
                } => *out.summary.listening_ms.entry(profile).or_default() += duration,
                Activity::Transmit { .. } => out.summary.transmit_ms += duration,
                Activity::Quiet { cause } => {
                    out.summary.quiet_ms += duration;
                    if let Some((_, total)) = out
                        .summary
                        .quiet_by_cause_ms
                        .iter_mut()
                        .find(|(recorded, _)| *recorded == cause)
                    {
                        *total += duration;
                    } else {
                        out.summary.quiet_by_cause_ms.push((cause, duration));
                    }
                }
                Activity::Sleep => out.summary.sleep_ms += duration,
                _ => {}
            }
        }
    }
    out.summary.missing_records = out.gaps.iter().map(|gap| u128::from(gap.gap.count)).sum();
    for event in &out.events {
        match event.kind {
            ObservationKind::RxCaptured { .. } => out.summary.captures += 1,
            ObservationKind::RxDamaged { .. } => out.summary.damaged += 1,
            ObservationKind::WorkRefused { .. } => out.summary.refusals += 1,
            _ => {}
        }
    }
    Ok(out)
}
