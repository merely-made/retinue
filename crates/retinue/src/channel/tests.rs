//! Channel and Buffer tests.

// The crate is `no_std`, so the tests take these from alloc rather than the std prelude.
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

mod buffer;
mod delivery;
mod window;
mod wire;

fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn hex_str(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
