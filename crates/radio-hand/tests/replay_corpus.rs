//! The fixture corpus, replayed through a node, with the bytes it must produce.
//!
//! Gate N3's done condition: `ingest` and `poll` replayed against the desktop fixture corpus
//! produce identical Actions on the board and on the desk. This file is the desk half and the
//! definition of "identical" — the exact bytes, committed. The board half is a hardware
//! receipt driving the same fixtures through `replay` over the host link and comparing what
//! comes back to these strings.
//!
//! Every fixture here was captured from the RNS 1.3.8 reference implementation, so a passing
//! comparison says the board agrees with the desktop about packets a real Reticulum emitted,
//! not about packets this workspace invented.

use radio_hand::replay::{encode_actions, encode_nothing, replay_node, to_hex};
use retinue::announce::{AnnounceBlob, RAND_HASH_LEN};
use retinue::packet::Packet;

/// The clock every replay runs at. Fixed, so neither side depends on when it ran.
const NOW: u64 = 1_000;

/// The interface a replayed packet arrives on. The radio, as the board numbers it.
const IFACE: u32 = 0;

/// Entropy for a replayed `poll`. Caller-supplied by the protocol's own design, which is
/// exactly what makes an announce reproducible.
const SEED: [u8; RAND_HASH_LEN] = [0xA5; RAND_HASH_LEN];

fn fixture(name: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../retinue/tests/fixtures/");
    std::fs::read(format!("{path}{name}")).unwrap_or_else(|e| panic!("missing fixture {name}: {e}"))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = vec![0_u8; bytes.len() * 2];
    let written = to_hex(bytes, &mut out);
    String::from_utf8(out[..written].to_vec()).unwrap()
}

/// Ingest one fixture into a fresh node and return the encoded actions as hex.
fn ingest(name: &str) -> String {
    let raw = fixture(name);
    let mut node = replay_node::<32, 8, 4>();
    let Ok(packet) = Packet::decode(&raw) else {
        // A fixture that is not even a packet still has an answer: no actions. The board
        // reaches the same place by a different route, refusing it before ingest.
        return hex(&encode_nothing());
    };
    hex(&encode_actions(&node.ingest(IFACE, &packet, NOW)))
}

/// Every fixture the corpus offers as a raw packet, and the actions it must produce.
///
/// The invalid announces all produce an empty set, which is the point of including them: a
/// board that accepted one would differ from the desktop in the direction that matters.
/// A set with no actions. Named rather than spelled out, so it reads as a decision.
const NOTHING: &str = "01000000";

const CORPUS: &[(&str, &str)] = &[
    (
        "announce_plain.bin",
        "0101000002a8725a7e212dace39e9f99a8ac5da28c",
    ),
    (
        "announce_appdata.bin",
        "0101000002a8725a7e212dace39e9f99a8ac5da28c",
    ),
    (
        "announce_ratchet.bin",
        "0101000002a8725a7e212dace39e9f99a8ac5da28c",
    ),
    (
        "announce_ratchet_appdata.bin",
        "0101000002a8725a7e212dace39e9f99a8ac5da28c",
    ),
    ("announce_invalid_signature.bin", NOTHING),
    ("announce_invalid_pubkey.bin", NOTHING),
    ("announce_invalid_desthash.bin", NOTHING),
    ("announce_invalid_namehash.bin", NOTHING),
    ("announce_invalid_randhash.bin", NOTHING),
    ("announce_invalid_appdata.bin", NOTHING),
    ("link_proof.bin", NOTHING),
    ("link_rns_data.bin", NOTHING),
];

#[test]
fn the_corpus_produces_the_committed_actions() {
    for (name, expected) in CORPUS {
        assert_eq!(&ingest(name), expected, "fixture {name}");
    }
}

/// A first `poll` announces, and the announce it builds is reproducible from a fixed seed.
///
/// This is the half of the done condition `ingest` cannot cover: the board's own decision to
/// speak, rather than its reaction to something it heard.
#[test]
fn a_first_poll_announces_reproducibly() {
    let mut node = replay_node::<32, 8, 4>();
    let blob = AnnounceBlob::from_wire(SEED);
    let actions = node.poll(NOW, IFACE, Some(&blob));
    assert_eq!(hex(&encode_actions(&actions)), POLL_ANNOUNCE);
}

const POLL_ANNOUNCE: &str = concat!(
    "010100000100000000a7000100185efd55eca87398a50cdc3a78979a8b007b4e909bbe7ffe44c465a2200",
    "37d608ee35897d31ef972f07f74892cb0f73f13d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016b",
    "af8520a332c9778737bdbdf263608fdbc917efa5a5a5a5a5a5a5a5a5a55d7adf1ffe04f05c9f8b50abe3c",
    "4104d39442d4c27af73197adf6fc6b2de80c95e88a6f1ed83490bceb4860b7a910568b5995e7741826090",
    "057178a6fd6c8900",
);

/// Printed rather than asserted, so a hardware receipt has the exact lines to send.
///
/// Run with `cargo test -p radio-hand --features replay --test replay_corpus -- --nocapture`.
#[test]
fn print_the_board_script() {
    for (name, expected) in CORPUS {
        println!("replay {NOW} {}", hex(&fixture(name)));
        println!("  expect actions {expected}");
    }
    println!("replay poll {NOW} {}", hex(&SEED));
    println!("  expect actions {POLL_ANNOUNCE}");
}
