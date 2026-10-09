//! Each owner page, from first survey to recovery.

use linkboy::device::BoardSelection;
use linkboy::executor::{ExecutionStage, RecoveryFacts};
use linkboy::package::RecoveryInstructions;
use linkboy::{BoardFamily, FlashEvent, OwnerStage, ReceiptResult};
use signalman_desktop::state::Request;

use crate::{harness, settle, state, to_review, v4_observation};

#[test]
fn the_flow_opens_on_choose_device_and_lists_what_answered() {
    let h = harness(state());
    assert_eq!(h.state().stage(), OwnerStage::ChooseDevice);
    h.with_surfaces(|s| {
        assert!(genet_probe::text_present(s, "Choose device"));
        assert!(
            genet_probe::text_present(s, "COM7"),
            "the surveyed port is on the page",
        );
        assert!(
            genet_probe::text_present(s, "US915"),
            "with what it said about itself",
        );
    });
}

/// The revision refusal is text rather than a disabled button, and it says why:
/// nothing on the wire names the revision.
#[test]
fn a_missing_board_revision_refuses_in_words() {
    let mut h = harness(state());
    h.update(|state| {
        state.select_device(0);
        state.request(Request::ConfirmDevice);
    });
    settle(&mut h);

    assert_eq!(
        h.state().stage(),
        OwnerStage::ChooseDevice,
        "the flow did not advance",
    );
    assert!(!h.state().refusal.is_empty());
    h.with_surfaces(|s| {
        assert!(
            genet_probe::text_present(s, "exact board revision"),
            "the refusal says what is missing",
        );
        assert!(
            genet_probe::text_present(s, "refuses to plan a flash without a source"),
            "and why it matters",
        );
    });
}

/// Choosing a package is where compatibility is decided, and where the review
/// page's data comes from. Every field the scope names is on the page.
#[test]
fn the_review_page_shows_every_plan_fact() {
    let mut h = harness(state());
    to_review(&mut h);
    assert_eq!(h.state().stage(), OwnerStage::ReviewChanges);
    assert!(
        h.state().refusal.is_empty(),
        "a compatible package is not refused: {:?}",
        h.state().refusal,
    );

    let review = h.state().view().review.expect("the flow produced a review");
    h.with_surfaces(|s| {
        for (what, expected) in [
            ("package id", review.package_id.as_str()),
            ("display name", review.display_name.as_str()),
            ("version", review.version.as_str()),
            ("publisher", review.publisher.as_str()),
            ("license", review.license.as_str()),
            ("source url", review.source_url.as_str()),
            ("origin url", review.origin_url.as_str()),
            ("board revision", review.board_revision.as_str()),
            (
                "board revision evidence",
                review.board_revision_evidence.as_str(),
            ),
            ("helper", review.helper.as_str()),
            ("helper license", review.helper_license.as_str()),
            ("helper source", review.helper_source_url.as_str()),
            (
                "recovery before write",
                review.recovery_before_write.as_str(),
            ),
            (
                "recovery after failure",
                review.recovery_after_failure.as_str(),
            ),
        ] {
            assert!(
                genet_probe::text_present(s, expected),
                "the review page must show the {what}: {expected:?}",
            );
        }
        for part in &review.package_parts {
            assert!(
                genet_probe::text_present(s, &part.sha256),
                "the review page must show each verified artifact hash: {:?}",
                part.sha256,
            );
        }
        assert!(genet_probe::text_present(s, "0x00000000"));
        assert!(genet_probe::text_present(s, "0x003f0000"));
        assert!(genet_probe::text_present(s, "Preserved"));
    });
}

#[test]
fn the_review_keeps_a_documented_v4_profile_as_revision_evidence() {
    let mut h = harness(state());
    let mut observation = v4_observation();
    observation.selected_board = Some(BoardSelection::documented_product_profile(
        BoardFamily::HeltecV4,
        "4.2",
        "Meshnology N39 WiFi LoRa 32 V4 kit",
        "https://wiki.meshnology.com/N39/Meshnology%20N39/",
    ));
    h.update(|state| {
        state
            .installer
            .choose_device(observation)
            .expect("the documented profile has matching V4 facts");
        state.select_package(0);
        state.request(Request::ConfirmFirmware);
    });
    settle(&mut h);

    let review = h
        .state()
        .view()
        .review
        .expect("the documented plan has a review");
    assert!(review.board_revision_evidence.contains("Meshnology N39"));
    assert!(
        review
            .board_revision_evidence
            .contains("wiki.meshnology.com")
    );
    h.with_surfaces(|s| {
        assert!(genet_probe::text_present(
            s,
            "Meshnology N39 WiFi LoRa 32 V4 kit"
        ));
        assert!(genet_probe::text_present(
            s,
            "https://wiki.meshnology.com/N39/Meshnology%20N39/"
        ));
    });
}

/// Approving moves to the preparation page, which repeats the recovery
/// instructions *before* anything irreversible starts.
#[test]
fn approving_reaches_prepare_with_the_before_write_instructions() {
    let mut h = harness(state());
    to_review(&mut h);
    h.update(|state| state.request(Request::ApproveChanges));
    settle(&mut h);
    assert_eq!(h.state().stage(), OwnerStage::PrepareDevice);
    h.with_surfaces(|s| {
        assert!(genet_probe::text_present(s, "Prepare the device"));
        assert!(genet_probe::text_present(s, "Keep the USB cable attached",));
    });
}

/// Events progress the install page: each one becomes a line an owner can
/// read, and the write becomes a percentage rather than a spinner.
#[test]
fn events_progress_the_install_page() {
    let mut h = harness(state());
    to_review(&mut h);
    h.update(|state| state.request(Request::ApproveChanges));
    settle(&mut h);
    h.update(|state| {
        state.apply_event(&FlashEvent::Inspecting {
            device: "COM7".into(),
            package_id: "retinue.heltec-v4".into(),
        });
        state.apply_event(&FlashEvent::Erasing);
        state.apply_event(&FlashEvent::Writing {
            written: 1_000,
            total: 4_000,
        });
    });
    assert_eq!(h.state().stage(), OwnerStage::Install);
    assert_eq!(h.state().progress, Some(0.25));
    h.with_surfaces(|s| {
        assert!(genet_probe::text_present(s, "Inspecting COM7"));
        assert!(genet_probe::text_present(s, "Erasing"));
        assert!(genet_probe::text_present(s, "1000 of 4000 bytes (25%)"));
    });

    // A second write replaces the first line rather than stacking per chunk.
    h.update(|state| {
        state.apply_event(&FlashEvent::Writing {
            written: 3_000,
            total: 4_000,
        });
    });
    assert_eq!(
        h.state()
            .notes
            .iter()
            .filter(|n| n.starts_with("Writing "))
            .count(),
        1,
    );
    h.with_surfaces(|s| assert!(genet_probe::text_present(s, "(75%)")));
}

/// A recovery event ends on the recovery page with the package's own
/// after-failure instructions and the facts a person needs to act on.
#[test]
fn a_recovery_event_shows_the_recovery_context() {
    let mut h = harness(state());
    to_review(&mut h);
    h.update(|state| state.request(Request::ApproveChanges));
    settle(&mut h);

    let plan = h
        .state()
        .installer
        .plan()
        .cloned()
        .expect("an approved plan exists");
    let facts = RecoveryFacts {
        stage: ExecutionStage::Transfer,
        transport: "COM7".into(),
        last_known_port: Some("COM7".into()),
        write_started: true,
        detail: "the transfer stopped part-way".into(),
    };
    let receipt = linkboy::FlashReceipt::recovery_required(&plan, Vec::new());
    h.update(|state| {
        state.apply_event(&FlashEvent::RecoveryRequired {
            facts: facts.clone(),
            instructions: RecoveryInstructions {
                before_write: "Keep the cable attached.".into(),
                after_failure: "Re-enter the ROM loader and retry the same package.".into(),
            },
            receipt: receipt.clone(),
        });
    });

    assert_eq!(h.state().stage(), OwnerStage::VerifyOrRecover);
    assert!(h.state().needs_recovery());
    assert_eq!(
        h.state().view().result,
        Some(ReceiptResult::RecoveryRequired)
    );
    h.with_surfaces(|s| {
        assert!(genet_probe::text_present(s, "Recovery required"));
        assert!(genet_probe::text_present(s, "during the transfer"));
        assert!(genet_probe::text_present(
            s,
            "Re-enter the ROM loader and retry the same package.",
        ));
        assert!(
            genet_probe::text_present(s, "COM7"),
            "the last known port, so a person can find the board again",
        );
    });
}
