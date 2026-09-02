//! FS2: the signed command envelope.
//!
//! A field node holds operator *public* keys and nothing that authorizes anything. A command
//! is therefore a signed statement that travels as data, verified against a stored allowlist,
//! and valid however it arrived. Serial, LoRa, BLE, and a foreign mesh bearer all hand the
//! same bytes to the same verifier, because [`Verifier::accept`] takes bytes and nothing
//! else. There is deliberately no entry point that accepts a link, a session, or a peer: a
//! hijacked session cannot become authority it never had.
//!
//! ```text
//! command  = envelope || ed25519_signature(64)
//! envelope = version(1) || class(1) || key_id(16) || target(16)
//!            || counter(8, big-endian) || opcode(1) || payload_len(2) || payload
//! signed   = DOMAIN || envelope
//! ```
//!
//! # Why this and not the signed artifact
//!
//! [`crate::artifact`] is an interoperable carrier and could hold a command payload. It is
//! not used here. Two reasons, recorded in
//! `design_docs/2026-08-10_fs2_command_carrier_decision.md`: a fixed-offset envelope gives
//! the on-metal path one bounded parser rather than a MessagePack reader, which is what FS1
//! asks for; and the artifact re-sends the signer's 64-byte public key on every command,
//! which the node already holds in its allowlist and must not learn from the message anyway.
//!
//! # Freshness without a clock
//!
//! A field node has no RTC, no trustworthy time at reboot, and sits on a mesh with hour-scale
//! delivery, so wall-clock expiry is the wrong primitive. Freshness is a per-operator
//! monotonic counter instead. Acceptance is a *window*: strictly greater than the last
//! accepted value, and no more than [`COUNTER_WINDOW`] beyond it. The upper bound is the
//! half that is easy to miss. Without it, one intercepted command carrying a counter near
//! `u64::MAX` would lock its operator out of that node permanently, which is a denial of
//! service anyone who can listen could mount.
//!
//! The counter advances only after a signature verifies, and [`Verifier::ledger`] exposes it
//! so FS3 can make it durable across reboots. Until FS3 lands, a reboot resets the ledger and
//! the replay window with it; [`Verifier::restore`] is the seam that closes.

use core::fmt;

use heapless::Vec as FixedVec;

use crate::hash::{ADDRESS_HASH_LEN, AddressHash};
use crate::identity::{Identity, PrivateIdentity, SIGNATURE_LEN};

/// Envelope version. A node refuses anything else rather than guessing at the layout.
pub const VERSION: u8 = 1;

/// Domain separation, prefixed to the signed bytes and never sent.
///
/// Signatures elsewhere in this crate cover announces and link proofs. Without a domain
/// prefix, bytes that happened to validate in one context could be replayed into another;
/// with it, a command signature is only ever a command signature.
pub const DOMAIN: &[u8] = b"retinue-command-v1";

/// Fixed-offset header length: version, class, key id, target, counter, opcode, length.
pub const HEADER_LEN: usize = 1 + 1 + ADDRESS_HASH_LEN + ADDRESS_HASH_LEN + 8 + 1 + 2;

/// Largest payload a command may carry.
///
/// A command is an instruction, not a transfer. Anything that needs bulk should be a
/// resource the command *names*. This keeps a whole command inside a single Reticulum packet
/// and inside a LoRa frame, so authorization never depends on reassembly.
pub const MAX_PAYLOAD: usize = 256;

/// Largest complete command, envelope plus signature.
pub const MAX_COMMAND_LEN: usize = HEADER_LEN + MAX_PAYLOAD + SIGNATURE_LEN;

/// How far ahead of the last accepted counter a command may be.
///
/// Generous on purpose: an operator's counter is theirs, not this node's, so a node that has
/// been unreachable while the operator commanded the rest of the fleet must still be able to
/// catch up. Bounding it at all is what stops a single intercepted command from locking an
/// operator out forever.
pub const COUNTER_WINDOW: u64 = 4096;

/// Who a command is addressed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetClass {
    /// The target field holds one node's address hash. Nobody else acts on it.
    Node,
    /// The target field holds a fleet id. Every node configured with that id acts on it.
    Fleet,
}

impl TargetClass {
    const fn code(self) -> u8 {
        match self {
            Self::Node => 0,
            Self::Fleet => 1,
        }
    }

    const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Node),
            1 => Some(Self::Fleet),
            _ => None,
        }
    }
}

/// A command, either about to be signed or just verified.
#[derive(Clone, PartialEq, Eq)]
pub struct Command<'a> {
    /// The operator identity hash that signed, or will sign. On [`Command::sign`] this field
    /// is overwritten with the signer's own hash, so the two can never disagree on the wire.
    pub key_id: AddressHash,
    pub class: TargetClass,
    /// A node address hash or a fleet id, according to `class`.
    pub target: AddressHash,
    pub counter: u64,
    /// What to do. Opcode semantics belong to the consuming firmware, not to this module.
    pub opcode: u8,
    pub payload: &'a [u8],
}

impl fmt::Debug for Command<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Command")
            .field("key_id", &self.key_id)
            .field("class", &self.class)
            .field("target", &self.target)
            .field("counter", &self.counter)
            .field("opcode", &self.opcode)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

impl<'a> Command<'a> {
    /// Sign and serialize.
    ///
    /// The returned bytes are the whole command: envelope then signature.
    pub fn sign(&self, signer: &PrivateIdentity) -> Result<FixedVec<u8, MAX_COMMAND_LEN>, Refusal> {
        if self.payload.len() > MAX_PAYLOAD {
            return Err(Refusal::PayloadTooLong);
        }
        let mut out = FixedVec::new();
        let push = |out: &mut FixedVec<u8, MAX_COMMAND_LEN>, bytes: &[u8]| {
            out.extend_from_slice(bytes)
                .map_err(|_| Refusal::PayloadTooLong)
        };
        push(&mut out, &[VERSION, self.class.code()])?;
        push(&mut out, signer.hash().as_slice())?;
        push(&mut out, self.target.as_slice())?;
        push(&mut out, &self.counter.to_be_bytes())?;
        push(&mut out, &[self.opcode])?;
        push(&mut out, &(self.payload.len() as u16).to_be_bytes())?;
        push(&mut out, self.payload)?;

        let mut signed = FixedVec::<u8, { DOMAIN.len() + HEADER_LEN + MAX_PAYLOAD }>::new();
        signed
            .extend_from_slice(DOMAIN)
            .map_err(|_| Refusal::PayloadTooLong)?;
        signed
            .extend_from_slice(&out)
            .map_err(|_| Refusal::PayloadTooLong)?;
        let signature = signer.sign(&signed);
        push(&mut out, &signature)?;
        Ok(out)
    }

    /// Parse without verifying anything. Private on purpose: an unverified command is not a
    /// command, and a public parser is an invitation to act on one.
    fn parse(bytes: &'a [u8]) -> Result<(Self, &'a [u8], &'a [u8]), Refusal> {
        if bytes.len() < HEADER_LEN + SIGNATURE_LEN {
            return Err(Refusal::Malformed);
        }
        if bytes[0] != VERSION {
            return Err(Refusal::UnknownVersion);
        }
        let class = TargetClass::from_code(bytes[1]).ok_or(Refusal::Malformed)?;
        let key_id = AddressHash::from_slice(&bytes[2..]).ok_or(Refusal::Malformed)?;
        let target =
            AddressHash::from_slice(&bytes[2 + ADDRESS_HASH_LEN..]).ok_or(Refusal::Malformed)?;
        let at = 2 + ADDRESS_HASH_LEN * 2;
        let counter = u64::from_be_bytes(bytes[at..at + 8].try_into().expect("8 bytes"));
        let opcode = bytes[at + 8];
        let payload_len = usize::from(u16::from_be_bytes(
            bytes[at + 9..at + 11].try_into().expect("2 bytes"),
        ));
        if payload_len > MAX_PAYLOAD {
            return Err(Refusal::PayloadTooLong);
        }
        // The declared length has to account for every byte: envelope, payload, signature,
        // and nothing over. A command with a tail is not the command that was signed.
        if bytes.len() != HEADER_LEN + payload_len + SIGNATURE_LEN {
            return Err(Refusal::Malformed);
        }
        let payload = &bytes[HEADER_LEN..HEADER_LEN + payload_len];
        let (envelope, signature) = bytes.split_at(HEADER_LEN + payload_len);
        Ok((
            Self {
                key_id,
                class,
                target,
                counter,
                opcode,
                payload,
            },
            envelope,
            signature,
        ))
    }
}

/// A command whose target, signer, counter window, and signature have all been checked.
///
/// Only [`Verifier::verify`] constructs this witness. Its borrowed payload is still the caller's
/// input, but its metadata is authenticated and its counter has already advanced in the
/// verifier's ledger. Carrier adapters can inspect it or project it as a read-only [`Command`],
/// but cannot manufacture authority by constructing one themselves.
pub struct VerifiedCommand<'a> {
    command: Command<'a>,
}

impl fmt::Debug for VerifiedCommand<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedCommand")
            .field("key_id", &self.key_id())
            .field("class", &self.class())
            .field("target", &self.target())
            .field("counter", &self.counter())
            .field("opcode", &self.opcode())
            .field("payload_len", &self.payload().len())
            .finish()
    }
}

impl<'a> VerifiedCommand<'a> {
    /// The allowlisted operator that signed this command.
    pub const fn key_id(&self) -> AddressHash {
        self.command.key_id
    }

    /// Whether this was addressed to one node or to its configured fleet.
    pub const fn class(&self) -> TargetClass {
        self.command.class
    }

    /// The authenticated node or fleet target.
    pub const fn target(&self) -> AddressHash {
        self.command.target
    }

    /// The accepted, monotonic operator counter.
    pub const fn counter(&self) -> u64 {
        self.command.counter
    }

    /// The authenticated firmware-defined operation code.
    pub const fn opcode(&self) -> u8 {
        self.command.opcode
    }

    /// The authenticated operation body.
    pub const fn payload(&self) -> &'a [u8] {
        self.command.payload
    }

    /// Project the authenticated command for read-only consumers.
    pub const fn as_command(&self) -> &Command<'a> {
        &self.command
    }

    fn into_command(self) -> Command<'a> {
        self.command
    }
}

/// Why a node refused to act.
///
/// Every variant is a counted refusal rather than a panic. On the RF path a loud assert is a
/// remote reset button, so this path returns and increments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Too short, wrong length for its declared payload, or an unknown target class.
    Malformed,
    /// A version this node does not implement.
    UnknownVersion,
    /// The payload exceeds [`MAX_PAYLOAD`].
    PayloadTooLong,
    /// The key id is not on this node's allowlist. Not an error: it is the ordinary fate of
    /// a fleet command from an operator this node does not answer to.
    UnknownKey,
    /// Addressed to another node, or to a fleet this node is not in.
    WrongTarget,
    /// The counter is at or behind the last accepted one: a replay.
    CounterReplayed,
    /// The counter is further ahead than [`COUNTER_WINDOW`] allows.
    CounterTooFar,
    /// The signature did not verify against the allowlisted key.
    BadSignature,
    /// The allowlist is full.
    AllowlistFull,
}

/// One allowlisted operator and the highest counter this node has accepted from them.
#[derive(Debug, Clone, Copy)]
pub struct Operator {
    pub key_id: AddressHash,
    pub identity: Identity,
    pub counter: u64,
}

/// The one verifier every transport hands bytes to.
///
/// `N` bounds the allowlist at compile time, because a field node's tables are all bounded
/// (FS6) and an allowlist that can grow is a table an attacker can grow.
pub struct Verifier<const N: usize> {
    node: AddressHash,
    fleet: Option<AddressHash>,
    operators: FixedVec<Operator, N>,
    refusals: u32,
    accepted: u32,
}

impl<const N: usize> Verifier<N> {
    /// A verifier for one node, in no fleet, trusting nobody yet.
    pub fn new(node: AddressHash) -> Self {
        Self {
            node,
            fleet: None,
            operators: FixedVec::new(),
            refusals: 0,
            accepted: 0,
        }
    }

    /// Join a fleet, making [`TargetClass::Fleet`] commands for that id acceptable.
    pub fn join_fleet(&mut self, fleet: AddressHash) {
        self.fleet = Some(fleet);
    }

    /// Add an operator public key to the allowlist.
    ///
    /// Re-authorizing a key already present is idempotent and leaves its counter alone, so a
    /// configuration replay cannot reopen a replay window.
    pub fn authorize(&mut self, identity: Identity) -> Result<(), Refusal> {
        let key_id = identity.hash();
        if self.operators.iter().any(|op| op.key_id == key_id) {
            return Ok(());
        }
        self.operators
            .push(Operator {
                key_id,
                identity,
                counter: 0,
            })
            .map_err(|_| Refusal::AllowlistFull)
    }

    /// Drop an operator key. Returns whether it was there.
    pub fn revoke(&mut self, key_id: AddressHash) -> bool {
        let Some(at) = self.operators.iter().position(|op| op.key_id == key_id) else {
            return false;
        };
        self.operators.swap_remove(at);
        true
    }

    /// The allowlist with its counters: what FS3 must make durable.
    pub fn ledger(&self) -> impl Iterator<Item = &Operator> {
        self.operators.iter()
    }

    /// Restore a counter read back from durable storage.
    ///
    /// Only ever moves forward. A restore that tried to lower a counter would be a replay
    /// window handed out by the storage layer, so it is refused silently rather than obeyed.
    pub fn restore(&mut self, key_id: AddressHash, counter: u64) -> bool {
        let Some(op) = self.operators.iter_mut().find(|op| op.key_id == key_id) else {
            return false;
        };
        op.counter = op.counter.max(counter);
        true
    }

    /// Commands refused, for the counters-not-panics posture.
    pub fn refusals(&self) -> u32 {
        self.refusals
    }

    /// Commands accepted.
    pub fn accepted(&self) -> u32 {
        self.accepted
    }

    /// Verify a command and return the proof that may authorize a consumer to act.
    ///
    /// Checks run cheapest first: shape, then version, then allowlist, then addressing, then
    /// the counter window, and only then the signature. An unsigned flood therefore costs
    /// lookups rather than curve operations. It cannot be made free: an attacker who knows a
    /// live key id and a plausible counter still forces one verification per attempt, which
    /// is a rate-limiting problem for the interface admission layer and not something a
    /// signature scheme can solve here.
    pub fn verify<'a>(&mut self, bytes: &'a [u8]) -> Result<VerifiedCommand<'a>, Refusal> {
        match self.check(bytes) {
            Ok(command) => {
                self.accepted = self.accepted.saturating_add(1);
                Ok(VerifiedCommand { command })
            }
            Err(refusal) => {
                self.refusals = self.refusals.saturating_add(1);
                Err(refusal)
            }
        }
    }

    /// Verify a command and project it into the historical command result.
    ///
    /// This compatibility API follows [`Self::verify`]'s single verification path; it does not
    /// parse, validate, or advance the counter a second time.
    pub fn accept<'a>(&mut self, bytes: &'a [u8]) -> Result<Command<'a>, Refusal> {
        self.verify(bytes).map(VerifiedCommand::into_command)
    }

    fn check<'a>(&mut self, bytes: &'a [u8]) -> Result<Command<'a>, Refusal> {
        let (command, envelope, signature) = Command::parse(bytes)?;

        let expected = match command.class {
            TargetClass::Node => self.node,
            TargetClass::Fleet => self.fleet.ok_or(Refusal::WrongTarget)?,
        };
        if command.target != expected {
            return Err(Refusal::WrongTarget);
        }

        let operator = self
            .operators
            .iter()
            .position(|op| op.key_id == command.key_id)
            .ok_or(Refusal::UnknownKey)?;
        let last = self.operators[operator].counter;
        if command.counter <= last {
            return Err(Refusal::CounterReplayed);
        }
        if command.counter - last > COUNTER_WINDOW {
            return Err(Refusal::CounterTooFar);
        }

        let signature: [u8; SIGNATURE_LEN] =
            signature.try_into().map_err(|_| Refusal::Malformed)?;
        let mut signed = FixedVec::<u8, { DOMAIN.len() + HEADER_LEN + MAX_PAYLOAD }>::new();
        signed
            .extend_from_slice(DOMAIN)
            .map_err(|_| Refusal::Malformed)?;
        signed
            .extend_from_slice(envelope)
            .map_err(|_| Refusal::Malformed)?;
        if !self.operators[operator]
            .identity
            .verify(&signed, &signature)
        {
            return Err(Refusal::BadSignature);
        }

        // Only now. A counter that advanced on an unverified command would let anyone
        // desynchronize an operator by shouting.
        self.operators[operator].counter = command.counter;
        Ok(command)
    }
}
