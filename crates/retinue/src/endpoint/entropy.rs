//! OS randomness for link secrets and IVs.

use crate::token::IV_LEN;

/// Fill `buf` with OS randomness. Link secrecy rests on these bytes being unpredictable, so
/// this panics rather than hand back weak ones.
pub(super) fn fill_random(buf: &mut [u8]) {
    getrandom::getrandom(buf).expect("OS CSPRNG unavailable");
}

/// A fresh 64-byte link ephemeral seed (`x25519_secret(32) || ed25519_seed(32)`), unique and
/// unpredictable per link.
pub(super) fn ephemeral_seed() -> [u8; 64] {
    let mut seed = [0u8; 64];
    fill_random(&mut seed);
    seed
}

/// A fresh AES-CBC IV. Must be unpredictable per packet under a given link key.
pub(super) fn next_iv() -> [u8; IV_LEN] {
    let mut iv = [0u8; IV_LEN];
    fill_random(&mut iv);
    iv
}
