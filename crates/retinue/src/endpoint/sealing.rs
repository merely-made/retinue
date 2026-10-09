//! Identity tokens sealed to and opened by destinations outside any packet, for payloads an
//! application stores and forwards itself (an LXMF propagated message, for one).

use alloc::vec::Vec;

use std::io;

use crate::destination::DestinationName;
use crate::hash::{AddressHash, NameHash};
use crate::identity::{Identity, KEY_LEN};
use crate::ratchet::RatchetStore;

use super::entropy::{fill_random, next_iv};
use super::runtime::Endpoint;
use super::shared::Shared;

impl Endpoint {
    /// Encrypt `plaintext` to an announced destination as [`send_single`] does: to its
    /// advertised ratchet, or to its identity key when it advertises none (RNS
    /// `Destination.encrypt`). Returns the token and the ratchet it was sealed to.
    ///
    /// [`send_single`]: Self::send_single
    pub fn encrypt_for(
        &self,
        dest: AddressHash,
        plaintext: &[u8],
    ) -> io::Result<(Vec<u8>, Option<NameHash>)> {
        let sealed = seal(&self.shared, dest, plaintext)?;
        Ok((sealed.token, sealed.ratchet_id))
    }

    /// Decrypt a token addressed to a registered destination as inbound single packets are:
    /// its retained ratchets first, then the identity key unless it enforces ratchets (RNS
    /// `Destination.decrypt`). Returns the plaintext and the ratchet that opened it, `None`
    /// for the identity key.
    pub fn decrypt_for(
        &self,
        name: &DestinationName,
        token: &[u8],
    ) -> io::Result<(Vec<u8>, Option<NameHash>)> {
        let dest = name.destination_hash(self.shared.identity.public());
        let (ratchets, enforce) = self
            .shared
            .registered
            .lock()
            .unwrap()
            .iter()
            .find(|registration| registration.dest == dest)
            .map(|registration| (registration.ratchets.clone(), registration.enforce_ratchets))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "destination is not registered")
            })?;
        open(&self.shared, ratchets.as_deref(), enforce, token)
    }
}

/// A token sealed to an announced destination, with the ratchet and identity it went to.
pub(super) struct Sealed {
    pub(super) token: Vec<u8>,
    pub(super) ratchet_id: Option<NameHash>,
    pub(super) peer: Identity,
}

/// Seal to `dest`'s advertised ratchet, or to its identity key when it advertises none
/// (RNS `Destination.encrypt`). Every outbound single token is sealed here.
pub(super) fn seal(shared: &Shared, dest: AddressHash, plaintext: &[u8]) -> io::Result<Sealed> {
    let (peer, ratchet) = {
        let mut address_book = shared.address_book.lock().unwrap();
        address_book.mark_used(dest, super::known_destinations::book_clock_ms());
        let peer = address_book.resolve(dest).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "destination has not announced")
        })?;
        (peer.identity, peer.ratchet)
    };
    let mut ephemeral = [0u8; KEY_LEN];
    fill_random(&mut ephemeral);
    let token = match &ratchet {
        Some(ratchet) => {
            crate::token::encrypt_to_ratchet(&peer, ratchet, &ephemeral, &next_iv(), plaintext)
        }
        None => crate::token::encrypt_to_identity(&peer, &ephemeral, &next_iv(), plaintext),
    };
    Ok(Sealed {
        token,
        ratchet_id: ratchet.map(|ratchet| NameHash::of(&ratchet)),
        peer,
    })
}

/// Open a token with a registration's retained ratchets, then the identity key unless it
/// enforces ratchets (RNS `Identity.decrypt`). Every inbound single token is opened here.
pub(super) fn open(
    shared: &Shared,
    ratchets: Option<&RatchetStore>,
    enforce: bool,
    token: &[u8],
) -> io::Result<(Vec<u8>, Option<NameHash>)> {
    if let Some((plaintext, id)) =
        ratchets.and_then(|ratchets| ratchets.decrypt(&shared.identity, token).ok())
    {
        return Ok((plaintext, Some(id)));
    }
    if enforce {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "token is not sealed to a retained ratchet",
        ));
    }
    crate::token::decrypt_to_identity(&shared.identity, token)
        .map(|plaintext| (plaintext, None))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
