//! LXMF 1.3.1 accepts a stamp over the bare material as well as the expanded workblock.
//! Every vector here was mined by stock LXMF 1.3.1 (`LXStamper.generate_stamp`, cost 8),
//! and each bare stamp fails the expanded check, so a pass proves the bare path.

use outrider::stamp::{
    MESSAGE_WORKBLOCK_ROUNDS, PROPAGATION_WORKBLOCK_ROUNDS, STAMP_LEN, StampCost, bare_cost,
    valid_streamed_cost,
};
use outrider::{StampOutcome, ticket::check_stamp_cost};

fn stamp(hex: &str) -> [u8; STAMP_LEN] {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    bytes.try_into().unwrap()
}

const COST: StampCost = StampCost::from_integer(8);

#[test]
fn delivery_accepts_stock_bare_and_expanded_stamps() {
    let message_id: [u8; 32] = core::array::from_fn(|i| i as u8);
    let bare = stamp("1bac7ec0af34919d3f4b9ef5c722d66de0318e8f9d396c11765da59bbf4559b0");
    let expanded = stamp("320eb65508e501837320982cf7ce2d12012db5a69bb299c11a57f47accf05664");
    assert!(!valid_streamed_cost(
        &message_id,
        MESSAGE_WORKBLOCK_ROUNDS,
        &bare,
        COST
    ));
    assert_eq!(
        check_stamp_cost(&message_id, Some(&bare), COST, &[]),
        Ok(StampOutcome::Work(8))
    );
    assert_eq!(
        check_stamp_cost(&message_id, Some(&expanded), COST, &[]),
        Ok(StampOutcome::Work(9))
    );
}

#[test]
fn propagation_accepts_stock_bare_and_expanded_stamps() {
    let transient_id: [u8; 32] = core::array::from_fn(|i| 32 + i as u8);
    let bare = stamp("d23f6585aa0a0c10533469a70f198e91afcbe883494de45f54408f21fdf0605c");
    let expanded = stamp("b2b209a04d290b2be4b7ad3de4c60e87b08cdb8cfd307ca5e35b9c402ed29444");
    assert!(!valid_streamed_cost(
        &transient_id,
        PROPAGATION_WORKBLOCK_ROUNDS,
        &bare,
        COST
    ));
    assert_eq!(bare_cost(&transient_id, &bare, COST), Some(8));
    assert!(bare_cost(&transient_id, &expanded, COST).is_none());
    assert!(valid_streamed_cost(
        &transient_id,
        PROPAGATION_WORKBLOCK_ROUNDS,
        &expanded,
        COST
    ));
}
