use core::fmt::Write as _;

use embedded_graphics::{
    mono_font::{
        MonoFont, MonoTextStyleBuilder,
        ascii::{FONT_4X6, FONT_5X7, FONT_6X10, FONT_9X15},
    },
    pixelcolor::PixelColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle},
    text::{Baseline, Text as EgText},
};

use crate::{
    controller::{MenuItem, Page, Screen},
    status::{
        EventSource, HostSnapshot, HostState, IfacState, LocalStatus, PeerPath, PowerSource,
        RadioState, SleepState, Text, TxResult, WakeSource,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Oled128x64,
    Tft240x135,
}

impl Surface {
    pub const fn size(self) -> Size {
        match self {
            Self::Oled128x64 => Size::new(128, 64),
            Self::Tft240x135 => Size::new(240, 135),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme<C: PixelColor> {
    pub background: C,
    pub foreground: C,
    pub muted: C,
    pub accent: C,
}

impl<C: PixelColor> Theme<C> {
    pub const fn new(background: C, foreground: C, muted: C, accent: C) -> Self {
        Self {
            background,
            foreground,
            muted,
            accent,
        }
    }
}

struct Layout {
    width: i32,
    height: i32,
    header_divider_y: i32,
    body_y: i32,
    second_row_y: i32,
    ticker_divider_y: i32,
    ticker_y: i32,
    column_width: i32,
    header_font: &'static MonoFont<'static>,
    label_font: &'static MonoFont<'static>,
    value_font: &'static MonoFont<'static>,
    ticker_font: &'static MonoFont<'static>,
    list_font: &'static MonoFont<'static>,
    label_to_value: i32,
    list_step: i32,
    menu_step: i32,
    menu_rows: u8,
}

impl Layout {
    fn new(surface: Surface) -> Self {
        match surface {
            Surface::Oled128x64 => Self {
                width: 128,
                height: 64,
                header_divider_y: 9,
                body_y: 12,
                second_row_y: 34,
                ticker_divider_y: 54,
                ticker_y: 57,
                column_width: 64,
                header_font: &FONT_5X7,
                label_font: &FONT_4X6,
                value_font: &FONT_6X10,
                ticker_font: &FONT_4X6,
                list_font: &FONT_5X7,
                label_to_value: 7,
                list_step: 13,
                menu_step: 10,
                menu_rows: 4,
            },
            Surface::Tft240x135 => Self {
                width: 240,
                height: 135,
                header_divider_y: 17,
                body_y: 22,
                second_row_y: 67,
                ticker_divider_y: 118,
                ticker_y: 123,
                column_width: 120,
                header_font: &FONT_6X10,
                label_font: &FONT_6X10,
                value_font: &FONT_9X15,
                ticker_font: &FONT_6X10,
                list_font: &FONT_9X15,
                label_to_value: 12,
                list_step: 27,
                menu_step: 19,
                menu_rows: 5,
            },
        }
    }
}

pub fn render<D>(
    target: &mut D,
    surface: Surface,
    theme: Theme<D::Color>,
    screen: Screen,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    let layout = Layout::new(surface);
    target.clear(theme.background)?;

    match screen {
        Screen::Boot => render_boot(target, &layout, theme, local),
        Screen::Page(page) => render_page(target, &layout, theme, page, local, host),
        Screen::Menu {
            selected,
            selected_index,
        } => render_menu(target, &layout, theme, selected, selected_index, host),
        Screen::Verify => render_verify(target, &layout, theme, host),
        Screen::Fault => render_fault(target, &layout, theme, local),
        Screen::DisplayOff => render_display_off(target, &layout, theme),
    }
}

fn render_page<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    page: Page,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    match page {
        Page::Status => render_status(target, layout, theme, local),
        Page::Power => render_power(target, layout, theme, local),
        Page::Radio => render_radio(target, layout, theme, local),
        Page::Traffic => render_traffic(target, layout, theme, local, host),
        Page::Identity => render_identity(target, layout, theme, host),
        Page::Links => render_links(target, layout, theme, local, host),
        Page::Peers => render_peers(target, layout, theme, host),
    }
}

fn render_status<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    local: &LocalStatus,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    header(target, layout, theme, "STATUS", radio_label(local.radio))?;
    field(
        target,
        layout,
        theme,
        0,
        0,
        "BOARD",
        value_or_dash(&local.board),
    )?;
    field(
        target,
        layout,
        theme,
        1,
        0,
        "FIRMWARE",
        value_or_dash(&local.firmware),
    )?;
    field(target, layout, theme, 0, 1, "HOST", host_label(local.host))?;
    let mut uptime = Text::<24>::empty();
    format_duration(&mut uptime, local.uptime_secs);
    field(target, layout, theme, 1, 1, "UPTIME", uptime.as_str())?;
    ticker(target, layout, theme, "LOCAL MODEM TRUTH")
}

fn render_power<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    local: &LocalStatus,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    header(
        target,
        layout,
        theme,
        "POWER",
        power_label(local.power_source),
    )?;
    field(
        target,
        layout,
        theme,
        0,
        0,
        "SOURCE",
        power_label(local.power_source),
    )?;

    let mut battery = Text::<24>::empty();
    match (local.battery_percent, local.millivolts) {
        (Some(percent), Some(millivolts)) => {
            let _ = write!(
                &mut battery,
                "{percent}% {}.{}V",
                millivolts / 1000,
                millivolts % 1000
            );
        }
        (Some(percent), None) => {
            let _ = write!(&mut battery, "{percent}%");
        }
        (None, Some(millivolts)) => {
            let _ = write!(&mut battery, "{}.{}V", millivolts / 1000, millivolts % 1000);
        }
        (None, None) => {
            let _ = battery.write_str("--");
        }
    }
    field(target, layout, theme, 1, 0, "BATTERY", battery.as_str())?;
    field(
        target,
        layout,
        theme,
        0,
        1,
        "DISPLAY",
        if local.display_on { "ON" } else { "OFF" },
    )?;
    field(
        target,
        layout,
        theme,
        1,
        1,
        "CPU SLEEP",
        sleep_label(local.sleep),
    )?;
    let mut wake = Text::<24>::from_truncated("WAKE ");
    let _ = wake.write_str(wake_label(local.last_wake));
    ticker(target, layout, theme, wake.as_str())
}

fn render_radio<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    local: &LocalStatus,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    header(target, layout, theme, "RADIO", radio_label(local.radio))?;
    let mut frequency = Text::<24>::empty();
    if let Some(hz) = local.profile.frequency_hz {
        let _ = write!(
            &mut frequency,
            "{}.{:03}MHZ",
            hz / 1_000_000,
            (hz / 1_000) % 1_000
        );
    } else {
        let _ = frequency.write_str("--");
    }
    field(target, layout, theme, 0, 0, "FREQ", frequency.as_str())?;

    let mut modulation = Text::<24>::empty();
    match (local.profile.spreading_factor, local.profile.bandwidth_hz) {
        (Some(sf), Some(bw)) => {
            let _ = write!(&mut modulation, "SF{sf}/{}K", bw / 1_000);
        }
        _ => {
            let _ = modulation.write_str("--");
        }
    }
    field(target, layout, theme, 1, 0, "SF / BW", modulation.as_str())?;

    let mut power = Text::<16>::empty();
    if let Some(dbm) = local.profile.tx_power_dbm {
        let _ = write!(&mut power, "{dbm}DBM");
    } else {
        let _ = power.write_str("--");
    }
    field(target, layout, theme, 0, 1, "TX POWER", power.as_str())?;
    field(
        target,
        layout,
        theme,
        1,
        1,
        "PROFILE",
        value_or_dash(&local.profile.name),
    )?;

    let mut footer = Text::<32>::empty();
    if let Some(cr) = local.profile.coding_rate_denominator {
        let _ = write!(&mut footer, "CR 4:{cr}");
    }
    if let Some(sync) = local.profile.sync_word {
        if !footer.is_empty() {
            let _ = footer.write_str("  ");
        }
        let _ = write!(&mut footer, "SYNC {sync:02X}");
    }
    ticker(
        target,
        layout,
        theme,
        if footer.is_empty() {
            "APPLIED PROFILE"
        } else {
            footer.as_str()
        },
    )
}

fn render_traffic<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    header(target, layout, theme, "TRAFFIC", radio_label(local.radio))?;
    let mut counts = Text::<24>::empty();
    let _ = write!(&mut counts, "{}/{}", local.tx_frames, local.rx_frames);
    field(target, layout, theme, 0, 0, "TX / RX", counts.as_str())?;

    let mut queue = Text::<16>::empty();
    if let Some(host) = host {
        let _ = write!(&mut queue, "{}", host.queue_depth);
    } else {
        let _ = queue.write_str("--");
    }
    field(target, layout, theme, 1, 0, "HOST QUEUE", queue.as_str())?;

    let mut rx = Text::<24>::empty();
    if let Some(last) = local.last_rx {
        let sign = if last.snr_tenths_db < 0 { "-" } else { "" };
        let snr = last.snr_tenths_db.unsigned_abs();
        let _ = write!(
            &mut rx,
            "{}/{}{}.{}",
            last.rssi_dbm,
            sign,
            snr / 10,
            snr % 10
        );
    } else {
        let _ = rx.write_str("--");
    }
    field(target, layout, theme, 0, 1, "LAST RX", rx.as_str())?;

    let mut tx = Text::<16>::empty();
    match local.last_tx {
        TxResult::None => {
            let _ = tx.write_str("--");
        }
        TxResult::Sent { frame_len } => {
            let _ = write!(&mut tx, "OK {frame_len}B");
        }
        TxResult::Failed { code } => {
            let _ = write!(&mut tx, "FAIL {code}");
        }
    }
    field(target, layout, theme, 1, 1, "LAST TX", tx.as_str())?;
    ticker_event(target, layout, theme, local, host)
}

fn render_identity<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    host: Option<&HostSnapshot>,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    header(target, layout, theme, "IDENTITY", "HOST")?;
    let Some(node) = host.and_then(HostSnapshot::named_node) else {
        return centered(target, layout, theme, "HOST IDENTITY --");
    };
    field(
        target,
        layout,
        theme,
        0,
        0,
        "NAME",
        value_or_dash(&node.name),
    )?;
    let mut address = Text::<20>::empty();
    let _ = write!(
        &mut address,
        "{:02X}{:02X}..{:02X}{:02X}",
        node.address_tail[0], node.address_tail[1], node.address_tail[6], node.address_tail[7]
    );
    field(target, layout, theme, 1, 0, "ADDR", address.as_str())?;
    field(
        target,
        layout,
        theme,
        0,
        1,
        "ROLE",
        value_or_dash(&node.role),
    )?;
    let mut uptime = Text::<24>::empty();
    format_duration(&mut uptime, node.uptime_secs);
    field(target, layout, theme, 1, 1, "NODE UP", uptime.as_str())?;
    ticker(target, layout, theme, "HOST-SUPPLIED NODE TRUTH")
}

fn render_links<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    header(target, layout, theme, "LINKS", "HOST")?;
    let Some(host) = host else {
        return centered(target, layout, theme, "HOST SNAPSHOT --");
    };
    let mut links = Text::<16>::empty();
    let _ = write!(&mut links, "{}/{} UP", host.admitted_links, host.link_count);
    field(target, layout, theme, 0, 0, "ADMITTED", links.as_str())?;
    field(target, layout, theme, 1, 0, "IFAC", ifac_label(host.ifac))?;
    let mut queue = Text::<16>::empty();
    let _ = write!(&mut queue, "{}", host.queue_depth);
    field(target, layout, theme, 0, 1, "QUEUE", queue.as_str())?;
    field(target, layout, theme, 1, 1, "MODEM", host_label(local.host))?;
    ticker_event(target, layout, theme, local, Some(host))
}

fn render_peers<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    host: Option<&HostSnapshot>,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    header(target, layout, theme, "PEERS", "HOST")?;
    let Some(host) = host else {
        return centered(target, layout, theme, "HOST PEERS --");
    };

    for (row, peer) in host.peers.iter().flatten().enumerate() {
        let mut line = Text::<48>::empty();
        let path = match peer.path {
            PeerPath::Direct => "^",
            PeerPath::Via => "VIA",
        };
        let _ = write!(&mut line, "{}  {} ", peer.name, path);
        format_age(&mut line, peer.age_secs);
        draw_fit(
            target,
            Point::new(1, layout.body_y + row as i32 * layout.list_step),
            layout.width - 2,
            line.as_str(),
            layout.list_font,
            theme.foreground,
            None,
        )?;
    }

    let mut footer = Text::<32>::empty();
    if host.peer_overflow > 0 {
        let _ = write!(&mut footer, "+{} MORE", host.peer_overflow);
    } else if host.peer_count() == 0 {
        let _ = footer.write_str("NO HOST PEERS");
    } else {
        let _ = footer.write_str("HOST PEER SNAPSHOT");
    }
    ticker(target, layout, theme, footer.as_str())
}

fn render_boot<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    local: &LocalStatus,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    let title_y = if layout.height == 64 { 8 } else { 18 };
    draw_fit(
        target,
        Point::new(0, title_y),
        layout.width,
        "RETINUE",
        layout.value_font,
        theme.accent,
        None,
    )?;
    let mut board = Text::<40>::empty();
    let _ = write!(
        &mut board,
        "{} / {}",
        value_or_dash(&local.board),
        value_or_dash(&local.firmware)
    );
    draw_fit(
        target,
        Point::new(0, title_y + layout.label_to_value + 12),
        layout.width,
        board.as_str(),
        layout.label_font,
        theme.foreground,
        None,
    )?;
    draw_fit(
        target,
        Point::new(0, title_y + layout.label_to_value + 24),
        layout.width,
        "DISPLAY OK / RADIO ...",
        layout.label_font,
        theme.muted,
        None,
    )
}

fn render_menu<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    selected: MenuItem,
    selected_index: u8,
    host: Option<&HostSnapshot>,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    header(target, layout, theme, "MENU", "LOCAL")?;
    let (items, len) = menu_items(host);
    let rows = layout.menu_rows.min(len);
    let first = if selected_index < rows {
        0
    } else {
        selected_index + 1 - rows
    };

    for visible_row in 0..rows {
        let index = first + visible_row;
        let item = items[usize::from(index)];
        let y = layout.body_y + i32::from(visible_row) * layout.menu_step;
        let is_selected = item == selected && index == selected_index;
        if is_selected {
            Rectangle::new(
                Point::new(0, y - 1),
                Size::new(layout.width as u32, layout.menu_step as u32),
            )
            .into_styled(PrimitiveStyle::with_fill(theme.foreground))
            .draw(target)?;
        }
        draw_fit(
            target,
            Point::new(2, y),
            layout.width - 4,
            menu_label(item),
            layout.label_font,
            if is_selected {
                theme.background
            } else {
                theme.foreground
            },
            None,
        )?;
    }
    ticker(target, layout, theme, "MOVE / SELECT / BACK")
}

fn render_verify<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    host: Option<&HostSnapshot>,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    header(target, layout, theme, "VERIFY", "HOST")?;
    let Some(node) = host.and_then(HostSnapshot::named_node) else {
        return centered(target, layout, theme, "FINGERPRINT --");
    };
    let mut first = Text::<24>::empty();
    let mut second = Text::<24>::empty();
    for (index, byte) in node.fingerprint.iter().enumerate() {
        let line = if index < 8 { &mut first } else { &mut second };
        if index % 4 == 0 && index % 8 != 0 {
            let _ = line.write_str(" ");
        }
        let _ = write!(line, "{byte:02X}");
    }
    let step = if layout.height == 64 { 14 } else { 24 };
    draw_fit(
        target,
        Point::new(2, layout.body_y + 2),
        layout.width - 4,
        first.as_str(),
        layout.list_font,
        theme.foreground,
        None,
    )?;
    draw_fit(
        target,
        Point::new(2, layout.body_y + 2 + step),
        layout.width - 4,
        second.as_str(),
        layout.list_font,
        theme.foreground,
        None,
    )?;
    ticker(target, layout, theme, "COMPARE IN PERSON")
}

fn render_fault<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    local: &LocalStatus,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    let code = local.fault.map(|fault| fault.code).unwrap_or(0);
    let mut right = Text::<8>::empty();
    let _ = write!(&mut right, "E{code:02}");
    header(target, layout, theme, "FAULT", right.as_str())?;
    let message = local
        .fault
        .as_ref()
        .map(|fault| fault.message.as_str())
        .unwrap_or("UNKNOWN");
    let banner_height = if layout.height == 64 { 17_i32 } else { 28_i32 };
    Rectangle::new(
        Point::new(0, layout.body_y),
        Size::new(layout.width as u32, banner_height as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(theme.foreground))
    .draw(target)?;
    draw_fit(
        target,
        Point::new(2, layout.body_y + 3),
        layout.width - 4,
        message,
        layout.label_font,
        theme.background,
        Some(theme.foreground),
    )?;
    draw_fit(
        target,
        Point::new(2, layout.body_y + banner_height + 4),
        layout.width - 4,
        "SEE HOST LOG",
        layout.label_font,
        theme.foreground,
        None,
    )?;
    ticker(target, layout, theme, "LOCAL RADIO FAULT")
}

fn render_display_off<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    header(target, layout, theme, "DISPLAY OFF", "LOCAL")?;
    centered(target, layout, theme, "KEY TO WAKE")?;
    ticker(target, layout, theme, "CPU SLEEP IS SEPARATE")
}

fn header<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    title: &str,
    right: &str,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    draw_fit(
        target,
        Point::new(1, 1),
        layout.width * 2 / 3,
        title,
        layout.header_font,
        theme.accent,
        None,
    )?;
    let right_width = text_width(right, layout.header_font).min(layout.width / 2);
    draw_fit(
        target,
        Point::new(layout.width - right_width - 1, 1),
        right_width,
        right,
        layout.header_font,
        theme.muted,
        None,
    )?;
    Line::new(
        Point::new(0, layout.header_divider_y),
        Point::new(layout.width - 1, layout.header_divider_y),
    )
    .into_styled(PrimitiveStyle::with_stroke(theme.foreground, 1))
    .draw(target)?;
    Ok(())
}

fn field<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    column: i32,
    row: i32,
    label: &str,
    value: &str,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    let x = column * layout.column_width + 1;
    let y = if row == 0 {
        layout.body_y
    } else {
        layout.second_row_y
    };
    let width = layout.column_width - 3;
    draw_fit(
        target,
        Point::new(x, y),
        width,
        label,
        layout.label_font,
        theme.muted,
        None,
    )?;
    draw_fit(
        target,
        Point::new(x, y + layout.label_to_value),
        width,
        value,
        layout.value_font,
        theme.foreground,
        None,
    )
}

fn ticker<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    value: &str,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    Line::new(
        Point::new(0, layout.ticker_divider_y),
        Point::new(layout.width - 1, layout.ticker_divider_y),
    )
    .into_styled(PrimitiveStyle::with_stroke(theme.muted, 1))
    .draw(target)?;
    draw_fit(
        target,
        Point::new(1, layout.ticker_y),
        layout.width - 2,
        value,
        layout.ticker_font,
        theme.muted,
        None,
    )
}

fn ticker_event<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    if let Some(event) = host.and_then(|snapshot| snapshot.event.as_ref()) {
        let mut text = Text::<32>::empty();
        let _ = text.write_str(match event.source {
            EventSource::Local => "LOCAL ",
            EventSource::Host => "HOST ",
        });
        let _ = text.write_str(event.text.as_str());
        return ticker(target, layout, theme, text.as_str());
    }
    if let Some(rx) = local.last_rx {
        let mut text = Text::<32>::empty();
        let _ = write!(&mut text, "LOCAL RX {}B {}DBM", rx.frame_len, rx.rssi_dbm);
        ticker(target, layout, theme, text.as_str())
    } else {
        ticker(target, layout, theme, "NO RECENT EVENT")
    }
}

fn centered<D>(
    target: &mut D,
    layout: &Layout,
    theme: Theme<D::Color>,
    value: &str,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    let width = text_width(value, layout.value_font).min(layout.width);
    draw_fit(
        target,
        Point::new(
            (layout.width - width) / 2,
            layout.body_y + (layout.ticker_divider_y - layout.body_y) / 2,
        ),
        width,
        value,
        layout.value_font,
        theme.foreground,
        None,
    )
}

fn draw_fit<D>(
    target: &mut D,
    position: Point,
    width: i32,
    value: &str,
    font: &'static MonoFont<'static>,
    color: D::Color,
    background: Option<D::Color>,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    if width <= 0 {
        return Ok(());
    }
    let character_width = font.character_size.width as usize;
    let max_characters = (width as usize) / character_width;
    let visible = &value[..value.len().min(max_characters)];
    let builder = MonoTextStyleBuilder::new().font(font).text_color(color);
    let style = if let Some(background) = background {
        builder.background_color(background).build()
    } else {
        builder.build()
    };
    EgText::with_baseline(visible, position, style, Baseline::Top).draw(target)?;
    Ok(())
}

fn text_width(value: &str, font: &MonoFont<'_>) -> i32 {
    value.len() as i32 * font.character_size.width as i32
}

fn value_or_dash<const N: usize>(value: &Text<N>) -> &str {
    if value.is_empty() {
        "--"
    } else {
        value.as_str()
    }
}

fn radio_label(value: RadioState) -> &'static str {
    match value {
        RadioState::Booting => "RAD ...",
        RadioState::Online => "RAD OK",
        RadioState::Fault => "RAD ERR",
    }
}

fn host_label(value: HostState) -> &'static str {
    match value {
        HostState::Detached => "DETACHED",
        HostState::Attached => "ATTACHED",
        HostState::Fault => "FAULT",
    }
}

fn power_label(value: PowerSource) -> &'static str {
    match value {
        PowerSource::Unknown => "--",
        PowerSource::Usb => "USB",
        PowerSource::Battery => "BATTERY",
        PowerSource::Solar => "SOLAR",
    }
}

fn sleep_label(value: SleepState) -> &'static str {
    match value {
        SleepState::Disabled => "DISABLED",
        SleepState::Awake => "AWAKE",
        SleepState::Armed => "ARMED",
        SleepState::Sleeping => "SLEEPING",
    }
}

fn wake_label(value: WakeSource) -> &'static str {
    match value {
        WakeSource::Unknown => "--",
        WakeSource::Button => "BUTTON",
        WakeSource::Host => "HOST",
        WakeSource::Radio => "RADIO",
    }
}

fn ifac_label(value: IfacState) -> &'static str {
    match value {
        IfacState::Unknown => "--",
        IfacState::Off => "OFF",
        IfacState::On => "ON",
    }
}

fn format_duration(output: &mut Text<24>, seconds: u32) {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if hours > 0 {
        let _ = write!(output, "{hours}H {minutes}M");
    } else {
        let _ = write!(output, "{minutes}M {}S", seconds % 60);
    }
}

fn format_age(output: &mut Text<48>, seconds: u32) {
    if seconds >= 3_600 {
        let _ = write!(output, "{}H", seconds / 3_600);
    } else if seconds >= 60 {
        let _ = write!(output, "{}M", seconds / 60);
    } else {
        let _ = write!(output, "{seconds}S");
    }
}

fn menu_items(host: Option<&HostSnapshot>) -> ([MenuItem; 6], u8) {
    if host.and_then(HostSnapshot::named_node).is_some() {
        (
            [
                MenuItem::Brightness,
                MenuItem::Detail,
                MenuItem::Verify,
                MenuItem::DisplayOff,
                MenuItem::Reboot,
                MenuItem::Back,
            ],
            6,
        )
    } else {
        (
            [
                MenuItem::Brightness,
                MenuItem::Detail,
                MenuItem::DisplayOff,
                MenuItem::Reboot,
                MenuItem::Back,
                MenuItem::Back,
            ],
            5,
        )
    }
}

fn menu_label(item: MenuItem) -> &'static str {
    match item {
        MenuItem::Brightness => "BRIGHTNESS",
        MenuItem::Detail => "STATUS DETAIL",
        MenuItem::Verify => "VERIFY",
        MenuItem::DisplayOff => "DISPLAY OFF",
        MenuItem::Reboot => "REBOOT",
        MenuItem::Back => "BACK",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{
        DetailPolicy, EventKind, Fault, NodeSummary, PeerSummary, RadioProfile, RxSummary,
        TxResult, UiEvent,
    };
    use embedded_graphics::pixelcolor::Rgb888;
    use std::vec;
    use std::vec::Vec;

    struct Canvas {
        size: Size,
        pixels: Vec<Rgb888>,
        out_of_bounds: usize,
    }

    impl Canvas {
        fn new(size: Size) -> Self {
            Self {
                size,
                pixels: vec![Rgb888::BLACK; (size.width * size.height) as usize],
                out_of_bounds: 0,
            }
        }

        fn lit_pixels(&self) -> usize {
            self.pixels
                .iter()
                .filter(|pixel| **pixel != Rgb888::BLACK)
                .count()
        }

        fn digest(&self) -> u64 {
            self.pixels
                .iter()
                .fold(0xcbf29ce484222325, |mut hash, pixel| {
                    for byte in [pixel.r(), pixel.g(), pixel.b()] {
                        hash ^= u64::from(byte);
                        hash = hash.wrapping_mul(0x100000001b3);
                    }
                    hash
                })
        }
    }

    impl OriginDimensions for Canvas {
        fn size(&self) -> Size {
            self.size
        }
    }

    impl DrawTarget for Canvas {
        type Color = Rgb888;
        type Error = core::convert::Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(point, color) in pixels {
                if point.x < 0
                    || point.y < 0
                    || point.x >= self.size.width as i32
                    || point.y >= self.size.height as i32
                {
                    self.out_of_bounds += 1;
                    continue;
                }
                let index = point.y as usize * self.size.width as usize + point.x as usize;
                self.pixels[index] = color;
            }
            Ok(())
        }
    }

    fn theme() -> Theme<Rgb888> {
        Theme::new(
            Rgb888::BLACK,
            Rgb888::WHITE,
            Rgb888::new(128, 128, 128),
            Rgb888::new(255, 176, 0),
        )
    }

    fn fixture() -> (LocalStatus, HostSnapshot) {
        let local = LocalStatus {
            board: Text::from_truncated("HELTEC V4"),
            firmware: Text::from_truncated("PHY V10"),
            uptime_secs: 14_719,
            radio: RadioState::Online,
            host: HostState::Attached,
            power_source: PowerSource::Usb,
            battery_percent: Some(73),
            millivolts: Some(3_920),
            display_on: true,
            sleep: SleepState::Disabled,
            last_wake: WakeSource::Radio,
            profile: RadioProfile {
                frequency_hz: Some(906_875_000),
                bandwidth_hz: Some(250_000),
                spreading_factor: Some(11),
                coding_rate_denominator: Some(5),
                tx_power_dbm: Some(17),
                sync_word: Some(0x2b),
                name: Text::from_truncated("LONGFAST"),
            },
            tx_frames: 128,
            rx_frames: 342,
            last_rx: Some(RxSummary {
                frame_len: 243,
                rssi_dbm: -97,
                snr_tenths_db: 62,
            }),
            last_tx: TxResult::Sent { frame_len: 247 },
            fault: None,
            gnss: crate::status::GnssState::Absent,
        };
        let host = HostSnapshot {
            valid_for_secs: 15,
            personality: crate::status::Personality::Retinue,
            detail: DetailPolicy::Named,
            node: Some(NodeSummary {
                name: Text::from_truncated("HERALD"),
                address_tail: [0x4c, 0x9f, 0x03, 0xaa, 0x77, 0xe2, 0xbd, 0x08],
                fingerprint: [
                    0x4c, 0x9f, 0x03, 0xaa, 0x77, 0xe2, 0x1b, 0x0d, 0x92, 0xc4, 0xe8, 0xf1, 0x5a,
                    0x36, 0xbd, 0x08,
                ],
                role: Text::from_truncated("NODE"),
                uptime_secs: 13_700,
            }),
            link_count: 4,
            admitted_links: 2,
            queue_depth: 3,
            ifac: IfacState::On,
            peers: [
                Some(PeerSummary {
                    name: Text::from_truncated("ESQUIRE"),
                    path: PeerPath::Direct,
                    age_secs: 120,
                }),
                Some(PeerSummary {
                    name: Text::from_truncated("MARSHAL"),
                    path: PeerPath::Direct,
                    age_secs: 3_600,
                }),
                Some(PeerSummary {
                    name: Text::from_truncated("OUTRIDER"),
                    path: PeerPath::Via,
                    age_secs: 720,
                }),
            ],
            peer_overflow: 1,
            event: Some(UiEvent {
                source: EventSource::Host,
                kind: EventKind::Delivered,
                text: Text::from_truncated("DIRECT DELIVERED"),
            }),
        };
        (local, host)
    }

    #[test]
    fn every_face_stays_inside_both_real_display_bounds() {
        let (mut local, host) = fixture();
        let screens = [
            Screen::Boot,
            Screen::Page(Page::Status),
            Screen::Page(Page::Power),
            Screen::Page(Page::Radio),
            Screen::Page(Page::Traffic),
            Screen::Page(Page::Identity),
            Screen::Page(Page::Links),
            Screen::Page(Page::Peers),
            Screen::Menu {
                selected: MenuItem::Verify,
                selected_index: 2,
            },
            Screen::Verify,
            Screen::DisplayOff,
        ];
        for surface in [Surface::Oled128x64, Surface::Tft240x135] {
            for screen in screens {
                let mut canvas = Canvas::new(surface.size());
                render(&mut canvas, surface, theme(), screen, &local, Some(&host)).unwrap();
                assert_eq!(
                    canvas.out_of_bounds, 0,
                    "{surface:?} {screen:?} drew outside the panel"
                );
                assert!(
                    canvas.lit_pixels() > 20,
                    "{surface:?} {screen:?} rendered blank"
                );
            }

            local.fault = Some(Fault {
                code: 1,
                message: Text::from_truncated("SX1262 INIT FAILED"),
            });
            let mut canvas = Canvas::new(surface.size());
            render(
                &mut canvas,
                surface,
                theme(),
                Screen::Fault,
                &local,
                Some(&host),
            )
            .unwrap();
            assert_eq!(canvas.out_of_bounds, 0);
            assert!(canvas.lit_pixels() > 20);
            local.fault = None;
        }
    }

    #[test]
    fn worst_case_bounded_values_still_clip_at_glyph_boundaries() {
        let (mut local, mut host) = fixture();
        local.board = Text::from_truncated("ABCDEFGHIJKLMNOP");
        local.firmware = Text::from_truncated("ABCDEFGHIJKL");
        local.uptime_secs = u32::MAX;
        local.tx_frames = u32::MAX;
        local.rx_frames = u32::MAX;
        local.profile.frequency_hz = Some(u32::MAX);
        local.profile.bandwidth_hz = Some(u32::MAX);
        local.profile.name = Text::from_truncated("ABCDEFGHIJKLMNOP");
        host.queue_depth = u16::MAX;
        host.peer_overflow = u8::MAX;

        for surface in [Surface::Oled128x64, Surface::Tft240x135] {
            for page in [
                Page::Status,
                Page::Power,
                Page::Radio,
                Page::Traffic,
                Page::Identity,
                Page::Links,
                Page::Peers,
            ] {
                let mut canvas = Canvas::new(surface.size());
                render(
                    &mut canvas,
                    surface,
                    theme(),
                    Screen::Page(page),
                    &local,
                    Some(&host),
                )
                .unwrap();
                assert_eq!(canvas.out_of_bounds, 0, "{surface:?} {page:?}");
            }
        }
    }

    #[test]
    fn selected_faces_have_stable_pixel_goldens() {
        let (local, host) = fixture();
        let mut actual = [0_u64; 4];
        for (index, (surface, screen)) in [
            (Surface::Oled128x64, Screen::Page(Page::Status)),
            (
                Surface::Oled128x64,
                Screen::Menu {
                    selected: MenuItem::Verify,
                    selected_index: 2,
                },
            ),
            (Surface::Tft240x135, Screen::Page(Page::Status)),
            (
                Surface::Tft240x135,
                Screen::Menu {
                    selected: MenuItem::Verify,
                    selected_index: 2,
                },
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let mut canvas = Canvas::new(surface.size());
            render(&mut canvas, surface, theme(), screen, &local, Some(&host)).unwrap();
            actual[index] = canvas.digest();
        }

        assert_eq!(
            actual,
            [
                4_785_255_477_419_125_564,
                7_929_273_586_648_350_406,
                15_393_597_127_465_717_303,
                6_764_923_977_634_548_512,
            ]
        );
    }
}
