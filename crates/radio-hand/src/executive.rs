//! What sits below every channel: the radio, the radio's settings, and the face.
//!
//! Structural decision 4 rules that personalities become channels and that the executive
//! owns the hardware beneath them. This is that owner. The point is not tidiness — it is the
//! authority boundary the decision's panic-domain examination asks for:
//!
//! > A berserk channel can request nonsense; it cannot bypass the clamp, touch the store, or
//! > hold the radio.
//!
//! So `lora` is private and stays private. A channel transmits by asking, and because every
//! transmission crosses one line, both things that must sit below every personality now do:
//! the regulatory floor (pressure point 1 — region gate, power clamp, duty ledger) and
//! channel citizenship (pressure point 2 — listen before talk). Neither is skippable by
//! construction, which is the whole reason the seam was cut here before either existed.
//!
//! What this deliberately is not: a scheduler, an isolation boundary, or anything that
//! deserves the word hypervisor. Channels are trusted safe Rust in the same address space.

use embassy_time::{Duration, Instant, Ticker, with_timeout};
use lora_phy::mod_params::{ModulationParams, PacketParams};
use lora_phy::mod_traits::RadioKind;
use lora_phy::{DelayNs, LoRa, RxMode};
use radio_face::{HostSnapshot, LedSignal, LocalStatus};
use selvage::PhyProfile;

use crate::region::Region;
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
/// Bounded, and what happens at the bound is the whole design. Deferring *indefinitely*
/// starves against a peer that transmits blind: its retransmissions keep the channel
/// occupied, the polite board never gets a turn, so the peer never gets its answer and
/// retransmits again. That livelock was measured on hardware — a byte-exact exchange that
/// had run at 7 of 8 for weeks fell to 4 of 8, with `cadgiveup=0` proving nothing was
/// dropped and everything was merely too late.
///
/// So courtesy is bounded: defer while it is cheap, then transmit. A collision is one
/// retransmit; starvation is a node that never speaks. Where a region *mandates* carrier
/// sense the refusal is the correct outcome instead, which is what
/// [`crate::region::RegionProfile::listen_required`] selects.
const CAD_ATTEMPTS: u8 = 8;

/// Backoff floor between listen attempts, in milliseconds. The randomised part is added on
/// top and widens with each attempt, so two boards that collide once do not collide
/// identically again.
const CAD_BACKOFF_FLOOR_MS: u64 = 20;

/// The radio settings a profile owns, held here so a rejected profile cannot half-apply:
/// [`crate::service::apply_profile`] builds a replacement and only a complete one is swapped
/// in.
pub struct RadioState {
    pub modulation: ModulationParams,
    pub tx: PacketParams,
    pub rx: PacketParams,
    pub tx_power_dbm: i32,
    /// Set when the radio must be put back into receive before the next wait.
    pub prepare_rx: bool,
}

/// The board's local face. Function pointers rather than a trait: both images already expose
/// exactly these two signatures as free functions, so this costs nothing and needs no borrow
/// of the board's UI state.
pub struct Face {
    pub publish: fn(LocalStatus, LedSignal),
    pub publish_host: fn(HostSnapshot),
}

/// A board's ability to read its radio chip's diagnostic registers.
///
/// Chip-specific: `sx126x_diagnostics` lives on the SX126x kind, not on `LoRa`, so this
/// cannot be called generically. It takes a `lora` borrow rather than holding one, which is
/// what lets the executive lend out its own radio for the length of the call.
///
/// The ordering it exists to preserve is load-bearing. The host takes the most recent
/// diagnostic when a transmit *fails* and attaches it to that failure, so a diagnostic
/// emitted after its `EVENT_TX` reply would be lost from the error it explains and
/// misattributed to the next one.
/// Auto traits are deliberately unbounded: these futures are polled by a
/// single-threaded embassy executor on the board, never sent across threads, so a
/// `Send` bound would buy nothing and would exclude the HAL types that implement this.
#[allow(async_fn_in_trait)]
pub trait ChipDiagnostics<RK: RadioKind, DLY: DelayNs> {
    /// The seven-byte `EVENT_DIAGNOSTIC` body, including its marker.
    async fn read(&self, lora: &mut LoRa<RK, DLY>) -> [u8; 7];
}

/// The board's own persistent facts and its entropy.
///
/// One trait rather than two because one object holds both on real hardware: the T114's
/// `SettingsStore` owns the NVMC pages *and* the hardware RNG, so a pair of traits would
/// need two mutable borrows of the same thing.
///
/// It lives on the executive for the reason structural decision 4 gives — the executive owns
/// the flash, so a channel cannot reach past it to the store — and because it is the seam a
/// channel switch goes through: the switch is a persisted field plus a reboot.
pub trait BoardStore {
    /// Fill `out` with random bytes.
    ///
    /// Fallible on purpose. A board can genuinely have no entropy source, and the heltec
    /// doc's adversarial set asks for a bounded outcome when entropy fails, which is only
    /// expressible if failing is representable. Filling with zeros would be silently worse
    /// than refusing.
    fn random(&mut self, out: &mut [u8]) -> Result<(), StoreFault>;

    /// Persist new settings, keeping the identity already stored.
    ///
    /// Erase stalls the CPU for tens of milliseconds and blanks receive, so a caller either
    /// runs before the radio starts or resets the board immediately after. Pressure point 3
    /// holds by that contract rather than by a staged-commit window; a caller that can do
    /// neither has to build one.
    fn save(&mut self, settings: &crate::settings::Settings) -> Result<(), StoreFault>;
}

/// Why the board's store could not do what was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreFault {
    /// This board has no such facility. Not an error in the store; an absence of one.
    Unavailable,
    /// The write did not land, or did not read back.
    Write,
}

/// A board with neither persistence nor entropy.
///
/// The V4's honest state: gate N0 gave the T114 a store and left this board without one. It
/// refuses rather than pretending, so a channel that needs entropy fails loudly here instead
/// of announcing itself with zeros.
pub struct NoStore;

impl BoardStore for NoStore {
    fn random(&mut self, _out: &mut [u8]) -> Result<(), StoreFault> {
        Err(StoreFault::Unavailable)
    }

    fn save(&mut self, _settings: &crate::settings::Settings) -> Result<(), StoreFault> {
        Err(StoreFault::Unavailable)
    }
}

/// A received frame's length and signal report.
pub struct Received {
    pub len: usize,
    pub rssi: i16,
    pub snr: i16,
}

/// The radio would not do what was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioFault;

/// What the executive has actually done with the radio, counted.
///
/// Diagnostics assert presence, not just cost: when a path is silently dead — a receive that
/// never completes, an ensure that always fails — these counters are what says so, loudly and
/// attributably, on a board with no console. The `air` probe prints them.
#[derive(Debug, Clone, Copy, Default)]
pub struct AirDiag {
    /// Times `ensure_rx` re-armed the receiver.
    pub rx_armed: u16,
    /// Times `ensure_rx` failed. Nonzero means the radio refused receive setup.
    pub rx_arm_failed: u16,
    /// Frames `receive` returned.
    pub rx_ok: u16,
    /// Times `receive` returned a fault.
    pub rx_err: u16,
    /// Transmissions accepted on the air.
    pub tx_ok: u16,
    /// Transmissions refused or timed out.
    pub tx_err: u16,
    /// Transmissions refused because no region is configured. The regulatory floor
    /// working, not a fault.
    pub tx_no_region: u16,
    /// Transmissions refused because the region's duty budget was spent.
    pub tx_over_duty: u16,
    /// Listen-before-talk checks that found the channel clear on the first look.
    pub cad_clear: u16,
    /// Listen-before-talk checks that found activity and backed off.
    pub cad_busy: u16,
    /// Transmissions refused because the channel stayed busy where a region mandates
    /// carrier sense.
    pub tx_channel_busy: u16,
    /// Transmissions that took their turn after deferring for the whole courtesy budget.
    /// Nonzero means the band is contended, not that anything is wrong.
    pub cad_override: u16,
    /// Times the radio could not perform a listen check at all. Counted rather than fatal:
    /// see `listen_before_talk` for why this fails open.
    pub cad_fault: u16,
    /// Times the unattended wait woke for its heartbeat.
    pub wait_beats: u16,
    /// Times the unattended wait woke for a received frame.
    pub wait_frames: u16,
}

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
    /// **Off by default, on measurement.** The mechanism works — a jammed board defers and
    /// then takes its turn, receipted — but on this bench it costs the flagship exchange
    /// real reliability: same image, same session, `listen=on` passed 3 of 8 byte-exact
    /// runs and `listen=off` passed 8 of 8. The cost is latency, not loss (`cadgiveup=0`,
    /// `cadover=0` throughout): ~66 ms of carrier sense before every frame, plus backoffs,
    /// which desyncs a request/response loop whose retry intervals were tuned without it.
    ///
    /// That is the same defect N4 found in link setup — retry floors must derive from the
    /// profile's airtime rather than from constants picked for fast links — and it is the
    /// recorded follow-up this waits on. Turning citizenship on before then would trade a
    /// measured, load-bearing receipt for a politeness nobody has asked for yet.
    listen_first: bool,
    /// Transmit milliseconds spent in the current duty window.
    duty_spent_ms: u64,
    /// When the current duty window opened.
    duty_window_start: Option<Instant>,
    diag: AirDiag,
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

    /// Count an unattended-wait wakeup; called by [`crate::channel::await_host`].
    pub fn note_wait(&mut self, frame: bool) {
        if frame {
            self.diag.wait_frames = self.diag.wait_frames.saturating_add(1);
        } else {
            self.diag.wait_beats = self.diag.wait_beats.saturating_add(1);
        }
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
        if self
            .lora
            .prepare_for_rx(RxMode::Continuous, &self.radio.modulation, &self.radio.rx)
            .await
            .is_err()
        {
            self.diag.rx_arm_failed = self.diag.rx_arm_failed.saturating_add(1);
            return Err(RadioFault);
        }
        self.diag.rx_armed = self.diag.rx_armed.saturating_add(1);
        self.radio.prepare_rx = false;
        Ok(true)
    }

    /// Wait for one frame.
    ///
    /// Cancellation-safe on this hardware: the SX1262 keeps receiving in the background and
    /// holds DIO1 high until its interrupt flags are cleared, so a dropped wait is resumed
    /// rather than lost. That is what lets the serve loop select this against a host read and
    /// a heartbeat without dropping packets.
    pub async fn receive(&mut self, buffer: &mut [u8]) -> Result<Received, RadioFault> {
        match self.lora.rx(&self.radio.rx, buffer).await {
            Ok((len, status)) => {
                self.diag.rx_ok = self.diag.rx_ok.saturating_add(1);
                Ok(Received {
                    len: usize::from(len),
                    rssi: status.rssi,
                    snr: status.snr,
                })
            }
            Err(_) => {
                self.diag.rx_err = self.diag.rx_err.saturating_add(1);
                self.radio.prepare_rx = true;
                Err(RadioFault)
            }
        }
    }

    /// Put one frame on the air, returning its `EVENT_TX` result code.
    ///
    /// Every transmission in the firmware passes through here, which is what makes the
    /// regulatory floor unskippable by construction: no region, no transmit; a spent duty
    /// budget refuses the frame rather than sending it over the limit. Channel-citizenship
    /// gating (CAD) goes above the same line when it lands.
    pub async fn transmit(&mut self, frame: &[u8]) -> u8 {
        // The regulatory floor. `Unset` has no profile, so "no region, no transmit" falls
        // out of the type rather than out of a flag.
        let Some(profile) = self.region.profile() else {
            self.diag.tx_no_region = self.diag.tx_no_region.saturating_add(1);
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
                return selvage::TX_CHANNEL_BUSY;
            }
            // Courtesy spent; take the turn. See CAD_ATTEMPTS for why deferring forever is
            // the worse failure.
            self.diag.cad_override = self.diag.cad_override.saturating_add(1);
        }

        let prepared = self
            .lora
            .prepare_for_tx(
                &self.radio.modulation,
                &mut self.radio.tx,
                self.radio.tx_power_dbm,
                frame,
            )
            .await
            .is_ok();
        self.radio.prepare_rx = true;
        if !prepared {
            return selvage::TX_RADIO_FAULT;
        }
        let tx_started = Instant::now();
        let code = match with_timeout(TX_DEADLINE, self.lora.tx()).await {
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
        } else {
            self.diag.tx_err = self.diag.tx_err.saturating_add(1);
        }
        code
    }

    /// Listen for activity, backing off until the channel is clear or the budget is spent.
    ///
    /// Returns whether it is our turn to talk.
    ///
    /// **Fails open.** A radio that cannot perform the check at all transmits anyway, with
    /// `cad_fault` counted. Failing closed would turn one broken register write into a
    /// silent board — the exact class of failure that cost this project a whole session
    /// at N4 — and listen-before-talk here is citizenship, not a regulatory requirement in
    /// the regions this table ships. Where a region does mandate LBT, that belongs in the
    /// region entry, and this must fail closed for it.
    async fn listen_before_talk(&mut self) -> bool {
        for attempt in 0..CAD_ATTEMPTS {
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
        let Some(region) = self.region.profile() else {
            return selvage::CONFIG_OUT_OF_REGION;
        };
        if !region.allows_frequency(profile.frequency_hz) {
            return selvage::CONFIG_OUT_OF_REGION;
        }
        let mut clamped = *profile;
        clamped.tx_power_dbm = region.clamp_power(profile.tx_power_dbm, HARDWARE_MAX_DBM);

        match service::apply_profile(self.lora, &clamped).await {
            Ok(applied) => {
                self.radio.modulation = applied.modulation;
                self.radio.tx = applied.tx;
                self.radio.rx = applied.rx;
                self.radio.tx_power_dbm = applied.tx_power_dbm;
                self.radio.prepare_rx = true;
                crate::board_status::apply_profile(self.status, clamped);
                self.publish(LedSignal::Idle);
                service::ACCEPTED
            }
            Err(code) => code,
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
