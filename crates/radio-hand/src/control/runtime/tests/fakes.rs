use core::cell::{Cell, RefCell};
use std::rc::Rc;

use heapless::Vec;

use super::super::*;
use crate::control::{
    BoardRecoveryFacts, COMMIT_TOKEN_LEN, ChangeId, ConfigGeneration, ControllerRole,
    ManagementCarrier, ManagementCarrierSet, NodeId, Operation, OwnerGrant, PublicConfigurationV1,
    RecoveryClause, RecoveryPathFacts, RecoveryPolicy, Request, ReticulumTransportPolicy,
    TransactionId,
};
use crate::region::Region;
use crate::store::Slot;
use retinue::identity::PrivateIdentity;

pub const PAGE: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    EnterQuiet,
    AbortEnteringQuiet,
    FinishQuiet(QuietExit),
    AbortQuiet,
    Read(Slot),
    Erase(Slot),
    Program(Slot),
    Apply(PublicConfigurationV1),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuietFailure {
    Enter,
    Finish,
}

pub struct FakeLiveOwner<'a> {
    trace: &'a Trace,
    pub store: FakeStore,
    pub applier: FakeApplier,
    pub fail_enter: bool,
    pub fail_finish: bool,
    pub pending_finish: bool,
    pub exit: QuietExit,
}

pub struct FakeGuard<'a> {
    trace: &'a Trace,
    store: &'a mut FakeStore,
    applier: &'a mut FakeApplier,
    fail_finish: bool,
    pending_finish: bool,
    exit: QuietExit,
}

impl<'a> FakeLiveOwner<'a> {
    pub fn new(trace: &'a Trace, store: FakeStore, applier: FakeApplier) -> Self {
        Self {
            trace,
            store,
            applier,
            fail_enter: false,
            fail_finish: false,
            pending_finish: false,
            exit: QuietExit::Resumed,
        }
    }
}

impl QuietWindow for FakeLiveOwner<'_> {
    type Error = QuietFailure;
    type StoreError = StoreFailure;
    type ApplyError = ApplyFailure;
    type Guard<'a>
        = FakeGuard<'a>
    where
        Self: 'a;

    async fn enter(&mut self) -> Result<Self::Guard<'_>, Self::Error> {
        if self.fail_enter {
            return Err(QuietFailure::Enter);
        }
        self.trace.push(Call::EnterQuiet);
        Ok(FakeGuard {
            trace: self.trace,
            store: &mut self.store,
            applier: &mut self.applier,
            fail_finish: self.fail_finish,
            pending_finish: self.pending_finish,
            exit: self.exit,
        })
    }
}

impl QuietGuard for FakeGuard<'_> {
    type Error = QuietFailure;

    fn abort(&mut self) {
        self.trace.push(Call::AbortQuiet)
    }

    async fn finish(&mut self) -> Result<QuietExit, Self::Error> {
        self.trace.push(Call::FinishQuiet(self.exit));
        if self.pending_finish {
            core::future::pending::<()>().await;
        }
        if self.fail_finish {
            Err(QuietFailure::Finish)
        } else {
            Ok(self.exit)
        }
    }
}

impl AbSlotStore for FakeGuard<'_> {
    type Error = StoreFailure;

    fn read_slot(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error> {
        self.store.read_slot(slot, out)
    }

    fn erase_slot(&mut self, slot: Slot) -> Result<(), Self::Error> {
        self.store.erase_slot(slot)
    }

    fn program_slot(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error> {
        self.store.program_slot(slot, record)
    }
}

impl ConfigApplier for FakeGuard<'_> {
    type Error = ApplyFailure;

    async fn apply(&mut self, configuration: &DurableConfig) -> Result<(), Self::Error> {
        self.applier.apply(configuration).await
    }
}

/// Models the part of `QuietWindow::enter` before a `QuietGuard` exists. Its entry future
/// stops radio work, then never returns a guard; dropping that future must synchronously latch
/// reset because the runtime has no guard through which it can abort.
pub struct PendingEntryQuiet<'a> {
    trace: &'a Trace,
    radio_stopped: Cell<bool>,
    reset_latched: Cell<bool>,
}

impl<'a> PendingEntryQuiet<'a> {
    pub fn new(trace: &'a Trace) -> Self {
        Self {
            trace,
            radio_stopped: Cell::new(false),
            reset_latched: Cell::new(false),
        }
    }

    pub fn radio_stopped(&self) -> bool {
        self.radio_stopped.get()
    }

    pub fn reset_latched(&self) -> bool {
        self.reset_latched.get()
    }
}

struct EnterAbort<'a> {
    trace: &'a Trace,
    radio_stopped: &'a Cell<bool>,
    reset_latched: &'a Cell<bool>,
}

impl Drop for EnterAbort<'_> {
    fn drop(&mut self) {
        self.radio_stopped.set(true);
        self.reset_latched.set(true);
        self.trace.push(Call::AbortEnteringQuiet);
    }
}

impl QuietWindow for PendingEntryQuiet<'_> {
    type Error = QuietFailure;
    type StoreError = StoreFailure;
    type ApplyError = ApplyFailure;
    type Guard<'a>
        = FakeGuard<'a>
    where
        Self: 'a;

    async fn enter(&mut self) -> Result<Self::Guard<'_>, Self::Error> {
        self.trace.push(Call::EnterQuiet);
        self.radio_stopped.set(true);
        let _abort = EnterAbort {
            trace: self.trace,
            radio_stopped: &self.radio_stopped,
            reset_latched: &self.reset_latched,
        };
        core::future::pending::<()>().await;
        unreachable!("pending entry cannot complete")
    }
}

#[derive(Clone, Default)]
pub struct Trace {
    calls: Rc<RefCell<Vec<Call, 64>>>,
}

impl Trace {
    fn push(&self, call: Call) {
        self.calls.borrow_mut().push(call).unwrap();
    }

    pub fn clear(&self) {
        self.calls.borrow_mut().clear();
    }

    pub fn snapshot(&self) -> Vec<Call, 64> {
        self.calls.borrow().clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreFailure {
    Read,
    Erase,
    Program,
}

pub struct FakeStore {
    pub a: [u8; PAGE],
    pub b: [u8; PAGE],
    trace: Trace,
    pub read_count: usize,
    pub fail_read_at: Option<usize>,
    pub fail_erase: bool,
    pub fail_program: bool,
    pub corrupt_readback: bool,
    corrupt_next_read: Option<Slot>,
}

impl FakeStore {
    pub fn blank(trace: &Trace) -> Self {
        Self {
            a: [0xFF; PAGE],
            b: [0xFF; PAGE],
            trace: trace.clone(),
            read_count: 0,
            fail_read_at: None,
            fail_erase: false,
            fail_program: false,
            corrupt_readback: false,
            corrupt_next_read: None,
        }
    }
}

impl AbSlotStore for FakeStore {
    type Error = StoreFailure;

    fn read_slot(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error> {
        self.trace.push(Call::Read(slot));
        self.read_count += 1;
        if self.fail_read_at == Some(self.read_count) {
            return Err(StoreFailure::Read);
        }
        match slot {
            Slot::A => out.copy_from_slice(&self.a),
            Slot::B => out.copy_from_slice(&self.b),
        }
        if self.corrupt_next_read == Some(slot) {
            out[0] ^= 1;
            self.corrupt_next_read = None;
        }
        Ok(())
    }

    fn erase_slot(&mut self, slot: Slot) -> Result<(), Self::Error> {
        self.trace.push(Call::Erase(slot));
        if self.fail_erase {
            return Err(StoreFailure::Erase);
        }
        match slot {
            Slot::A => self.a.fill(0xFF),
            Slot::B => self.b.fill(0xFF),
        }
        Ok(())
    }

    fn program_slot(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error> {
        self.trace.push(Call::Program(slot));
        if self.fail_program {
            return Err(StoreFailure::Program);
        }
        match slot {
            Slot::A => self.a[..record.len()].copy_from_slice(record),
            Slot::B => self.b[..record.len()].copy_from_slice(record),
        }
        if self.corrupt_readback {
            self.corrupt_readback = false;
            self.corrupt_next_read = Some(slot);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyFailure {
    Apply,
}

pub struct FakeApplier {
    trace: Trace,
    pub calls: usize,
    pub fail_at: Option<usize>,
    pub pending: bool,
    pub last_public: Option<PublicConfigurationV1>,
}

impl FakeApplier {
    pub fn new(trace: &Trace) -> Self {
        Self {
            trace: trace.clone(),
            calls: 0,
            fail_at: None,
            pending: false,
            last_public: None,
        }
    }
}

impl ConfigApplier for FakeApplier {
    type Error = ApplyFailure;

    async fn apply(&mut self, configuration: &DurableConfig) -> Result<(), Self::Error> {
        self.trace.push(Call::Apply(configuration.public));
        self.calls += 1;
        if self.pending {
            core::future::pending::<()>().await;
        }
        if self.fail_at == Some(self.calls) {
            return Err(ApplyFailure::Apply);
        }
        self.last_public = Some(configuration.public);
        Ok(())
    }
}

pub fn configuration(public: &[u8]) -> DurableConfig {
    DurableConfig {
        public: PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(
                902_000_000 + u32::from(public.first().copied().unwrap_or(0)) * 100_000,
            ),
            ReticulumTransportPolicy::new(true, true, 8).unwrap(),
            ManagementCarrierSet::from_mask(0b1001).unwrap(),
        )
        .unwrap(),
        sealed_credentials: Vec::try_from(b"sealed".as_slice()).unwrap(),
    }
}

pub fn recovery_policy() -> RecoveryPolicy {
    RecoveryPolicy::new(
        RecoveryClause::new(ManagementCarrierSet::from_mask(1).unwrap(), 1).unwrap(),
        RecoveryClause::disabled(),
    )
    .unwrap()
}

pub fn recovery_facts() -> BoardRecoveryFacts {
    BoardRecoveryFacts::new(
        Vec::from_slice(&[
            RecoveryPathFacts::new(ManagementCarrier::Usb, true, false, false).unwrap(),
            RecoveryPathFacts::new(ManagementCarrier::Reticulum, false, true, true).unwrap(),
            RecoveryPathFacts::new(ManagementCarrier::Ip, false, true, true).unwrap(),
            RecoveryPathFacts::new(ManagementCarrier::Ble, true, false, false).unwrap(),
        ])
        .unwrap(),
    )
    .unwrap()
}

pub fn state() -> DurableState {
    DurableState::new(
        NodeId([0x10; 16]),
        Vec::from_slice(&[OwnerGrant::from_public_identity(
            public_identity(0x30),
            ControllerRole::Owner,
        )])
        .unwrap(),
        ConfigGeneration(7),
        configuration(b"old"),
        recovery_policy(),
        &recovery_facts(),
    )
    .unwrap()
}

pub fn controller() -> VerifiedController {
    VerifiedController::from_verified_key(
        OwnerGrant::from_public_identity(public_identity(0x30), ControllerRole::Owner).controller(),
    )
}

pub fn public_identity(value: u8) -> [u8; 64] {
    PrivateIdentity::from_secret_bytes(&[value; 64])
        .public()
        .to_public_bytes()
}

pub fn key() -> SemanticTagKey {
    SemanticTagKey::from_bytes([0x80; 32])
}

pub fn apply_request(transaction: u8) -> Request {
    Request {
        transaction: TransactionId([transaction; 16]),
        transaction_sequence: u64::from(transaction),
        expected_generation: ConfigGeneration(7),
        operation: Operation::ProvisionalApply,
        arguments: Vec::try_from(b"configuration".as_slice()).unwrap(),
    }
}

#[cfg(feature = "control-retinue")]
pub fn encoded_request(request: &Request) -> Vec<u8, { crate::control::MAX_REQUEST_LEN }> {
    let mut bytes = [0; crate::control::MAX_REQUEST_LEN];
    let length = crate::control::encode_request(request, &mut bytes).unwrap();
    Vec::try_from(&bytes[..length]).unwrap()
}

pub fn commit_request(transaction: u8) -> Request {
    Request {
        transaction: TransactionId([transaction; 16]),
        transaction_sequence: u64::from(transaction),
        expected_generation: ConfigGeneration(7),
        operation: Operation::Commit,
        arguments: Vec::try_from(b"commit".as_slice()).unwrap(),
    }
}

pub fn prepared(change: u8, deadline_ms: u64) -> PreparedProvisional {
    PreparedProvisional {
        change: ChangeId([change; 16]),
        candidate: configuration(b"candidate"),
        deadline_ms,
        commit_token: [0xA5; COMMIT_TOKEN_LEN],
        result: Vec::try_from(b"applied".as_slice()).unwrap(),
    }
}

pub fn unsafe_prepared(change: u8, deadline_ms: u64) -> PreparedProvisional {
    let mut prepared = prepared(change, deadline_ms);
    prepared.candidate.public = PublicConfigurationV1::new(
        Region::Us915,
        selvage::PhyProfile::meshtastic_long_fast(906_875_000),
        ReticulumTransportPolicy::new(true, true, 8).unwrap(),
        ManagementCarrierSet::from_mask(0b1000).unwrap(),
    )
    .unwrap();
    prepared
}

pub fn seed(store: &mut FakeStore, state: &DurableState) {
    let mut body = [0; MAX_DURABLE_BODY];
    let mut page = [0xFF; PAGE];
    let write = next_record(&store.a, &store.b, state, &mut body, &mut page).unwrap();
    match write.slot {
        Slot::A => store.a = page,
        Slot::B => store.b = page,
    }
}

pub fn scratch<'a>(
    a: &'a mut [u8; PAGE],
    b: &'a mut [u8; PAGE],
    body: &'a mut [u8; MAX_DURABLE_BODY],
    page: &'a mut [u8; PAGE],
) -> DurableScratch<'a> {
    DurableScratch::new(a, b, body, page).unwrap()
}

#[cfg(feature = "control-retinue")]
pub fn operator() -> retinue::identity::PrivateIdentity {
    let mut secret = [0; retinue::identity::IDENTITY_LEN];
    secret[..32].fill(0x11);
    secret[32..].fill(0xEE);
    retinue::identity::PrivateIdentity::from_secret_bytes(&secret)
}

#[cfg(feature = "control-retinue")]
pub fn state_for_operator(operator: &retinue::identity::PrivateIdentity) -> DurableState {
    DurableState::new(
        NodeId([0x10; 16]),
        Vec::from_slice(&[OwnerGrant::from_retinue_identity(
            operator.public(),
            ControllerRole::Owner,
        )])
        .unwrap(),
        ConfigGeneration(7),
        configuration(b"old"),
        recovery_policy(),
        &recovery_facts(),
    )
    .unwrap()
}

#[cfg(feature = "control-retinue")]
pub fn signed_command(
    operator: &retinue::identity::PrivateIdentity,
    payload: &[u8],
    counter: u64,
) -> heapless::Vec<u8, { retinue::command::MAX_COMMAND_LEN }> {
    retinue::command::Command {
        key_id: operator.hash(),
        class: retinue::command::TargetClass::Node,
        target: retinue::hash::AddressHash::from_bytes([0x10; 16]),
        counter,
        opcode: crate::control::COMMAND_OPCODE,
        payload,
    }
    .sign(operator)
    .unwrap()
}
