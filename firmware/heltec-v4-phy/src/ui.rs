//! V4 OLED, button, and LED adapter for `radio-face`.

use core::convert::Infallible;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::{
    Pixel,
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    pixelcolor::BinaryColor,
};
use esp_hal::{
    Async,
    gpio::{Input, Output},
    i2c::master::I2c,
};
use radio_face::{
    Action, Button, Controller, InputEvent, InputProfile, LedIntent, LedSignal, LocalStatus, Page,
    PressClassifier, Screen, Surface, Theme, WakeSource, led_intent, render,
};

use crate::board::OLED_ADDRESS;

const WIDTH: usize = 128;
const HEIGHT: usize = 64;
const BUFFER_LEN: usize = WIDTH * HEIGHT / 8;
const BOOT_HOLD: Duration = Duration::from_millis(700);
const DISPLAY_OFF_DELAY: Duration = Duration::from_millis(700);
const UI_TICK: Duration = Duration::from_secs(1);
const BUTTON_DEBOUNCE: Duration = Duration::from_millis(20);

#[derive(Clone, Copy)]
struct UiUpdate {
    status: LocalStatus,
    led: LedSignal,
}

static STATUS: Signal<CriticalSectionRawMutex, UiUpdate> = Signal::new();
static BUTTON: Signal<CriticalSectionRawMutex, InputEvent> = Signal::new();
static HEALTH: AtomicU32 = AtomicU32::new(0);

const HEALTH_INITIALIZED: u32 = 1 << 0;
const HEALTH_FRAME_OK: u32 = 1 << 1;
const HEALTH_DISPLAY_ON: u32 = 1 << 2;
const HEALTH_BUTTON_SEEN: u32 = 1 << 3;
const HEALTH_ERROR: u32 = 1 << 4;
const HEALTH_SCREEN_SHIFT: u32 = 8;
const HEALTH_SCREEN_MASK: u32 = 0x0f << HEALTH_SCREEN_SHIFT;

pub fn publish(status: LocalStatus, led: LedSignal) {
    STATUS.signal(UiUpdate { status, led });
}

pub struct Diagnostic {
    pub state: &'static str,
    pub display: &'static str,
    pub screen: &'static str,
    pub button: u8,
}

pub fn diagnostic() -> Diagnostic {
    let health = HEALTH.load(Ordering::Relaxed);
    let state = if health & HEALTH_INITIALIZED == 0 {
        "starting"
    } else if health & HEALTH_ERROR != 0 || health & HEALTH_FRAME_OK == 0 {
        "error"
    } else {
        "ok"
    };
    Diagnostic {
        state,
        display: if health & HEALTH_DISPLAY_ON != 0 {
            "on"
        } else {
            "off"
        },
        screen: match (health & HEALTH_SCREEN_MASK) >> HEALTH_SCREEN_SHIFT {
            1 => "boot",
            2 => "status",
            3 => "power",
            4 => "radio",
            5 => "traffic",
            6 => "identity",
            7 => "links",
            8 => "peers",
            9 => "menu",
            10 => "verify",
            11 => "fault",
            12 => "display-off",
            _ => "unknown",
        },
        button: u8::from(health & HEALTH_BUTTON_SEEN != 0),
    }
}

#[embassy_executor::task]
pub async fn button_task(mut button: Input<'static>) {
    let mut classifier = PressClassifier::default();
    loop {
        if button.is_low() {
            button.wait_for_rising_edge().await;
        }
        button.wait_for_falling_edge().await;
        Timer::after(BUTTON_DEBOUNCE).await;
        if !button.is_low() {
            continue;
        }

        let pressed_at = Instant::now().as_millis() as u32;
        let _ = classifier.edge(Button::A, true, pressed_at);
        button.wait_for_rising_edge().await;
        let released_at = Instant::now().as_millis() as u32;
        Timer::after(BUTTON_DEBOUNCE).await;
        if let Some(event) = classifier.edge(Button::A, false, released_at) {
            HEALTH.fetch_or(HEALTH_BUTTON_SEEN, Ordering::Relaxed);
            BUTTON.signal(event);
        }
    }
}

#[embassy_executor::task]
pub async fn screen_task(
    i2c: I2c<'static, Async>,
    mut reset: Output<'static>,
    mut vext: Output<'static>,
    mut led: Output<'static>,
    initial: LocalStatus,
) {
    led.set_low();
    vext.set_low();
    Timer::after_millis(10).await;
    reset.set_high();
    Timer::after_millis(1).await;
    reset.set_low();
    Timer::after_millis(10).await;
    reset.set_high();
    Timer::after_millis(10).await;

    let mut oled = Oled::new(i2c);
    if oled.init().await.is_err() {
        HEALTH.store(HEALTH_ERROR, Ordering::Relaxed);
        loop {
            fault_triple(&mut led).await;
            Timer::after_millis(500).await;
        }
    }
    HEALTH.store(HEALTH_INITIALIZED | HEALTH_DISPLAY_ON, Ordering::Relaxed);

    let mut frame = FrameBuffer::new();
    let mut controller = Controller::default();
    let mut local = initial;
    let started = Instant::now();
    render_screen(&mut oled, &mut frame, Screen::Boot, &local).await;
    Timer::after(BOOT_HOLD).await;

    loop {
        let event = select(select(STATUS.wait(), BUTTON.wait()), Timer::after(UI_TICK)).await;

        match event {
            Either::First(Either::First(update)) => {
                local = update.status;
                refresh_clock(&mut local, started);
                local.display_on = controller.display_on();

                run_led(&mut led, led_intent(&local, update.led)).await;
                if local.fault.is_some() {
                    set_display(&mut oled, true).await;
                    render_current(&mut oled, &mut frame, &controller, &local).await;
                } else if controller.display_on() {
                    render_current(&mut oled, &mut frame, &controller, &local).await;
                } else {
                    set_display(&mut oled, false).await;
                }
            }
            Either::First(Either::Second(input)) => {
                refresh_clock(&mut local, started);
                local.last_wake = WakeSource::Button;
                let action = controller.handle(InputProfile::OneButton, input, &local, None);
                local.display_on = controller.display_on();

                match action {
                    Action::DisplayWoke => {
                        set_display(&mut oled, true).await;
                    }
                    Action::DisplayTurnedOff => {
                        render_screen(&mut oled, &mut frame, Screen::DisplayOff, &local).await;
                        Timer::after(DISPLAY_OFF_DELAY).await;
                        if local.fault.is_none() {
                            set_display(&mut oled, false).await;
                        }
                        led.set_low();
                        continue;
                    }
                    Action::BrightnessChanged(level) => {
                        let _ = oled.set_brightness(level).await;
                    }
                    Action::RequestReboot => esp_hal::system::software_reset(),
                    Action::None | Action::DetailPolicyChanged(_) => {}
                }

                if controller.display_on() || local.fault.is_some() {
                    render_current(&mut oled, &mut frame, &controller, &local).await;
                }
                led.set_low();
            }
            Either::Second(()) => {
                refresh_clock(&mut local, started);
                if local.fault.is_some() {
                    set_display(&mut oled, true).await;
                    render_current(&mut oled, &mut frame, &controller, &local).await;
                    fault_triple(&mut led).await;
                } else if controller.display_on() {
                    render_current(&mut oled, &mut frame, &controller, &local).await;
                    led.set_low();
                }
            }
        }
    }
}

fn refresh_clock(local: &mut LocalStatus, started: Instant) {
    local.uptime_secs = started.elapsed().as_secs().min(u64::from(u32::MAX)) as u32;
}

async fn render_current<I>(
    oled: &mut Oled<I>,
    frame: &mut FrameBuffer,
    controller: &Controller,
    local: &LocalStatus,
) where
    I: embedded_hal_async::i2c::I2c,
{
    render_screen(oled, frame, controller.screen(local, None), local).await;
}

async fn render_screen<I>(
    oled: &mut Oled<I>,
    frame: &mut FrameBuffer,
    screen: Screen,
    local: &LocalStatus,
) where
    I: embedded_hal_async::i2c::I2c,
{
    let screen_code = match screen {
        Screen::Boot => 1,
        Screen::Page(Page::Status) => 2,
        Screen::Page(Page::Power) => 3,
        Screen::Page(Page::Radio) => 4,
        Screen::Page(Page::Traffic) => 5,
        Screen::Page(Page::Identity) => 6,
        Screen::Page(Page::Links) => 7,
        Screen::Page(Page::Peers) => 8,
        Screen::Menu { .. } => 9,
        Screen::Verify => 10,
        Screen::Fault => 11,
        Screen::DisplayOff => 12,
    };
    let _ = HEALTH.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |health| {
        Some((health & !HEALTH_SCREEN_MASK) | (screen_code << HEALTH_SCREEN_SHIFT))
    });
    let theme = Theme::new(
        BinaryColor::Off,
        BinaryColor::On,
        BinaryColor::On,
        BinaryColor::On,
    );
    let rendered = render(frame, Surface::Oled128x64, theme, screen, local, None).is_ok();
    let flushed = oled.flush(frame).await.is_ok();
    if rendered && flushed {
        HEALTH.fetch_or(HEALTH_FRAME_OK, Ordering::Relaxed);
        HEALTH.fetch_and(!HEALTH_ERROR, Ordering::Relaxed);
    } else {
        HEALTH.fetch_and(!HEALTH_FRAME_OK, Ordering::Relaxed);
        HEALTH.fetch_or(HEALTH_ERROR, Ordering::Relaxed);
    }
}

async fn set_display<I>(oled: &mut Oled<I>, on: bool)
where
    I: embedded_hal_async::i2c::I2c,
{
    let result = if on {
        oled.display_on().await
    } else {
        oled.display_off().await
    };
    if result.is_ok() {
        if on {
            HEALTH.fetch_or(HEALTH_DISPLAY_ON, Ordering::Relaxed);
        } else {
            HEALTH.fetch_and(!HEALTH_DISPLAY_ON, Ordering::Relaxed);
        }
    } else {
        HEALTH.fetch_or(HEALTH_ERROR, Ordering::Relaxed);
    }
}

async fn run_led(led: &mut Output<'_>, intent: LedIntent) {
    match intent {
        LedIntent::Off => led.set_low(),
        LedIntent::DoublePulse => double_pulse(led).await,
        LedIntent::SlowPulse => {
            led.set_high();
            Timer::after_millis(250).await;
            led.set_low();
        }
        LedIntent::FaultTriple => fault_triple(led).await,
    }
}

async fn double_pulse(led: &mut Output<'_>) {
    for _ in 0..2 {
        led.set_high();
        Timer::after_millis(55).await;
        led.set_low();
        Timer::after_millis(90).await;
    }
}

async fn fault_triple(led: &mut Output<'_>) {
    for _ in 0..3 {
        led.set_high();
        Timer::after_millis(80).await;
        led.set_low();
        Timer::after_millis(100).await;
    }
}

struct FrameBuffer {
    bytes: [u8; BUFFER_LEN],
}

impl FrameBuffer {
    const fn new() -> Self {
        Self {
            bytes: [0; BUFFER_LEN],
        }
    }
}

impl OriginDimensions for FrameBuffer {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

impl DrawTarget for FrameBuffer {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<T>(&mut self, pixels: T) -> Result<(), Self::Error>
    where
        T: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x < 0 || point.y < 0 || point.x >= WIDTH as i32 || point.y >= HEIGHT as i32 {
                continue;
            }
            let x = point.x as usize;
            let y = point.y as usize;
            let index = x + (y / 8) * WIDTH;
            let mask = 1 << (y % 8);
            match color {
                BinaryColor::On => self.bytes[index] |= mask,
                BinaryColor::Off => self.bytes[index] &= !mask,
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.bytes.fill(match color {
            BinaryColor::On => u8::MAX,
            BinaryColor::Off => 0,
        });
        Ok(())
    }
}

struct Oled<I> {
    i2c: I,
}

impl<I> Oled<I>
where
    I: embedded_hal_async::i2c::I2c,
{
    fn new(i2c: I) -> Self {
        Self { i2c }
    }

    async fn init(&mut self) -> Result<(), I::Error> {
        self.command(&[
            0xae, 0xd5, 0x80, 0xa8, 0x3f, 0xd3, 0x00, 0x40, 0x8d, 0x14, 0x20, 0x00, 0xa1, 0xc8,
            0xda, 0x12, 0x81, 0x9f, 0xd9, 0xf1, 0xdb, 0x40, 0xa4, 0xa6, 0x2e, 0xaf,
        ])
        .await
    }

    async fn display_on(&mut self) -> Result<(), I::Error> {
        self.command(&[0xaf]).await
    }

    async fn display_off(&mut self) -> Result<(), I::Error> {
        self.command(&[0xae]).await
    }

    async fn set_brightness(&mut self, level: u8) -> Result<(), I::Error> {
        let contrast = match level.clamp(1, 5) {
            1 => 0x20,
            2 => 0x50,
            3 => 0x80,
            4 => 0xb0,
            _ => 0xff,
        };
        self.command(&[0x81, contrast]).await
    }

    async fn flush(&mut self, frame: &FrameBuffer) -> Result<(), I::Error> {
        self.command(&[0x21, 0, 127, 0x22, 0, 7]).await?;
        let mut packet = [0_u8; 32];
        packet[0] = 0x40;
        for chunk in frame.bytes.chunks(31) {
            packet[1..1 + chunk.len()].copy_from_slice(chunk);
            self.i2c
                .write(OLED_ADDRESS, &packet[..1 + chunk.len()])
                .await?;
        }
        Ok(())
    }

    async fn command(&mut self, commands: &[u8]) -> Result<(), I::Error> {
        let mut packet = [0_u8; 32];
        for chunk in commands.chunks(31) {
            packet[0] = 0;
            packet[1..1 + chunk.len()].copy_from_slice(chunk);
            self.i2c
                .write(OLED_ADDRESS, &packet[..1 + chunk.len()])
                .await?;
        }
        Ok(())
    }
}
