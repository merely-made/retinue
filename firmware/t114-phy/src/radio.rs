//! This board's SX1262: how it is wired, and how to read its registers.
//!
//! All of it is T114-specific, which is why it is here rather than in `radio-hand`. The
//! SPI is bit-banged in software because the T114 routes the radio to pins the nRF52840's
//! SPIM cannot serve alongside the display's, and the interrupt line is polled rather than
//! sensed so nothing depends on GPIO wakeups surviving the SoftDevice bootloader boundary.

use core::convert::Infallible;

use embassy_nrf::gpio::{Input, Output};
use embassy_time::Timer;
use embedded_hal::spi::ErrorType;
use embedded_hal_async::spi::SpiBus;
use lora_phy::LoRa;
use lora_phy::mod_params::RadioError;
use lora_phy::mod_traits::InterfaceVariant;
use lora_phy::sx126x::Sx126x;
use radio_hand::executive::ChipDiagnostics;
use selvage::EVENT_DIAGNOSTIC;

/// Software SPI over three GPIOs.
pub struct T114Spi<'d> {
    pub sck: Output<'d>,
    pub mosi: Output<'d>,
    pub miso: Input<'d>,
}

impl T114Spi<'_> {
    fn transfer_byte(&mut self, output: u8) -> u8 {
        let mut input = 0u8;
        for bit in (0..8).rev() {
            if output & (1 << bit) == 0 {
                self.mosi.set_low();
            } else {
                self.mosi.set_high();
            }
            cortex_m::asm::delay(64);
            self.sck.set_high();
            input = (input << 1) | u8::from(self.miso.is_high());
            cortex_m::asm::delay(64);
            self.sck.set_low();
        }
        input
    }
}

impl ErrorType for T114Spi<'_> {
    type Error = Infallible;
}

impl SpiBus<u8> for T114Spi<'_> {
    async fn read(&mut self, words: &mut [u8]) -> Result<(), Self::Error> {
        for word in words {
            *word = self.transfer_byte(0);
        }
        Ok(())
    }

    async fn write(&mut self, words: &[u8]) -> Result<(), Self::Error> {
        for &word in words {
            let _ = self.transfer_byte(word);
        }
        Ok(())
    }

    async fn transfer(&mut self, read: &mut [u8], write: &[u8]) -> Result<(), Self::Error> {
        for index in 0..read.len().max(write.len()) {
            let input = self.transfer_byte(write.get(index).copied().unwrap_or(0));
            if let Some(word) = read.get_mut(index) {
                *word = input;
            }
        }
        Ok(())
    }

    async fn transfer_in_place(&mut self, words: &mut [u8]) -> Result<(), Self::Error> {
        for word in words {
            *word = self.transfer_byte(*word);
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// The radio's reset, interrupt, and busy lines.
pub struct T114Interface<'d> {
    pub reset: Output<'d>,
    pub dio1: Input<'d>,
    pub busy: Input<'d>,
}

impl InterfaceVariant for T114Interface<'_> {
    async fn reset(&mut self, delay: &mut impl lora_phy::DelayNs) -> Result<(), RadioError> {
        delay.delay_ms(10).await;
        self.reset.set_low();
        delay.delay_ms(20).await;
        self.reset.set_high();
        delay.delay_ms(10).await;
        Ok(())
    }

    async fn wait_on_busy(&mut self) -> Result<(), RadioError> {
        while self.busy.is_high() {
            Timer::after_micros(50).await;
        }
        Ok(())
    }

    async fn await_irq(&mut self) -> Result<(), RadioError> {
        // SX1262 holds DIO1 high until its IRQ flags are cleared. Polling the level avoids
        // depending on GPIO sense wakeups across the T114's SoftDevice bootloader boundary
        // and cannot miss the latched event.
        while self.dio1.is_low() {
            Timer::after_millis(1).await;
        }
        Ok(())
    }

    async fn await_irq_low(&mut self) -> Result<(), RadioError> {
        while self.dio1.is_high() {
            Timer::after_millis(1).await;
        }
        Ok(())
    }

    async fn enable_rf_switch_rx(&mut self) -> Result<(), RadioError> {
        Ok(())
    }

    async fn enable_rf_switch_tx(&mut self) -> Result<(), RadioError> {
        Ok(())
    }

    async fn disable_rf_switch(&mut self) -> Result<(), RadioError> {
        Ok(())
    }
}

/// The board's own SX1262 registers.
///
/// Chip-specific — `sx126x_diagnostics` is on the SX126x kind, not on `LoRa` — so the shared
/// executive reaches it through [`ChipDiagnostics`] rather than calling it directly.
pub struct Sx126xDiagnostics;

impl<SPI, IV, C, DLY> ChipDiagnostics<Sx126x<SPI, IV, C>, DLY> for Sx126xDiagnostics
where
    SPI: embedded_hal_async::spi::SpiDevice<u8>,
    IV: InterfaceVariant,
    C: lora_phy::sx126x::Sx126xVariant,
    DLY: lora_phy::DelayNs,
{
    async fn read(&self, lora: &mut LoRa<Sx126x<SPI, IV, C>, DLY>) -> [u8; 7] {
        match lora.sx126x_diagnostics().await {
            Ok(d) => diagnostic_event(d.irq_status, d.device_errors, d.sync_word),
            Err(_) => [EVENT_DIAGNOSTIC, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        }
    }
}

fn diagnostic_event(irq_status: u16, device_errors: u16, sync_word: [u8; 2]) -> [u8; 7] {
    let irq = irq_status.to_le_bytes();
    let errors = device_errors.to_le_bytes();
    [
        EVENT_DIAGNOSTIC,
        irq[0],
        irq[1],
        errors[0],
        errors[1],
        sync_word[0],
        sync_word[1],
    ]
}
