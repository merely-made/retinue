use heapless::Vec;

use super::model::*;

pub fn encode_request(request: &Request, out: &mut [u8]) -> Result<usize, EncodeError> {
    let mut writer = Writer::new(out);
    writer.bytes(&MAGIC)?;
    writer.byte(VERSION)?;
    writer.bytes(&request.transaction.0)?;
    writer.u64(request.transaction_sequence)?;
    writer.u64(request.expected_generation.0)?;
    writer.byte(request.operation as u8)?;
    writer.bounded(&request.arguments)?;
    Ok(writer.finish())
}

pub fn decode_request(bytes: &[u8]) -> Result<Request, DecodeError> {
    let mut reader = Reader::new(bytes);
    reader.request_header()?;
    let request = Request {
        transaction: TransactionId(reader.array()?),
        transaction_sequence: reader.u64()?,
        expected_generation: ConfigGeneration(reader.u64()?),
        operation: Operation::decode(reader.byte()?)?,
        arguments: reader.bounded()?,
    };
    reader.finish()?;
    Ok(request)
}

pub fn encode_response(response: &Response, out: &mut [u8]) -> Result<usize, EncodeError> {
    let mut writer = Writer::new(out);
    writer.bytes(&MAGIC)?;
    writer.byte(VERSION)?;
    writer.byte(RESPONSE)?;
    writer.bytes(&response.node.0)?;
    writer.bytes(&response.transaction.0)?;
    writer.u64(response.known_good_generation.0)?;
    match response.effective_generation {
        Some(generation) => {
            writer.boolean(true)?;
            writer.u64(generation.0)?;
        }
        None => writer.boolean(false)?,
    }
    match &response.body {
        ResponseBody::Applied(result) => {
            writer.byte(0)?;
            writer.bounded(result)?;
        }
        ResponseBody::Provisional {
            deadline_ms,
            commit_token,
            result,
        } => {
            writer.byte(1)?;
            writer.u64(*deadline_ms)?;
            writer.bytes(commit_token)?;
            writer.bounded(result)?;
        }
        ResponseBody::Refused { reason, result } => {
            writer.byte(2)?;
            writer.byte(*reason as u8)?;
            writer.bounded(result)?;
        }
        ResponseBody::Capabilities(capabilities) => {
            writer.byte(3)?;
            encode_capabilities(capabilities, &mut writer)?;
        }
        ResponseBody::Observed(result) => {
            writer.byte(4)?;
            writer.bounded(result)?;
        }
    }
    Ok(writer.finish())
}

pub fn decode_response(bytes: &[u8]) -> Result<Response, DecodeError> {
    let mut reader = Reader::new(bytes);
    reader.response_header()?;
    let node = NodeId(reader.array()?);
    let transaction = TransactionId(reader.array()?);
    let known_good_generation = ConfigGeneration(reader.u64()?);
    let effective_generation = if reader.boolean()? {
        Some(ConfigGeneration(reader.u64()?))
    } else {
        None
    };
    let body = match reader.byte()? {
        0 => ResponseBody::Applied(reader.bounded()?),
        1 => ResponseBody::Provisional {
            deadline_ms: reader.u64()?,
            commit_token: reader.array()?,
            result: reader.bounded()?,
        },
        2 => ResponseBody::Refused {
            reason: Refusal::decode(reader.byte()?)?,
            result: reader.bounded()?,
        },
        3 => ResponseBody::Capabilities(decode_capabilities(&mut reader)?),
        4 => ResponseBody::Observed(reader.bounded()?),
        tag => return Err(DecodeError::WrongFrameKind(tag)),
    };
    reader.finish()?;
    Ok(Response {
        node,
        transaction,
        known_good_generation,
        effective_generation,
        body,
    })
}

fn encode_capabilities(
    capabilities: &Capabilities,
    writer: &mut Writer<'_>,
) -> Result<(), EncodeError> {
    writer.byte(capabilities.board_class as u8)?;
    match capabilities.controller_role {
        Some(role) => {
            writer.boolean(true)?;
            writer.byte(role as u8)?;
        }
        None => writer.boolean(false)?,
    }
    writer.count(capabilities.image_slots.len())?;
    for slot in &capabilities.image_slots {
        writer.byte(slot.slot)?;
        writer.byte(slot.kind as u8)?;
        writer.boolean(slot.verified)?;
        writer.boolean(slot.active)?;
        writer.boolean(slot.trial)?;
    }
    writer.count(capabilities.adapters.len())?;
    for adapter in &capabilities.adapters {
        writer.byte(adapter.adapter as u8)?;
        writer.boolean(adapter.enabled)?;
        writer.byte(adapter.radio_leases)?;
    }
    writer.count(capabilities.radios.len())?;
    for radio in &capabilities.radios {
        writer.byte(radio.radio as u8)?;
        writer.byte(radio.simultaneous_receive_profiles)?;
        writer.boolean(radio.tx)?;
    }
    writer.count(capabilities.carriers.len())?;
    for carrier in &capabilities.carriers {
        writer.byte(carrier.carrier as u8)?;
        writer.boolean(carrier.authenticated)?;
        writer.u16(carrier.max_frame)?;
    }
    writer.count(capabilities.recovery_paths.len())?;
    for path in &capabilities.recovery_paths {
        writer.byte(path.carrier as u8)?;
        writer.boolean(path.enabled)?;
        writer.boolean(path.remote)?;
        writer.boolean(path.physical_presence)?;
    }
    Ok(())
}

fn decode_capabilities(reader: &mut Reader<'_>) -> Result<Capabilities, DecodeError> {
    let mut out = Capabilities::empty(BoardClass::decode(reader.byte()?)?);
    out.controller_role = if reader.boolean()? {
        Some(ControllerRole::decode(reader.byte()?)?)
    } else {
        None
    };
    for _ in 0..reader.count(MAX_IMAGE_SLOTS)? {
        push(
            &mut out.image_slots,
            ImageSlot {
                slot: reader.byte()?,
                kind: ImageKind::decode(reader.byte()?)?,
                verified: reader.boolean()?,
                active: reader.boolean()?,
                trial: reader.boolean()?,
            },
            MAX_IMAGE_SLOTS,
        )?;
    }
    for _ in 0..reader.count(MAX_ADAPTERS)? {
        push(
            &mut out.adapters,
            AdapterCapability {
                adapter: ResidentAdapter::decode(reader.byte()?)?,
                enabled: reader.boolean()?,
                radio_leases: reader.byte()?,
            },
            MAX_ADAPTERS,
        )?;
    }
    for _ in 0..reader.count(MAX_RADIOS)? {
        push(
            &mut out.radios,
            RadioCapability {
                radio: RadioKind::decode(reader.byte()?)?,
                simultaneous_receive_profiles: reader.byte()?,
                tx: reader.boolean()?,
            },
            MAX_RADIOS,
        )?;
    }
    for _ in 0..reader.count(MAX_CARRIERS)? {
        push(
            &mut out.carriers,
            CarrierCapability {
                carrier: ManagementCarrier::decode(reader.byte()?)?,
                authenticated: reader.boolean()?,
                max_frame: reader.u16()?,
            },
            MAX_CARRIERS,
        )?;
    }
    for _ in 0..reader.count(MAX_RECOVERY_PATHS)? {
        push(
            &mut out.recovery_paths,
            RecoveryPath {
                carrier: ManagementCarrier::decode(reader.byte()?)?,
                enabled: reader.boolean()?,
                remote: reader.boolean()?,
                physical_presence: reader.boolean()?,
            },
            MAX_RECOVERY_PATHS,
        )?;
    }
    Ok(out)
}

fn push<T, const N: usize>(
    out: &mut Vec<T, N>,
    value: T,
    maximum: usize,
) -> Result<(), DecodeError> {
    out.push(value).map_err(|_| DecodeError::OversizedField {
        declared: maximum + 1,
        maximum,
    })
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
    fn bytes(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
        let end = self
            .cursor
            .checked_add(bytes.len())
            .filter(|end| *end <= self.out.len())
            .ok_or(EncodeError::BufferTooSmall)?;
        self.out[self.cursor..end].copy_from_slice(bytes);
        self.cursor = end;
        Ok(())
    }
    fn byte(&mut self, value: u8) -> Result<(), EncodeError> {
        self.bytes(&[value])
    }
    fn boolean(&mut self, value: bool) -> Result<(), EncodeError> {
        self.byte(u8::from(value))
    }
    fn count(&mut self, value: usize) -> Result<(), EncodeError> {
        self.byte(value as u8)
    }
    fn bounded<const N: usize>(&mut self, value: &Vec<u8, N>) -> Result<(), EncodeError> {
        self.count(value.len())?;
        self.bytes(value)
    }
    fn u16(&mut self, value: u16) -> Result<(), EncodeError> {
        self.bytes(&value.to_le_bytes())
    }
    fn u64(&mut self, value: u64) -> Result<(), EncodeError> {
        self.bytes(&value.to_le_bytes())
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
    fn request_header(&mut self) -> Result<(), DecodeError> {
        if self.array::<4>()? != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        let version = self.byte()?;
        if version != VERSION {
            return Err(DecodeError::UnsupportedVersion(version));
        }
        Ok(())
    }
    fn response_header(&mut self) -> Result<(), DecodeError> {
        self.request_header()?;
        let tag = self.byte()?;
        if tag == RESPONSE {
            Ok(())
        } else {
            Err(DecodeError::WrongFrameKind(tag))
        }
    }
    fn finish(&self) -> Result<(), DecodeError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes)
        }
    }
    fn take(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.cursor.checked_add(len).ok_or(DecodeError::Truncated)?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or(DecodeError::Truncated)?;
        self.cursor = end;
        Ok(value)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }
    fn byte(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn boolean(&mut self) -> Result<bool, DecodeError> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(DecodeError::InvalidBoolean(value)),
        }
    }
    fn count(&mut self, maximum: usize) -> Result<usize, DecodeError> {
        let declared = usize::from(self.byte()?);
        if declared > maximum {
            Err(DecodeError::OversizedField { declared, maximum })
        } else {
            Ok(declared)
        }
    }
    fn bounded<const N: usize>(&mut self) -> Result<Vec<u8, N>, DecodeError> {
        let len = self.count(N)?;
        let mut out = Vec::new();
        out.extend_from_slice(self.take(len)?)
            .map_err(|_| DecodeError::OversizedField {
                declared: len,
                maximum: N,
            })?;
        Ok(out)
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
}
