//! Retained protocol instances behind one confirmed radio activation.
//!
//! Time is monotonic milliseconds. Construct after confirmed home RX; acknowledge
//! exact profile transitions through the radio owner. RX cannot request switches.
//! The caller must bound TX, collect completed RX before switching and reset on
//! uncertain cancellation. Interruption close packets are reported **unsent**.
//! Observing caller time advances the runtime clock even when a command is
//! refused. Protocol losses remain available through `take_report` on errors.
use crate::work::{Activation, Transmission, WorkError, WorkId, WorkQueue};
use heapless::Vec;
use retinue::{
    announce::AnnounceBlob,
    node::{Action, Actions, InterruptionPermission, PauseBlocked},
};
use selvage::{PhyProfile, personality::*};
pub const RETINUE: PersonalityId = PersonalityId(0);
pub const SENNET: PersonalityId = PersonalityId(1);
pub const TUCKET: PersonalityId = PersonalityId(2);
mod switching;
mod traffic;
mod types;

pub use types::{Config, Error, Event, Report, Step};

pub type RetinueNode = retinue::node::Node<8, 4, 1, 4>;
pub struct Runtime {
    config: Config,
    controller: Controller,
    last_now: u64,
    retinue: retinue::instance::Instance,
    carrier: crate::retinue_carrier::RetinueCarrier,
    carrier_rejected_rx: u64,
    carrier_rejected_tx: u64,
    sennet: sennet::instance::SennetInstance,
    tucket: tucket::instance::Instance,
    work: WorkQueue<8>,
    report: Report,
}
impl Runtime {
    /// Inputs are fresh active instances; profiles use the three public IDs.
    pub fn new(
        now: u64,
        config: Config,
        node: RetinueNode,
        sennet: sennet::instance::SennetInstance,
        tucket: tucket::instance::Instance,
    ) -> Result<Self, Error> {
        Self::new_with_carrier(now, config, node, sennet, tucket, Default::default())
    }
    /// Configure the carrier before retaining instances or queueing frames.
    pub fn new_with_carrier(
        now: u64,
        config: Config,
        mut node: RetinueNode,
        sennet: sennet::instance::SennetInstance,
        tucket: tucket::instance::Instance,
        carrier: crate::retinue_carrier::RetinueCarrier,
    ) -> Result<Self, Error> {
        carrier
            .configure_node(&mut node)
            .map_err(Error::CarrierConfiguration)?;
        // The Retinue personality's modulation sets its first-hop airtime allowance
        // (Ruling 50), so a link request's deadline covers the radio's own slowness.
        let profile = config
            .profiles
            .get(usize::from(RETINUE.0))
            .ok_or(Error::Configuration)?;
        let allowance = crate::phy::nominal_bits_ms(
            profile.spreading_factor,
            profile.bandwidth_hz,
            profile.coding_rate_denominator,
            retinue::node::FIRST_HOP_ALLOWANCE_BITS,
        )
        .ok_or(Error::Configuration)?;
        node.set_first_hop_airtime(retinue::instance::INTERFACE, allowance)
            .map_err(|_| Error::Configuration)?;
        let ids = [RETINUE, SENNET, TUCKET];
        if !ids.contains(&config.controller.home)
            || config.tx_budget_ms == 0
            || config.frame_ttl_ms <= config.tx_budget_ms
            || config.controller.installed != InstalledPersonalitySet::new(&ids).unwrap()
            || sennet.is_paused()
            || tucket.is_paused()
        {
            return Err(Error::Configuration);
        }
        let controller =
            Controller::new(config.controller, now).map_err(|_| Error::Configuration)?;
        let mut result = Self {
            config,
            carrier,
            carrier_rejected_rx: 0,
            carrier_rejected_tx: 0,
            controller,
            last_now: now,
            retinue: retinue::instance::Instance::new(node, now),
            sennet,
            tucket,
            work: WorkQueue::new(now),
            report: Report::default(),
        };
        for id in ids {
            if id != config.controller.home {
                result.pause(now, id, now, false, || [0; 16])?;
            }
        }
        result.work.activate(now, config.controller.home)?;
        Ok(result)
    }
    /// Rejected physical Retinue ingress and final egress, including authentication failures.
    pub fn carrier_rejections(&self) -> (u64, u64) {
        (self.carrier_rejected_rx, self.carrier_rejected_tx)
    }
    pub fn state(&self) -> ControllerState {
        self.controller.state()
    }
    pub fn active(&self) -> Option<Activation> {
        self.work.activation()
    }
    pub fn tx_budget_ms(&self) -> u64 {
        self.config.tx_budget_ms
    }
    pub fn profile(&self, id: PersonalityId) -> Result<PhyProfile, Error> {
        self.config
            .profiles
            .get(usize::from(id.0))
            .copied()
            .ok_or(Error::WrongInstance)
    }
    pub fn retinue(&self) -> &retinue::instance::Instance {
        &self.retinue
    }
    pub fn sennet(&self) -> &sennet::instance::SennetInstance {
        &self.sennet
    }
    pub fn tucket(&self) -> &tucket::instance::Instance {
        &self.tucket
    }
    /// Refused commands retain accumulated events until explicitly drained.
    pub fn take_report(&mut self) -> Report {
        core::mem::take(&mut self.report)
    }
    fn event(&mut self, event: Event) {
        if self.report.events.push(event).is_err() {
            self.report.overflowed = self.report.overflowed.saturating_add(1);
        }
    }
    fn time(&mut self, now: u64) -> Result<(), Error> {
        if now < self.last_now {
            return Err(Error::ClockRegression);
        }
        self.last_now = now;
        Ok(())
    }
    fn require(&mut self, now: u64, id: PersonalityId) -> Result<Activation, Error> {
        self.time(now)?;
        let a = self.active().ok_or(Error::Inactive)?;
        if a.instance != id {
            return Err(Error::WrongInstance);
        }
        Ok(a)
    }
    fn work_until(&self) -> u64 {
        match self.state() {
            ControllerState::Away {
                excursion_deadline, ..
            } => excursion_deadline,
            ControllerState::ReturnRequired { return_by, .. }
            | ControllerState::DeferredReturn { return_by, .. } => {
                return_by.saturating_sub(self.config.controller.transition_timeout_ms)
            }
            ControllerState::Home | ControllerState::DeferredExcursion { .. } => u64::MAX,
            _ => 0,
        }
    }
    pub fn next_deadline(&self) -> Option<u64> {
        let controller = match self.state() {
            ControllerState::Away {
                excursion_deadline, ..
            } => Some(excursion_deadline),
            ControllerState::Transitioning { transition, .. } => Some(transition.deadline),
            ControllerState::ReturnRequired { return_by, .. } => Some(return_by),
            ControllerState::DeferredReturn {
                retry_at,
                return_by,
                ..
            } => Some(retry_at.min(return_by)),
            ControllerState::DeferredExcursion {
                retry_at,
                defer_deadline,
                ..
            } => Some(retry_at.min(defer_deadline)),
            ControllerState::RecoveryRequired => Some(0),
            _ => None,
        };
        [
            controller,
            self.work.next_deadline(),
            self.sennet.next_deadline(),
            self.tucket.next_deadline(),
            self.retinue.node().pause_assessment().earliest_link_expiry,
        ]
        .into_iter()
        .flatten()
        .min()
    }
    fn ttl(&self, now: u64) -> Result<u64, Error> {
        now.checked_add(self.config.frame_ttl_ms)
            .ok_or(Error::TimeOverflow)
    }
    fn queue(&mut self, now: u64, op: Option<u64>, deadline: u64, frame: &[u8]) {
        if let Some(a) = self.active()
            && let Err(reason) =
                self.work
                    .enqueue(now, a, op, deadline.min(self.work_until()), frame)
        {
            self.event(Event::FrameDropped {
                instance: a.instance,
                reason,
            });
        }
    }
    fn retinue_actions(&mut self, now: u64, actions: Actions<4>) -> Result<(), Error> {
        if actions.overflowed() != 0 {
            self.event(Event::ActionsOverflowed(actions.overflowed()));
        }
        let deadline = self.ttl(now)?;
        for a in actions {
            match a {
                Action::Send { packet, .. } => match self.carrier.encode(&packet) {
                    Ok(frame) => self.queue(now, None, deadline, &frame),
                    Err(reason) => {
                        self.carrier_rejected_tx = self.carrier_rejected_tx.saturating_add(1);
                        self.event(Event::RetinueCarrierDropped { reason });
                    }
                },
                e => self.event(Event::Retinue(e)),
            }
        }
        Ok(())
    }
    fn settle_loss(&mut self, now: u64, id: WorkId) -> Result<(), Error> {
        self.event(Event::WorkLost(id));
        if id.activation.instance == SENNET
            && let (Some(op), Some(identity)) = (id.operation, self.sennet.pending_identity())
        {
            let event = self.sennet.fail_tx(now, op as u32, identity)?;
            self.event(Event::Sennet(event));
        }
        Ok(())
    }
    fn tucket_lost(&mut self, now: u64, report: tucket::instance::LossReport) -> Result<(), Error> {
        for op in report.operations {
            if self.active().is_some_and(|a| a.instance == TUCKET) {
                for id in self.work.cancel_operation(now, u64::from(op.0))? {
                    self.event(Event::WorkLost(id));
                }
            }
            self.event(Event::TucketLost(op));
        }
        Ok(())
    }
    fn expiry(&mut self, now: u64) -> Result<(), Error> {
        for id in self.work.expire(now)? {
            self.settle_loss(now, id)?;
        }
        if self.work.inflight().is_none()
            && let Some(e) = self.sennet.advance(now)?
        {
            self.event(Event::Sennet(e));
        }
        let t = self.tucket.advance(now)?;
        self.tucket_lost(now, t)?;
        let r = self.retinue.advance(now)?;
        self.retinue_expired(r);
        Ok(())
    }
    /// Report what a Retinue expiry pass removed. An unanswered link request is the
    /// `LinkRequestTimedOut` action the channel node surfaces, not part of the expiry report
    /// (Ruling 74), so a pass that expired only requests still reports them.
    fn retinue_expired(&mut self, mut r: retinue::node::SessionExpiryReport<1>) {
        let timed_out = core::mem::take(&mut r.pending_links);
        if !r.links.is_empty()
            || !r.inbound_resources.is_empty()
            || !r.outbound_resources.is_empty()
        {
            self.event(Event::RetinueExpired(r));
        }
        for link_id in timed_out {
            self.event(Event::Retinue(Action::LinkRequestTimedOut { link_id }));
        }
    }
}
