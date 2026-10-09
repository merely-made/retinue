//! Fixed labels and number formats for status values.

use core::fmt::Write as _;

use crate::{
    controller::MenuItem,
    status::{
        HostSnapshot, HostState, IfacState, PowerSource, RadioState, SleepState, Text, WakeSource,
    },
};

pub(super) fn value_or_dash<const N: usize>(value: &Text<N>) -> &str {
    if value.is_empty() {
        "--"
    } else {
        value.as_str()
    }
}

pub(super) fn radio_label(value: RadioState) -> &'static str {
    match value {
        RadioState::Booting => "RAD ...",
        RadioState::Online => "RAD OK",
        RadioState::Fault => "RAD ERR",
    }
}

pub(super) fn host_label(value: HostState) -> &'static str {
    match value {
        HostState::Detached => "DETACHED",
        HostState::Attached => "ATTACHED",
        HostState::Fault => "FAULT",
    }
}

pub(super) fn power_label(value: PowerSource) -> &'static str {
    match value {
        PowerSource::Unknown => "--",
        PowerSource::Usb => "USB",
        PowerSource::Battery => "BATTERY",
        PowerSource::Solar => "SOLAR",
    }
}

pub(super) fn sleep_label(value: SleepState) -> &'static str {
    match value {
        SleepState::Disabled => "DISABLED",
        SleepState::Awake => "AWAKE",
        SleepState::Armed => "ARMED",
        SleepState::Sleeping => "SLEEPING",
    }
}

pub(super) fn wake_label(value: WakeSource) -> &'static str {
    match value {
        WakeSource::Unknown => "--",
        WakeSource::Button => "BUTTON",
        WakeSource::Host => "HOST",
        WakeSource::Radio => "RADIO",
    }
}

pub(super) fn ifac_label(value: IfacState) -> &'static str {
    match value {
        IfacState::Unknown => "--",
        IfacState::Off => "OFF",
        IfacState::On => "ON",
    }
}

pub(super) fn format_duration(output: &mut Text<24>, seconds: u32) {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if hours > 0 {
        let _ = write!(output, "{hours}H {minutes}M");
    } else {
        let _ = write!(output, "{minutes}M {}S", seconds % 60);
    }
}

pub(super) fn format_age(output: &mut Text<48>, seconds: u32) {
    if seconds >= 3_600 {
        let _ = write!(output, "{}H", seconds / 3_600);
    } else if seconds >= 60 {
        let _ = write!(output, "{}M", seconds / 60);
    } else {
        let _ = write!(output, "{seconds}S");
    }
}

pub(super) fn menu_items(host: Option<&HostSnapshot>) -> ([MenuItem; 6], u8) {
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

pub(super) fn menu_label(item: MenuItem) -> &'static str {
    match item {
        MenuItem::Brightness => "BRIGHTNESS",
        MenuItem::Detail => "STATUS DETAIL",
        MenuItem::Verify => "VERIFY",
        MenuItem::DisplayOff => "DISPLAY OFF",
        MenuItem::Reboot => "REBOOT",
        MenuItem::Back => "BACK",
    }
}
