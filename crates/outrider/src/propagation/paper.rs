//! Paper messages: a propagation-shaped message carried as an `lxm://` URI, for a QR code
//! or any other channel outside Reticulum.

use retinue::endpoint::Endpoint;
use retinue::hash::{AddressHash, NameHash};
use retinue::identity::PrivateIdentity;

use super::{PropagationError, PropagationMessage};
use crate::announce::delivery_destination;
use crate::codec::{LxmfPayload, prepare};

pub const URI_SCHEMA: &str = "lxm";
/// What a version-40 QR code holds at error correction L.
const QR_MAX_STORAGE: usize = 2953;
/// The largest paper message: what fits the QR code after the scheme, as base64. 2210 bytes.
pub const PAPER_MDU: usize = (QR_MAX_STORAGE - URI_SCHEMA.len() - "://".len()) * 6 / 8;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Clone, Debug)]
pub struct PreparedPaper {
    pub message_id: [u8; 32],
    pub packed_message: Vec<u8>,
    pub message: PropagationMessage,
    /// The recipient ratchet the message was sealed to, `None` for its identity key.
    pub ratchet_id: Option<NameHash>,
}

/// Build, sign and seal one message for paper delivery to the delivery destination
/// `recipient`, to its advertised ratchet when `endpoint` has heard one, as stock packs a
/// paper message (`LXMessage.py` 451-463). Refused when it would not fit [`PAPER_MDU`].
pub fn prepare_paper(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    recipient: AddressHash,
    payload: &LxmfPayload,
) -> Result<PreparedPaper, PropagationError> {
    if sender.public() != endpoint.identity() {
        return Err(PropagationError::LocalIdentityMismatch);
    }
    prepare_paper_with(sender, recipient, payload, |plaintext| {
        endpoint
            .encrypt_for(recipient, plaintext)
            .map_err(|_| PropagationError::UnknownRecipient(recipient))
    })
}

/// [`prepare_paper`] with the sealing supplied by the caller, which returns the token and
/// the ratchet it used, for a host that knows the recipient only by its key.
pub fn prepare_paper_with(
    sender: &PrivateIdentity,
    recipient: AddressHash,
    payload: &LxmfPayload,
    seal: impl FnOnce(&[u8]) -> Result<(Vec<u8>, Option<NameHash>), PropagationError>,
) -> Result<PreparedPaper, PropagationError> {
    let source = delivery_destination(sender.public());
    let prepared = prepare(*recipient.as_bytes(), *source.as_bytes(), payload)?;
    let message_id = prepared.message_id;
    let signature = sender.sign(prepared.signing_bytes());
    let packed_message = prepared.finish(signature);
    let (encrypted, ratchet_id) = seal(&packed_message[16..])?;
    let message = PropagationMessage {
        destination: *recipient.as_bytes(),
        encrypted,
    };
    if 16 + message.encrypted.len() > PAPER_MDU {
        return Err(PropagationError::PaperTooLarge);
    }
    Ok(PreparedPaper {
        message_id,
        packed_message,
        message,
        ratchet_id,
    })
}

impl PropagationMessage {
    /// `lxm://` and the message in unpadded URL-safe base64. Refused past [`PAPER_MDU`].
    pub fn to_uri(&self) -> Result<String, PropagationError> {
        let bytes = self.encode();
        if bytes.len() > PAPER_MDU {
            return Err(PropagationError::PaperTooLarge);
        }
        let mut uri = String::with_capacity(6 + bytes.len().div_ceil(3) * 4);
        uri.push_str(URI_SCHEMA);
        uri.push_str("://");
        for chunk in bytes.chunks(3) {
            let group = chunk
                .iter()
                .enumerate()
                .fold(0_u32, |group, (i, b)| group | u32::from(*b) << (16 - 8 * i));
            for sextet in 0..=chunk.len() {
                uri.push(ALPHABET[(group >> (18 - 6 * sextet) & 0x3f) as usize] as char);
            }
        }
        Ok(uri)
    }

    /// Read an `lxm://` URI, scheme in any case. Like stock, every `/` after the scheme is
    /// dropped and missing padding is restored; ASCII whitespace, such as a scanner's
    /// trailing newline or a line wrap, is dropped too, as stock's lenient base64 decoder
    /// drops it (`LXMRouter.py` 2611-2627). Other characters outside the alphabet, which
    /// stock would also drop, are refused.
    pub fn from_uri(uri: &str, max_message_bytes: usize) -> Result<Self, PropagationError> {
        let prefix = URI_SCHEMA.len() + "://".len();
        let body = uri
            .get(..prefix)
            .filter(|scheme| scheme.eq_ignore_ascii_case("lxm://"))
            .and_then(|_| uri.get(prefix..))
            .ok_or(PropagationError::InvalidUri)?;
        let sextets = body
            .trim_end_matches('=')
            .bytes()
            .filter(|b| *b != b'/' && !b.is_ascii_whitespace())
            .map(sextet)
            .collect::<Option<Vec<u8>>>()
            .ok_or(PropagationError::InvalidUri)?;
        if sextets.len() % 4 == 1 || sextets.len() * 3 / 4 > max_message_bytes {
            return Err(PropagationError::InvalidUri);
        }
        let mut bytes = Vec::with_capacity(sextets.len() * 3 / 4);
        for chunk in sextets.chunks(4) {
            let group = chunk
                .iter()
                .enumerate()
                .fold(0_u32, |group, (i, s)| group | u32::from(*s) << (18 - 6 * i));
            bytes.extend((0..chunk.len() - 1).map(|i| (group >> (16 - 8 * i)) as u8));
        }
        Self::decode(&bytes, max_message_bytes)
    }
}

/// A base64 digit, URL-safe or standard: stock's decoder takes both.
fn sextet(digit: u8) -> Option<u8> {
    match digit {
        b'A'..=b'Z' => Some(digit - b'A'),
        b'a'..=b'z' => Some(digit - b'a' + 26),
        b'0'..=b'9' => Some(digit - b'0' + 52),
        b'-' | b'+' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use retinue::token::{IV_LEN, encrypt_to_identity};

    use super::*;

    fn message(len: usize) -> PropagationMessage {
        PropagationMessage {
            destination: [7; 16],
            encrypted: (0..len).map(|i| (i * 37) as u8).collect(),
        }
    }

    #[test]
    fn the_paper_limit_is_stocks() {
        assert_eq!(PAPER_MDU, 2210);
    }

    #[test]
    fn uris_round_trip_at_every_padding_length() {
        for len in 96..=99 {
            let original = message(len);
            let uri = original.to_uri().unwrap();
            assert!(uri.starts_with("lxm://"));
            assert!(!uri.contains('='));
            assert_eq!(PropagationMessage::from_uri(&uri, 4096).unwrap(), original);
        }
    }

    #[test]
    fn decoding_tolerates_what_stock_does() {
        let original = message(97);
        let uri = original.to_uri().unwrap();
        let body = &uri[6..];
        let (head, tail) = body.split_at(40);
        for variant in [
            format!("LXM://{body}"),
            format!("Lxm://{head}/{tail}/"),
            format!("lxm://{body}=="),
            format!("lxm://{head}\r\n{tail} \n"),
        ] {
            assert_eq!(
                PropagationMessage::from_uri(&variant, 4096).unwrap(),
                original,
                "{variant}"
            );
        }
    }

    #[test]
    fn malformed_and_oversized_uris_are_refused() {
        let uri = message(98).to_uri().unwrap();
        for bad in [
            "lxmf://AAAA".to_string(),
            "lxm:/AAAA".to_string(),
            format!("{uri}!"),
            format!("{}={}", &uri[..20], &uri[20..]),
            format!("{uri}A"),
            "lxm://".to_string(),
        ] {
            assert!(PropagationMessage::from_uri(&bad, 4096).is_err(), "{bad}");
        }
        assert!(PropagationMessage::from_uri(&uri, 100).is_err());
        assert!(matches!(
            message(PAPER_MDU - 15).to_uri(),
            Err(PropagationError::PaperTooLarge)
        ));
        assert!(message(PAPER_MDU - 16).to_uri().is_ok());
    }

    #[test]
    fn a_prepared_paper_message_decrypts_and_verifies() {
        let sender = PrivateIdentity::from_secret_bytes(&[0x21; 64]);
        let recipient = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
        let to_key = |plaintext: &[u8]| {
            let token = encrypt_to_identity(recipient.public(), &[9; 32], &[3; IV_LEN], plaintext);
            Ok((token, None))
        };
        let destination = delivery_destination(recipient.public());
        let payload = LxmfPayload::text(1_753_603_202.5, b"paper", b"by hand");
        let paper = prepare_paper_with(&sender, destination, &payload, to_key).unwrap();
        let read = PropagationMessage::from_uri(&paper.message.to_uri().unwrap(), 4096).unwrap();
        let decoded = read.decrypt(&recipient, 4096).unwrap();
        assert_eq!(decoded.message_id, paper.message_id);
        assert_eq!(decoded.payload.content, b"by hand");
        assert!(decoded.verify_with(|bytes, sig| sender.public().verify(bytes, sig)));

        let large = LxmfPayload::text(1.0, b"", vec![0; PAPER_MDU]);
        assert!(matches!(
            prepare_paper_with(&sender, destination, &large, to_key),
            Err(PropagationError::PaperTooLarge)
        ));
    }
}
