#![no_std]
#![allow(async_fn_in_trait)]
#![warn(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc = include_str!("../README.md")]

#[cfg(feature = "lorawan-radio")]
#[cfg_attr(docsrs, doc(cfg(feature = "lorawan-radio")))]
/// Provides an implementation of the async LoRaWAN device trait.
pub mod lorawan_radio;

/// The read/write interface between an embedded framework/MCU combination and a LoRa chip
pub(crate) mod interface;
/// InterfaceVariant implementations using `embedded-hal`.
pub mod iv;
/// Parameters used across the lora-phy crate to support various use cases
pub mod mod_params;
/// Traits implemented externally or internally to support control of LoRa chips
pub mod mod_traits;
/// Specific implementation to support Semtech Sx126x chips
pub mod sx126x;
/// Specific implementation to support Semtech Sx127x chips
pub mod sx127x;

pub use crate::mod_params::RxMode;

pub use embedded_hal_async::delay::DelayNs;
use interface::*;
use mod_params::*;
use mod_traits::*;

/// Provides the physical layer API to support LoRa chips
pub struct LoRa<RK, DLY>
where
    RK: RadioKind,
    DLY: DelayNs,
{
    radio_kind: RK,
    delay: DLY,
    radio_mode: RadioMode,
    sync_word: u8,
    cold_start: bool,
    calibrate_image: bool,
}

fn record_standby(radio_mode: &mut RadioMode, result: Result<(), RadioError>) -> Result<(), RadioError> {
    match result {
        Ok(()) => {
            *radio_mode = RadioMode::Standby;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

trait QuietIrqSettlement {
    async fn clear_irq_flags(&mut self) -> Result<(), RadioError>;
    async fn await_irq_low(&mut self) -> Result<(), RadioError>;
}

impl<RK> QuietIrqSettlement for RK
where
    RK: RadioKind,
{
    async fn clear_irq_flags(&mut self) -> Result<(), RadioError> {
        RadioKind::clear_irq_flags(self).await
    }

    async fn await_irq_low(&mut self) -> Result<(), RadioError> {
        RadioKind::await_irq_low(self).await
    }
}

async fn settle_quiet_irq(radio: &mut impl QuietIrqSettlement) -> Result<(), RadioError> {
    radio.clear_irq_flags().await?;
    radio.await_irq_low().await
}

impl<RK, DLY> LoRa<RK, DLY>
where
    RK: RadioKind,
    DLY: DelayNs,
{
    /// Build and return a new instance of the LoRa physical layer API to control an initialized LoRa radio
    pub async fn new(radio_kind: RK, enable_public_network: bool, delay: DLY) -> Result<Self, RadioError> {
        let sync_word = if enable_public_network { 0x34 } else { 0x12 };
        Self::new_with_sync_word(radio_kind, sync_word, delay).await
    }

    /// Build and initialize a LoRa radio with an explicit canonical LoRa sync word.
    ///
    /// SX126x radios use a two-byte register representation internally. The radio
    /// implementation performs that conversion so callers can use the same one-byte
    /// value used by SX127x radios and protocol specifications.
    pub async fn new_with_sync_word(radio_kind: RK, sync_word: u8, delay: DLY) -> Result<Self, RadioError> {
        let mut lora = Self {
            radio_kind,
            delay,
            radio_mode: RadioMode::Sleep,
            sync_word,
            cold_start: true,
            calibrate_image: true,
        };
        lora.init().await?;

        Ok(lora)
    }

    /// Wait for an IRQ event to occur
    pub async fn wait_for_irq(&mut self) -> Result<(), RadioError> {
        self.radio_kind.await_irq().await
    }

    /// Process an IRQ event and return the new state of the radio
    pub async fn process_irq_event(&mut self) -> Result<Option<IrqState>, RadioError> {
        self.radio_kind.process_irq_event(self.radio_mode, None, false).await
    }

    /// Create modulation parameters for a communication channel
    pub fn create_modulation_params(
        &mut self,
        spreading_factor: SpreadingFactor,
        bandwidth: Bandwidth,
        coding_rate: CodingRate,
        frequency_in_hz: u32,
    ) -> Result<ModulationParams, RadioError> {
        self.radio_kind
            .create_modulation_params(spreading_factor, bandwidth, coding_rate, frequency_in_hz)
    }

    /// Create packet parameters for a send operation on a communication channel
    pub fn create_tx_packet_params(
        &mut self,
        preamble_length: u16,
        implicit_header: bool,
        crc_on: bool,
        iq_inverted: bool,
        modulation_params: &ModulationParams,
    ) -> Result<PacketParams, RadioError> {
        self.radio_kind.create_packet_params(
            preamble_length,
            implicit_header,
            0,
            crc_on,
            iq_inverted,
            modulation_params,
        )
    }

    /// Create packet parameters for a receive operation on a communication channel
    pub fn create_rx_packet_params(
        &mut self,
        preamble_length: u16,
        implicit_header: bool,
        max_payload_length: u8,
        crc_on: bool,
        iq_inverted: bool,
        modulation_params: &ModulationParams,
    ) -> Result<PacketParams, RadioError> {
        self.radio_kind.create_packet_params(
            preamble_length,
            implicit_header,
            max_payload_length,
            crc_on,
            iq_inverted,
            modulation_params,
        )
    }

    /// Initialize a Semtech chip as the radio for LoRa physical layer communications
    pub async fn init(&mut self) -> Result<(), RadioError> {
        self.cold_start = true;
        self.radio_kind.reset(&mut self.delay).await?;
        self.radio_kind.ensure_ready(self.radio_mode).await?;
        self.radio_kind.set_standby().await?;
        self.radio_mode = RadioMode::Standby;
        self.do_cold_start().await
    }

    /// Change the canonical LoRa sync word without resetting the radio.
    pub async fn set_sync_word(&mut self, sync_word: u8) -> Result<(), RadioError> {
        self.radio_kind.ensure_ready(self.radio_mode).await?;
        self.radio_kind.set_standby().await?;
        self.radio_mode = RadioMode::Standby;
        self.radio_kind.set_sync_word(sync_word).await?;
        self.sync_word = sync_word;
        Ok(())
    }

    async fn do_cold_start(&mut self) -> Result<(), RadioError> {
        self.radio_kind.init_lora(self.sync_word).await?;
        self.radio_kind.set_tx_power_and_ramp_time(0, None, false).await?;
        self.radio_kind.set_irq_params(Some(self.radio_mode)).await?;
        self.cold_start = false;
        self.calibrate_image = true;
        Ok(())
    }

    /// Place the LoRa physical layer in standby mode.
    ///
    /// The caller must be the radio owner and must first prove that no TX, SPI, IRQ, or receive
    /// collection operation is in flight. This is a chip-side stop only; it does not cancel or
    /// release any board-level interrupt or wake lease.
    ///
    /// `ensure_ready` performs the required busy wait and wakes chips whose current operation
    /// may have put them to sleep, including duty-cycle receive. The driver's mode record changes
    /// only after both that step and the standby command succeed. A standby-command or RF-switch
    /// failure can leave the hardware state uncertain even though the record retains its previous
    /// value; the owner must recover the radio before starting another operation.
    pub async fn enter_standby(&mut self) -> Result<(), RadioError> {
        self.radio_kind.ensure_ready(self.radio_mode).await?;
        let result = self.radio_kind.set_standby().await;
        record_standby(&mut self.radio_mode, result)
    }

    /// Settle the radio IRQ line before an exclusive, bounded board-level quiet operation.
    ///
    /// Call [`Self::enter_standby`] first, then call this method while exclusively owning the
    /// radio, its SPI bus, and the IRQ input. It clears all latched chip IRQ flags before it
    /// waits for the physical IRQ input to become low. It neither masks an MCU interrupt nor
    /// establishes a lasting quiet lease: the board must keep exclusive ownership until its
    /// flash or other quiet operation completes.
    ///
    /// The method refuses a non-standby logical radio mode. A clear failure leaves the radio's
    /// IRQ state uncertain. A low-wait failure means flags may already be clear, but the board
    /// has no proof that the physical line is low. Cancellation has the same consequence as a
    /// failure: it never grants a quiet-work claim, and cancellation after clearing may require
    /// a fresh settlement before work resumes.
    pub async fn settle_irq_for_quiet_work(&mut self) -> Result<(), RadioError> {
        if self.radio_mode != RadioMode::Standby {
            return Err(RadioError::InvalidRadioMode);
        }
        settle_quiet_irq(&mut self.radio_kind).await
    }

    /// Place the LoRa physical layer in low power mode, specifying cold or warm start (if the Semtech chip supports it)
    pub async fn sleep(&mut self, warm_start_if_possible: bool) -> Result<(), RadioError> {
        if self.radio_mode != RadioMode::Sleep {
            self.radio_kind.ensure_ready(self.radio_mode).await?;
            self.radio_kind
                .set_sleep(warm_start_if_possible, &mut self.delay)
                .await?;
            if !warm_start_if_possible {
                self.cold_start = true;
            }
            self.radio_mode = RadioMode::Sleep;
        }
        Ok(())
    }

    /// Prepare the Semtech chip for a send operation
    pub async fn prepare_for_tx(
        &mut self,
        mdltn_params: &ModulationParams,
        tx_pkt_params: &mut PacketParams,
        output_power: i32,
        buffer: &[u8],
    ) -> Result<(), RadioError> {
        self.prepare_modem(mdltn_params).await?;

        self.radio_kind.set_modulation_params(mdltn_params).await?;
        self.radio_kind
            .set_tx_power_and_ramp_time(output_power, Some(mdltn_params), true)
            .await?;
        self.radio_kind.ensure_ready(self.radio_mode).await?;
        if self.radio_mode != RadioMode::Standby {
            self.radio_kind.set_standby().await?;
            self.radio_mode = RadioMode::Standby;
        }

        tx_pkt_params.set_payload_length(buffer.len())?;
        self.radio_kind.set_packet_params(tx_pkt_params).await?;
        self.radio_kind.set_channel(mdltn_params.frequency_in_hz).await?;
        self.radio_kind.set_payload(buffer).await?;
        self.radio_mode = RadioMode::Transmit;
        self.radio_kind.set_irq_params(Some(self.radio_mode)).await?;
        Ok(())
    }

    /// Execute a send operation
    pub async fn tx(&mut self) -> Result<(), RadioError> {
        if let RadioMode::Transmit = self.radio_mode {
            self.radio_kind.do_tx().await?;
            loop {
                self.wait_for_irq().await?;
                match self.radio_kind.process_irq_event(self.radio_mode, None, true).await {
                    Ok(Some(IrqState::Done | IrqState::PreambleReceived)) => {
                        self.radio_mode = RadioMode::Standby;
                        return Ok(());
                    }
                    Ok(None) => continue,
                    Err(err) => {
                        self.radio_kind.ensure_ready(self.radio_mode).await?;
                        self.radio_kind.set_standby().await?;
                        self.radio_mode = RadioMode::Standby;
                        return Err(err);
                    }
                }
            }
        } else {
            Err(RadioError::InvalidRadioMode)
        }
    }

    /// Prepare radio to receive a frame in either single or continuous packet mode.
    /// Notes:
    /// * sx126x SetRx(0 < timeout < MAX) will listen util LoRa packet header is detected,
    /// therefore we only use 0 (Single Mode) and MAX (continuous) values.
    /// TODO: Find a way to express timeout for sx126x, allowing waiting for packet upto 262s
    /// TODO: Allow DutyCycle as well?
    pub async fn prepare_for_rx(
        &mut self,
        listen_mode: RxMode,
        mdltn_params: &ModulationParams,
        rx_pkt_params: &PacketParams,
    ) -> Result<(), RadioError> {
        defmt::trace!("RX mode: {}", listen_mode);
        self.prepare_modem(mdltn_params).await?;

        self.radio_kind.set_modulation_params(mdltn_params).await?;
        self.radio_kind.set_packet_params(rx_pkt_params).await?;
        self.radio_kind.set_channel(mdltn_params.frequency_in_hz).await?;
        self.radio_mode = listen_mode.into();
        self.radio_kind.set_irq_params(Some(self.radio_mode)).await?;
        Ok(())
    }

    /// Start receiving and wait for result
    pub async fn rx(
        &mut self,
        packet_params: &PacketParams,
        receiving_buffer: &mut [u8],
    ) -> Result<(u8, PacketStatus), RadioError> {
        if let RadioMode::Receive(listen_mode) = self.radio_mode {
            self.radio_kind.do_rx(listen_mode).await?;
            loop {
                self.wait_for_irq().await?;
                match self.radio_kind.process_irq_event(self.radio_mode, None, true).await {
                    Ok(Some(actual_state)) => match actual_state {
                        IrqState::PreambleReceived => continue,
                        IrqState::Done => {
                            let received_len = self.radio_kind.get_rx_payload(packet_params, receiving_buffer).await?;
                            let rx_pkt_status = self.radio_kind.get_rx_packet_status().await?;
                            return Ok((received_len, rx_pkt_status));
                        }
                    },
                    Ok(None) => continue,
                    Err(err) => {
                        // if in rx continuous mode, allow the caller to determine whether to keep receiving
                        if self.radio_mode != RadioMode::Receive(RxMode::Continuous) {
                            self.radio_kind.ensure_ready(self.radio_mode).await?;
                            self.radio_kind.set_standby().await?;
                            self.radio_mode = RadioMode::Standby;
                        }
                        return Err(err);
                    }
                }
            }
        } else {
            Err(RadioError::InvalidRadioMode)
        }
    }

    /// Put the radio into receive without waiting for anything.
    ///
    /// The first half of [`Self::rx`], split out so a caller that must race the radio
    /// against another event can do so safely. See [`Self::rx_collect`] for why.
    pub async fn rx_arm(&mut self) -> Result<(), RadioError> {
        if let RadioMode::Receive(listen_mode) = self.radio_mode {
            self.radio_kind.do_rx(listen_mode).await
        } else {
            Err(RadioError::InvalidRadioMode)
        }
    }

    /// Collect a frame whose interrupt has already been observed. **Never race this.**
    ///
    /// The second half of [`Self::rx`], for the pattern that makes racing the radio safe:
    ///
    /// ```text
    /// lora.rx_arm().await?;                                  // once
    /// loop {
    ///     match select(other_event, lora.wait_for_irq()).await {
    ///         First(..) => { /* the irq waiter is dropped, which is harmless */ }
    ///         Second(..) => { lora.rx_collect(params, buf).await?; }
    ///     }
    /// }
    /// ```
    ///
    /// [`Self::rx`] cannot be used that way. It spends nearly all its time parked in
    /// [`Self::wait_for_irq`], where cancelling costs nothing, but the moment the interrupt
    /// fires it opens SPI transactions to read the IRQ cause and pull the payload out of the
    /// chip's FIFO. Cancelling *there* loses the frame outright: the interrupt has been
    /// consumed, the bytes are still in the FIFO, and the next packet overwrites them, so
    /// nothing anywhere records a loss. Splitting the wait from the collection means only
    /// the interrupt wait is ever raced, and it is the half that is safe to abandon.
    ///
    /// One call consumes one IRQ. A preamble-only or otherwise nonterminal IRQ returns
    /// [`RadioError::ReceivePending`] so the caller's outer loop can race the next IRQ against
    /// its other work. Waiting for a second IRQ inside this method would strand that outer
    /// loop when a partial preamble has no terminal successor.
    ///
    /// The radio stays in continuous receive across a collection, so `rx_arm` is called once
    /// rather than per frame.
    pub async fn rx_collect(
        &mut self,
        packet_params: &PacketParams,
        receiving_buffer: &mut [u8],
    ) -> Result<(u8, PacketStatus), RadioError> {
        if !matches!(self.radio_mode, RadioMode::Receive(_)) {
            return Err(RadioError::InvalidRadioMode);
        }
        match self.radio_kind.process_irq_event(self.radio_mode, None, true).await {
            Ok(Some(IrqState::PreambleReceived)) | Ok(None) => Err(RadioError::ReceivePending),
            Ok(Some(IrqState::Done)) => {
                let received_len = self.radio_kind.get_rx_payload(packet_params, receiving_buffer).await?;
                let rx_pkt_status = self.radio_kind.get_rx_packet_status().await?;
                Ok((received_len, rx_pkt_status))
            }
            Err(err) => {
                if self.radio_mode != RadioMode::Receive(RxMode::Continuous) {
                    self.radio_kind.ensure_ready(self.radio_mode).await?;
                    self.radio_kind.set_standby().await?;
                    self.radio_mode = RadioMode::Standby;
                }
                Err(err)
            }
        }
    }

    /// Prepare the Semtech chip for a channel activity detection operation
    pub async fn prepare_for_cad(&mut self, mdltn_params: &ModulationParams) -> Result<(), RadioError> {
        self.prepare_modem(mdltn_params).await?;

        self.radio_kind.set_modulation_params(mdltn_params).await?;
        self.radio_kind.set_channel(mdltn_params.frequency_in_hz).await?;
        self.radio_mode = RadioMode::ChannelActivityDetection;
        self.radio_kind.set_irq_params(Some(self.radio_mode)).await?;
        Ok(())
    }

    /// Start channel activity detection operation and return the result
    pub async fn cad(&mut self, mdltn_params: &ModulationParams) -> Result<bool, RadioError> {
        if self.radio_mode == RadioMode::ChannelActivityDetection {
            self.radio_kind.do_cad(mdltn_params).await?;
            self.wait_for_irq().await?;
            let mut cad_activity_detected = false;
            match self
                .radio_kind
                .process_irq_event(self.radio_mode, Some(&mut cad_activity_detected), true)
                .await
            {
                Ok(Some(IrqState::Done)) => Ok(cad_activity_detected),
                Err(err) => {
                    self.radio_kind.ensure_ready(self.radio_mode).await?;
                    self.radio_kind.set_standby().await?;
                    self.radio_mode = RadioMode::Standby;
                    Err(err)
                }
                Ok(_) => unreachable!(),
            }
        } else {
            Err(RadioError::InvalidRadioMode)
        }
    }

    /// Place radio in continuous wave mode, generally for regulatory testing
    ///
    /// SemTech app note AN1200.26 “Semtech LoRa FCC 15.247 Guidance” covers usage.
    ///
    /// Presumes that init() is called before this function
    pub async fn continuous_wave(
        &mut self,
        mdltn_params: &ModulationParams,
        output_power: i32,
    ) -> Result<(), RadioError> {
        self.prepare_modem(mdltn_params).await?;

        let tx_pkt_params = self
            .radio_kind
            .create_packet_params(0, false, 16, false, false, mdltn_params)?;
        self.radio_kind.set_packet_params(&tx_pkt_params).await?;
        self.radio_kind.set_modulation_params(mdltn_params).await?;
        self.radio_kind
            .set_tx_power_and_ramp_time(output_power, Some(mdltn_params), true)
            .await?;

        self.radio_kind.ensure_ready(self.radio_mode).await?;
        if self.radio_mode != RadioMode::Standby {
            self.radio_kind.set_standby().await?;
            self.radio_mode = RadioMode::Standby;
        }
        self.radio_kind.set_channel(mdltn_params.frequency_in_hz).await?;
        self.radio_mode = RadioMode::Transmit;
        self.radio_kind.set_irq_params(Some(self.radio_mode)).await?;
        self.radio_kind.set_tx_continuous_wave_mode().await
    }

    async fn prepare_modem(&mut self, mdltn_params: &ModulationParams) -> Result<(), RadioError> {
        self.radio_kind.ensure_ready(self.radio_mode).await?;
        if self.radio_mode != RadioMode::Standby {
            self.radio_kind.set_standby().await?;
            self.radio_mode = RadioMode::Standby;
        }

        if self.cold_start {
            self.do_cold_start().await?;
        }

        if self.calibrate_image {
            self.radio_kind.calibrate_image(mdltn_params.frequency_in_hz).await?;
            self.calibrate_image = false;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::future::Future;
    use core::task::{Context, Poll, Waker};

    fn block_on_ready<F: Future>(future: F) -> F::Output {
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        let mut future = core::pin::pin!(future);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("test future unexpectedly pending"),
        }
    }

    #[derive(Clone, Copy)]
    enum QuietFailure {
        Clear,
        Low,
    }

    struct TestQuietIrq {
        calls: [u8; 2],
        call_count: usize,
        failure: Option<QuietFailure>,
    }

    impl TestQuietIrq {
        fn new(failure: Option<QuietFailure>) -> Self {
            Self {
                calls: [0; 2],
                call_count: 0,
                failure,
            }
        }

        fn record(&mut self, call: u8) {
            self.calls[self.call_count] = call;
            self.call_count += 1;
        }
    }

    impl QuietIrqSettlement for TestQuietIrq {
        async fn clear_irq_flags(&mut self) -> Result<(), RadioError> {
            self.record(1);
            match self.failure {
                Some(QuietFailure::Clear) => Err(RadioError::SPI),
                _ => Ok(()),
            }
        }

        async fn await_irq_low(&mut self) -> Result<(), RadioError> {
            self.record(2);
            match self.failure {
                Some(QuietFailure::Low) => Err(RadioError::Irq),
                _ => Ok(()),
            }
        }
    }

    #[test]
    fn record_standby_updates_mode_only_after_hardware_success() {
        let mut mode = RadioMode::Receive(RxMode::Continuous);

        assert_eq!(record_standby(&mut mode, Ok(())), Ok(()));
        assert!(mode == RadioMode::Standby);

        mode = RadioMode::Receive(RxMode::Continuous);
        assert_eq!(record_standby(&mut mode, Err(RadioError::Busy)), Err(RadioError::Busy));
        assert!(mode == RadioMode::Receive(RxMode::Continuous));
    }

    #[test]
    fn quiet_irq_settlement_clears_before_awaiting_low() {
        let mut irq = TestQuietIrq::new(None);

        assert_eq!(block_on_ready(settle_quiet_irq(&mut irq)), Ok(()));
        assert_eq!(irq.call_count, 2);
        assert_eq!(irq.calls, [1, 2]);
    }

    #[test]
    fn quiet_irq_settlement_propagates_failures_without_skipping_the_boundary() {
        let mut clear_failure = TestQuietIrq::new(Some(QuietFailure::Clear));
        assert_eq!(
            block_on_ready(settle_quiet_irq(&mut clear_failure)),
            Err(RadioError::SPI)
        );
        assert_eq!(clear_failure.call_count, 1);
        assert_eq!(clear_failure.calls[0], 1);

        let mut low_failure = TestQuietIrq::new(Some(QuietFailure::Low));
        assert_eq!(block_on_ready(settle_quiet_irq(&mut low_failure)), Err(RadioError::Irq));
        assert_eq!(low_failure.call_count, 2);
        assert_eq!(low_failure.calls, [1, 2]);
    }
}
