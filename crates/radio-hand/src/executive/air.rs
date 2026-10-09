//! Transmission, listen-before-talk, and profile changes: the regulatory floor and channel
//! citizenship, both on the one line every frame crosses.

use embassy_time::{Duration, Instant, with_timeout};
use lora_phy::DelayNs;
use lora_phy::mod_traits::RadioKind;
use radio_face::LedSignal;
use selvage::PhyProfile;

use super::{CadObservation, CaptureArm, Executive, RadioFault};
use crate::observation::owner::{
    CONTINUITY_CAD, CONTINUITY_RETUNE, CONTINUITY_TRANSMIT, OwnerObservations,
};
use crate::observation::{RefusalReason, RequestKind};
use crate::service;

/// How long a transmission may run before the firmware gives up on it.
const TX_DEADLINE: Duration = Duration::from_secs(3);

/// The SX1262's conducted-power ceiling in dBm. Applied on top of every regional cap: the
/// power actually used is the minimum of request, region, and this.
const HARDWARE_MAX_DBM: i8 = 22;

/// The duty-cycle accounting window, one hour, matching how the EU limits are stated.
const DUTY_WINDOW_MS: u64 = 3_600_000;

/// How many times a frame defers to a busy channel before taking its turn anyway.
///
/// Deferring indefinitely livelocks against a peer that transmits blind (measured on
/// hardware), so courtesy is bounded: a collision is one retransmit, starvation is a node
/// that never speaks. Where a region mandates carrier sense, refusal is correct instead;
/// see [`crate::region::RegionProfile::listen_required`].
const CAD_ATTEMPTS: u8 = 8;

/// Backoff floor between listen attempts, in milliseconds. The randomised part is added on
/// top and widens with each attempt, so two boards that collide once do not collide
/// identically again.
const CAD_BACKOFF_FLOOR_MS: u64 = 20;

impl<'r, RK: RadioKind, DLY: DelayNs> Executive<'r, RK, DLY> {
    /// Put one frame on the air, returning its `EVENT_TX` result code.
    ///
    /// Every transmission in the firmware passes through here, which is what makes the
    /// regulatory floor unskippable by construction: no region, no transmit; a spent duty
    /// budget refuses the frame rather than sending it over the limit. Channel-citizenship
    /// gating (CAD) goes above the same line when it lands.
    pub async fn transmit(&mut self, frame: &[u8]) -> u8 {
        let work = self
            .observations
            .as_deref_mut()
            .and_then(OwnerObservations::next_work);
        // The regulatory floor. `Unset` has no profile, so "no region, no transmit" falls
        // out of the type rather than out of a flag.
        let Some(profile) = self.region.profile() else {
            self.diag.tx_no_region = self.diag.tx_no_region.saturating_add(1);
            self.refuse_observation(RequestKind::Transmit, RefusalReason::MissingRegion, work);
            return selvage::TX_NO_REGION;
        };

        // The duty ledger: a fixed window matching how the limits are stated. Zero permille
        // means the region imposes none.
        if profile.duty_permille > 0 {
            let now = Instant::now();
            match self.duty_window_start {
                Some(start) if now.duration_since(start).as_millis() < DUTY_WINDOW_MS => {}
                _ => {
                    self.duty_window_start = Some(now);
                    self.duty_spent_ms = 0;
                }
            }
            let budget_ms = DUTY_WINDOW_MS / 1_000 * u64::from(profile.duty_permille);
            if self.duty_spent_ms >= budget_ms {
                self.diag.tx_over_duty = self.diag.tx_over_duty.saturating_add(1);
                self.refuse_observation(RequestKind::Transmit, RefusalReason::DutyBudget, work);
                return selvage::TX_OVER_DUTY;
            }
        }

        // Channel citizenship. Pressure point 2: the band is shared, and a blind transmit
        // steps on whoever is already talking. This is the MAC answer the collision-
        // mitigation notes concluded was the reachable one on stock certified radios.
        if self.listen_first && !self.listen_before_talk().await {
            if profile.listen_required {
                self.diag.tx_channel_busy = self.diag.tx_channel_busy.saturating_add(1);
                // The radio is left in standby by the listen check, so ask for receive
                // back: a refused transmit must never be a deaf board.
                self.radio.prepare_rx = true;
                self.refuse_observation(RequestKind::Transmit, RefusalReason::ChannelBusy, work);
                return selvage::TX_CHANNEL_BUSY;
            }
            // Courtesy spent; take the turn. See CAD_ATTEMPTS for why deferring forever is
            // the worse failure.
            self.diag.cad_override = self.diag.cad_override.saturating_add(1);
        }

        let prepared = {
            let observations = &mut self.observations;
            self.lora
                .prepare_for_tx_with_stopped(
                    &self.radio.modulation,
                    &mut self.radio.tx,
                    self.radio.tx_power_dbm,
                    frame,
                    || {
                        if let Some(observations) = observations.as_deref_mut() {
                            observations.listening_stopped(Self::observation_uptime_ms(), 0);
                        }
                    },
                )
                .await
                .is_ok()
        };
        self.radio.prepare_rx = true;
        if !prepared {
            self.invalidate_observation(CONTINUITY_TRANSMIT);
            self.refuse_observation(RequestKind::Transmit, RefusalReason::RadioFault, work);
            return selvage::TX_RADIO_FAULT;
        }
        let tx_started = Instant::now();
        let profile = self.radio.profile;
        let observations = &mut self.observations;
        let code = match with_timeout(
            TX_DEADLINE,
            self.lora.tx_with_started(|| {
                if let (Some(observations), Some(work)) = (observations.as_deref_mut(), work) {
                    observations.tx_started(
                        Self::observation_uptime_ms(),
                        profile,
                        frame.len(),
                        work,
                    );
                }
            }),
        )
        .await
        {
            Ok(Ok(())) => selvage::TX_ACCEPTED,
            Ok(Err(_)) => selvage::TX_RADIO_FAULT,
            Err(_) => selvage::TX_TIMEOUT,
        };
        // Measured airtime, charged to the duty ledger. Measured rather than predicted:
        // what the ledger owes is what the antenna actually did.
        self.duty_spent_ms = self
            .duty_spent_ms
            .saturating_add(tx_started.elapsed().as_millis());
        if code == selvage::TX_ACCEPTED {
            self.diag.tx_ok = self.diag.tx_ok.saturating_add(1);
            if let (Some(observations), Some(work)) = (self.observations.as_deref_mut(), work) {
                observations.tx_finished(Self::observation_uptime_ms(), work);
            }
        } else {
            self.diag.tx_err = self.diag.tx_err.saturating_add(1);
            self.invalidate_observation(CONTINUITY_TRANSMIT);
        }
        code
    }

    /// Listen for activity, backing off until the channel is clear or the budget is spent.
    ///
    /// Returns whether it is our turn to talk.
    ///
    /// **Fails open**, counting `cad_fault`: failing closed would turn one broken register
    /// write into a silent board, and LBT here is citizenship, not regulation. A region that
    /// mandates LBT must fail closed via its region entry.
    async fn listen_before_talk(&mut self) -> bool {
        for attempt in 0..CAD_ATTEMPTS {
            self.invalidate_observation(CONTINUITY_CAD);
            if self
                .lora
                .prepare_for_cad(&self.radio.modulation)
                .await
                .is_err()
            {
                self.diag.cad_fault = self.diag.cad_fault.saturating_add(1);
                return true;
            }
            match self.lora.cad(&self.radio.modulation).await {
                Ok(false) => {
                    self.diag.cad_clear = self.diag.cad_clear.saturating_add(1);
                    return true;
                }
                Ok(true) => {
                    self.diag.cad_busy = self.diag.cad_busy.saturating_add(1);
                }
                Err(_) => {
                    self.diag.cad_fault = self.diag.cad_fault.saturating_add(1);
                    return true;
                }
            }
            embassy_time::Timer::after_millis(self.backoff_ms(attempt)).await;
        }
        false
    }

    /// A randomised backoff that widens with each attempt.
    ///
    /// Random so two boards colliding once do not collide identically again; widening so a
    /// genuinely busy band is not hammered. Entropy failure falls back to the floor rather
    /// than refusing, because a deterministic backoff is merely worse, not wrong.
    fn backoff_ms(&mut self, attempt: u8) -> u64 {
        let mut byte = [0_u8; 1];
        let jitter = match self.store.random(&mut byte) {
            Ok(()) => u64::from(byte[0]),
            Err(_) => 0,
        };
        let width = CAD_BACKOFF_FLOOR_MS << u32::from(attempt.min(3));
        CAD_BACKOFF_FLOOR_MS + (jitter * width / 256)
    }

    /// Apply a host profile, committing it only if every step passed.
    ///
    /// Returns the `EVENT_CONFIG` result code. The regulatory floor rules first: frequency
    /// outside the region's band rejects the profile whole, and power clamps to the minimum
    /// of request, region, and hardware — with the *clamped* value applied, stored, and
    /// reported on the face, never the requested one.
    pub async fn apply_profile(&mut self, profile: &PhyProfile) -> u8 {
        let work = self
            .observations
            .as_deref_mut()
            .and_then(OwnerObservations::next_work);
        let Some(region) = self.region.profile() else {
            self.refuse_observation(RequestKind::Retune, RefusalReason::MissingRegion, work);
            return selvage::CONFIG_OUT_OF_REGION;
        };
        if !region.allows_frequency(profile.frequency_hz) {
            self.refuse_observation(RequestKind::Retune, RefusalReason::InvalidProfile, work);
            return selvage::CONFIG_OUT_OF_REGION;
        }
        let mut clamped = *profile;
        clamped.tx_power_dbm = region.clamp_power(profile.tx_power_dbm, HARDWARE_MAX_DBM);
        self.invalidate_observation(CONTINUITY_RETUNE);

        match service::apply_profile(self.lora, &clamped).await {
            Ok(applied) => {
                self.radio.modulation = applied.modulation;
                self.radio.tx = applied.tx;
                self.radio.rx = applied.rx;
                self.radio.tx_power_dbm = applied.tx_power_dbm;
                self.radio.profile = clamped;
                self.radio.prepare_rx = true;
                crate::board_status::apply_profile(self.status, clamped);
                self.publish(LedSignal::Idle);
                service::ACCEPTED
            }
            Err(code) => {
                let reason = if code == selvage::CONFIG_RADIO_FAULT {
                    RefusalReason::RadioFault
                } else {
                    RefusalReason::InvalidProfile
                };
                self.refuse_observation(RequestKind::Retune, reason, work);
                code
            }
        }
    }

    /// Run one eight-symbol CAD observation under an exact hardware profile.
    ///
    /// The vendored SX126x driver fixes CAD at eight symbols. Keeping this operation
    /// here preserves the authority boundary: scan consumers can request an observation,
    /// but still cannot reach the radio directly.
    pub async fn observe_cad(
        &mut self,
        profile: &PhyProfile,
    ) -> Result<CadObservation, RadioFault> {
        let apply_started = Instant::now();
        if self.apply_profile(profile).await != service::ACCEPTED {
            return Err(RadioFault);
        }
        let apply_us = apply_started.elapsed().as_micros();

        let retune_started = Instant::now();
        self.lora
            .prepare_for_cad(&self.radio.modulation)
            .await
            .map_err(|_| RadioFault)?;
        let retune_us = retune_started.elapsed().as_micros();

        let cad_started = Instant::now();
        let activity = self
            .lora
            .cad(&self.radio.modulation)
            .await
            .map_err(|_| RadioFault)?;
        let cad_us = cad_started.elapsed().as_micros();
        self.radio.prepare_rx = true;
        Ok(CadObservation {
            apply_us,
            retune_us,
            cad_us,
            activity,
        })
    }

    /// Enter continuous receive under one exact capture profile.
    pub async fn arm_capture(&mut self, profile: &PhyProfile) -> Result<CaptureArm, RadioFault> {
        let apply_started = Instant::now();
        if self.apply_profile(profile).await != service::ACCEPTED {
            return Err(RadioFault);
        }
        let apply_us = apply_started.elapsed().as_micros();

        let handoff_started = Instant::now();
        self.ensure_rx().await?;
        let handoff_us = handoff_started.elapsed().as_micros();
        Ok(CaptureArm {
            apply_us,
            handoff_us,
        })
    }
}
