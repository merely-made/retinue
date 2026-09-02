use super::fakes::*;
use super::{buffers, runtime, state};
use crate::control::*;
use futures::executor::block_on;

#[test]
fn hash_mismatched_grant_journal_poisons_before_any_apply() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    let mut body = [0; MAX_DURABLE_BODY];
    let len = encode_durable(&state(), &mut body).unwrap();
    // RHD1 header: magic(4), version(1), node(16), watermark(8), grant_count(1).
    body[30] ^= 1;
    let page_len = crate::store::encode(0, &body[..len], &mut s.a).unwrap();
    s.a[page_len..].fill(0xFF);
    let mut a = FakeApplier::new(&t);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    assert!(matches!(
        block_on(r.boot_pre_radio(&mut s, &mut a, &mut scratch(&mut x, &mut y, &mut b, &mut p))),
        Err(RuntimeError::Load(DurableLoadError::State(
            DurableError::Malformed
        )))
    ));
    assert!(r.is_poisoned());
    assert_eq!(a.calls, 0);
    assert!(
        !t.snapshot()
            .iter()
            .any(|call| matches!(call, Call::Apply(_)))
    );
}
