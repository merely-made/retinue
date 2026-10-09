//! The pages the user cycles through.

use core::fmt::Write as _;

use embedded_graphics::prelude::*;

use super::labels::{
    format_age, format_duration, host_label, ifac_label, power_label, radio_label, sleep_label,
    value_or_dash, wake_label,
};
use super::layout::{Layout, Theme};
use super::painter::{Painter, TextRole};
use super::widgets::{centered, draw_fit, field, header, ticker, ticker_event};
use crate::{
    controller::Page,
    status::{HostSnapshot, LocalStatus, PeerPath, Text, TxResult},
};

pub(super) fn render_page<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    page: Page,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Result<(), P::Error>
where
    P: Painter,
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

fn render_status<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    local: &LocalStatus,
) -> Result<(), P::Error>
where
    P: Painter,
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

fn render_power<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    local: &LocalStatus,
) -> Result<(), P::Error>
where
    P: Painter,
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

fn render_radio<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    local: &LocalStatus,
) -> Result<(), P::Error>
where
    P: Painter,
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

fn render_traffic<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Result<(), P::Error>
where
    P: Painter,
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

fn render_identity<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    host: Option<&HostSnapshot>,
) -> Result<(), P::Error>
where
    P: Painter,
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

fn render_links<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Result<(), P::Error>
where
    P: Painter,
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

fn render_peers<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    host: Option<&HostSnapshot>,
) -> Result<(), P::Error>
where
    P: Painter,
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
            TextRole::Line,
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
