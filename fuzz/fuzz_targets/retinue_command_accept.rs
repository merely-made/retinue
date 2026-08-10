#![no_main]
#![forbid(unsafe_code)]

// Design derived from Prns, github.com/KenAKAFrosty/Prns, Copyright (c) 2026 The Prns
// Authors, MIT OR Apache-2.0 (MIT elected). The discipline is theirs; no Prns text is
// copied here. See THIRD_PARTY_NOTICES.md and
// design_docs/2026-08-10_prns_donor_ledger.md.

use libfuzzer_sys::fuzz_target;
use retinue::command::{Command, MAX_PAYLOAD, Refusal, TargetClass, Verifier};
use retinue::hash::AddressHash;
use retinue::identity::{IDENTITY_LEN, PrivateIdentity};

const NODE: AddressHash = AddressHash::from_bytes([0x0a; 16]);
const FLEET: AddressHash = AddressHash::from_bytes([0xf1; 16]);

fn operator(fill: u8) -> PrivateIdentity {
    let mut secret = [0u8; IDENTITY_LEN];
    secret[..32].fill(fill);
    secret[32..].fill(fill ^ 0xff);
    PrivateIdentity::from_secret_bytes(&secret)
}

fn mutate(command: &mut [u8], bytes: &[u8]) {
    if command.is_empty() {
        return;
    }
    for (index, byte) in bytes.iter().take(16).enumerate() {
        let offset = (usize::from(*byte) + index * 37) % command.len();
        command[offset] ^= byte.rotate_left((index % 8) as u32);
    }
}

fuzz_target!(|input: &[u8]| {
    // The FS2 authorization boundary. A field node holds no secret that authorizes anything,
    // so everything an attacker can reach is here: bytes in, an accept-or-refuse decision
    // out. Three properties are asserted rather than merely observed.
    //
    //   1. No input panics. On the RF path a panic is a remote reset button.
    //   2. Nothing is ever accepted from a key that is not allowlisted.
    //   3. The same bytes are never accepted twice, which is the replay claim itself.
    let Some((&selector, body)) = input.split_first() else {
        return;
    };

    let trusted = operator(0x11);
    let stranger = operator(0x22);
    let mut verifier: Verifier<4> = Verifier::new(NODE);
    verifier
        .authorize(*trusted.public())
        .expect("allowlist has room");
    if selector & 0b100 != 0 {
        verifier.join_fleet(FLEET);
    }

    // The signer the fuzzer's bytes will be attributed to. Half the branches sign with a key
    // the node has never heard of, so "unknown key" is exercised as heavily as forgery.
    let signer = if selector & 0b1000 != 0 {
        &stranger
    } else {
        &trusted
    };

    let wire = match selector & 0b11 {
        // Raw bytes: the shape, version, and length checks.
        0 => body.to_vec(),
        // A genuine command, unmodified: the path that must keep working.
        1 => {
            let payload = &body[..body.len().min(MAX_PAYLOAD)];
            let command = Command {
                key_id: signer.hash(),
                class: TargetClass::Node,
                target: NODE,
                counter: 1,
                opcode: body.first().copied().unwrap_or(0),
                payload,
            };
            match command.sign(signer) {
                Ok(bytes) => bytes.to_vec(),
                Err(_) => return,
            }
        }
        // A genuine command with the fuzzer's damage applied. This is the branch that reaches
        // past the header into the counter window and the signature check with a message that
        // was well-formed a moment ago.
        _ => {
            let payload = &body[..body.len().min(MAX_PAYLOAD)];
            let command = Command {
                key_id: signer.hash(),
                class: if selector & 0b10000 != 0 {
                    TargetClass::Fleet
                } else {
                    TargetClass::Node
                },
                target: if selector & 0b10000 != 0 { FLEET } else { NODE },
                // Let the fuzzer choose the counter: the window has two edges and both are
                // reachable from bytes it controls.
                counter: u64::from_be_bytes({
                    let mut wide = [0u8; 8];
                    for (slot, byte) in wide.iter_mut().zip(body.iter()) {
                        *slot = *byte;
                    }
                    wide
                }),
                opcode: body.first().copied().unwrap_or(0),
                payload,
            };
            let Ok(signed) = command.sign(signer) else {
                return;
            };
            let mut bytes = signed.to_vec();
            mutate(&mut bytes, body);
            bytes
        }
    };

    let first = verifier.accept(&wire);

    if let Ok(accepted) = &first {
        // Property 2: only the allowlisted operator can ever be the accepted signer.
        assert_eq!(
            accepted.key_id,
            trusted.hash(),
            "accepted a command attributed to a key that is not allowlisted"
        );
        // And it must have been addressed to somewhere this node actually is.
        match accepted.class {
            TargetClass::Node => assert_eq!(accepted.target, NODE),
            TargetClass::Fleet => assert_eq!(accepted.target, FLEET),
        }
        assert!(accepted.payload.len() <= MAX_PAYLOAD);

        // Property 3: replay. The identical bytes that just succeeded must now fail, and
        // fail specifically as a replay rather than by accident of some other check.
        assert_eq!(
            verifier.accept(&wire).err(),
            Some(Refusal::CounterReplayed),
            "the same command was accepted twice"
        );
    }

    // Refusals and acceptances are counted, never panicked. The counters are the FS1
    // diagnostics posture, so a run that accepted nothing must still have counted it.
    assert_eq!(
        u64::from(verifier.accepted()) + u64::from(verifier.refusals()),
        if first.is_ok() { 2 } else { 1 },
        "an ingest attempt went uncounted"
    );
});
