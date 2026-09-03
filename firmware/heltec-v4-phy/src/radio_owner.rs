//! The V4's single owner for live radio and durable board state.
//!
//! The modem and RNode loops deliberately retain their different host and sleep policies,
//! but neither gets an independent hand on LoRa, radio parameters, status, or flash. This is
//! the narrow board boundary beneath both personalities: call sites decide when it is safe to
//! sleep; this type decides how a receive is armed, waited, and collected.

use lora_phy::mod_traits::RadioKind;
use lora_phy::{DelayNs, LoRa, RxMode};
use radio_hand::control::{
    AbSlotStore, ConfigApplier, DurableConfig, FirstWriteStore, ManagementCarrier, QuietExit,
    QuietGuard, QuietWindow,
};
use radio_hand::executive::{Executive, Face, RadioFault, RadioState, Received};
use radio_hand::link::HostLink;
use radio_hand::region::Region;
use radio_hand::settings::Settings;
use radio_hand::store::Slot;

use crate::{channels, control_store, power, store, wake_input};

/// The non-copy custody boundary for one V4 image.
///
/// It is constructed once after SX1262 setup. A boot-selected RNode compatibility session
/// consumes it forever; the direct modem loop retains it for its own lifetime. The private
/// fields are intentional: a board loop may ask for a bounded operation, but cannot split out
/// a second radio or flash owner.
pub struct V4RadioOwner<RK: RadioKind, DLY: DelayNs> {
    lora: LoRa<RK, DLY>,
    radio: RadioState,
    local_status: radio_face::LocalStatus,
    face: Face,
    store: store::SettingsStore,
    settings: Option<Settings>,
    region: Region,
    boot_owner_available: bool,
    radio_service_started: bool,
}

/// The sole boot-only handoff into WN1's durable recovery path.
///
/// This borrow can be taken once, before any receive service begins. It deliberately does not
/// implement `QuietWindow`: this is recovery immediately after an ESP reset, before RX is
/// armed, not a live control dispatcher capability.
pub(crate) struct V4BootOwner<'a, RK: RadioKind, DLY: DelayNs> {
    owner: &'a mut V4RadioOwner<RK, DLY>,
}

/// Which bounded receive setup step failed. The V4 keeps these distinct in its existing host
/// and face reports, so the owner preserves that diagnosis while retaining exclusive custody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RxSetupFault {
    Prepare,
    Arm,
}

/// The synchronous fact a completed-event boundary must establish before it may stop RX.
///
/// Exclusive `&mut V4RadioOwner` custody proves there is no active TX, SPI operation, or
/// collection future; the DIO1 wake-lease bit independently proves that no IRQ waiter survived
/// its outer select. `CompletedFramePending` is checked separately because an already-high DIO1
/// means the completed frame still belongs to the ordinary receive/delivery path. It must be
/// collected before attempting a quiet operation, never erased as a side effect of stopping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V4QuietPreflight {
    Ready,
    /// The DIO1 Light-sleep wake lease says an IRQ waiter still owns the input.
    RadioWaitArmed,
    CompletedFramePending,
    ReceiveSetupOwed,
}

/// Why the board could not enter or leave a live quiet window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V4QuietError {
    NotReady(V4QuietPreflight),
    Standby,
    EntryIrqSettle,
    ExitIrqSettle,
    ResumeRx(RxSetupFault),
}

/// A configuration the current direct V4 image cannot honestly realize.
///
/// The image presently has only its local wired/USB host link. It does not run a resident
/// Reticulum relay and has no WN2, Wi-Fi, BLE, control carrier, or credential subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V4ConfigError {
    PublicConfigurationInvalid,
    SealedCredentialsUnsupported,
    ResidentReticulumRelayUnsupported,
    UnsupportedManagementCarriers { requested_mask: u8 },
    ProfileRejected { code: u8 },
}

/// One portable first-write transaction may touch both independently-owned V4
/// sector pairs.  This wrapper keeps the exact failed half visible without
/// allowing callers to take the raw settings store out of boot-owner custody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum V4FirstWriteStoreError {
    Control(control_store::ControlError),
    Pending(crate::commissioning_store::PendingStoreError),
}

const LOCAL_WIRED_USB_MASK: u8 = 1 << (ManagementCarrier::Usb as u8);

/// Whether an executive profile result may return to the control runtime.
///
/// `CONFIG_UNSUPPORTED` and `CONFIG_OUT_OF_REGION` reject before the driver changes the old
/// profile. `CONFIG_RADIO_FAULT` is expressly different: the driver may have reached an
/// unknown hardware state. Every other result is impossible for this executive call, so it is
/// treated as unsafe rather than relying on a future wire-code extension to preserve reset
/// discipline by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum V4ProfileApplyResult {
    Accepted,
    SafeRefusal,
    ResetRequired,
}

const fn classify_profile_apply_result(code: u8) -> V4ProfileApplyResult {
    match code {
        selvage::CONFIG_ACCEPTED => V4ProfileApplyResult::Accepted,
        selvage::CONFIG_UNSUPPORTED | selvage::CONFIG_OUT_OF_REGION => {
            V4ProfileApplyResult::SafeRefusal
        }
        _ => V4ProfileApplyResult::ResetRequired,
    }
}

/// Pure part of the preflight decision, kept separately so its safety ordering has host-test
/// coverage without an ESP reset or an SX1262.
const fn classify_quiet_preflight(
    radio_wait_armed: bool,
    dio1_high: bool,
    receive_setup_owed: bool,
) -> V4QuietPreflight {
    if radio_wait_armed {
        V4QuietPreflight::RadioWaitArmed
    } else if dio1_high {
        V4QuietPreflight::CompletedFramePending
    } else if receive_setup_owed {
        V4QuietPreflight::ReceiveSetupOwed
    } else {
        V4QuietPreflight::Ready
    }
}

/// Pure V4 feasibility check for a durable first-write candidate.
///
/// This runs before any pending-sector erase. It rechecks the portable
/// region/PHY validation as well as the V4-only absence of credentials,
/// resident relay transport, and non-USB management. Hardware application is
/// deliberately later, after a valid durable resume/boot path owns the radio.
pub(crate) fn first_write_configuration_feasible(
    configuration: &DurableConfig,
) -> Result<(), V4ConfigError> {
    configuration
        .public
        .validate()
        .map_err(|_| V4ConfigError::PublicConfigurationInvalid)?;
    if !configuration.sealed_credentials.is_empty() {
        return Err(V4ConfigError::SealedCredentialsUnsupported);
    }
    let transport = configuration.public.reticulum_transport();
    if transport.relay_announces || transport.relay_packets {
        return Err(V4ConfigError::ResidentReticulumRelayUnsupported);
    }
    let requested_mask = configuration.public.enabled_management_carriers().mask();
    if requested_mask != LOCAL_WIRED_USB_MASK {
        return Err(V4ConfigError::UnsupportedManagementCarriers { requested_mask });
    }
    Ok(())
}

/// The armed portion is deliberately separate from the reset-on-drop wrapper so the state
/// transitions can be unit-tested without invoking a board reset.
struct ResetLatch {
    armed: bool,
}

impl ResetLatch {
    const fn armed() -> Self {
        Self { armed: true }
    }

    const fn needs_reset(&self) -> bool {
        self.armed
    }

    fn complete(&mut self) {
        self.armed = false;
    }
}

/// Once radio stopping begins, cancellation has one safe outcome: reset.
///
/// This is non-copy by construction. It protects the entry future before a guard exists and
/// then moves into that guard. Calling `complete` is permitted only after continuous RX has
/// been prepared and armed again.
struct ResetOnDrop {
    latch: ResetLatch,
}

impl ResetOnDrop {
    const fn armed() -> Self {
        Self {
            latch: ResetLatch::armed(),
        }
    }

    fn complete(&mut self) {
        self.latch.complete();
    }
}

impl Drop for ResetOnDrop {
    fn drop(&mut self) {
        if self.latch.needs_reset() {
            // This does not return. A stopped or half-reconfigured radio must never be resumed
            // after a cancelled/erroring quiet operation.
            esp_hal::system::software_reset();
        }
    }
}

/// Borrow-scoped V4 quiet work. It is intentionally not constructible outside this module.
pub struct V4QuietGuard<'a, RK: RadioKind, DLY: DelayNs> {
    owner: &'a mut V4RadioOwner<RK, DLY>,
    // Keep the Light-sleep gate closed from immediately before the first stop await until the
    // guard is dropped after a successful resume, or until the reset path takes over.
    _awake: power::Awake,
    reset: ResetOnDrop,
    completed: bool,
}

impl<RK: RadioKind, DLY: DelayNs> V4RadioOwner<RK, DLY> {
    pub fn new(
        lora: LoRa<RK, DLY>,
        radio: RadioState,
        local_status: radio_face::LocalStatus,
        face: Face,
        store: store::SettingsStore,
        settings: Option<Settings>,
    ) -> Self {
        Self {
            lora,
            radio,
            local_status,
            face,
            store,
            settings,
            region: settings.map(|settings| settings.region).unwrap_or_default(),
            boot_owner_available: true,
            radio_service_started: false,
        }
    }

    /// Consume the one pre-radio WN1 recovery capability.
    ///
    /// Calling any radio-service entry point permanently closes this handoff. The wrapper is
    /// also consumed on a blank journal, so ordinary later code cannot obtain a boot applier.
    pub(crate) fn take_boot_owner(&mut self) -> Option<V4BootOwner<'_, RK, DLY>> {
        if !self.boot_owner_available || self.radio_service_started {
            return None;
        }
        self.close_boot_owner();
        Some(V4BootOwner { owner: self })
    }

    /// Permanently close the pre-radio recovery handoff once an ordinary owner entry begins.
    fn close_boot_owner(&mut self) {
        self.boot_owner_available = false;
    }

    /// Borrow the shared command executive for one bounded operation.
    ///
    /// The V4's receive policy stays board-local, including its low-power proof path, but
    /// commands still cross the shared executive so they cannot bypass radio or store custody.
    pub fn executive(&mut self) -> Executive<'_, RK, DLY> {
        self.close_boot_owner();
        self.radio_service_started = true;
        Executive::new(
            &mut self.lora,
            &mut self.radio,
            &mut self.local_status,
            &self.face,
            &mut self.store,
            self.region,
        )
    }

    /// Check whether a completed host/event-frame boundary may enter a quiet window.
    ///
    /// The existing modem and RNode loops own every live radio future through `&mut self`; the
    /// DIO1 wake-lease check below also refuses a waiter that has not released its input lease.
    /// Together these facts exclude wait, transmit, SPI, and collection work in flight.
    /// It must call this only after its command/event framing is complete. A high DIO1 is an
    /// already-completed RX frame and is refused here so the caller can collect/deliver it and
    /// retry. A frame that arrives after this synchronous sample is the deliberately bounded
    /// transition blackout: `enter` stops RX and settles/clears that IRQ before it grants flash
    /// work. It is not a delivered frame and must not be described as one.
    pub fn quiet_preflight(&self) -> V4QuietPreflight {
        classify_quiet_preflight(
            wake_input::radio_wake_armed(),
            wake_input::radio_is_high(),
            self.radio.prepare_rx,
        )
    }

    /// Prepare and arm continuous receive before creating an interrupt waiter.
    pub async fn ensure_rx(&mut self) -> Result<bool, RxSetupFault> {
        self.close_boot_owner();
        self.radio_service_started = true;
        if !self.radio.prepare_rx {
            return Ok(false);
        }
        if self
            .lora
            .prepare_for_rx(RxMode::Continuous, &self.radio.modulation, &self.radio.rx)
            .await
            .is_err()
        {
            return Err(RxSetupFault::Prepare);
        }
        if self.lora.rx_arm().await.is_err() {
            return Err(RxSetupFault::Arm);
        }
        self.radio.prepare_rx = false;
        Ok(true)
    }

    /// The only receive future callers may race against host input.
    pub async fn wait_rx_irq(&mut self) -> Result<(), RadioFault> {
        self.executive().wait_rx_irq().await
    }

    /// Collect a frame after [`Self::wait_rx_irq`] wins. Callers must never race this.
    pub async fn collect(&mut self, buffer: &mut [u8]) -> Result<Option<Received>, RadioFault> {
        self.executive().collect(buffer).await
    }

    pub fn radio_online(&mut self) {
        self.local_status.radio = radio_face::RadioState::Online;
        self.local_status.fault = None;
        (self.face.publish)(self.local_status, radio_face::LedSignal::Idle);
    }

    pub fn radio_fault(&mut self, code: u8, message: &'static str) {
        self.local_status.radio = radio_face::RadioState::Fault;
        self.local_status.fault = Some(radio_face::Fault {
            code,
            message: radio_face::Text::from_truncated(message),
        });
        (self.face.publish)(self.local_status, radio_face::LedSignal::Idle);
    }

    pub fn note_radio_frame(&mut self, frame: &Received) {
        self.local_status.rx_frames = self.local_status.rx_frames.saturating_add(1);
        self.local_status.last_rx = Some(radio_face::RxSummary {
            frame_len: frame.len as u16,
            rssi_dbm: frame.rssi,
            snr_tenths_db: frame.snr.saturating_mul(10),
        });
        self.local_status.last_wake = radio_face::WakeSource::Radio;
        (self.face.publish)(self.local_status, radio_face::LedSignal::Activity);
    }

    pub fn note_host_activity(&mut self) {
        self.local_status.host = radio_face::HostState::Attached;
        self.local_status.last_wake = radio_face::WakeSource::Host;
        (self.face.publish)(self.local_status, radio_face::LedSignal::Idle);
    }

    #[cfg(feature = "rf-sleep-proof")]
    pub fn status(&self) -> radio_face::LocalStatus {
        self.local_status
    }

    #[cfg(feature = "rf-sleep-proof")]
    pub fn note_proof_tx(&mut self, frame_len: usize, sent: bool) {
        if sent {
            self.local_status.tx_frames = self.local_status.tx_frames.saturating_add(1);
            self.local_status.last_tx = radio_face::TxResult::Sent {
                frame_len: frame_len as u16,
            };
        } else {
            self.local_status.last_tx = radio_face::TxResult::Failed { code: 1 };
        }
        (self.face.publish)(self.local_status, radio_face::LedSignal::Activity);
    }

    /// The RF sleep proof is a measured bench exchange, not a shipping transmit path. Keep
    /// its raw TX behavior exactly where the proof needs it while still retaining radio custody.
    #[cfg(feature = "rf-sleep-proof")]
    pub async fn proof_transmit(&mut self, frame: &[u8]) -> bool {
        self.close_boot_owner();
        self.radio_service_started = true;
        let sent = self
            .lora
            .prepare_for_tx(
                &self.radio.modulation,
                &mut self.radio.tx,
                self.radio.tx_power_dbm,
                frame,
            )
            .await
            .is_ok()
            && self.lora.tx().await.is_ok();
        self.radio.prepare_rx = true;
        sent
    }

    /// Mint commit-token bytes from the board's true entropy source for the live carrier.
    #[cfg_attr(feature = "host-uart-low-power", allow(dead_code))]
    pub(crate) fn fill_true_random(
        &mut self,
        out: &mut [u8],
    ) -> Result<(), radio_hand::executive::StoreFault> {
        self.store.fill_true_random(out)
    }

    /// Board-local probes need the same durable store as every command, but only this owner
    /// can lend it out. Persisted changes still reboot immediately in `channels::probe`.
    pub async fn probe<L: HostLink>(
        &mut self,
        packet: &[u8],
        online: &'static [u8],
        identity_line: &[u8],
        host: &mut L,
    ) -> channels::Outcome {
        self.close_boot_owner();
        channels::probe(
            packet,
            online,
            identity_line,
            self.settings,
            &mut self.store,
            host,
        )
        .await
    }

    /// Apply a durable configuration through the one board-owned regulatory transaction.
    ///
    /// A typed refusal is the only recoverable result after the region assignment: Selvage
    /// guarantees it leaves the old profile standing, so restore the software region. A radio
    /// fault or an unknown result leaves hardware state uncertain and resets directly.
    async fn apply_configuration(
        &mut self,
        configuration: &DurableConfig,
    ) -> Result<(), V4ConfigError> {
        first_write_configuration_feasible(configuration)?;

        let previous_region = self.region;
        self.region = configuration.public.region();
        let code = self
            .executive()
            .apply_profile(&configuration.public.requested_reticulum_phy())
            .await;
        match classify_profile_apply_result(code) {
            V4ProfileApplyResult::Accepted => Ok(()),
            V4ProfileApplyResult::SafeRefusal => {
                self.region = previous_region;
                Err(V4ConfigError::ProfileRejected { code })
            }
            V4ProfileApplyResult::ResetRequired => {
                esp_hal::system::software_reset();
            }
        }
    }
}

impl<RK: RadioKind, DLY: DelayNs> AbSlotStore for V4BootOwner<'_, RK, DLY> {
    type Error = control_store::ControlError;

    fn read_slot(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error> {
        AbSlotStore::read_slot(&mut self.owner.store, slot, out)
    }

    fn erase_slot(&mut self, slot: Slot) -> Result<(), Self::Error> {
        AbSlotStore::erase_slot(&mut self.owner.store, slot)
    }

    fn program_slot(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error> {
        AbSlotStore::program_slot(&mut self.owner.store, slot, record)
    }
}

impl<RK: RadioKind, DLY: DelayNs> ConfigApplier for V4BootOwner<'_, RK, DLY> {
    type Error = V4ConfigError;

    async fn apply(&mut self, configuration: &DurableConfig) -> Result<(), Self::Error> {
        self.owner.apply_configuration(configuration).await
    }
}

impl<RK: RadioKind, DLY: DelayNs> V4BootOwner<'_, RK, DLY> {
    /// Read one staged first-write slot during the one pre-radio arbitration window.
    pub(crate) fn read_pending_slot(
        &mut self,
        slot: Slot,
        out: &mut [u8],
    ) -> Result<(), crate::commissioning_store::PendingStoreError> {
        self.owner.store.read_pending_slot(slot, out)
    }

    /// Mint commissioning challenge bytes from the board's maintained true
    /// entropy source while this one-shot boot owner still has store custody.
    pub(crate) fn fill_commissioning_entropy(
        &mut self,
        out: &mut [u8],
    ) -> Result<(), radio_hand::executive::StoreFault> {
        self.owner.store.fill_true_random(out)
    }
}

impl<RK: RadioKind, DLY: DelayNs> FirstWriteStore for V4BootOwner<'_, RK, DLY> {
    type Error = V4FirstWriteStoreError;

    fn read_control(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error> {
        AbSlotStore::read_slot(self, slot, out).map_err(V4FirstWriteStoreError::Control)
    }

    fn erase_control(&mut self, slot: Slot) -> Result<(), Self::Error> {
        AbSlotStore::erase_slot(self, slot).map_err(V4FirstWriteStoreError::Control)
    }

    fn program_control(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error> {
        AbSlotStore::program_slot(self, slot, record).map_err(V4FirstWriteStoreError::Control)
    }

    fn read_pending(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error> {
        self.read_pending_slot(slot, out)
            .map_err(V4FirstWriteStoreError::Pending)
    }

    fn erase_pending(&mut self, slot: Slot) -> Result<(), Self::Error> {
        self.owner
            .store
            .erase_pending_slot(slot)
            .map_err(V4FirstWriteStoreError::Pending)
    }

    fn program_pending(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error> {
        self.owner
            .store
            .program_pending_slot(slot, record)
            .map_err(V4FirstWriteStoreError::Pending)
    }
}

impl<RK: RadioKind, DLY: DelayNs> QuietWindow for V4RadioOwner<RK, DLY> {
    type Error = V4QuietError;
    type StoreError = control_store::ControlError;
    type ApplyError = V4ConfigError;
    type Guard<'a>
        = V4QuietGuard<'a, RK, DLY>
    where
        Self: 'a;

    /// Stop only at a caller-established completed event/frame boundary.
    ///
    /// `quiet_preflight` is intentionally repeated here. The first check lets a future control
    /// carrier decline before it starts changing radio state; this one is authoritative. The
    /// borrow itself rules out any in-flight owner operation. A DIO1 edge between the sample
    /// and standby is the documented transition blackout, resolved by IRQ settlement below.
    async fn enter(&mut self) -> Result<Self::Guard<'_>, Self::Error> {
        let preflight = self.quiet_preflight();
        if preflight != V4QuietPreflight::Ready {
            return Err(V4QuietError::NotReady(preflight));
        }

        // Both must exist before the first await. From this point, cancellation or any error
        // drops `reset` and synchronously resets instead of returning to live traffic.
        let awake = power::Awake::new();
        let reset = ResetOnDrop::armed();

        self.lora
            .enter_standby()
            .await
            .map_err(|_| V4QuietError::Standby)?;
        self.lora
            .settle_irq_for_quiet_work()
            .await
            .map_err(|_| V4QuietError::EntryIrqSettle)?;

        // The chip is quiet, its latched IRQ state was cleared, and the driver's settlement
        // method has waited for DIO1 low. Returning to RX is now owed even if the guarded work
        // itself only read flash.
        self.radio.prepare_rx = true;
        Ok(V4QuietGuard {
            owner: self,
            _awake: awake,
            reset,
            completed: false,
        })
    }
}

impl<RK: RadioKind, DLY: DelayNs> QuietGuard for V4QuietGuard<'_, RK, DLY> {
    type Error = V4QuietError;

    fn abort(&mut self) {
        if !self.completed {
            // Direct callers and ActiveQuietGuard both reach this synchronous, non-returning
            // reset path when a live transition is abandoned.
            esp_hal::system::software_reset();
        }
    }

    async fn finish(&mut self) -> Result<QuietExit, Self::Error> {
        if self.completed {
            return Ok(QuietExit::Resumed);
        }

        // A profile application may have touched the modem and its IRQ state. Settle again
        // before arming receive; a dropped/erroring finish retains the reset-on-drop latch.
        self.owner
            .lora
            .settle_irq_for_quiet_work()
            .await
            .map_err(|_| V4QuietError::ExitIrqSettle)?;
        self.owner.radio.prepare_rx = true;
        self.owner
            .ensure_rx()
            .await
            .map_err(V4QuietError::ResumeRx)?;

        self.completed = true;
        self.reset.complete();
        Ok(QuietExit::Resumed)
    }
}

impl<RK: RadioKind, DLY: DelayNs> Drop for V4QuietGuard<'_, RK, DLY> {
    fn drop(&mut self) {
        if !self.completed {
            // Run before field destruction: in particular, do not release the Awake hold and
            // let Light-sleep resume while a stopped radio/flash transition is uncertain.
            esp_hal::system::software_reset();
        }
    }
}

impl<RK: RadioKind, DLY: DelayNs> AbSlotStore for V4QuietGuard<'_, RK, DLY> {
    type Error = control_store::ControlError;

    fn read_slot(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error> {
        AbSlotStore::read_slot(&mut self.owner.store, slot, out)
    }

    fn erase_slot(&mut self, slot: Slot) -> Result<(), Self::Error> {
        AbSlotStore::erase_slot(&mut self.owner.store, slot)
    }

    fn program_slot(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error> {
        AbSlotStore::program_slot(&mut self.owner.store, slot, record)
    }
}

impl<RK: RadioKind, DLY: DelayNs> ConfigApplier for V4QuietGuard<'_, RK, DLY> {
    type Error = V4ConfigError;

    async fn apply(&mut self, configuration: &DurableConfig) -> Result<(), Self::Error> {
        self.owner.apply_configuration(configuration).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use heapless::Vec;
    use radio_hand::control::{
        ManagementCarrierSet, PublicConfigurationV1, ReticulumTransportPolicy,
    };

    fn config(relay: bool, carriers: u8, sealed: bool) -> DurableConfig {
        let transport =
            ReticulumTransportPolicy::new(relay, false, if relay { 1 } else { 0 }).unwrap();
        let public = PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(906_875_000),
            transport,
            ManagementCarrierSet::from_mask(carriers).unwrap(),
        )
        .unwrap();
        let mut sealed_credentials = Vec::new();
        if sealed {
            sealed_credentials.push(1).unwrap();
        }
        DurableConfig {
            public,
            sealed_credentials,
        }
    }

    #[test]
    fn preflight_refuses_a_pending_frame_before_an_rx_rearm() {
        assert_eq!(
            classify_quiet_preflight(false, true, true),
            V4QuietPreflight::CompletedFramePending
        );
        assert_eq!(
            classify_quiet_preflight(false, false, true),
            V4QuietPreflight::ReceiveSetupOwed
        );
        assert_eq!(
            classify_quiet_preflight(false, false, false),
            V4QuietPreflight::Ready
        );
        assert_eq!(
            classify_quiet_preflight(true, false, false),
            V4QuietPreflight::RadioWaitArmed
        );
    }

    #[test]
    fn direct_image_capability_check_is_fail_closed() {
        assert_eq!(
            first_write_configuration_feasible(&config(false, LOCAL_WIRED_USB_MASK, false)),
            Ok(())
        );
        assert_eq!(
            first_write_configuration_feasible(&config(true, LOCAL_WIRED_USB_MASK, false)),
            Err(V4ConfigError::ResidentReticulumRelayUnsupported)
        );
        assert_eq!(
            first_write_configuration_feasible(&config(false, LOCAL_WIRED_USB_MASK | 2, false)),
            Err(V4ConfigError::UnsupportedManagementCarriers {
                requested_mask: LOCAL_WIRED_USB_MASK | 2
            })
        );
        assert_eq!(
            first_write_configuration_feasible(&config(false, LOCAL_WIRED_USB_MASK, true)),
            Err(V4ConfigError::SealedCredentialsUnsupported)
        );
    }

    #[test]
    fn reset_latch_only_clears_after_completion() {
        let mut latch = ResetLatch::armed();
        assert!(latch.needs_reset());
        latch.complete();
        assert!(!latch.needs_reset());
    }

    #[test]
    fn radio_fault_and_unknown_profile_results_require_reset() {
        assert_eq!(
            classify_profile_apply_result(selvage::CONFIG_RADIO_FAULT),
            V4ProfileApplyResult::ResetRequired
        );
        assert_eq!(
            classify_profile_apply_result(0xff),
            V4ProfileApplyResult::ResetRequired
        );
        assert_ne!(
            classify_profile_apply_result(selvage::CONFIG_RADIO_FAULT),
            V4ProfileApplyResult::SafeRefusal
        );
    }
}
