//! What sits below every channel: the radio, the radio's settings, and the face.
//!
//! Structural decision 4: personalities become channels and the executive owns the hardware
//! beneath them, so a berserk channel can request nonsense but cannot bypass the clamp, touch
//! the store, or hold the radio. `lora` stays private; every transmission crosses one line, so
//! the regulatory floor (pressure point 1) and listen-before-talk (pressure point 2) are
//! unskippable by construction.
//!
//! Not a scheduler or an isolation boundary: channels are trusted safe Rust in the same
//! address space.

use embassy_time::{Duration, Instant, Ticker};
use lora_phy::mod_params::RadioError;
use lora_phy::mod_traits::RadioKind;
use lora_phy::{DelayNs, LoRa, RxMode};
use radio_face::{HostSnapshot, LedSignal, LocalStatus};
use selvage::PhyProfile;

use crate::observation::owner::{CONTINUITY_RETUNE, CONTINUITY_SETTINGS, OwnerObservations};
use crate::observation::{RefusalReason, RequestKind};
use crate::region::Region;

mod air;
mod types;

pub use types::{
    AirDiag, BoardStore, CadObservation, CaptureArm, ChipDiagnostics, Face, NoStore, RadioFault,
    RadioState, Received, StoreFault,
};

/// The hardware every channel shares, and the only way a channel reaches it.
///
/// A borrowed view rather than an owner. A board that holds one for the whole of `main` gets
/// the full boundary — its own `lora` is unreachable for as long as the executive lives — and
/// a board still carrying a bespoke radio path can construct one per call and adopt the seam
/// incrementally. The V4 does the latter while its low-power receive keeps its own hand on
/// the radio; the T114 does the former.
pub struct Executive<'r, RK: RadioKind, DLY: DelayNs> {
    lora: &'r mut LoRa<RK, DLY>,
    radio: &'r mut RadioState,
    status: &'r mut LocalStatus,
    face: &'r Face,
    store: &'r mut dyn BoardStore,
    region: Region,
    /// Whether listen-before-talk gates transmission.
    ///
    /// Off by default, on measurement: carrier sense adds latency that desyncs request/response
    /// retry intervals tuned without it (the N4 airtime-derived retry floor follow-up).
    listen_first: bool,
    /// Transmit milliseconds spent in the current duty window.
    duty_spent_ms: u64,
    /// When the current duty window opened.
    duty_window_start: Option<Instant>,
    diag: AirDiag,
    observations: Option<&'r mut OwnerObservations>,
}

impl<'r, RK: RadioKind, DLY: DelayNs> Executive<'r, RK, DLY> {
    pub fn new(
        lora: &'r mut LoRa<RK, DLY>,
        radio: &'r mut RadioState,
        status: &'r mut LocalStatus,
        face: &'r Face,
        store: &'r mut dyn BoardStore,
        region: Region,
    ) -> Self {
        Self {
            lora,
            radio,
            status,
            face,
            store,
            region,
            listen_first: false,
            duty_spent_ms: 0,
            duty_window_start: None,
            diag: AirDiag::default(),
            observations: None,
        }
    }

    /// The region this executive enforces.
    pub fn region(&self) -> Region {
        self.region
    }

    /// Duty milliseconds spent in the current window, for probes.
    pub fn duty_spent_ms(&self) -> u64 {
        self.duty_spent_ms
    }

    /// Whether listen-before-talk is gating transmission.
    pub fn listen_first(&self) -> bool {
        self.listen_first
    }

    /// Turn listen-before-talk on or off. Runtime only, never persisted: every boot is a
    /// good citizen, and a bench that wants the comparison asks for it each time.
    pub fn set_listen_first(&mut self, on: bool) {
        self.listen_first = on;
    }

    /// The executive's own account of the radio. See [`AirDiag`].
    pub fn diag(&self) -> AirDiag {
        self.diag
    }

    /// The exact profile currently represented by the driver state.
    pub fn profile(&self) -> PhyProfile {
        self.radio.profile
    }

    /// Attach the T114 owner's optional, RAM-only observation state.  V4's
    /// ephemeral executive keeps this unattached.
    pub fn attach_observations(&mut self, observations: &'r mut OwnerObservations) {
        self.observations = Some(observations);
    }

    /// Read the attached owner state without granting a caller a recorder write.
    pub fn observations(&self) -> Option<&OwnerObservations> {
        self.observations.as_deref()
    }

    fn observation_uptime_ms() -> u64 {
        Instant::now().as_millis()
    }

    fn invalidate_observation(&mut self, reason: u8) {
        if let Some(observations) = self.observations.as_deref_mut() {
            observations.invalidate_hardware(Self::observation_uptime_ms(), reason);
        }
    }

    fn refuse_observation(
        &mut self,
        request: RequestKind,
        reason: RefusalReason,
        work: Option<u32>,
    ) {
        if let (Some(observations), Some(work)) = (self.observations.as_deref_mut(), work) {
            observations.refused(Self::observation_uptime_ms(), request, reason, work);
        }
    }

    /// Count an unattended-wait wakeup; called by [`crate::channel::await_host`].
    pub fn note_wait(&mut self, frame: bool) {
        if frame {
            self.diag.wait_frames = self.diag.wait_frames.saturating_add(1);
        } else {
            self.diag.wait_beats = self.diag.wait_beats.saturating_add(1);
        }
    }

    /// Count a scan-plan CAD result under its stable profile id.
    pub fn note_scan_cad(&mut self, id: u8, activity: bool) {
        let Some(index) = id
            .checked_sub(1)
            .map(usize::from)
            .filter(|index| *index < 4)
        else {
            return;
        };
        let counter = if activity {
            &mut self.diag.scan_cad_hits[index]
        } else {
            &mut self.diag.scan_cad_misses[index]
        };
        *counter = counter.saturating_add(1);
    }

    /// Count an exact capture window under its stable ReceiveProfile id.
    pub fn note_scan_capture(&mut self, id: u8, captured: bool) {
        let Some(index) = id
            .checked_sub(1)
            .map(usize::from)
            .filter(|index| *index < 4)
        else {
            return;
        };
        let counter = if captured {
            &mut self.diag.scan_rx_captures[index]
        } else {
            &mut self.diag.scan_rx_misses[index]
        };
        *counter = counter.saturating_add(1);
    }

    /// Draw random bytes from the board.
    pub fn random(&mut self, out: &mut [u8]) -> Result<(), StoreFault> {
        self.store.random(out)
    }

    /// Persist new settings. See [`BoardStore::save`] for when this is legal.
    pub fn save_settings(
        &mut self,
        settings: &crate::settings::Settings,
    ) -> Result<(), StoreFault> {
        self.invalidate_observation(CONTINUITY_SETTINGS);
        self.store.save(settings)
    }

    /// The board's status, as the face last saw it.
    pub fn status(&self) -> LocalStatus {
        *self.status
    }

    /// The board's status, to amend before publishing.
    pub fn status_mut(&mut self) -> &mut LocalStatus {
        self.status
    }

    /// Show the current status on the face.
    pub fn publish(&mut self, signal: LedSignal) {
        (self.face.publish)(*self.status, signal);
    }

    /// Show the host's own status on the face.
    pub fn publish_host(&self, snapshot: HostSnapshot) {
        (self.face.publish_host)(snapshot);
    }

    /// Note that the radio must be returned to receive before the next wait.
    pub fn request_rx(&mut self) {
        self.radio.prepare_rx = true;
    }

    /// Put the radio back into continuous receive if anything has disturbed it.
    ///
    /// Idempotent and cheap when nothing is owed, so a serve loop can call it every turn
    /// without thinking about whether it needs to. Returns whether the radio was actually
    /// re-prepared, because a caller that reports "online" on the face should say so when
    /// something changed rather than on every turn of the loop.
    pub async fn ensure_rx(&mut self) -> Result<bool, RadioFault> {
        if !self.radio.prepare_rx {
            return Ok(false);
        }
        // `prepare_for_rx` can enter standby and retune even if a caller merely
        // re-requested receive while an earlier interval was open.  The next
        // start is emitted only after `rx_arm` confirms the new hardware edge.
        self.invalidate_observation(CONTINUITY_RETUNE);
        if self
            .lora
            .prepare_for_rx(RxMode::Continuous, &self.radio.modulation, &self.radio.rx)
            .await
            .is_err()
        {
            self.diag.rx_arm_failed = self.diag.rx_arm_failed.saturating_add(1);
            self.invalidate_observation(CONTINUITY_RETUNE);
            return Err(RadioFault);
        }
        // Put the chip into continuous receive here, rather than leaving it to the first
        // poll of a receive future. A caller that races the radio against its host must be
        // able to abandon the wait without abandoning half-finished SPI, and that is only
        // true if the arming already happened. See [`Self::wait_rx_irq`].
        if self.lora.rx_arm().await.is_err() {
            self.diag.rx_arm_failed = self.diag.rx_arm_failed.saturating_add(1);
            self.invalidate_observation(CONTINUITY_RETUNE);
            return Err(RadioFault);
        }
        self.diag.rx_armed = self.diag.rx_armed.saturating_add(1);
        self.radio.prepare_rx = false;
        let profile = self.radio.profile;
        if let Some(observations) = self.observations.as_deref_mut() {
            observations.listening_started(Self::observation_uptime_ms(), 0, profile);
        }
        Ok(true)
    }

    /// Wait until the radio has something to say. **This is the only radio future that is
    /// safe to race**, and racing it is the whole point.
    ///
    /// A loop that selects a whole receive against host input cancels that receive wherever
    /// it happens to be. Almost always that is inside this wait, where abandoning costs
    /// nothing. But once the interrupt fires, a receive opens SPI transactions to read the
    /// cause and pull the payload out of the chip; cancelling *there* consumes the interrupt,
    /// leaves the bytes in a FIFO the next packet overwrites, and reports nothing. Rare per
    /// event, certain over a week, and silent.
    ///
    /// So: race this, and when it returns, call [`Self::collect`] without racing it.
    pub async fn wait_rx_irq(&mut self) -> Result<(), RadioFault> {
        self.lora.wait_for_irq().await.map_err(|_| {
            self.diag.rx_err = self.diag.rx_err.saturating_add(1);
            self.radio.prepare_rx = true;
            self.invalidate_observation(CONTINUITY_RETUNE);
            RadioFault
        })
    }

    /// Take the frame whose interrupt [`Self::wait_rx_irq`] just reported. Never race this.
    ///
    /// A CRC failure is the air being the air rather than a fault, so it is counted and the
    /// caller is told there is nothing to deliver; the radio stays in continuous receive and
    /// the next frame is the recovery.
    pub async fn collect(&mut self, buffer: &mut [u8]) -> Result<Option<Received>, RadioFault> {
        match self.lora.rx_collect(&self.radio.rx, buffer).await {
            Ok((len, status)) => {
                self.diag.rx_ok = self.diag.rx_ok.saturating_add(1);
                if let Some(observations) = self.observations.as_deref_mut() {
                    observations.rx_captured(
                        Self::observation_uptime_ms(),
                        self.radio.profile,
                        usize::from(len),
                        status.rssi,
                        status.snr,
                    );
                }
                Ok(Some(Received {
                    len: usize::from(len),
                    rssi: status.rssi,
                    snr: status.snr,
                }))
            }
            Err(RadioError::ReceivePending) => Ok(None),
            Err(RadioError::PayloadCrcError | RadioError::HeaderError) => {
                self.diag.rx_damaged = self.diag.rx_damaged.saturating_add(1);
                if let Some(observations) = self.observations.as_deref_mut() {
                    observations.rx_damaged(Self::observation_uptime_ms(), self.radio.profile);
                }
                Ok(None)
            }
            Err(_) => {
                self.diag.rx_err = self.diag.rx_err.saturating_add(1);
                self.radio.prepare_rx = true;
                self.invalidate_observation(CONTINUITY_RETUNE);
                Err(RadioFault)
            }
        }
    }

    /// Wait for one frame.
    ///
    /// Cancellation-safe on this hardware: the SX1262 keeps receiving in the background and
    /// holds DIO1 high until its interrupt flags are cleared, so a dropped wait is resumed
    /// rather than lost. That is what lets the serve loop select this against a host read and
    /// a heartbeat without dropping packets.
    pub async fn receive(&mut self, buffer: &mut [u8]) -> Result<Received, RadioFault> {
        loop {
            match self.lora.rx(&self.radio.rx, buffer).await {
                Ok((len, status)) => {
                    self.diag.rx_ok = self.diag.rx_ok.saturating_add(1);
                    if let Some(observations) = self.observations.as_deref_mut() {
                        observations.rx_captured(
                            Self::observation_uptime_ms(),
                            self.radio.profile,
                            usize::from(len),
                            status.rssi,
                            status.snr,
                        );
                    }
                    return Ok(Received {
                        len: usize::from(len),
                        rssi: status.rssi,
                        snr: status.snr,
                    });
                }
                // A packet that did not survive the air. Counted and dropped here rather
                // than reported upward, because the two are genuinely different events and
                // the difference is visible from nowhere else: the radio is fine, so a board
                // that answered this with its "radio rx failed" line would be lying to its
                // host — and on a channel speaking somebody else's binary protocol, it would
                // be injecting text into their stream.
                //
                // The radio is still in continuous receive (the driver leaves the mode alone
                // on an error there), so listening again is the whole recovery.
                Err(RadioError::PayloadCrcError | RadioError::HeaderError) => {
                    self.diag.rx_damaged = self.diag.rx_damaged.saturating_add(1);
                    if let Some(observations) = self.observations.as_deref_mut() {
                        observations.rx_damaged(Self::observation_uptime_ms(), self.radio.profile);
                    }
                }
                Err(_) => {
                    self.diag.rx_err = self.diag.rx_err.saturating_add(1);
                    self.radio.prepare_rx = true;
                    self.invalidate_observation(CONTINUITY_RETUNE);
                    return Err(RadioFault);
                }
            }
        }
    }

    /// Read the radio chip's diagnostic registers.
    ///
    /// Takes the board's [`ChipDiagnostics`] rather than holding one, because the call is
    /// chip-specific and needs this executive's own radio borrow.
    pub async fn diagnostics<D: ChipDiagnostics<RK, DLY>>(&mut self, chip: &D) -> [u8; 7] {
        chip.read(self.lora).await
    }
}

/// A channel's own clock.
///
/// Most channels have nothing to do on a timer, and a periodic wake that exists only to be
/// ignored is a real cost on a battery board. So the absent case is a future that never
/// completes rather than a fast tick with an empty body: the modem channel's serve loop waits
/// on exactly the two things it did before this existed.
pub struct Heartbeat {
    ticker: Option<Ticker>,
}

impl Heartbeat {
    /// A heartbeat at `interval`, or none at all.
    pub fn new(interval: Option<Duration>) -> Self {
        Self {
            ticker: interval.map(Ticker::every),
        }
    }

    /// Wait for the next beat.
    ///
    /// A `Ticker` rather than a fresh timer each turn, so a busy host cannot starve the beat
    /// by keeping the serve loop cycling faster than the interval.
    pub async fn next(&mut self) {
        match &mut self.ticker {
            Some(ticker) => ticker.next().await,
            None => core::future::pending().await,
        }
    }
}
