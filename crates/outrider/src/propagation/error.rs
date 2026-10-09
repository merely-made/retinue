//! The propagation error vocabulary.

use retinue::hash::AddressHash;

use crate::codec::CodecError;

#[derive(Debug, thiserror::Error)]
pub enum PropagationError {
    #[error("Retinue propagation transfer failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("propagation offer request failed: {0}")]
    OfferRequest(#[source] std::io::Error),
    #[error("propagation selection request failed: {0}")]
    SelectionRequest(#[source] std::io::Error),
    #[error("Retinue identity-token operation failed: {0}")]
    Crypto(#[from] retinue::Error),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("propagation MessagePack could not be encoded")]
    Encode,
    #[error("propagation data is not one complete MessagePack value")]
    MalformedMessagePack,
    #[error("propagation announce exceeds the byte limit")]
    AnnounceTooLarge,
    #[error("propagation announce has the wrong shape")]
    InvalidAnnounce,
    #[error("propagation batch exceeds the byte limit")]
    BatchTooLarge,
    #[error("propagation batch has the wrong shape")]
    InvalidBatch,
    #[error("propagation batch has an invalid transfer timestamp")]
    InvalidTransferTime,
    #[error("propagation batch has too many entries")]
    TooManyEntries,
    #[error("propagation entry exceeds the byte limit")]
    EntryTooLarge,
    #[error("propagation entry is truncated")]
    TruncatedEntry,
    #[error("propagation entry stamp does not meet the node cost")]
    InvalidStamp,
    #[error("the configured proof-of-work attempt budget was exhausted")]
    StampBudgetExhausted,
    #[error("the propagation destination does not match the recipient or node identity")]
    WrongDestination,
    #[error("the decrypted LXMF source does not match the supplied source identity")]
    WrongSource,
    #[error("the decrypted LXMF signature is invalid")]
    BadSignature,
    #[error("the announced propagation node is inactive")]
    InactiveNode,
    #[error("the local recipient identity is not the endpoint identity")]
    LocalIdentityMismatch,
    #[error("propagation fetch response has the wrong shape")]
    InvalidFetchResponse,
    #[error("propagation node returned an entry it did not offer")]
    UnexpectedTransientId,
    #[error("the decrypted message source {0} has no validated delivery announce")]
    UnknownSource(AddressHash),
    #[error("propagation fetch request has the wrong shape")]
    InvalidFetchRequest,
    #[error("propagation fetch link did not identify its owner")]
    UnidentifiedFetch,
    #[error("propagation fetch identity changed during the session")]
    FetchIdentityChanged,
    #[error("propagation-store snapshot exceeds the byte limit")]
    StoreSnapshotTooLarge,
    #[error("propagation-store snapshot has the wrong shape")]
    InvalidStoreSnapshot,
    #[error("unsupported propagation-store snapshot version {0}")]
    UnsupportedStoreSnapshotVersion(u64),
    #[error("the submitting identity is throttled for an invalid stamp")]
    Throttled,
    #[error("a client may submit only one propagation entry per transfer")]
    UnpeeredBatch,
}
