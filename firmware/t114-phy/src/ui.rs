//! T114 ST7789, user-button, and green-LED adapter for `radio-face`.

use core::convert::Infallible;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_futures::select::{Either, select};
use embassy_nrf::{
    gpio::{Input, Output},
    spim::Spim,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::{
    Pixel,
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    pixelcolor::BinaryColor,
};
use radio_face::{
    Action, Button, Controller, HostSnapshot, InputEvent, InputProfile, LedIntent, LedSignal,
    LocalStatus, Page, PressClassifier, Screen, Surface, Theme, WakeSource, led_intent, render,
};

const WIDTH: usize = 240;
const HEIGHT: usize = 135;
const BUFFER_LEN: usize = WIDTH * HEIGHT / 8;
const LINE_BYTES: usize = WIDTH * 2;
const COLUMN_OFFSET: u16 = 40;
const ROW_OFFSET: u16 = 53;
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
static HOST: Signal<CriticalSectionRawMutex, HostSnapshot> = Signal::new();
static HEALTH: AtomicU32 = AtomicU32::new(0);

const HEALTH_INITIALIZED: u32 = 1 << 0;
const HEALTH_FRAME_OK: u32 = 1 << 1;
const HEALTH_DISPLAY_ON: u32 = 1 << 2;
const HEALTH_BUTTON_SEEN: u32 = 1 << 3;
const HEALTH_ERROR: u32 = 1 << 4;
const HEALTH_BUTTON_REV21: u32 = 1 << 5;
const HEALTH_BUTTON_VARIANT: u32 = 1 << 6;
const HEALTH_HOST_PENDING: u32 = 1 << 7;
const HEALTH_SCREEN_SHIFT: u32 = 8;
const HEALTH_SCREEN_MASK: u32 = 0x0f << HEALTH_SCREEN_SHIFT;
const HEALTH_HOST_FRESH: u32 = 1 << 12;

pub fn publish(status: LocalStatus, led: LedSignal) {
    STATUS.signal(UiUpdate { status, led });
}

pub fn publish_host(snapshot: HostSnapshot) {
    HEALTH.fetch_and(!HEALTH_HOST_FRESH, Ordering::Relaxed);
    HEALTH.fetch_or(HEALTH_HOST_PENDING, Ordering::Relaxed);
    HOST.signal(snapshot);
}

#[derive(Clone, Copy)]
struct ActiveHost {
    snapshot: HostSnapshot,
    received_at: Instant,
}

pub struct Diagnostic {
    pub state: &'static str,
    pub display: &'static str,
    pub screen: &'static str,
    pub button: &'static str,
    pub host: &'static str,
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
    let button = if health & HEALTH_BUTTON_REV21 != 0 {
        "p1.11"
    } else if health & HEALTH_BUTTON_VARIANT != 0 {
        "p1.10"
    } else if health & HEALTH_BUTTON_SEEN != 0 {
        "seen"
    } else {
        "none"
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
        button,
        host: if health & HEALTH_HOST_FRESH != 0 {
            "fresh"
        } else if health & HEALTH_HOST_PENDING != 0 {
            "pending"
        } else {
            "none"
        },
    }
}

#[embassy_executor::task]
pub async fn button_task(mut rev21: Input<'static>, mut variant: Input<'static>) {
    let mut classifier = PressClassifier::default();
    loop {
        while rev21.is_low() || variant.is_low() {
            Timer::after(BUTTON_DEBOUNCE).await;
        }

        let source = match select(
            rev21.wait_for_falling_edge(),
            variant.wait_for_falling_edge(),
        )
        .await
        {
            Either::First(()) => HEALTH_BUTTON_REV21,
            Either::Second(()) => HEALTH_BUTTON_VARIANT,
        };
        Timer::after(BUTTON_DEBOUNCE).await;
        if rev21.is_high() && variant.is_high() {
            continue;
        }

        let pressed_at = Instant::now().as_millis() as u32;
        let _ = classifier.edge(Button::A, true, pressed_at);
        match source {
            HEALTH_BUTTON_REV21 => rev21.wait_for_rising_edge().await,
            _ => variant.wait_for_rising_edge().await,
        }
        let released_at = Instant::now().as_millis() as u32;
        Timer::after(BUTTON_DEBOUNCE).await;
        if let Some(event) = classifier.edge(Button::A, false, released_at) {
            HEALTH.fetch_or(HEALTH_BUTTON_SEEN | source, Ordering::Relaxed);
            BUTTON.signal(event);
        }
    }
}

pub struct ScreenHardware {
    spim: Spim<'static>,
    cs: Output<'static>,
    dc: Output<'static>,
    reset: Output<'static>,
    power: Output<'static>,
    backlight: Output<'static>,
    led: Output<'static>,
}

pub fn screen_hardware(
    spim: Spim<'static>,
    cs: Output<'static>,
    dc: Output<'static>,
    reset: Output<'static>,
    power: Output<'static>,
    backlight: Output<'static>,
    led: Output<'static>,
) -> ScreenHardware {
    ScreenHardware {
        spim,
        cs,
        dc,
        reset,
        power,
        backlight,
        led,
    }
}

#[embassy_executor::task]
pub async fn screen_task(hardware: ScreenHardware, initial: LocalStatus) {
    let mut tft = Tft::new(hardware);
    if tft.init().await.is_err() {
        HEALTH.store(HEALTH_ERROR, Ordering::Relaxed);
        loop {
            tft.fault_triple().await;
            Timer::after_millis(500).await;
        }
    }
    HEALTH.store(HEALTH_INITIALIZED | HEALTH_DISPLAY_ON, Ordering::Relaxed);

    let mut frame = FrameBuffer::new();
    let mut controller = Controller::default();
    let mut local = initial;
    let mut active_host = None;
    let started = Instant::now();
    render_screen(&mut tft, &mut frame, Screen::Boot, &local, None).await;
    Timer::after(BOOT_HOLD).await;

    loop {
        let event = select(
            select(select(STATUS.wait(), BUTTON.wait()), HOST.wait()),
            Timer::after(UI_TICK),
        )
        .await;
        match event {
            Either::First(Either::First(Either::First(update))) => {
                local = update.status;
                refresh_clock(&mut local, started);
                local.display_on = controller.display_on();
                let host = fresh_host(&mut active_host);
                tft.run_led(led_intent(&local, update.led)).await;
                if local.fault.is_some() {
                    set_display(&mut tft, true).await;
                    render_current(&mut tft, &mut frame, &controller, &local, host.as_ref()).await;
                } else if controller.display_on() {
                    render_current(&mut tft, &mut frame, &controller, &local, host.as_ref()).await;
                } else {
                    set_display(&mut tft, false).await;
                }
            }
            Either::First(Either::First(Either::Second(input))) => {
                refresh_clock(&mut local, started);
                local.last_wake = WakeSource::Button;
                let host = fresh_host(&mut active_host);
                let action =
                    controller.handle(InputProfile::OneButton, input, &local, host.as_ref());
                local.display_on = controller.display_on();
                match action {
                    Action::DisplayWoke => set_display(&mut tft, true).await,
                    Action::DisplayTurnedOff => {
                        render_screen(
                            &mut tft,
                            &mut frame,
                            Screen::DisplayOff,
                            &local,
                            host.as_ref(),
                        )
                        .await;
                        Timer::after(DISPLAY_OFF_DELAY).await;
                        if local.fault.is_none() {
                            set_display(&mut tft, false).await;
                        }
                        tft.led.set_high();
                        continue;
                    }
                    Action::RequestReboot => cortex_m::peripheral::SCB::sys_reset(),
                    Action::BrightnessChanged(_)
                    | Action::None
                    | Action::DetailPolicyChanged(_) => {}
                }
                if controller.display_on() || local.fault.is_some() {
                    render_current(&mut tft, &mut frame, &controller, &local, host.as_ref()).await;
                }
                tft.led.set_high();
            }
            Either::First(Either::Second(snapshot)) => {
                active_host = Some(ActiveHost {
                    snapshot,
                    received_at: Instant::now(),
                });
                HEALTH.fetch_and(!HEALTH_HOST_PENDING, Ordering::Relaxed);
                HEALTH.fetch_or(HEALTH_HOST_FRESH, Ordering::Relaxed);
                let host = Some(snapshot);
                if controller.display_on() || local.fault.is_some() {
                    render_current(&mut tft, &mut frame, &controller, &local, host.as_ref()).await;
                }
            }
            Either::Second(()) => {
                refresh_clock(&mut local, started);
                let host = fresh_host(&mut active_host);
                if local.fault.is_some() {
                    set_display(&mut tft, true).await;
                    render_current(&mut tft, &mut frame, &controller, &local, host.as_ref()).await;
                    tft.fault_triple().await;
                } else if controller.display_on() {
                    render_current(&mut tft, &mut frame, &controller, &local, host.as_ref()).await;
                    tft.led.set_high();
                }
            }
        }
    }
}

fn refresh_clock(local: &mut LocalStatus, started: Instant) {
    local.uptime_secs = started.elapsed().as_secs().min(u64::from(u32::MAX)) as u32;
}

fn fresh_host(active: &mut Option<ActiveHost>) -> Option<HostSnapshot> {
    if active.as_ref().is_some_and(|host| {
        !host.snapshot.is_fresh(
            host.received_at
                .elapsed()
                .as_secs()
                .min(u64::from(u32::MAX)) as u32,
        )
    }) {
        *active = None;
        HEALTH.fetch_and(!HEALTH_HOST_FRESH, Ordering::Relaxed);
    }
    active.as_ref().map(|host| host.snapshot)
}

async fn render_current(
    tft: &mut Tft,
    frame: &mut FrameBuffer,
    controller: &Controller,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) {
    render_screen(tft, frame, controller.screen(local, host), local, host).await;
}

async fn render_screen(
    tft: &mut Tft,
    frame: &mut FrameBuffer,
    screen: Screen,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) {
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
    let rendered = render(frame, Surface::Tft240x135, theme, screen, local, host).is_ok();
    let flushed = tft.flush(frame).await.is_ok();
    if rendered && flushed {
        HEALTH.fetch_or(HEALTH_FRAME_OK, Ordering::Relaxed);
        HEALTH.fetch_and(!HEALTH_ERROR, Ordering::Relaxed);
    } else {
        HEALTH.fetch_and(!HEALTH_FRAME_OK, Ordering::Relaxed);
        HEALTH.fetch_or(HEALTH_ERROR, Ordering::Relaxed);
    }
}

async fn set_display(tft: &mut Tft, on: bool) {
    let result = if on {
        tft.display_on().await
    } else {
        tft.display_off().await
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

struct FrameBuffer {
    bytes: [u8; BUFFER_LEN],
}

impl FrameBuffer {
    const fn new() -> Self {
        Self {
            bytes: [0; BUFFER_LEN],
        }
    }

    fn pixel(&self, x: usize, y: usize) -> bool {
        let index = y * WIDTH + x;
        self.bytes[index / 8] & (1 << (7 - index % 8)) != 0
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
            let index = point.y as usize * WIDTH + point.x as usize;
            let mask = 1 << (7 - index % 8);
            match color {
                BinaryColor::On => self.bytes[index / 8] |= mask,
                BinaryColor::Off => self.bytes[index / 8] &= !mask,
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

struct Tft {
    spim: Spim<'static>,
    cs: Output<'static>,
    dc: Output<'static>,
    reset: Output<'static>,
    power: Output<'static>,
    backlight: Output<'static>,
    led: Output<'static>,
}

impl Tft {
    fn new(hardware: ScreenHardware) -> Self {
        Self {
            spim: hardware.spim,
            cs: hardware.cs,
            dc: hardware.dc,
            reset: hardware.reset,
            power: hardware.power,
            backlight: hardware.backlight,
            led: hardware.led,
        }
    }

    async fn init(&mut self) -> Result<(), embassy_nrf::spim::Error> {
        self.cs.set_high();
        self.dc.set_low();
        self.led.set_high();
        self.backlight.set_high();
        self.power.set_low();
        Timer::after_millis(20).await;
        self.reset.set_low();
        Timer::after_millis(20).await;
        self.reset.set_high();
        Timer::after_millis(120).await;

        self.command(0x01, &[]).await?;
        Timer::after_millis(150).await;
        self.command(0x11, &[]).await?;
        Timer::after_millis(120).await;
        self.command(0x3a, &[0x55]).await?;
        self.command(0x36, &[0x60]).await?;
        self.command(0x21, &[]).await?;
        self.command(0x13, &[]).await?;
        Timer::after_millis(10).await;
        self.command(0x29, &[]).await?;
        Timer::after_millis(100).await;
        self.backlight.set_low();
        Ok(())
    }

    async fn display_on(&mut self) -> Result<(), embassy_nrf::spim::Error> {
        self.power.set_low();
        Timer::after_millis(5).await;
        self.command(0x11, &[]).await?;
        Timer::after_millis(120).await;
        self.command(0x29, &[]).await?;
        self.backlight.set_low();
        Ok(())
    }

    async fn display_off(&mut self) -> Result<(), embassy_nrf::spim::Error> {
        self.backlight.set_high();
        self.command(0x28, &[]).await?;
        self.command(0x10, &[]).await?;
        Timer::after_millis(5).await;
        self.power.set_high();
        Ok(())
    }

    async fn flush(&mut self, frame: &FrameBuffer) -> Result<(), embassy_nrf::spim::Error> {
        let x0 = COLUMN_OFFSET;
        let x1 = x0 + WIDTH as u16 - 1;
        let y0 = ROW_OFFSET;
        let y1 = y0 + HEIGHT as u16 - 1;
        self.command(
            0x2a,
            &[(x0 >> 8) as u8, x0 as u8, (x1 >> 8) as u8, x1 as u8],
        )
        .await?;
        self.command(
            0x2b,
            &[(y0 >> 8) as u8, y0 as u8, (y1 >> 8) as u8, y1 as u8],
        )
        .await?;

        self.cs.set_low();
        self.dc.set_low();
        self.spim.write(&[0x2c]).await?;
        self.dc.set_high();
        let mut line = [0_u8; LINE_BYTES];
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let color = if frame.pixel(x, y) { u16::MAX } else { 0 };
                line[x * 2] = (color >> 8) as u8;
                line[x * 2 + 1] = color as u8;
            }
            self.spim.write(&line).await?;
        }
        self.cs.set_high();
        Ok(())
    }

    async fn command(&mut self, command: u8, data: &[u8]) -> Result<(), embassy_nrf::spim::Error> {
        self.cs.set_low();
        self.dc.set_low();
        self.spim.write(&[command]).await?;
        if !data.is_empty() {
            self.dc.set_high();
            self.spim.write(data).await?;
        }
        self.cs.set_high();
        Ok(())
    }

    async fn run_led(&mut self, intent: LedIntent) {
        match intent {
            LedIntent::Off => self.led.set_high(),
            LedIntent::DoublePulse => self.double_pulse().await,
            LedIntent::SlowPulse => {
                self.led.set_low();
                Timer::after_millis(250).await;
                self.led.set_high();
            }
            LedIntent::FaultTriple => self.fault_triple().await,
        }
    }

    async fn double_pulse(&mut self) {
        for _ in 0..2 {
            self.led.set_low();
            Timer::after_millis(55).await;
            self.led.set_high();
            Timer::after_millis(90).await;
        }
    }

    async fn fault_triple(&mut self) {
        for _ in 0..3 {
            self.led.set_low();
            Timer::after_millis(80).await;
            self.led.set_high();
            Timer::after_millis(100).await;
        }
    }
}
