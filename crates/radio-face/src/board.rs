//! The state a board's UI loop keeps around [`Controller`].
//!
//! Every board runs the same few lines between its events and the controller:
//! `LocalStatus::display_on` follows the controller, a press records
//! `WakeSource::Button`, and a host snapshot is dropped once older than its
//! validity. [`BoardState`] holds them once, for the firmwares and the mirror.
//! Panel power, frames and LEDs stay with the board.

use crate::{
    controller::{Action, Controller, InputEvent, InputProfile, Screen},
    status::{HostSnapshot, LocalStatus, WakeSource},
};

/// Controller, local status and the current host snapshot.
///
/// `T` is the board's timestamp for when the snapshot arrived (an
/// `embassy_time::Instant` on the boards, `()` when the caller tracks age
/// itself); [`BoardState::expire_host`] turns it into an age.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardState<T> {
    profile: InputProfile,
    controller: Controller,
    local: LocalStatus,
    host: Option<(HostSnapshot, T)>,
}

impl<T: Copy> BoardState<T> {
    pub fn new(profile: InputProfile, local: LocalStatus) -> Self {
        let controller = Controller::default();
        Self {
            profile,
            controller,
            local: LocalStatus {
                display_on: controller.display_on(),
                ..local
            },
            host: None,
        }
    }

    pub const fn profile(&self) -> InputProfile {
        self.profile
    }

    pub fn set_profile(&mut self, profile: InputProfile) {
        self.profile = profile;
    }

    pub const fn controller(&self) -> &Controller {
        &self.controller
    }

    pub const fn local(&self) -> &LocalStatus {
        &self.local
    }

    pub fn host(&self) -> Option<&HostSnapshot> {
        self.host.as_ref().map(|(snapshot, _)| snapshot)
    }

    pub fn screen(&self) -> Screen {
        self.controller.screen(&self.local, self.host())
    }

    /// Whether the panel should be lit: on, or forced on by a fault.
    pub fn panel_lit(&self) -> bool {
        self.controller.display_on() || self.local.fault.is_some()
    }

    /// Replaces local status; `display_on` stays the controller's.
    pub fn set_local(&mut self, local: LocalStatus) {
        self.local = local;
        self.local.display_on = self.controller.display_on();
    }

    pub fn set_uptime(&mut self, uptime_secs: u32) {
        self.local.uptime_secs = uptime_secs;
    }

    pub fn set_host(&mut self, snapshot: HostSnapshot, received_at: T) {
        self.host = Some((snapshot, received_at));
    }

    pub fn clear_host(&mut self) {
        self.host = None;
    }

    /// Drops the snapshot if `age_secs(received_at)` has reached its validity.
    /// Returns whether it was dropped.
    pub fn expire_host(&mut self, age_secs: impl FnOnce(T) -> u32) -> bool {
        let expired = self
            .host
            .is_some_and(|(snapshot, at)| !snapshot.is_fresh(age_secs(at)));
        if expired {
            self.host = None;
        }
        expired
    }

    /// A classified press: records the wake source, then lets the controller act.
    pub fn press(&mut self, event: InputEvent) -> Action {
        self.local.last_wake = WakeSource::Button;
        let host = self.host.as_ref().map(|(snapshot, _)| snapshot);
        let action = self
            .controller
            .handle(self.profile, event, &self.local, host);
        self.local.display_on = self.controller.display_on();
        action
    }
}
