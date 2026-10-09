//! The encrypted token.
//!
//! Reticulum's token is derived from Fernet but is deliberately not Fernet: the version
//! byte and the 8-byte timestamp are stripped, because they cost bytes and leak initiator
//! metadata. What remains is:
//!
//! ```text
//! token = IV(16) || AES-256-CBC(PKCS7(plaintext)) || HMAC-SHA256(32)
//! ```
//!
//! with the HMAC covering `IV || ciphertext`.
//!
//! When encrypting *to an identity* (rather than over an established link) an ephemeral
//! X25519 public key is prepended, giving the full on-wire form:
//!
//! ```text
//! ephemeral_x25519_pub(32) || IV(16) || ciphertext || HMAC-SHA256(32)
//! ```
//!
//! Note the ephemeral key is **not** covered by the HMAC.
//!
//! Keys come from:
//!
//! ```text
//! derived  = HKDF-SHA256(ikm = x25519_shared, salt = identity_hash(16), info = <empty>, len = 64)
//! sign_key = derived[0..32]     (HMAC-SHA256)
//! enc_key  = derived[32..64]    (AES-256)
//! ```
//!
//! Every line of this was settled by decrypting a real RNS 1.3.8 token: all four
//! combinations of {AES-128, AES-256} x {sign-key-first, enc-key-first} were tried, and
//! only AES-256 with the signing key first both authenticates and decrypts. The Beechat
//! crate gets this right on one code path and wrong on another, so it could not be trusted
//! here.

use alloc::vec::Vec;

use aes::Aes256;
use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, InnerIvInit};
use hkdf::Hkdf;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use x25519_dalek::PublicKey as XPublicKey;

use crate::hash::{AddressHash, NameHash};
use crate::identity::{Identity, KEY_LEN, PrivateIdentity};
use crate::{Error, Result};

type Aes256CbcEnc = cbc::Encryptor<Aes256>;
type Aes256CbcDec = cbc::Decryptor<Aes256>;
type HmacSha256 = Hmac<Sha256>;

/// Length of the AES-CBC initialisation vector.
pub const IV_LEN: usize = 16;

/// Length of the HMAC-SHA256 tag.
pub const MAC_LEN: usize = 32;

/// Bytes a token adds on top of the padded plaintext: `IV + HMAC`.
///
/// RNS calls this `TOKEN_OVERHEAD` and reports 48, which agrees.
pub const TOKEN_OVERHEAD: usize = IV_LEN + MAC_LEN;

/// Total bytes the HKDF produces, split evenly into signing and encryption keys.
pub const DERIVED_KEY_LEN: usize = 64;

/// The two symmetric keys for a token, held pre-keyed: the AES key schedule and the HMAC
/// pads are computed once, at derivation, not per packet.
#[derive(Clone)]
pub struct DerivedKeys {
    mac: HmacSha256,
    aes: Aes256,
}

impl DerivedKeys {
    /// Stretch an X25519 shared secret into the signing and encryption keys.
    ///
    /// The salt is the recipient's identity hash. The info string is empty.
    pub fn derive(shared_secret: &[u8; KEY_LEN], salt: AddressHash) -> Self {
        let hk = Hkdf::<Sha256>::new(Some(salt.as_slice()), shared_secret);
        let mut okm = [0u8; DERIVED_KEY_LEN];
        hk.expand(&[], &mut okm)
            .expect("64 bytes is a valid HKDF-SHA256 output length");
        let (sign, enc) = okm.split_at(32);
        #[cfg(test)]
        crate::probe::hit(crate::probe::Probe::TokenKeying);
        Self {
            mac: <HmacSha256 as KeyInit>::new_from_slice(sign).expect("HMAC accepts a 32-byte key"),
            aes: Aes256::new_from_slice(enc).expect("AES-256 takes a 32-byte key"),
        }
    }

    /// Encrypt, producing `IV || ciphertext || HMAC`.
    ///
    /// `iv` is supplied by the caller so this stays free of any RNG and reproducible in
    /// tests. It must be fresh and unpredictable in production.
    pub fn encrypt(&self, plaintext: &[u8], iv: &[u8; IV_LEN]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(IV_LEN + plaintext.len() + 16 + MAC_LEN);
        buf.extend_from_slice(iv);
        buf.extend_from_slice(plaintext);
        self.seal_in_place(buf, iv)
    }

    /// [`encrypt`](Self::encrypt), in place: the plaintext's own buffer becomes the token.
    pub fn encrypt_owned(&self, mut plaintext: Vec<u8>, iv: &[u8; IV_LEN]) -> Vec<u8> {
        plaintext.reserve_exact(IV_LEN + 16 + MAC_LEN);
        plaintext.splice(0..0, iv.iter().copied());
        self.seal_in_place(plaintext, iv)
    }

    /// Encrypt `buf`, which holds `iv || plaintext`, behind its IV and append the HMAC,
    /// using the cached key schedule and HMAC pads.
    fn seal_in_place(&self, mut buf: Vec<u8>, iv: &[u8; IV_LEN]) -> Vec<u8> {
        let len = buf.len() - IV_LEN;
        // PKCS7 always pads, up to a whole block.
        buf.resize(IV_LEN + (len / 16 + 1) * 16, 0);
        let ct_len = Aes256CbcEnc::inner_iv_init(self.aes.clone(), iv.into())
            .encrypt_padded::<Pkcs7>(&mut buf[IV_LEN..], len)
            .expect("buffer has a full block of headroom")
            .len();
        buf.truncate(IV_LEN + ct_len);
        let mut mac = self.mac.clone();
        mac.update(&buf);
        buf.extend_from_slice(&mac.finalize().into_bytes());
        buf
    }

    /// Verify and decrypt `IV || ciphertext || HMAC`.
    ///
    /// The HMAC is checked before anything is decrypted, and in constant time.
    pub fn decrypt(&self, token: &[u8]) -> Result<Vec<u8>> {
        let ciphertext = self.authenticate(token)?;
        let mut buf = ciphertext.to_vec();
        let len = self.decrypt_block(&token[..IV_LEN], &mut buf)?;
        buf.truncate(len);
        Ok(buf)
    }

    /// [`decrypt`](Self::decrypt), in place: the token's own buffer becomes the plaintext.
    pub fn decrypt_owned(&self, mut token: Vec<u8>) -> Result<Vec<u8>> {
        let end = IV_LEN + self.authenticate(&token)?.len();
        let (iv, body) = token[..end].split_at_mut(IV_LEN);
        let len = self.decrypt_block(iv, body)?;
        token.copy_within(IV_LEN..IV_LEN + len, 0);
        token.truncate(len);
        Ok(token)
    }

    /// Check the HMAC, returning the ciphertext it covers.
    fn authenticate<'a>(&self, token: &'a [u8]) -> Result<&'a [u8]> {
        if token.len() <= TOKEN_OVERHEAD {
            return Err(Error::Truncated);
        }
        let (body, tag) = token.split_at(token.len() - MAC_LEN);

        let mut mac = self.mac.clone();
        mac.update(body);
        mac.verify_slice(tag).map_err(|_| Error::BadMac)?;

        let ciphertext = &body[IV_LEN..];
        if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
            return Err(Error::BadPadding);
        }
        Ok(ciphertext)
    }

    /// Decrypt `buf` in place under `iv` with the cached key schedule, returning the
    /// unpadded length.
    fn decrypt_block(&self, iv: &[u8], buf: &mut [u8]) -> Result<usize> {
        let iv: &[u8; IV_LEN] = iv.try_into().expect("an IV_LEN prefix");
        Aes256CbcDec::inner_iv_init(self.aes.clone(), iv.into())
            .decrypt_padded::<Pkcs7>(buf)
            .map(<[u8]>::len)
            .map_err(|_| Error::BadPadding)
    }
}

/// Decrypt a token addressed to us: `ephemeral_pub(32) || IV || ciphertext || HMAC`.
///
/// The ephemeral public key is read off the front, ECDH'd against our X25519 secret, and
/// the result stretched with our own identity hash as the salt.
pub fn decrypt_to_identity(recipient: &PrivateIdentity, token: &[u8]) -> Result<Vec<u8>> {
    if token.len() < KEY_LEN + TOKEN_OVERHEAD {
        return Err(Error::Truncated);
    }
    let eph: [u8; KEY_LEN] = token[..KEY_LEN].try_into().expect("checked length");
    let shared = recipient.diffie_hellman(&XPublicKey::from(eph));
    let keys = DerivedKeys::derive(&shared, recipient.hash());
    keys.decrypt(&token[KEY_LEN..])
}

/// Decrypt a token against retained ratchet secrets.
///
/// Ratcheted single packets carry no ratchet id. RNS therefore tries retained private
/// ratchets until one authenticates the token. The identity hash, not the ratchet id,
/// remains the HKDF salt. The returned id identifies the ratchet that succeeded.
pub fn decrypt_with_ratchets<'a>(
    recipient: &PrivateIdentity,
    retained_secrets: impl IntoIterator<Item = &'a [u8; KEY_LEN]>,
    token: &[u8],
) -> Result<(Vec<u8>, NameHash)> {
    let (plaintext, _, secret) = trial_decrypt(recipient, retained_secrets, token)?;
    // Only the winning epoch's public key is derived: a failed attempt costs one ECDH.
    let public = XPublicKey::from(&x25519_dalek::StaticSecret::from(*secret));
    Ok((plaintext, NameHash::of(public.as_bytes())))
}

/// Trial-decrypt `token` against `retained_secrets` in order, returning the plaintext with
/// the index and secret of the epoch that authenticated it.
pub(crate) fn trial_decrypt<'a>(
    recipient: &PrivateIdentity,
    retained_secrets: impl IntoIterator<Item = &'a [u8; KEY_LEN]>,
    token: &[u8],
) -> Result<(Vec<u8>, usize, &'a [u8; KEY_LEN])> {
    if token.len() < KEY_LEN + TOKEN_OVERHEAD {
        return Err(Error::Truncated);
    }
    let eph: [u8; KEY_LEN] = token[..KEY_LEN].try_into().expect("checked length");
    let eph = XPublicKey::from(eph);

    for (index, retained) in retained_secrets.into_iter().enumerate() {
        let shared = x25519_dalek::StaticSecret::from(*retained)
            .diffie_hellman(&eph)
            .to_bytes();
        let keys = DerivedKeys::derive(&shared, recipient.hash());
        match keys.decrypt(&token[KEY_LEN..]) {
            Ok(plaintext) => return Ok((plaintext, index, retained)),
            Err(Error::BadMac) => {}
            Err(error) => return Err(error),
        }
    }

    Err(Error::BadMac)
}

/// Encrypt a token to a peer identity, given a caller-supplied ephemeral secret and IV.
///
/// Both are parameters rather than generated here so this module needs no RNG and stays
/// reproducible. In production the runtime layer must supply a fresh, unpredictable
/// ephemeral secret for every single token: reuse destroys the security of the scheme.
pub fn encrypt_to_identity(
    recipient: &Identity,
    ephemeral_secret: &[u8; KEY_LEN],
    iv: &[u8; IV_LEN],
    plaintext: &[u8],
) -> Vec<u8> {
    let secret = x25519_dalek::StaticSecret::from(*ephemeral_secret);
    let eph_public = XPublicKey::from(&secret);
    let shared = secret.diffie_hellman(recipient.x25519()).to_bytes();

    let keys = DerivedKeys::derive(&shared, recipient.hash());

    let mut out = Vec::new();
    out.extend_from_slice(eph_public.as_bytes());
    out.extend_from_slice(&keys.encrypt(plaintext, iv));
    out
}

/// Encrypt a token to a destination's advertised ratchet public key.
///
/// This has the same wire layout as [`encrypt_to_identity`]. Only the X25519 peer changes:
/// the ephemeral secret is combined with the ratchet public key, while the destination
/// identity hash remains the HKDF salt.
pub fn encrypt_to_ratchet(
    recipient: &Identity,
    ratchet_public: &[u8; KEY_LEN],
    ephemeral_secret: &[u8; KEY_LEN],
    iv: &[u8; IV_LEN],
    plaintext: &[u8],
) -> Vec<u8> {
    let secret = x25519_dalek::StaticSecret::from(*ephemeral_secret);
    let eph_public = XPublicKey::from(&secret);
    let shared = secret
        .diffie_hellman(&XPublicKey::from(*ratchet_public))
        .to_bytes();

    let keys = DerivedKeys::derive(&shared, recipient.hash());

    let mut out = Vec::new();
    out.extend_from_slice(eph_public.as_bytes());
    out.extend_from_slice(&keys.encrypt(plaintext, iv));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn a_key_pair_is_keyed_once_for_any_number_of_tokens() {
        use crate::probe::{Probe, take};

        take(Probe::TokenKeying);
        let keys = DerivedKeys::derive(&[5u8; KEY_LEN], AddressHash::from_bytes([6; 16]));
        for len in [0, 1, 15, 16, 17, 383] {
            let plaintext = vec![0xA5; len];
            let token = keys.encrypt(&plaintext, &[len as u8; IV_LEN]);
            assert_eq!(token.len(), TOKEN_OVERHEAD + (len / 16 + 1) * 16);
            assert_eq!(keys.decrypt(&token).unwrap(), plaintext);
        }
        assert_eq!(take(Probe::TokenKeying), 1);
    }

    /// The cached schedules produce what keying the primitives afresh from the HKDF output
    /// does, as the token was built before they were cached.
    #[test]
    fn cached_keys_match_freshly_keyed_primitives() {
        use aes::cipher::KeyIvInit;

        let (secret, salt) = ([5u8; KEY_LEN], AddressHash::from_bytes([6; 16]));
        let mut okm = [0u8; DERIVED_KEY_LEN];
        Hkdf::<Sha256>::new(Some(salt.as_slice()), &secret)
            .expand(&[], &mut okm)
            .unwrap();
        let (iv, plaintext) = ([0x3C; IV_LEN], b"seventeen bytes!!");
        let mut expected = iv.to_vec();
        let mut buf = [0u8; 32];
        expected.extend_from_slice(
            Aes256CbcEnc::new(okm[32..].try_into().unwrap(), (&iv).into())
                .encrypt_padded_b2b::<Pkcs7>(plaintext, &mut buf)
                .unwrap(),
        );
        let mut mac = <HmacSha256 as KeyInit>::new_from_slice(&okm[..32]).unwrap();
        mac.update(&expected);
        expected.extend_from_slice(&mac.finalize().into_bytes());

        assert_eq!(
            DerivedKeys::derive(&secret, salt).encrypt(plaintext, &iv),
            expected
        );
    }

    #[test]
    fn round_trip_through_our_own_code() {
        let recipient = PrivateIdentity::from_secret_bytes(&[7u8; 64]);
        let token = encrypt_to_identity(
            recipient.public(),
            &[9u8; 32],
            &[3u8; IV_LEN],
            b"hello retinue",
        );
        let back = decrypt_to_identity(&recipient, &token).unwrap();
        assert_eq!(back, b"hello retinue");
    }

    #[test]
    fn a_flipped_ciphertext_byte_fails_the_mac() {
        let recipient = PrivateIdentity::from_secret_bytes(&[7u8; 64]);
        let mut token = encrypt_to_identity(
            recipient.public(),
            &[9u8; 32],
            &[3u8; IV_LEN],
            b"hello retinue",
        );
        let n = token.len();
        token[n - MAC_LEN - 1] ^= 0xFF;
        assert!(matches!(
            decrypt_to_identity(&recipient, &token),
            Err(Error::BadMac)
        ));
    }

    #[test]
    fn retained_ratchets_are_tried_until_one_authenticates() {
        let recipient = PrivateIdentity::from_secret_bytes(&[7u8; 64]);
        let current = [11u8; KEY_LEN];
        let public = XPublicKey::from(&x25519_dalek::StaticSecret::from(current));
        let token = encrypt_to_ratchet(
            recipient.public(),
            public.as_bytes(),
            &[9u8; KEY_LEN],
            &[3u8; IV_LEN],
            b"hello retained ratchet",
        );

        let (plaintext, ratchet_id) =
            decrypt_with_ratchets(&recipient, &[[12u8; KEY_LEN], current], &token).unwrap();
        assert_eq!(plaintext, b"hello retained ratchet");
        assert_eq!(ratchet_id, NameHash::of(public.as_bytes()));
    }

    #[test]
    fn an_unknown_ratchet_fails_authentication() {
        let recipient = PrivateIdentity::from_secret_bytes(&[7u8; 64]);
        let public = XPublicKey::from(&x25519_dalek::StaticSecret::from([11u8; KEY_LEN]));
        let token = encrypt_to_ratchet(
            recipient.public(),
            public.as_bytes(),
            &[9u8; KEY_LEN],
            &[3u8; IV_LEN],
            b"not for the retained key",
        );

        assert_eq!(
            decrypt_with_ratchets(&recipient, &[[12u8; KEY_LEN]], &token).unwrap_err(),
            Error::BadMac,
        );
    }
}
