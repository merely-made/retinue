use core::fmt;

use heapless::Vec;

pub const VERSION: u8 = 2;
pub(crate) const MAGIC: [u8; 4] = *b"RHC0";
pub(crate) const RESPONSE: u8 = 2;
pub const COMMAND_OPCODE: u8 = 0x40;
pub const ID_LEN: usize = 16;
pub const COMMIT_TOKEN_LEN: usize = 16;
pub const MAX_ARGUMENTS: usize = 217;
pub const MAX_RESULT: usize = 175;
pub const MAX_RECOVERY_PATHS: usize = 8;
pub const MAX_IMAGE_SLOTS: usize = 4;
pub const MAX_ADAPTERS: usize = 8;
pub const MAX_RADIOS: usize = 4;
pub const MAX_CARRIERS: usize = 8;
pub const MAX_REQUEST_LEN: usize = 4 + 1 + ID_LEN + 8 + 8 + 1 + 1 + MAX_ARGUMENTS;
pub const MAX_RESPONSE_LEN: usize =
    4 + 1 + 1 + ID_LEN + ID_LEN + 8 + 1 + 8 + 1 + 8 + COMMIT_TOKEN_LEN + 1 + MAX_RESULT;

pub const GOLDEN_REQUEST: [u8; 42] = [
    b'R', b'H', b'C', b'0', 2, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
    0x30, 0x30, 0x30, 0x30, 0x30, 9, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 4, 3, 1, 2, 3,
];
pub const GOLDEN_RESPONSE: [u8; 83] = [
    b'R', b'H', b'C', b'0', 2, 2, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10,
    0x10, 0x10, 0x10, 0x10, 0x10, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
    0x30, 0x30, 0x30, 0x30, 0x30, 7, 0, 0, 0, 0, 0, 0, 0, 1, 8, 0, 0, 0, 0, 0, 0, 0, 1, 9, 0, 0, 0,
    0, 0, 0, 0, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40,
    0x40, 0x40, 2, 0xC0, 0xDE,
];

/// Opaque control-node identity. A `TargetClass::Node` AddressHash stores these bytes; it is
/// not necessarily a Reticulum destination or carrier address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub [u8; ID_LEN]);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControllerId(pub [u8; ID_LEN]);
/// Proof from WN1's verified outer-command and grant lookup. Its constructor is private to
/// this crate, so carrier adapters cannot manufacture authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedController(pub(crate) ControllerId);
impl VerifiedController {
    #[allow(dead_code)]
    pub(crate) const fn from_verified_key(id: ControllerId) -> Self {
        Self(id)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransactionId(pub [u8; ID_LEN]);
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ConfigGeneration(pub u64);
impl ConfigGeneration {
    pub const fn checked_successor(self) -> Result<Self, Refusal> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(Refusal::GenerationExhausted),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ControllerRole {
    Observer = 0,
    Operator = 1,
    Updater = 2,
    Owner = 3,
}
impl ControllerRole {
    pub(crate) fn decode(tag: u8) -> Result<Self, DecodeError> {
        match tag {
            0 => Ok(Self::Observer),
            1 => Ok(Self::Operator),
            2 => Ok(Self::Updater),
            3 => Ok(Self::Owner),
            other => Err(DecodeError::UnknownControllerRole(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Operation {
    Capabilities = 0,
    Status = 1,
    WifiScan = 2,
    OwnerClaim = 3,
    StageConfiguration = 4,
    ProvisionalApply = 5,
    Commit = 6,
    Revert = 7,
    Reboot = 8,
    RecoveryStatus = 9,
    FirmwareStage = 10,
    FirmwareActivate = 11,
    AdapterPolicy = 12,
}
impl Operation {
    pub const fn requires_generation(self) -> bool {
        !matches!(
            self,
            Self::Capabilities | Self::Status | Self::WifiScan | Self::RecoveryStatus
        )
    }
    pub(crate) fn decode(tag: u8) -> Result<Self, DecodeError> {
        match tag {
            0 => Ok(Self::Capabilities),
            1 => Ok(Self::Status),
            2 => Ok(Self::WifiScan),
            3 => Ok(Self::OwnerClaim),
            4 => Ok(Self::StageConfiguration),
            5 => Ok(Self::ProvisionalApply),
            6 => Ok(Self::Commit),
            7 => Ok(Self::Revert),
            8 => Ok(Self::Reboot),
            9 => Ok(Self::RecoveryStatus),
            10 => Ok(Self::FirmwareStage),
            11 => Ok(Self::FirmwareActivate),
            12 => Ok(Self::AdapterPolicy),
            other => Err(DecodeError::UnknownOperation(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Refusal {
    Unauthorized = 0,
    WrongNode = 1,
    StaleGeneration = 2,
    TransactionConflict = 3,
    TransactionExpired = 4,
    TransactionTooFar = 14,
    InvalidCommit = 5,
    UnsafeRecoveryPath = 6,
    UnsupportedOperation = 7,
    InvalidArguments = 8,
    Capacity = 9,
    PhysicalPresenceRequired = 10,
    Busy = 11,
    Internal = 12,
    GenerationExhausted = 13,
}
impl Refusal {
    pub(crate) fn decode(tag: u8) -> Result<Self, DecodeError> {
        match tag {
            0 => Ok(Self::Unauthorized),
            1 => Ok(Self::WrongNode),
            2 => Ok(Self::StaleGeneration),
            3 => Ok(Self::TransactionConflict),
            4 => Ok(Self::TransactionExpired),
            5 => Ok(Self::InvalidCommit),
            6 => Ok(Self::UnsafeRecoveryPath),
            7 => Ok(Self::UnsupportedOperation),
            8 => Ok(Self::InvalidArguments),
            9 => Ok(Self::Capacity),
            10 => Ok(Self::PhysicalPresenceRequired),
            11 => Ok(Self::Busy),
            12 => Ok(Self::Internal),
            13 => Ok(Self::GenerationExhausted),
            14 => Ok(Self::TransactionTooFar),
            other => Err(DecodeError::UnknownRefusal(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Applied,
    Observed,
    Provisional,
    Refused(Refusal),
}

/// The bounded inner payload carried by an authenticated outer command.
#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    pub transaction: TransactionId,
    /// Monotonic, controller-local mutation sequence. Durable admission binds it to the
    /// complete semantic request so eviction cannot make an old mutation reusable.
    pub transaction_sequence: u64,
    pub expected_generation: ConfigGeneration,
    pub operation: Operation,
    /// Opaque operation bytes. This may include a secret, so Debug reports only its length.
    pub arguments: Vec<u8, MAX_ARGUMENTS>,
}
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("transaction", &self.transaction)
            .field("transaction_sequence", &self.transaction_sequence)
            .field("expected_generation", &self.expected_generation)
            .field("operation", &self.operation)
            .field("arguments_len", &self.arguments.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum ResponseBody {
    Applied(Vec<u8, MAX_RESULT>),
    Observed(Vec<u8, MAX_RESULT>),
    Provisional {
        deadline_ms: u64,
        commit_token: [u8; COMMIT_TOKEN_LEN],
        result: Vec<u8, MAX_RESULT>,
    },
    Refused {
        reason: Refusal,
        result: Vec<u8, MAX_RESULT>,
    },
    Capabilities(Capabilities),
}
impl ResponseBody {
    pub fn disposition(&self) -> Disposition {
        match self {
            Self::Applied(_) => Disposition::Applied,
            Self::Observed(_) | Self::Capabilities(_) => Disposition::Observed,
            Self::Provisional { .. } => Disposition::Provisional,
            Self::Refused { reason, .. } => Disposition::Refused(*reason),
        }
    }
}
impl fmt::Debug for ResponseBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Applied(result) => f.debug_tuple("Applied").field(&result.len()).finish(),
            Self::Observed(result) => f.debug_tuple("Observed").field(&result.len()).finish(),
            Self::Provisional {
                deadline_ms,
                result,
                ..
            } => f
                .debug_struct("Provisional")
                .field("deadline_ms", deadline_ms)
                .field("commit_token", &"[redacted]")
                .field("result_len", &result.len())
                .finish(),
            Self::Refused { reason, result } => f
                .debug_struct("Refused")
                .field("reason", reason)
                .field("result_len", &result.len())
                .finish(),
            Self::Capabilities(capabilities) => {
                f.debug_tuple("Capabilities").field(capabilities).finish()
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Response {
    pub node: NodeId,
    pub transaction: TransactionId,
    pub known_good_generation: ConfigGeneration,
    pub effective_generation: Option<ConfigGeneration>,
    pub body: ResponseBody,
}
impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("node", &self.node)
            .field("transaction", &self.transaction)
            .field("known_good_generation", &self.known_good_generation)
            .field("effective_generation", &self.effective_generation)
            .field("body", &self.body)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BoardClass {
    Simple = 0,
    Switchable = 1,
    Multiplex = 2,
    Hybrid = 3,
}
impl BoardClass {
    pub(crate) fn decode(tag: u8) -> Result<Self, DecodeError> {
        match tag {
            0 => Ok(Self::Simple),
            1 => Ok(Self::Switchable),
            2 => Ok(Self::Multiplex),
            3 => Ok(Self::Hybrid),
            other => Err(DecodeError::UnknownBoardClass(other)),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ImageKind {
    Retinue = 0,
    Foreign = 1,
    Recovery = 2,
}
impl ImageKind {
    pub(crate) fn decode(tag: u8) -> Result<Self, DecodeError> {
        match tag {
            0 => Ok(Self::Retinue),
            1 => Ok(Self::Foreign),
            2 => Ok(Self::Recovery),
            other => Err(DecodeError::UnknownImageKind(other)),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageSlot {
    pub slot: u8,
    pub kind: ImageKind,
    pub verified: bool,
    pub active: bool,
    pub trial: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResidentAdapter {
    Reticulum = 0,
    MeshCore = 1,
    Meshtastic = 2,
    Other = 3,
}
impl ResidentAdapter {
    pub(crate) fn decode(tag: u8) -> Result<Self, DecodeError> {
        match tag {
            0 => Ok(Self::Reticulum),
            1 => Ok(Self::MeshCore),
            2 => Ok(Self::Meshtastic),
            3 => Ok(Self::Other),
            other => Err(DecodeError::UnknownAdapter(other)),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterCapability {
    pub adapter: ResidentAdapter,
    pub enabled: bool,
    pub radio_leases: u8,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RadioKind {
    Sx1262 = 0,
    Sx127x = 1,
    Other = 2,
}
impl RadioKind {
    pub(crate) fn decode(tag: u8) -> Result<Self, DecodeError> {
        match tag {
            0 => Ok(Self::Sx1262),
            1 => Ok(Self::Sx127x),
            2 => Ok(Self::Other),
            other => Err(DecodeError::UnknownRadio(other)),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioCapability {
    pub radio: RadioKind,
    pub simultaneous_receive_profiles: u8,
    pub tx: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ManagementCarrier {
    Usb = 0,
    Ble = 1,
    Ip = 2,
    Reticulum = 3,
}
impl ManagementCarrier {
    pub(crate) fn decode(tag: u8) -> Result<Self, DecodeError> {
        match tag {
            0 => Ok(Self::Usb),
            1 => Ok(Self::Ble),
            2 => Ok(Self::Ip),
            3 => Ok(Self::Reticulum),
            other => Err(DecodeError::UnknownCarrier(other)),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarrierCapability {
    pub carrier: ManagementCarrier,
    pub authenticated: bool,
    pub max_frame: u16,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryPath {
    pub carrier: ManagementCarrier,
    pub enabled: bool,
    pub remote: bool,
    pub physical_presence: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub board_class: BoardClass,
    pub controller_role: Option<ControllerRole>,
    pub image_slots: Vec<ImageSlot, MAX_IMAGE_SLOTS>,
    pub adapters: Vec<AdapterCapability, MAX_ADAPTERS>,
    pub radios: Vec<RadioCapability, MAX_RADIOS>,
    pub carriers: Vec<CarrierCapability, MAX_CARRIERS>,
    pub recovery_paths: Vec<RecoveryPath, MAX_RECOVERY_PATHS>,
}
impl Capabilities {
    pub fn empty(board_class: BoardClass) -> Self {
        Self {
            board_class,
            controller_role: None,
            image_slots: Vec::new(),
            adapters: Vec::new(),
            radios: Vec::new(),
            carriers: Vec::new(),
            recovery_paths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion(u8),
    WrongFrameKind(u8),
    UnknownControllerRole(u8),
    UnknownOperation(u8),
    UnknownRefusal(u8),
    UnknownBoardClass(u8),
    UnknownImageKind(u8),
    UnknownAdapter(u8),
    UnknownRadio(u8),
    UnknownCarrier(u8),
    OversizedField { declared: usize, maximum: usize },
    InvalidBoolean(u8),
    TrailingBytes,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    BufferTooSmall,
}
