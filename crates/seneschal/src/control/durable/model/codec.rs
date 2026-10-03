//! Durable state encoding over the existing CRC-checked A/B records.

use heapless::Vec;

use crate::store;

use super::super::super::{
    ConfigGeneration, ControllerId, ControllerRole, NodeId, Operation, Refusal, TransactionId,
};
use super::*;

/// Decodes one durable state body. `store` validates the surrounding CRC first.
pub fn decode_durable(bytes: &[u8]) -> Result<DurableState, DurableError> {
    let mut reader = Reader::new(bytes);
    if reader.array::<4>()? != MAGIC {
        return Err(DurableError::Malformed);
    }
    let version = reader.byte()?;
    if version != VERSION {
        return Err(DurableError::UnsupportedVersion(version));
    }
    let node = NodeId(reader.array()?);
    let generation_watermark = ConfigGeneration(reader.u64()?);
    let grants = read_grants(&mut reader)?;
    let recovery_policy = read_recovery_policy(&mut reader)?;
    let known_good = KnownGood {
        generation: ConfigGeneration(reader.u64()?),
        configuration: read_config(&mut reader)?,
    };
    if generation_watermark < known_good.generation {
        return Err(DurableError::Malformed);
    }
    let provisional = if reader.boolean()? {
        Some(read_provisional(&mut reader)?)
    } else {
        None
    };
    let receipt = if reader.boolean()? {
        Some(read_receipt(&mut reader)?)
    } else {
        None
    };
    reader.finish()?;
    let state = DurableState {
        node,
        owner_grants: grants,
        recovery_policy,
        known_good,
        generation_watermark,
        provisional,
        receipt,
    };
    state.validate_semantics()?;
    Ok(state)
}

/// Encodes one durable state body without allocating.
pub fn encode_durable(state: &DurableState, out: &mut [u8]) -> Result<usize, DurableError> {
    let mut writer = Writer::new(out);
    writer.bytes(&MAGIC)?;
    writer.byte(VERSION)?;
    writer.bytes(&state.node.0)?;
    writer.u64(state.generation_watermark.0)?;
    writer.count(state.owner_grants.len())?;
    for grant in &state.owner_grants {
        writer.bytes(&grant.controller.0)?;
        writer.bytes(&grant.retinue_public_identity)?;
        writer.byte(grant.role as u8)?;
        writer.u64(grant.accepted_outer_counter)?;
        writer.u64(grant.accepted_mutation_sequence)?;
    }
    write_recovery_policy(&mut writer, state.recovery_policy)?;
    writer.u64(state.known_good.generation.0)?;
    write_config(&mut writer, &state.known_good.configuration)?;
    writer.boolean(state.provisional.is_some())?;
    if let Some(provisional) = &state.provisional {
        write_provisional(&mut writer, provisional)?;
    }
    writer.boolean(state.receipt.is_some())?;
    if let Some(receipt) = &state.receipt {
        write_receipt(&mut writer, receipt)?;
    }
    Ok(writer.finish())
}

/// Reads two existing A/B pages. A valid outer record containing a malformed
/// state is corrupt, rather than an excuse to fall back through a newer write.
pub fn load(a: &[u8], b: &[u8]) -> Result<DurableState, DurableLoadError> {
    let selection = store::select(a, b);
    if let Some((_, record)) = selection.active {
        return decode_durable(record.body).map_err(DurableLoadError::State);
    }
    match (store::decode(a), store::decode(b)) {
        (Err(store::SlotError::Blank), Err(store::SlotError::Blank)) => {
            Err(DurableLoadError::Blank)
        }
        _ => Err(DurableLoadError::Corrupt),
    }
}

/// Prepares the next CRC-protected A/B record. The caller erases and programs
/// the returned target slot, then performs board-specific readback verification.
pub fn next_record(
    a: &[u8],
    b: &[u8],
    state: &DurableState,
    body_scratch: &mut [u8; MAX_DURABLE_BODY],
    page_out: &mut [u8],
) -> Result<JournalWrite, DurableError> {
    let selection = store::select(a, b);
    if selection.active.is_none()
        && !matches!(
            (store::decode(a), store::decode(b)),
            (Err(store::SlotError::Blank), Err(store::SlotError::Blank))
        )
    {
        return Err(DurableError::NoValidSlot);
    }
    let body_len = encode_durable(state, body_scratch)?;
    let len = store::encode(selection.next_sequence, &body_scratch[..body_len], page_out)
        .map_err(|_| DurableError::BufferTooSmall)?;
    Ok(JournalWrite {
        slot: selection.next,
        sequence: selection.next_sequence,
        len,
    })
}

fn write_config(writer: &mut Writer<'_>, config: &DurableConfig) -> Result<(), DurableError> {
    let public: Vec<u8, MAX_PUBLIC_CONFIG> =
        Vec::from_slice(&config.public.encode()).map_err(|_| DurableError::Capacity)?;
    writer.vec(&public)?;
    writer.vec(&config.sealed_credentials)?;
    Ok(())
}

fn read_config(reader: &mut Reader<'_>) -> Result<DurableConfig, DurableError> {
    let public: Vec<u8, MAX_PUBLIC_CONFIG> = reader.vec()?;
    let public = PublicConfigurationV1::decode(&public).map_err(|_| DurableError::Malformed)?;
    let sealed_credentials = reader.vec()?;
    Ok(DurableConfig {
        public,
        sealed_credentials,
    })
}

fn write_recovery_policy(
    writer: &mut Writer<'_>,
    policy: RecoveryPolicy,
) -> Result<(), DurableError> {
    let (physical, remote) = policy.encode_parts();
    writer.byte(physical.0)?;
    writer.byte(physical.1)?;
    writer.byte(remote.0)?;
    writer.byte(remote.1)
}

fn read_recovery_policy(reader: &mut Reader<'_>) -> Result<RecoveryPolicy, DurableError> {
    RecoveryPolicy::decode_parts(
        (reader.byte()?, reader.byte()?),
        (reader.byte()?, reader.byte()?),
    )
}

fn write_semantic(writer: &mut Writer<'_>, semantic: &SemanticKey) -> Result<(), DurableError> {
    writer.bytes(&semantic.transaction.0)?;
    writer.u64(semantic.transaction_sequence)?;
    writer.u64(semantic.expected_generation.0)?;
    writer.byte(semantic.operation as u8)?;
    writer.bytes(semantic.tag.as_bytes())
}

fn read_semantic(reader: &mut Reader<'_>) -> Result<SemanticKey, DurableError> {
    let transaction = TransactionId(reader.array()?);
    let transaction_sequence = reader.u64()?;
    let expected_generation = ConfigGeneration(reader.u64()?);
    let operation = decode_operation(reader.byte()?)?;
    let tag = SemanticTag::from_persisted(reader.array()?);
    Ok(SemanticKey {
        transaction,
        transaction_sequence,
        expected_generation,
        operation,
        tag,
    })
}

fn write_provisional(writer: &mut Writer<'_>, value: &Provisional) -> Result<(), DurableError> {
    writer.bytes(&value.controller.0)?;
    writer.bytes(&value.change.0)?;
    write_semantic(writer, &value.semantic)?;
    writer.u64(value.candidate_generation.0)?;
    write_config(writer, &value.candidate)?;
    writer.u64(value.deadline_ms)?;
    writer.bytes(&value.commit_token)?;
    writer.vec(&value.result)
}

fn read_provisional(reader: &mut Reader<'_>) -> Result<Provisional, DurableError> {
    Ok(Provisional {
        controller: ControllerId(reader.array()?),
        change: ChangeId(reader.array()?),
        semantic: read_semantic(reader)?,
        candidate_generation: ConfigGeneration(reader.u64()?),
        candidate: read_config(reader)?,
        deadline_ms: reader.u64()?,
        commit_token: reader.array()?,
        result: reader.vec()?,
    })
}

fn write_receipt(writer: &mut Writer<'_>, value: &CachedReceipt) -> Result<(), DurableError> {
    writer.bytes(&value.controller.0)?;
    write_semantic(writer, &value.semantic)?;
    match &value.body {
        ReceiptBody::Applied {
            known_good_generation,
            result,
        } => {
            writer.byte(0)?;
            writer.u64(known_good_generation.0)?;
            writer.vec(result)?;
        }
        ReceiptBody::Refused(reason) => {
            writer.byte(1)?;
            writer.byte(*reason as u8)?;
        }
    }
    Ok(())
}

fn read_receipt(reader: &mut Reader<'_>) -> Result<CachedReceipt, DurableError> {
    let controller = ControllerId(reader.array()?);
    let semantic = read_semantic(reader)?;
    let body = match reader.byte()? {
        0 => ReceiptBody::Applied {
            known_good_generation: ConfigGeneration(reader.u64()?),
            result: reader.vec()?,
        },
        1 => ReceiptBody::Refused(decode_refusal(reader.byte()?)?),
        _ => return Err(DurableError::Malformed),
    };
    Ok(CachedReceipt {
        controller,
        semantic,
        body,
    })
}

fn read_grants(reader: &mut Reader<'_>) -> Result<Vec<OwnerGrant, MAX_OWNER_GRANTS>, DurableError> {
    let count = reader.count(MAX_OWNER_GRANTS)?;
    let mut grants = Vec::new();
    for _ in 0..count {
        let controller = ControllerId(reader.array()?);
        let retinue_public_identity = reader.array()?;
        let role = match reader.byte()? {
            0 => ControllerRole::Observer,
            1 => ControllerRole::Operator,
            2 => ControllerRole::Updater,
            3 => ControllerRole::Owner,
            _ => return Err(DurableError::Malformed),
        };
        grants
            .push(OwnerGrant::from_durable_parts(
                controller,
                retinue_public_identity,
                role,
                reader.u64()?,
                reader.u64()?,
            ))
            .map_err(|_| DurableError::Capacity)?;
    }
    Ok(grants)
}

fn decode_operation(value: u8) -> Result<Operation, DurableError> {
    match value {
        0 => Ok(Operation::Capabilities),
        1 => Ok(Operation::Status),
        2 => Ok(Operation::WifiScan),
        3 => Ok(Operation::OwnerClaim),
        4 => Ok(Operation::StageConfiguration),
        5 => Ok(Operation::ProvisionalApply),
        6 => Ok(Operation::Commit),
        7 => Ok(Operation::Revert),
        8 => Ok(Operation::Reboot),
        9 => Ok(Operation::RecoveryStatus),
        10 => Ok(Operation::FirmwareStage),
        11 => Ok(Operation::FirmwareActivate),
        12 => Ok(Operation::AdapterPolicy),
        _ => Err(DurableError::Malformed),
    }
}

fn decode_refusal(value: u8) -> Result<Refusal, DurableError> {
    match value {
        0 => Ok(Refusal::Unauthorized),
        1 => Ok(Refusal::WrongNode),
        2 => Ok(Refusal::StaleGeneration),
        3 => Ok(Refusal::TransactionConflict),
        4 => Ok(Refusal::TransactionExpired),
        5 => Ok(Refusal::InvalidCommit),
        6 => Ok(Refusal::UnsafeRecoveryPath),
        7 => Ok(Refusal::UnsupportedOperation),
        8 => Ok(Refusal::InvalidArguments),
        9 => Ok(Refusal::Capacity),
        10 => Ok(Refusal::PhysicalPresenceRequired),
        11 => Ok(Refusal::Busy),
        12 => Ok(Refusal::Internal),
        13 => Ok(Refusal::GenerationExhausted),
        14 => Ok(Refusal::TransactionTooFar),
        _ => Err(DurableError::Malformed),
    }
}

struct Writer<'a> {
    out: &'a mut [u8],
    cursor: usize,
}
impl<'a> Writer<'a> {
    fn new(out: &'a mut [u8]) -> Self {
        Self { out, cursor: 0 }
    }
    fn finish(self) -> usize {
        self.cursor
    }
    fn bytes(&mut self, value: &[u8]) -> Result<(), DurableError> {
        let end = self
            .cursor
            .checked_add(value.len())
            .ok_or(DurableError::BufferTooSmall)?;
        let target = self
            .out
            .get_mut(self.cursor..end)
            .ok_or(DurableError::BufferTooSmall)?;
        target.copy_from_slice(value);
        self.cursor = end;
        Ok(())
    }
    fn byte(&mut self, value: u8) -> Result<(), DurableError> {
        self.bytes(&[value])
    }
    fn boolean(&mut self, value: bool) -> Result<(), DurableError> {
        self.byte(u8::from(value))
    }
    fn count(&mut self, value: usize) -> Result<(), DurableError> {
        self.byte(u8::try_from(value).map_err(|_| DurableError::Capacity)?)
    }
    fn u64(&mut self, value: u64) -> Result<(), DurableError> {
        self.bytes(&value.to_le_bytes())
    }
    fn vec<const N: usize>(&mut self, value: &Vec<u8, N>) -> Result<(), DurableError> {
        self.count(value.len())?;
        self.bytes(value)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }
    fn finish(&self) -> Result<(), DurableError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(DurableError::Malformed)
        }
    }
    fn take(&mut self, len: usize) -> Result<&'a [u8], DurableError> {
        let end = self
            .cursor
            .checked_add(len)
            .ok_or(DurableError::Malformed)?;
        let result = self
            .bytes
            .get(self.cursor..end)
            .ok_or(DurableError::Malformed)?;
        self.cursor = end;
        Ok(result)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], DurableError> {
        let mut value = [0; N];
        value.copy_from_slice(self.take(N)?);
        Ok(value)
    }
    fn byte(&mut self) -> Result<u8, DurableError> {
        Ok(self.take(1)?[0])
    }
    fn boolean(&mut self) -> Result<bool, DurableError> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DurableError::Malformed),
        }
    }
    fn count(&mut self, maximum: usize) -> Result<usize, DurableError> {
        let value = usize::from(self.byte()?);
        if value <= maximum {
            Ok(value)
        } else {
            Err(DurableError::Capacity)
        }
    }
    fn u64(&mut self) -> Result<u64, DurableError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    fn vec<const N: usize>(&mut self) -> Result<Vec<u8, N>, DurableError> {
        let len = self.count(N)?;
        let mut value = Vec::new();
        value
            .extend_from_slice(self.take(len)?)
            .map_err(|_| DurableError::Capacity)?;
        Ok(value)
    }
}
