//! Wire anchors are deployed state. Boards in the field hold records under these magics and
//! tags under these domains, so they keep their `radio-hand` bytes whatever this crate is
//! called. The test sits here because this is the one module every anchor is visible from.

use super::MAGIC as DURABLE_MAGIC;
use super::portable_first_write::CLAIM_DOMAIN;
use super::semantic::DOMAIN as SEMANTIC_TAG_DOMAIN;
use crate::control::model::MAGIC as CONTROL_MAGIC;
use crate::store::MAGIC as STORE_MAGIC;

#[test]
fn wire_anchors_keep_their_deployed_bytes() {
    assert_eq!(SEMANTIC_TAG_DOMAIN, b"radio-hand-semantic-tag-v2");
    assert_eq!(CONTROL_MAGIC, [0x52, 0x48, 0x43, 0x30]); // RHC0
    assert_eq!(DURABLE_MAGIC, [0x52, 0x48, 0x44, 0x31]); // RHD1
    assert_eq!(STORE_MAGIC, [0x52, 0x48, 0x53, 0x30]); // RHS0
    assert_eq!(CLAIM_DOMAIN, b"retinue:first-owner:claim:v1");
}
