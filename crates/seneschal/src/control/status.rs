//! Bounded, public evidence about a recovered control journal.
//!
//! The payload carries its own authority statement. `DiagnosticOnly` is the
//! unauthenticated normal-runtime diagnostic: anyone on the host stream may ask
//! for it, and it carries no authority. `VerifiedController` is the same fixed
//! payload produced as the `Observed` body of a WN0 `Operation::Status` answer to
//! a verified outer command; its nonce field then echoes that request's
//! transaction id. Either way it omits configuration bodies, owner grants,
//! cached receipts, sealed credentials, and every private key.

use super::{
    ConfigGeneration, DurableState, FirstWriteStatus, NodeId, PairEvidence, TransactionId,
};

/// Version of [`ControlStatusV1`]'s fixed public payload.
pub const CONTROL_STATUS_VERSION: u8 = 1;
/// Exact bytes in a version-one status payload.
pub const CONTROL_STATUS_NONCE_LEN: usize = 16;
pub const CONTROL_STATUS_V1_LEN: usize = 53;
/// Tag at the start of a KISS frame that carries one [`ControlStatusV1`].
///
/// KISS framing itself belongs to `selvage`; this tag keeps the diagnostic
/// distinct from normal direct-PHY events on the shared host byte stream.
pub const CONTROL_STATUS_FRAME_TAG: u8 = 0x43;
/// Exact unescaped bytes in one status KISS frame.
pub const CONTROL_STATUS_FRAME_LEN: usize = 1 + CONTROL_STATUS_V1_LEN;
/// Tag for the nonce-bearing, versioned diagnostic request KISS frame.
pub const CONTROL_STATUS_REQUEST_FRAME_TAG: u8 = 0x53;
/// Exact unescaped bytes in one V1 diagnostic request frame.
pub const CONTROL_STATUS_REQUEST_FRAME_LEN: usize = 2 + CONTROL_STATUS_NONCE_LEN;

/// Authority statement intentionally carried in every status response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlStatusAuthority {
    /// Public diagnostic evidence only. It is neither a signed response nor a
    /// proof that the host that read it is an authorized controller.
    DiagnosticOnly,
    /// Produced by the board for exactly one verified outer command from a
    /// controller holding a durable grant. The board persisted that command's
    /// outer counter before producing it, and `query_nonce` echoes the request
    /// transaction id. The payload itself is still not signed by the board.
    VerifiedController,
}

/// Public A/B-pair evidence. This is deliberately narrower than an Inspect
/// response: it says nothing about first-owner actions or claim material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlStatusEvidence {
    Blank,
    Valid,
    Corrupt,
}

impl From<PairEvidence> for ControlStatusEvidence {
    fn from(value: PairEvidence) -> Self {
        match value {
            PairEvidence::Blank => Self::Blank,
            PairEvidence::Valid => Self::Valid,
            PairEvidence::Corrupt => Self::Corrupt,
        }
    }
}

/// Which successful normal boot applied the current runtime configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlStatusBootFact {
    /// The durable journal had no provisional candidate, so normal boot
    /// applied its known-good configuration directly.
    KnownGoodApplied,
    /// Normal boot discarded a durable provisional candidate, restored the
    /// known-good configuration, and persisted that recovery before service.
    RecoveredRollback,
}

/// Fixed public evidence captured after successful ordinary control recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlStatusV1 {
    node: NodeId,
    authority: ControlStatusAuthority,
    control: ControlStatusEvidence,
    pending: ControlStatusEvidence,
    boot: ControlStatusBootFact,
    known_good_generation: ConfigGeneration,
    generation_watermark: ConfigGeneration,
    query_nonce: [u8; CONTROL_STATUS_NONCE_LEN],
}

/// The versioned, bounded request for one public diagnostic snapshot and a
/// host-generated freshness nonce.
///
/// It deliberately carries no controller, signature, or operation body. The
/// normal runtime treats it as a read-only diagnostic request and echoes only
/// its nonce in the public response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlStatusRequestV1([u8; CONTROL_STATUS_NONCE_LEN]);

impl ControlStatusRequestV1 {
    pub const fn new(nonce: [u8; CONTROL_STATUS_NONCE_LEN]) -> Self {
        Self(nonce)
    }
    pub const fn nonce(self) -> [u8; CONTROL_STATUS_NONCE_LEN] {
        self.0
    }
    /// Encodes the unescaped request frame before its carrier KISS-escapes it.
    pub fn encode_frame(self, out: &mut [u8]) -> Result<(), ControlStatusError> {
        if out.len() != CONTROL_STATUS_REQUEST_FRAME_LEN {
            return Err(ControlStatusError::Length { found: out.len() });
        }
        out[0] = CONTROL_STATUS_REQUEST_FRAME_TAG;
        out[1] = CONTROL_STATUS_VERSION;
        out[2..].copy_from_slice(&self.0);
        Ok(())
    }

    /// Decodes the exact unescaped request frame.
    pub fn decode_frame(frame: &[u8]) -> Result<Self, ControlStatusError> {
        if frame.len() != CONTROL_STATUS_REQUEST_FRAME_LEN {
            return Err(ControlStatusError::Length { found: frame.len() });
        }
        if frame[0] != CONTROL_STATUS_REQUEST_FRAME_TAG {
            return Err(ControlStatusError::UnexpectedRequestFrameTag(frame[0]));
        }
        if frame[1] != CONTROL_STATUS_VERSION {
            return Err(ControlStatusError::UnsupportedVersion(frame[1]));
        }
        let mut nonce = [0; CONTROL_STATUS_NONCE_LEN];
        nonce.copy_from_slice(&frame[2..]);
        Ok(Self(nonce))
    }
}

impl ControlStatusV1 {
    /// Captures only public metadata from the already-inspected A/B pairs and
    /// a successfully recovered runtime state. It performs no storage I/O.
    pub const fn from_recovered_state(
        first_write: FirstWriteStatus,
        state: &DurableState,
        recovered_rollback: bool,
    ) -> Self {
        Self {
            node: state.node(),
            authority: ControlStatusAuthority::DiagnosticOnly,
            control: match first_write.control {
                PairEvidence::Blank => ControlStatusEvidence::Blank,
                PairEvidence::Valid => ControlStatusEvidence::Valid,
                PairEvidence::Corrupt => ControlStatusEvidence::Corrupt,
            },
            pending: match first_write.pending {
                PairEvidence::Blank => ControlStatusEvidence::Blank,
                PairEvidence::Valid => ControlStatusEvidence::Valid,
                PairEvidence::Corrupt => ControlStatusEvidence::Corrupt,
            },
            boot: if recovered_rollback {
                ControlStatusBootFact::RecoveredRollback
            } else {
                ControlStatusBootFact::KnownGoodApplied
            },
            known_good_generation: state.known_good().generation,
            generation_watermark: state.generation_watermark(),
            query_nonce: [0; CONTROL_STATUS_NONCE_LEN],
        }
    }

    /// Captures the same public metadata as the `Observed` body of a verified
    /// `Operation::Status` response, bound to that request's transaction.
    pub const fn for_verified_controller(
        first_write: FirstWriteStatus,
        state: &DurableState,
        recovered_rollback: bool,
        transaction: TransactionId,
    ) -> Self {
        let mut status = Self::from_recovered_state(first_write, state, recovered_rollback);
        status.authority = ControlStatusAuthority::VerifiedController;
        status.query_nonce = transaction.0;
        status
    }

    pub const fn node(self) -> NodeId {
        self.node
    }

    pub const fn authority(self) -> ControlStatusAuthority {
        self.authority
    }

    pub const fn control(self) -> ControlStatusEvidence {
        self.control
    }

    pub const fn pending(self) -> ControlStatusEvidence {
        self.pending
    }

    pub const fn boot(self) -> ControlStatusBootFact {
        self.boot
    }

    pub const fn known_good_generation(self) -> ConfigGeneration {
        self.known_good_generation
    }

    pub const fn generation_watermark(self) -> ConfigGeneration {
        self.generation_watermark
    }
    pub const fn query_nonce(self) -> [u8; CONTROL_STATUS_NONCE_LEN] {
        self.query_nonce
    }
    pub const fn with_query_nonce(mut self, query_nonce: [u8; CONTROL_STATUS_NONCE_LEN]) -> Self {
        self.query_nonce = query_nonce;
        self
    }

    /// Encodes the exact fixed public payload.
    pub fn encode(self, out: &mut [u8]) -> Result<(), ControlStatusError> {
        if out.len() != CONTROL_STATUS_V1_LEN {
            return Err(ControlStatusError::Length { found: out.len() });
        }
        out[0] = CONTROL_STATUS_VERSION;
        out[1] = authority_tag(self.authority);
        out[2] = boot_tag(self.boot);
        out[3] = evidence_tag(self.control);
        out[4] = evidence_tag(self.pending);
        out[5..21].copy_from_slice(&self.node.0);
        out[21..29].copy_from_slice(&self.known_good_generation.0.to_le_bytes());
        out[29..37].copy_from_slice(&self.generation_watermark.0.to_le_bytes());
        out[37..53].copy_from_slice(&self.query_nonce);
        Ok(())
    }

    /// Decodes one exact payload, rejecting unknown version, tags, malformed
    /// generation ordering, and every length other than the fixed V1 length.
    pub fn decode(bytes: &[u8]) -> Result<Self, ControlStatusError> {
        if bytes.len() != CONTROL_STATUS_V1_LEN {
            return Err(ControlStatusError::Length { found: bytes.len() });
        }
        if bytes[0] != CONTROL_STATUS_VERSION {
            return Err(ControlStatusError::UnsupportedVersion(bytes[0]));
        }
        let authority = parse_authority(bytes[1])?;
        let boot = parse_boot(bytes[2])?;
        let control = parse_evidence(bytes[3])?;
        let pending = parse_evidence(bytes[4])?;
        let mut node = [0_u8; 16];
        node.copy_from_slice(&bytes[5..21]);
        let mut known_good = [0_u8; 8];
        known_good.copy_from_slice(&bytes[21..29]);
        let mut watermark = [0_u8; 8];
        watermark.copy_from_slice(&bytes[29..37]);
        let known_good_generation = ConfigGeneration(u64::from_le_bytes(known_good));
        let generation_watermark = ConfigGeneration(u64::from_le_bytes(watermark));
        if known_good_generation > generation_watermark {
            return Err(ControlStatusError::GenerationOrder);
        }
        let mut query_nonce = [0; CONTROL_STATUS_NONCE_LEN];
        query_nonce.copy_from_slice(&bytes[37..53]);
        Ok(Self {
            node: NodeId(node),
            authority,
            control,
            pending,
            boot,
            known_good_generation,
            generation_watermark,
            query_nonce,
        })
    }

    /// Encodes the bounded, tagged diagnostic frame body before the carrier
    /// KISS-escapes it.
    pub fn encode_frame(self, out: &mut [u8]) -> Result<(), ControlStatusError> {
        if out.len() != CONTROL_STATUS_FRAME_LEN {
            return Err(ControlStatusError::Length { found: out.len() });
        }
        out[0] = CONTROL_STATUS_FRAME_TAG;
        self.encode(&mut out[1..])
    }

    /// Decodes one unescaped, tagged diagnostic frame body.
    pub fn decode_frame(frame: &[u8]) -> Result<Self, ControlStatusError> {
        if frame.len() != CONTROL_STATUS_FRAME_LEN {
            return Err(ControlStatusError::Length { found: frame.len() });
        }
        if frame[0] != CONTROL_STATUS_FRAME_TAG {
            return Err(ControlStatusError::UnexpectedFrameTag(frame[0]));
        }
        Self::decode(&frame[1..])
    }
}

/// Fail-closed errors from the public status codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlStatusError {
    Length { found: usize },
    UnsupportedVersion(u8),
    UnsupportedAuthority(u8),
    UnsupportedBootFact(u8),
    UnsupportedEvidence(u8),
    UnexpectedFrameTag(u8),
    UnexpectedRequestFrameTag(u8),
    GenerationOrder,
}

const fn authority_tag(value: ControlStatusAuthority) -> u8 {
    match value {
        ControlStatusAuthority::DiagnosticOnly => 0,
        ControlStatusAuthority::VerifiedController => 1,
    }
}

const fn boot_tag(value: ControlStatusBootFact) -> u8 {
    match value {
        ControlStatusBootFact::KnownGoodApplied => 0,
        ControlStatusBootFact::RecoveredRollback => 1,
    }
}

const fn evidence_tag(value: ControlStatusEvidence) -> u8 {
    match value {
        ControlStatusEvidence::Blank => 0,
        ControlStatusEvidence::Valid => 1,
        ControlStatusEvidence::Corrupt => 2,
    }
}

const fn parse_authority(value: u8) -> Result<ControlStatusAuthority, ControlStatusError> {
    match value {
        0 => Ok(ControlStatusAuthority::DiagnosticOnly),
        1 => Ok(ControlStatusAuthority::VerifiedController),
        other => Err(ControlStatusError::UnsupportedAuthority(other)),
    }
}

const fn parse_boot(value: u8) -> Result<ControlStatusBootFact, ControlStatusError> {
    match value {
        0 => Ok(ControlStatusBootFact::KnownGoodApplied),
        1 => Ok(ControlStatusBootFact::RecoveredRollback),
        other => Err(ControlStatusError::UnsupportedBootFact(other)),
    }
}

const fn parse_evidence(value: u8) -> Result<ControlStatusEvidence, ControlStatusError> {
    match value {
        0 => Ok(ControlStatusEvidence::Blank),
        1 => Ok(ControlStatusEvidence::Valid),
        2 => Ok(ControlStatusEvidence::Corrupt),
        other => Err(ControlStatusError::UnsupportedEvidence(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{
        BoardRecoveryFacts, ControllerRole, DurableConfig, ManagementCarrier, ManagementCarrierSet,
        OwnerGrant, PublicConfigurationV1, RecoveryClause, RecoveryPathFacts, RecoveryPolicy,
        ReticulumTransportPolicy,
    };
    use crate::region::Region;
    use heapless::Vec;
    use retinue::identity::PrivateIdentity;

    fn state() -> DurableState {
        let public = PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(906_875_000),
            ReticulumTransportPolicy::new(false, false, 0).unwrap(),
            ManagementCarrierSet::from_mask(1).unwrap(),
        )
        .unwrap();
        let config = DurableConfig {
            public,
            sealed_credentials: Vec::new(),
        };
        let policy = RecoveryPolicy::new(
            RecoveryClause::new(ManagementCarrierSet::from_mask(1).unwrap(), 1).unwrap(),
            RecoveryClause::disabled(),
        )
        .unwrap();
        let facts = BoardRecoveryFacts::new(
            Vec::from_slice(&[
                RecoveryPathFacts::new(ManagementCarrier::Usb, true, false, false).unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        let owner = PrivateIdentity::from_secret_bytes(&[0x33; 64])
            .public()
            .to_public_bytes();
        DurableState::new(
            NodeId([0x5a; 16]),
            Vec::from_slice(&[OwnerGrant::from_public_identity(
                owner,
                ControllerRole::Owner,
            )])
            .unwrap(),
            ConfigGeneration(7),
            config,
            policy,
            &facts,
        )
        .unwrap()
    }

    fn status() -> ControlStatusV1 {
        ControlStatusV1::from_recovered_state(
            FirstWriteStatus {
                control: PairEvidence::Valid,
                pending: PairEvidence::Blank,
            },
            &state(),
            false,
        )
    }

    #[test]
    fn exact_payload_and_frame_round_trip() {
        let status = status();
        let mut payload = [0_u8; CONTROL_STATUS_V1_LEN];
        status.encode(&mut payload).unwrap();
        assert_eq!(ControlStatusV1::decode(&payload), Ok(status));

        let mut frame = [0_u8; CONTROL_STATUS_FRAME_LEN];
        status.encode_frame(&mut frame).unwrap();
        assert_eq!(ControlStatusV1::decode_frame(&frame), Ok(status));
        assert_eq!(status.authority(), ControlStatusAuthority::DiagnosticOnly);

        let mut request = [0_u8; CONTROL_STATUS_REQUEST_FRAME_LEN];
        ControlStatusRequestV1::new([0x44; CONTROL_STATUS_NONCE_LEN])
            .encode_frame(&mut request)
            .unwrap();
        assert_eq!(
            ControlStatusRequestV1::decode_frame(&request),
            Ok(ControlStatusRequestV1::new(
                [0x44; CONTROL_STATUS_NONCE_LEN]
            ))
        );
    }

    #[test]
    fn verified_controller_status_is_bound_to_its_transaction() {
        let verified = ControlStatusV1::for_verified_controller(
            FirstWriteStatus {
                control: PairEvidence::Valid,
                pending: PairEvidence::Blank,
            },
            &state(),
            true,
            TransactionId([0x77; 16]),
        );
        assert_eq!(
            verified.authority(),
            ControlStatusAuthority::VerifiedController
        );
        assert_eq!(verified.boot(), ControlStatusBootFact::RecoveredRollback);
        assert_eq!(verified.query_nonce(), [0x77; CONTROL_STATUS_NONCE_LEN]);
        assert_eq!(verified.known_good_generation(), ConfigGeneration(7));
        let mut payload = [0_u8; CONTROL_STATUS_V1_LEN];
        verified.encode(&mut payload).unwrap();
        assert_eq!(payload[1], 1);
        assert_eq!(ControlStatusV1::decode(&payload), Ok(verified));
        assert_ne!(status(), verified);
    }

    #[test]
    fn rejects_bad_length_version_tags_and_generation_order() {
        let status = status();
        let mut bytes = [0_u8; CONTROL_STATUS_V1_LEN];
        status.encode(&mut bytes).unwrap();
        assert!(matches!(
            ControlStatusV1::decode(&bytes[..CONTROL_STATUS_V1_LEN - 1]),
            Err(ControlStatusError::Length { .. })
        ));
        bytes[0] = CONTROL_STATUS_VERSION + 1;
        assert!(matches!(
            ControlStatusV1::decode(&bytes),
            Err(ControlStatusError::UnsupportedVersion(_))
        ));
        bytes[0] = CONTROL_STATUS_VERSION;
        bytes[1] = 9;
        assert!(matches!(
            ControlStatusV1::decode(&bytes),
            Err(ControlStatusError::UnsupportedAuthority(9))
        ));
        bytes[1] = 0;
        bytes[3] = 9;
        assert!(matches!(
            ControlStatusV1::decode(&bytes),
            Err(ControlStatusError::UnsupportedEvidence(9))
        ));
        bytes[3] = 1;
        bytes[21..29].copy_from_slice(&9_u64.to_le_bytes());
        bytes[29..37].copy_from_slice(&8_u64.to_le_bytes());
        assert_eq!(
            ControlStatusV1::decode(&bytes),
            Err(ControlStatusError::GenerationOrder)
        );
        assert!(matches!(
            ControlStatusRequestV1::decode_frame(&[CONTROL_STATUS_REQUEST_FRAME_TAG]),
            Err(ControlStatusError::Length { .. })
        ));
        assert!(matches!(
            ControlStatusRequestV1::decode_frame(&[0; CONTROL_STATUS_REQUEST_FRAME_LEN]),
            Err(ControlStatusError::UnexpectedRequestFrameTag(0))
        ));
    }
}
