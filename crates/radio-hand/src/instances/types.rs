//! Configuration, errors, and the events a runtime reports.

use super::*;

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub controller: ControllerConfig,
    pub profiles: [PhyProfile; 3],
    pub tx_budget_ms: u64,
    pub frame_ttl_ms: u64,
}
#[derive(Debug)]
pub enum Error {
    Configuration,
    CarrierConfiguration(retinue::node::LogicalMtuError),
    ClockRegression,
    Inactive,
    WrongInstance,
    FrameTooLong,
    Malformed,
    Carrier(retinue::Error),
    TimeOverflow,
    RecoveryRequired,
    Controller(ControllerError),
    Work(WorkError),
    Retinue(retinue::instance::InstanceError),
    Sennet(sennet::instance::InstanceError),
    Tucket(tucket::instance::InstanceError),
}
impl From<ControllerError> for Error {
    fn from(e: ControllerError) -> Self {
        Self::Controller(e)
    }
}
impl From<WorkError> for Error {
    fn from(e: WorkError) -> Self {
        Self::Work(e)
    }
}
impl From<retinue::instance::InstanceError> for Error {
    fn from(e: retinue::instance::InstanceError) -> Self {
        Self::Retinue(e)
    }
}
impl From<sennet::instance::InstanceError> for Error {
    fn from(e: sennet::instance::InstanceError) -> Self {
        Self::Sennet(e)
    }
}
impl From<tucket::instance::InstanceError> for Error {
    fn from(e: tucket::instance::InstanceError) -> Self {
        Self::Tucket(e)
    }
}
#[derive(Debug)]
pub enum Event {
    Retinue(Action),
    /// Established links and resource transfers removed by expiry. Its `pending_links` is
    /// always empty: an expired link request is reported as
    /// `Retinue(Action::LinkRequestTimedOut)`.
    RetinueExpired(retinue::node::SessionExpiryReport<1>),
    /// Contains locally discarded, unsent close packets, not remote receipts.
    RetinueInterrupted(retinue::node::InterruptionReport<1, 4>),
    SennetReceived(sennet::instance::ReceiveOutcome),
    Sennet(sennet::instance::InstanceEvent),
    Tucket(tucket::node::Event),
    TucketLost(tucket::instance::OperationId),
    TucketAcknowledged(tucket::instance::OperationId),
    WorkLost(WorkId),
    WorkCancelled(WorkId),
    FrameDropped {
        instance: PersonalityId,
        reason: WorkError,
    },
    ActionsOverflowed(u16),
    RetinueCarrierDropped {
        reason: retinue::Error,
    },
}
#[derive(Debug, Default)]
pub struct Report {
    pub events: Vec<Event, 32>,
    pub overflowed: u16,
}
impl Report {
    /// Bounded USB diagnostic record. Failed event formatting is rolled back
    /// whole and the reserved footer always accounts for every omitted event.
    pub fn format(&self, now: u64) -> heapless::String<2048> {
        use core::fmt::Write;
        struct EventWriter<'a>(&'a mut heapless::String<2048>);
        impl core::fmt::Write for EventWriter<'_> {
            fn write_str(&mut self, text: &str) -> core::fmt::Result {
                if self.0.len() + text.len() > 1920 {
                    return Err(core::fmt::Error);
                }
                self.0.push_str(text).map_err(|_| core::fmt::Error)
            }
        }
        let mut line = heapless::String::new();
        let mut omitted = 0;
        for event in &self.events {
            let start = line.len();
            if writeln!(EventWriter(&mut line), "resident event {event:?}").is_err() {
                line.truncate(start);
                omitted += 1;
            }
        }
        writeln!(
            line,
            "resident report now={now} events={} dropped={} omitted={omitted}",
            self.events.len(),
            self.overflowed
        )
        .expect("reserved 128-byte footer");
        line
    }
}
#[derive(Debug)]
pub struct Step {
    pub transition: Option<Transition>,
    pub report: Report,
}
