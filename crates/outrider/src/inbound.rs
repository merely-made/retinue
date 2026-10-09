//! What a receiving lane establishes about a message before handing it over: whether its
//! signature stands, and what its stamp was worth.

use std::time::{SystemTime, UNIX_EPOCH};

use retinue::endpoint::Endpoint;
use retinue::hash::AddressHash;
use retinue::identity::Identity;

use crate::announce::delivery_destination;
use crate::codec::DecodedLxmf;
use crate::ticket::{self, StampFault, StampOutcome, TICKET_LEN};

/// How a received message's signature stands (`LXMessage.py` 814-827).
///
/// The delivery lanes and propagation fetch refuse [`SignatureInvalid`](Self::SignatureInvalid)
/// and hand over the other two. A direct [`SourceUnknown`](Self::SourceUnknown) message was
/// proved, so its sender will not resend it: the host holds it until the sender's announce
/// arrives and [`reverify`] settles it. An opportunistic one need not be held: it was left
/// unproved, and the lane delivers the sender's retry once that verifies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verification {
    Verified,
    /// No announce has given us the sender's keys yet.
    SourceUnknown,
    SignatureInvalid,
}

/// Check `message` against `source`, the identity resolved for its source, if any.
pub fn verify(message: &DecodedLxmf, source: Option<&Identity>) -> Verification {
    let Some(identity) = source else {
        return Verification::SourceUnknown;
    };
    let derives = delivery_destination(identity).as_bytes() == &message.source;
    if derives && message.verify_with(|bytes, signature| identity.verify(bytes, signature)) {
        Verification::Verified
    } else {
        Verification::SignatureInvalid
    }
}

/// Settle a held [`Verification::SourceUnknown`] message against the identities `endpoint`
/// has learned since, returning the identity when it verifies. Asks the network for nothing
/// and records nothing: admit a verified message to the lane's
/// [`DeliveredCache`](crate::DeliveredCache), which says whether it is new, before handing
/// it over.
pub fn reverify(endpoint: &Endpoint, message: &DecodedLxmf) -> (Verification, Option<Identity>) {
    let identity = endpoint.resolve(AddressHash::from_bytes(message.source));
    let verification = verify(message, identity.as_ref());
    (
        verification,
        identity.filter(|_| verification == Verification::Verified),
    )
}

/// Score the stamp a destination with `cost` requires, as stock does under
/// `enforce_stamps` (`LXMRouter.py` 1924-1945): first against `tickets`, those this
/// destination issued to the sender, then as proof of work (`LXMessage.py` 278-299). No
/// cost, no check, no outcome. Every receiving lane checks stamps here.
pub(crate) fn check_stamp(
    message: &DecodedLxmf,
    cost: Option<u8>,
    tickets: &[[u8; TICKET_LEN]],
) -> Result<Option<StampOutcome>, StampFault> {
    let Some(cost) = cost else {
        return Ok(None);
    };
    let stamp = message.payload.stamp.as_deref();
    ticket::check_stamp(&message.message_id, stamp, cost, tickets).map(Some)
}

/// Seconds since the Unix epoch, the clock stock stamps its delivery cache with.
pub(crate) fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use retinue::identity::PrivateIdentity;

    use super::*;
    use crate::codec::{LxmfPayload, decode, prepare};
    use crate::stamp::{MESSAGE_WORKBLOCK_ROUNDS, STAMP_LEN};

    fn signed(sender: &PrivateIdentity, payload: &LxmfPayload) -> DecodedLxmf {
        let source = delivery_destination(sender.public());
        let prepared = prepare([0x11; 16], *source.as_bytes(), payload).unwrap();
        let signature = sender.sign(prepared.signing_bytes());
        decode(&prepared.finish(signature)).unwrap()
    }

    #[test]
    fn verification_distinguishes_unknown_from_invalid() {
        let sender = PrivateIdentity::from_secret_bytes(&[0x31; 64]);
        let other = PrivateIdentity::from_secret_bytes(&[0x32; 64]);
        let message = signed(&sender, &LxmfPayload::text(1.0, "t", "c"));
        assert_eq!(
            verify(&message, Some(sender.public())),
            Verification::Verified
        );
        assert_eq!(verify(&message, None), Verification::SourceUnknown);
        assert_eq!(
            verify(&message, Some(other.public())),
            Verification::SignatureInvalid
        );

        let mut forged = message.clone();
        forged.signature[0] ^= 1;
        assert_eq!(
            verify(&forged, Some(sender.public())),
            Verification::SignatureInvalid
        );
    }

    #[test]
    fn a_checked_stamp_reports_its_value() {
        let sender = PrivateIdentity::from_secret_bytes(&[0x33; 64]);
        let mut payload = LxmfPayload::text(2.0, "t", "c");
        assert_eq!(check_stamp(&signed(&sender, &payload), None, &[]), Ok(None));
        assert_eq!(
            check_stamp(&signed(&sender, &payload), Some(4), &[]),
            Err(StampFault::Missing)
        );

        let id = signed(&sender, &payload).message_id;
        let (stamp, value) =
            crate::stamp::find_streamed(&id, MESSAGE_WORKBLOCK_ROUNDS, 4, [0; STAMP_LEN], 1 << 12)
                .unwrap();
        payload.stamp = Some(stamp.to_vec());
        let stamped = signed(&sender, &payload);
        assert_eq!(
            check_stamp(&stamped, Some(4), &[]),
            Ok(Some(StampOutcome::Work(value)))
        );
        assert!(value >= 4);
        if value < 255 {
            let above = u8::try_from(value + 1).unwrap();
            assert_eq!(
                check_stamp(&stamped, Some(above), &[]),
                Err(StampFault::Invalid)
            );
        }
    }
}
