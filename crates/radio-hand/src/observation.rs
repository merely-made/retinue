//! Bounded, metadata-only radio observations.
//!
//! The wire format is deliberately independent of the recorder and carriers.  An
//! event has a per-boot sequence; a gap is a separate record class and therefore
//! never consumes an event sequence number.
//!
//! Every record is `magic:u8, version:u8, class:u8, length:u16` in big-endian,
//! followed by the class body and a big-endian CRC-32/IEEE over the header and
//! body. `length` is the complete record length, so trailing bytes are rejected.
//! The checksum detects corruption; it does not authenticate a board or carrier.
//! Unknown event kinds use all remaining body bytes as opaque payload.

pub mod collection;
pub mod owner;
pub mod recorder;

mod codec;

pub const MAX_RECORD_BYTES: usize = 64;
pub const UNKNOWN_DATA_MAX: usize = 30;
const MAGIC: u8 = 0x4f;
const VERSION: u8 = 1;
const EVENT_CLASS: u8 = 0;
const GAP_CLASS: u8 = 1;
const HEADER_BYTES: usize = 5;
const CHECKSUM_BYTES: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationEvent {
    pub boot_id: u64,
    pub sequence: u64,
    pub uptime_ms: u64,
    pub kind: ObservationKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationGap {
    pub boot_id: u64,
    pub first_missing: u64,
    pub count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationRecord {
    Event(ObservationEvent),
    Gap(ObservationGap),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationKind {
    ListeningStarted {
        assignment: u16,
        profile: u8,
    },
    ListeningStopped {
        assignment: u16,
        reason: StopReason,
    },
    RxCaptured {
        profile: u8,
        length: u16,
        rssi_dbm: i16,
        snr_tenths_db: i16,
        capture_tag: u32,
    },
    RxDamaged {
        profile: u8,
    },
    TxStarted {
        profile: u8,
        length: u16,
        work: u32,
    },
    TxFinished {
        work: u32,
        outcome: TxOutcome,
    },
    WorkRefused {
        request: RequestKind,
        reason: RefusalReason,
        work: u32,
    },
    QuietStarted {
        cause: QuietCause,
    },
    QuietStopped {
        cause: QuietCause,
    },
    SleepStarted,
    SleepStopped {
        cause: WakeCause,
    },
    /// The owner can no longer certify the previous radio state. This is not
    /// a hardware stop timestamp; causes are owner-defined and preserved raw.
    ContinuityLost {
        cause: u8,
    },
    Unknown {
        kind: u8,
        data: [u8; UNKNOWN_DATA_MAX],
        len: u8,
    },
}

macro_rules! raw_enum {
    ($name:ident { $($variant:ident = $value:expr),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum $name { $($variant,)+ Unknown(u8) }
        impl $name {
            fn byte(self) -> u8 { match self { $(Self::$variant => $value,)+ Self::Unknown(v) => v } }
            fn from_byte(v: u8) -> Self { match v { $($value => Self::$variant,)+ _ => Self::Unknown(v) } }
        }
    };
}

raw_enum!(StopReason { Completed = 0, Reconfigured = 1, Fault = 2, Power = 3 });
raw_enum!(TxOutcome { Sent = 0, Failed = 1, Cancelled = 2 });
raw_enum!(RequestKind { Transmit = 0, Retune = 1, Listen = 2, Sleep = 3 });
raw_enum!(RefusalReason { MissingRegion = 0, DutyBudget = 1, ChannelBusy = 2, InvalidProfile = 3, ConflictingLease = 4, RadioFault = 5, QuietWindow = 6, PowerPolicy = 7 });
raw_enum!(QuietCause { Configuration = 0, Settings = 1, Announce = 2, Recovery = 3, ProfileTransition = 4, Power = 5 });
raw_enum!(WakeCause { Timer = 0, Host = 1, Radio = 2, Power = 3 });

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError {
    BufferTooSmall,
    InvalidIdentity,
    InvalidRange,
    UnknownPayloadTooLong,
    LengthOverflow,
    InvalidKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    BadVersion,
    BadClass,
    BadLength,
    BadChecksum,
    InvalidIdentity,
    InvalidSequence,
    InvalidRange,
    InvalidKind,
    InvalidPayload,
}

#[cfg(test)]
mod tests;
