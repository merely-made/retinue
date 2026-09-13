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
pub type RetinueNode = retinue::node::Node<8, 4, 1, 4>;
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
    ClockRegression,
    Inactive,
    WrongInstance,
    FrameTooLong,
    Malformed,
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
pub struct Runtime {
    config: Config,
    controller: Controller,
    last_now: u64,
    retinue: retinue::instance::Instance,
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
                Action::Send { packet, .. } => self.queue(now, None, deadline, &packet.encode()),
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
        if !r.links.is_empty()
            || !r.inbound_resources.is_empty()
            || !r.outbound_resources.is_empty()
        {
            self.event(Event::RetinueExpired(r));
        }
        Ok(())
    }
    fn assessment(
        &self,
        now: u64,
        id: PersonalityId,
        return_by: u64,
        allow: bool,
    ) -> Result<PauseOutcome, Error> {
        if self.work.inflight().is_some() {
            return Ok(PauseOutcome::Busy {
                retry_at: now
                    .checked_add(self.config.tx_budget_ms)
                    .ok_or(Error::TimeOverflow)?,
            });
        }
        let outcome = match id {
            RETINUE => match self.retinue.assess_pause(now, return_by) {
                Ok(_) => PauseOutcome::Ready,
                Err(retinue::instance::InstanceError::PauseBlocked(reason)) => match reason {
                    PauseBlocked::ClockBeforeLinkActivity { .. }
                    | PauseBlocked::LinkExpiryOverflow
                    | PauseBlocked::ReturnBoundBeforeNow { .. } => {
                        return Err(Error::Retinue(
                            retinue::instance::InstanceError::PauseBlocked(reason),
                        ));
                    }
                    _ => PauseOutcome::RequiresSessionLoss {
                        affected_sessions: 1,
                    },
                },
                Err(e) => return Err(e.into()),
            },
            SENNET => match self.sennet.assess_pause(now, return_by)? {
                sennet::instance::PauseOutcome::Ready => PauseOutcome::Ready,
                sennet::instance::PauseOutcome::Busy { retry_at } => {
                    PauseOutcome::Busy { retry_at }
                }
                sennet::instance::PauseOutcome::RequiresLoss { pending } => {
                    PauseOutcome::RequiresSessionLoss {
                        affected_sessions: pending as u16,
                    }
                }
            },
            TUCKET => match self.tucket.assess_pause(now, return_by)? {
                tucket::instance::PauseAssessment::Ready => PauseOutcome::Ready,
                tucket::instance::PauseAssessment::Busy { retry_at } => PauseOutcome::Busy {
                    retry_at: retry_at.max(now),
                },
                tucket::instance::PauseAssessment::RequiresLoss { pending } => {
                    PauseOutcome::RequiresSessionLoss {
                        affected_sessions: pending.len() as u16,
                    }
                }
            },
            _ => return Err(Error::WrongInstance),
        };
        // Validate the protocol's clock and lifecycle before queue loss can
        // authorize departure. Queued work cannot hide a protocol refusal.
        if !self.work.is_empty() {
            return Ok(PauseOutcome::RequiresSessionLoss {
                affected_sessions: self.work.len() as u16,
            });
        }
        Ok(if allow && matches!(outcome, PauseOutcome::Busy { .. }) {
            PauseOutcome::RequiresSessionLoss {
                affected_sessions: 1,
            }
        } else {
            outcome
        })
    }
    fn pause(
        &mut self,
        now: u64,
        id: PersonalityId,
        return_by: u64,
        allow: bool,
        iv: impl FnMut() -> [u8; 16],
    ) -> Result<(), Error> {
        match id {
            RETINUE => {
                let permission = if allow {
                    InterruptionPermission::AllowSessionLoss
                } else {
                    InterruptionPermission::PreserveSessions
                };
                if let Some(report) = self.retinue.pause(now, return_by, permission, iv)? {
                    self.event(Event::RetinueInterrupted(report));
                }
            }
            SENNET => {
                if allow
                    && self.sennet.pending_identity().is_some()
                    && let Some(e) = self
                        .sennet
                        .discard_pending(now, sennet::instance::LossPermission::operator())?
                {
                    self.event(Event::Sennet(e));
                }
                self.sennet.pause(now, return_by)?;
            }
            TUCKET => {
                if allow
                    && self.tucket.assess_pause(now, return_by)?
                        != tucket::instance::PauseAssessment::Ready
                {
                    let r = self
                        .tucket
                        .interrupt(tucket::instance::LossPermission::Allow)?;
                    self.tucket_lost(now, r)?;
                }
                self.tucket.pause(now, return_by)?;
            }
            _ => return Err(Error::WrongInstance),
        }
        Ok(())
    }
    fn suspend(
        &mut self,
        now: u64,
        t: Transition,
        return_by: u64,
        allow: bool,
        iv: impl FnMut() -> [u8; 16],
    ) -> Result<(), Error> {
        for id in self.work.deactivate(now, allow)? {
            self.settle_loss(now, id)?;
        }
        self.pause(now, t.from, return_by, allow, iv)
    }
    fn return_bound(&self, now: u64, request: Excursion) -> Result<u64, Error> {
        now.checked_add(self.config.controller.transition_timeout_ms)
            .and_then(|n| n.checked_add(request.duration_ms))
            .and_then(|n| n.checked_add(self.config.controller.return_budget_ms))
            .ok_or(Error::TimeOverflow)
    }
    pub fn request(
        &mut self,
        now: u64,
        request: Excursion,
        iv: impl FnMut() -> [u8; 16],
    ) -> Result<Step, Error> {
        self.time(now)?;
        if self.state() != ControllerState::Home {
            return Err(ControllerError::NotHome.into());
        }
        self.expiry(now)?;
        let bound = self.return_bound(now, request)?;
        let allow = request.interruption == InterruptionPolicy::AllowSessionLoss;
        let pause = self.assessment(now, self.config.controller.home, bound, allow)?;
        // Inbound Retinue sessions can arrive unsolicited while away.
        let stop = if request.target == RETINUE {
            StopCapability::RequiresSessionLoss
        } else {
            StopCapability::Resumable
        };
        let transition = self.controller.request_excursion(
            now,
            request,
            CoverageEvidence { valid_until: None },
            pause,
            stop,
        )?;
        if let Some(t) = transition {
            self.suspend(now, t, bound, allow, iv)?;
        }
        Ok(Step {
            transition,
            report: self.take_report(),
        })
    }
    pub fn tick(&mut self, now: u64, iv: impl FnMut() -> [u8; 16]) -> Result<Step, Error> {
        self.time(now)?;
        self.expiry(now)?;
        if self
            .controller
            .tick(now, CoverageEvidence { valid_until: None })?
            == ControllerEvent::RecoveryRequired
        {
            return Err(Error::RecoveryRequired);
        }
        let mut transition = None;
        match self.state() {
            ControllerState::DeferredExcursion {
                request, retry_at, ..
            } if now >= retry_at => {
                let bound = self.return_bound(now, request)?;
                let allow = request.interruption == InterruptionPolicy::AllowSessionLoss;
                let pause = self.assessment(now, self.config.controller.home, bound, allow)?;
                transition = self.controller.continue_excursion(
                    now,
                    CoverageEvidence { valid_until: None },
                    pause,
                )?;
                if let Some(t) = transition {
                    self.suspend(now, t, bound, allow, iv)?;
                }
            }
            ControllerState::ReturnRequired {
                target,
                interruption,
                ..
            }
            | ControllerState::DeferredReturn {
                target,
                interruption,
                ..
            } => {
                let allow = interruption == InterruptionPolicy::AllowSessionLoss;
                // Alternate state has no promised next visit. Account for all
                // obligations that cannot survive indefinite local absence.
                let pause = self.assessment(now, target, u64::MAX, allow)?;
                transition = self.controller.begin_return(now, pause)?;
                if let Some(t) = transition {
                    self.suspend(now, t, u64::MAX, allow, iv)?;
                }
            }
            _ => {}
        }
        Ok(Step {
            transition,
            report: self.take_report(),
        })
    }
    pub fn cancel(&mut self, now: u64, iv: impl FnMut() -> [u8; 16]) -> Result<Step, Error> {
        self.time(now)?;
        self.controller.cancel(now)?;
        self.tick(now, iv)
    }
    pub fn acknowledge(
        &mut self,
        now: u64,
        id: u64,
        ack: Acknowledgement,
    ) -> Result<Report, Error> {
        self.time(now)?;
        let to = match self.state() {
            ControllerState::Transitioning { transition, .. } if transition.id == id => {
                transition.to
            }
            _ => {
                return Err(Error::Controller(ControllerError::WrongAcknowledgement {
                    expected: match self.state() {
                        ControllerState::Transitioning { transition, .. } => transition.id,
                        _ => 0,
                    },
                    received: id,
                }));
            }
        };
        self.controller.acknowledge(now, id, ack)?;
        if self.state() == ControllerState::RecoveryRequired {
            return Err(Error::RecoveryRequired);
        }
        match to {
            RETINUE => {
                let r = self.retinue.resume(now)?;
                if !r.links.is_empty()
                    || !r.inbound_resources.is_empty()
                    || !r.outbound_resources.is_empty()
                {
                    self.event(Event::RetinueExpired(r));
                }
            }
            SENNET => {
                if let Some(e) = self.sennet.resume(now)? {
                    self.event(Event::Sennet(e));
                }
            }
            TUCKET => {
                let r = self.tucket.resume(now)?;
                self.tucket_lost(now, r)?;
            }
            _ => return Err(Error::WrongInstance),
        }
        self.work.activate(now, to)?;
        Ok(self.take_report())
    }
    pub fn ingest(&mut self, now: u64, frame: &[u8]) -> Result<Report, Error> {
        self.time(now)?;
        if frame.len() > 255 {
            return Err(Error::FrameTooLong);
        }
        self.expiry(now)?;
        match self.active().ok_or(Error::Inactive)?.instance {
            RETINUE => {
                let p = retinue::Packet::decode(frame).map_err(|_| Error::Malformed)?;
                let a = self.retinue.ingest(now, &p)?;
                self.retinue_actions(now, a)?;
            }
            SENNET => {
                let e = self.sennet.receive(now, frame)?;
                self.event(Event::SennetReceived(e));
            }
            TUCKET => {
                let out = self.tucket.on_frame(now, frame)?;
                self.tucket_lost(now, out.expired)?;
                for op in out.acknowledged {
                    for id in self.work.cancel_operation(now, u64::from(op.0))? {
                        self.event(Event::WorkCancelled(id));
                    }
                    self.event(Event::TucketAcknowledged(op));
                }
                for e in out.events {
                    self.event(Event::Tucket(e));
                }
                let deadline = self.ttl(now)?;
                for f in out.outbound {
                    self.queue(now, None, deadline, &f);
                }
            }
            _ => return Err(Error::WrongInstance),
        }
        Ok(self.take_report())
    }
    pub fn poll(&mut self, now: u64, blob: Option<&AnnounceBlob>) -> Result<Report, Error> {
        self.time(now)?;
        self.expiry(now)?;
        match self.active().ok_or(Error::Inactive)?.instance {
            RETINUE => {
                let a = self.retinue.poll(now, blob)?;
                self.retinue_actions(now, a)?;
            }
            TUCKET => {
                let due: Vec<_, 8> = self
                    .tucket
                    .operations()
                    .filter(|op| op.retry_at <= now && op.attempts_remaining > 0)
                    .take(8)
                    .collect();
                for op in due {
                    if self.work.available() == 0 {
                        break;
                    }
                    if self.work.contains_operation(u64::from(op.id.0)) {
                        continue;
                    }
                    let finish = now
                        .checked_add(self.config.tx_budget_ms)
                        .ok_or(Error::TimeOverflow)?;
                    if finish >= op.expires_at
                        || finish > op.allowed_until
                        || finish > self.work_until()
                    {
                        continue;
                    }
                    if let Some(a) = self.tucket.next_retry(now, op.id, finish)? {
                        self.queue(
                            now,
                            Some(u64::from(op.id.0)),
                            op.expires_at.min(op.allowed_until),
                            &a.frame,
                        );
                    }
                }
            }
            SENNET => {}
            _ => return Err(Error::WrongInstance),
        }
        Ok(self.take_report())
    }
    pub fn send_sennet(
        &mut self,
        now: u64,
        header: sennet::transport::Header,
        text: &str,
    ) -> Result<Report, Error> {
        let a = self.require(now, SENNET)?;
        if self.work.available() == 0 {
            return Err(WorkError::Full.into());
        }
        let deadline = self.ttl(now)?.min(self.work_until());
        if now
            .checked_add(self.config.tx_budget_ms)
            .ok_or(Error::TimeOverflow)?
            > deadline
        {
            return Err(WorkError::Expired.into());
        }
        self.sennet.queue_text(now, header, text)?;
        if let Some(out) = self.sennet.take_outbound(now)?
            && let Err(reason) = self.work.enqueue(
                now,
                a,
                Some(u64::from(out.operation_id)),
                deadline.min(out.expires_at),
                &out.frame,
            )
        {
            let e = self.sennet.fail_tx(now, out.operation_id, out.identity)?;
            self.event(Event::Sennet(e));
            self.event(Event::FrameDropped {
                instance: SENNET,
                reason,
            });
        }
        Ok(self.take_report())
    }
    pub fn send_tucket(
        &mut self,
        now: u64,
        to: u8,
        text: &str,
        policy: tucket::node::TextRetryPolicy,
        timing: tucket::instance::SendTiming,
    ) -> Result<tucket::instance::OperationId, Error> {
        self.require(now, TUCKET)?;
        if now
            .checked_add(self.config.tx_budget_ms)
            .ok_or(Error::TimeOverflow)?
            > self.work_until()
        {
            return Err(WorkError::Expired.into());
        }
        Ok(self.tucket.begin_send(now, to, text, policy, timing)?)
    }
    pub fn advertise_tucket(
        &mut self,
        now: u64,
        timestamp: u32,
        data: &[u8],
    ) -> Result<Report, Error> {
        self.require(now, TUCKET)?;
        if data.len() > 32 {
            return Err(Error::FrameTooLong);
        }
        let f = self.tucket.active_node_mut()?.advert_frame(timestamp, data);
        let d = self.ttl(now)?;
        self.queue(now, None, d, &f);
        Ok(self.take_report())
    }
    pub fn begin_tx(&mut self, now: u64) -> Result<Option<Transmission>, Error> {
        self.time(now)?;
        let a = self.active().ok_or(Error::Inactive)?;
        let Some(deadline) = self.work.first_deadline() else {
            return Ok(None);
        };
        let finish = now
            .checked_add(self.config.tx_budget_ms)
            .ok_or(Error::TimeOverflow)?;
        if finish >= deadline.min(self.work_until()) {
            return Ok(None);
        }
        Ok(self.work.begin(now, a)?)
    }
    pub fn complete_tx(
        &mut self,
        now: u64,
        id: WorkId,
        transmitted: bool,
    ) -> Result<Report, Error> {
        self.time(now)?;
        self.work.complete(now, id)?;
        if id.activation.instance == SENNET {
            let identity = self.sennet.pending_identity().ok_or(Error::Inactive)?;
            let op = id.operation.ok_or(Error::Inactive)? as u32;
            let e = if transmitted {
                self.sennet.complete_tx(now, op, identity)?
            } else {
                self.sennet.fail_tx(now, op, identity)?
            };
            self.event(Event::Sennet(e));
        }
        if !transmitted {
            self.event(Event::WorkLost(id));
        }
        Ok(self.take_report())
    }
}
