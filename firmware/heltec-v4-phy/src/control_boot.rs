//! Boot-only WN1 durable recovery for the Heltec V4.
//!
//! This is deliberately before host splitting, RNode selection, and RX arming. It restores a
//! previously durable radio profile after a real ESP reset. On the literal USB image only, it
//! also hosts the bounded physical first-owner carrier before ordinary host construction.

#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
use embassy_futures::select::{Either, select};
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_io_async::{Read, Write};
use esp_hal::gpio::Input;
use heapless::Vec;
use hmac::{Hmac, KeyInit, Mac};
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
use radio_hand::control::{
    AbandonOutcome, AbandonResponse, ClaimChallenge, ClaimResponse, DurableState,
    FirstOwnerRequest, FirstOwnerResponse, INSPECT_RESPONSE_LEN, ResumeOutcome, ResumeResponse,
    abandon_first_write, resume_first_write, stage_first_write,
};
use radio_hand::control::{
    BoardRecoveryFacts, BootState, ControlRuntime, ControlStatusV1, DurableScratch,
    FirstWriteScratch, FirstWriteStatus, MAX_DURABLE_BODY, NodeId, RuntimeError, SemanticTagKey,
    inspect_first_write,
};
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
use radio_hand::control::{FirstWriteStore, load_first_write_state};
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
use radio_hand::control::{ManagementCarrier, RecoveryPathFacts};
use radio_hand::settings::IDENTITY_LEN;
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
use radio_hand::store::Slot;
use sha2::Sha256;
use static_cell::StaticCell;

#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
use crate::physical_presence::PhysicalPresence;
use crate::radio_owner::{V4BootOwner, V4ConfigError, V4FirstWriteStoreError, V4RadioOwner};

use crate::control_store::CONTROL_SLOT_LEN;

type HmacSha256 = Hmac<Sha256>;

/// Versioned domain for the V4's opaque WN1 node identifier.
const NODE_ID_DOMAIN: &[u8] = b"retinue-heltec-v4-control-node-id-v1";
/// Versioned domain for the V4's durable semantic-replay key.
const SEMANTIC_KEY_DOMAIN: &[u8] = b"retinue-heltec-v4-control-semantic-key-v1";

static CONTROL_SLOT_A: StaticCell<[u8; CONTROL_SLOT_LEN]> = StaticCell::new();
static CONTROL_SLOT_B: StaticCell<[u8; CONTROL_SLOT_LEN]> = StaticCell::new();
static CONTROL_BODY: StaticCell<[u8; MAX_DURABLE_BODY]> = StaticCell::new();
static CONTROL_PAGE: StaticCell<[u8; CONTROL_SLOT_LEN]> = StaticCell::new();
static SESSION_CONTROL_A: StaticCell<[u8; CONTROL_SLOT_LEN]> = StaticCell::new();
static SESSION_CONTROL_B: StaticCell<[u8; CONTROL_SLOT_LEN]> = StaticCell::new();
static SESSION_PENDING_A: StaticCell<[u8; CONTROL_SLOT_LEN]> = StaticCell::new();
static SESSION_PENDING_B: StaticCell<[u8; CONTROL_SLOT_LEN]> = StaticCell::new();
static SESSION_BODY: StaticCell<[u8; MAX_DURABLE_BODY]> = StaticCell::new();
static SESSION_PAGE: StaticCell<[u8; CONTROL_SLOT_LEN]> = StaticCell::new();
static SESSION_READBACK: StaticCell<[u8; CONTROL_SLOT_LEN]> = StaticCell::new();
// This pair is deliberately separate from `FirstWriteScratch`: its private buffers remain
// owned by portable transactions, while this V4-only read-only preflight must inspect the exact
// pending image before it can call the generic resume writer.
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
static RESUME_PENDING_A: StaticCell<[u8; CONTROL_SLOT_LEN]> = StaticCell::new();
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
static RESUME_PENDING_B: StaticCell<[u8; CONTROL_SLOT_LEN]> = StaticCell::new();

/// The pre-host claim carrier does not remain resident. Traffic cannot stretch
/// this deadline because the board must soon return to ordinary compatibility
/// service or a safe pending/status boot.
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
const FIRST_OWNER_SESSION_WINDOW: Duration = Duration::from_secs(45);
/// Accepted complete KISS requests in one physical-presence session.
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
const FIRST_OWNER_REQUEST_LIMIT: u8 = 8;
/// Malformed frames allowed before the bounded session ends.
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
const FIRST_OWNER_PARSE_FAILURE_LIMIT: u8 = 3;
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
const FIRST_OWNER_RESPONSE_SETTLE: Duration = Duration::from_millis(120);
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
const FIRST_OWNER_RESPONSE_WINDOW: Duration = Duration::from_secs(2);
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
const FIRST_OWNER_MAX_FRAME: usize = radio_hand::control::CLAIM_REQUEST_LEN;
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
const FIRST_OWNER_MAX_WIRE: usize = 2 + FIRST_OWNER_MAX_FRAME * 2;

/// A boot-only failure before any radio service begins.
#[derive(Debug)]
pub(crate) enum ControlBootError {
    /// The one pre-radio owner capability was already consumed or radio service began.
    OwnerUnavailable,
    /// First-write storage could not be read or changed during the pre-host session.
    FirstWriteStore(V4FirstWriteStoreError),
    /// The ADC-backed true entropy source could not mint a claim challenge.
    EntropyUnavailable,
    /// The durable runtime reached storage, recovery, or hardware application and failed.
    Runtime(RuntimeError<crate::control_store::ControlError, V4ConfigError>),
}

/// Proof that board startup observed an actual ESP reset before WN1 recovery.
///
/// This is deliberately non-`Copy`, has no public fields, and is consumed by the only safe boot
/// entry point below. It cannot become a live-control capability.
pub(super) struct HardwareResetToken {
    _private: (),
}

/// Mark the one board-startup transition after an actual hardware reset.
///
/// # Safety
/// Call exactly once from the board startup owner after an actual ESP reset, before host
/// construction, RNode selection, RX arming, `power::arm`, or any radio service. Calling this
/// without that reset would acknowledge unresolved durable/radio state.
pub(super) unsafe fn after_hardware_reset() -> HardwareResetToken {
    HardwareResetToken { _private: () }
}

fn derive_hmac(identity: &[u8; IDENTITY_LEN], domain: &[u8]) -> [u8; 32] {
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(identity)
        .expect("HMAC-SHA256 accepts the fixed board identity length");
    mac.update(domain);
    let digest = mac.finalize().into_bytes();
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&digest);
    bytes
}

/// Derive only opaque WN1 material from the board's persisted identity.
///
/// The 64-byte identity remains local. These domains partition the derived node identifier
/// from the semantic tag key; this derivation is not credential sealing.
pub(crate) fn derive_control_material(identity: &[u8; IDENTITY_LEN]) -> (NodeId, SemanticTagKey) {
    let node_digest = derive_hmac(identity, NODE_ID_DOMAIN);
    let mut node = [0_u8; 16];
    node.copy_from_slice(&node_digest[..16]);
    let semantic_key = SemanticTagKey::from_bytes(derive_hmac(identity, SEMANTIC_KEY_DOMAIN));
    (NodeId(node), semantic_key)
}

/// Recovery facts for the host transport this image actually built.
///
/// `ManagementCarrier::Usb` means the ESP32-S3 USB Serial/JTAG host only. It is not an alias
/// for UART0 or generic local wired access, so the UART low-power image advertises no current
/// WN1 recovery path. A durable journal requiring USB recovery therefore fails closed on that
/// image rather than silently treating a different physical transport as equivalent.
pub(crate) fn recovery_facts() -> BoardRecoveryFacts {
    #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
    let paths = {
        let usb = RecoveryPathFacts::new(ManagementCarrier::Usb, true, false, false)
            .expect("the fixed USB recovery facts are internally consistent");
        Vec::from_slice(&[usb]).expect("one recovery path fits the shared bound")
    };
    #[cfg(feature = "host-uart-low-power")]
    let paths = Vec::new();

    BoardRecoveryFacts::new(paths).expect("the fixed board recovery facts have no duplicates")
}

fn durable_scratch(
    control_a: &'static mut [u8; CONTROL_SLOT_LEN],
    control_b: &'static mut [u8; CONTROL_SLOT_LEN],
    body: &'static mut [u8; MAX_DURABLE_BODY],
    page: &'static mut [u8; CONTROL_SLOT_LEN],
) -> DurableScratch<'static> {
    DurableScratch::new(control_a, control_b, body, page)
        .expect("V4 control scratch matches the durable journal geometry")
}

fn first_write_scratch() -> FirstWriteScratch<'static> {
    FirstWriteScratch::new(
        SESSION_CONTROL_A.init([0; CONTROL_SLOT_LEN]),
        SESSION_CONTROL_B.init([0; CONTROL_SLOT_LEN]),
        SESSION_PENDING_A.init([0; CONTROL_SLOT_LEN]),
        SESSION_PENDING_B.init([0; CONTROL_SLOT_LEN]),
        SESSION_BODY.init([0; MAX_DURABLE_BODY]),
        SESSION_PAGE.init([0; CONTROL_SLOT_LEN]),
        SESSION_READBACK.init([0; CONTROL_SLOT_LEN]),
    )
    .expect("V4 first-write scratch matches the four fixed A/B slots")
}

/// Whether a post-witness carrier session can start for this exact evidence.
/// `Blank` is the only state allowed to return to ordinary service if its
/// claim-only session does not write pending state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FirstWriteBootGate {
    ControlPresent,
    ClaimOnly,
    StatusOnly,
}

const fn first_write_boot_gate(status: FirstWriteStatus) -> FirstWriteBootGate {
    if matches!(status.control, radio_hand::control::PairEvidence::Valid) {
        FirstWriteBootGate::ControlPresent
    } else if status.ordinary_service_eligible() {
        FirstWriteBootGate::ClaimOnly
    } else if status.actions().permits_resume() || status.actions().permits_abandon() {
        FirstWriteBootGate::ClaimOnly
    } else {
        FirstWriteBootGate::StatusOnly
    }
}

/// Board-local admission for a portable pending image. Portable resume proves
/// journal integrity and recovery facts; this extra gate proves the V4 can
/// actually host the selected profile before control storage is touched.
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
fn v4_resume_admissible(
    status: FirstWriteStatus,
    configuration: &radio_hand::control::DurableConfig,
) -> bool {
    status.actions().permits_resume()
        && crate::radio_owner::first_write_configuration_feasible(configuration).is_ok()
}

/// Read and decode the exact pending A/B image without mutating either
/// journal. A malformed or no-longer-resumable image is rejected rather than
/// being passed to the generic resume writer.
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
fn v4_pending_resume_admissible<RK, DLY>(
    owner: &mut V4BootOwner<'_, RK, DLY>,
    pending_a: &mut [u8],
    pending_b: &mut [u8],
    node: NodeId,
    facts: &BoardRecoveryFacts,
    status: FirstWriteStatus,
) -> Result<bool, V4FirstWriteStoreError>
where
    RK: lora_phy::mod_traits::RadioKind,
    DLY: lora_phy::DelayNs,
{
    FirstWriteStore::read_pending(owner, Slot::A, pending_a)?;
    FirstWriteStore::read_pending(owner, Slot::B, pending_b)?;
    let Ok(pending) = load_first_write_state(pending_a, pending_b, node, facts) else {
        return Ok(false);
    };
    Ok(v4_resume_admissible(
        status,
        &pending.known_good().configuration,
    ))
}

/// The bounded carrier ended without a durable mutation, or discovered an
/// uncertain first-write store result that must remain status-only.
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FirstOwnerSessionEnd {
    NoMutation,
    StagedPending,
    StorageFault,
    EntropyUnavailable,
}

#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
const fn no_mutation_session_end(staged: bool) -> FirstOwnerSessionEnd {
    if staged {
        FirstOwnerSessionEnd::StagedPending
    } else {
        FirstOwnerSessionEnd::NoMutation
    }
}

#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
fn session_expired(deadline: Instant) -> bool {
    Instant::now() >= deadline
}

#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
async fn send_first_owner_response<W: Write>(
    tx: &mut W,
    response: FirstOwnerResponse,
    deadline: Instant,
) -> bool {
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        return false;
    };
    let response_window = remaining.min(FIRST_OWNER_RESPONSE_WINDOW);
    let mut payload = [0_u8; INSPECT_RESPONSE_LEN];
    let length = match response {
        FirstOwnerResponse::Inspect { .. } => response.encode(&mut payload).ok(),
        FirstOwnerResponse::Claim(_)
        | FirstOwnerResponse::Resume(_)
        | FirstOwnerResponse::Abandon(_) => response.encode(&mut payload[..3]).ok(),
    };
    let Some(length) = length else {
        return false;
    };
    let mut wire = [0_u8; FIRST_OWNER_MAX_WIRE];
    let Some(wire_len) = selvage::kiss::encode_into(&payload[..length], &mut wire) else {
        return false;
    };
    with_timeout(response_window, async {
        tx.write_all(&wire[..wire_len]).await?;
        tx.flush().await
    })
    .await
    .is_ok_and(|result| result.is_ok())
}

#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
async fn respond_then_reset<W: Write>(tx: &mut W, response: FirstOwnerResponse) -> ! {
    let _ =
        send_first_owner_response(tx, response, Instant::now() + FIRST_OWNER_RESPONSE_WINDOW).await;
    Timer::after(FIRST_OWNER_RESPONSE_SETTLE).await;
    esp_hal::system::software_reset()
}

/// Host-USB-only KISS carrier. The physical-presence token is consumed by
/// entering this function; no ordinary host or UART path can construct it.
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
async fn first_owner_usb_session<R: Read, W: Write, RK, DLY>(
    _presence: PhysicalPresence,
    owner: &mut V4BootOwner<'_, RK, DLY>,
    rx: &mut R,
    tx: &mut W,
    node: NodeId,
    facts: &BoardRecoveryFacts,
    initial_status: FirstWriteStatus,
    scratch: &mut FirstWriteScratch<'_>,
    resume_pending_a: &mut [u8],
    resume_pending_b: &mut [u8],
) -> FirstOwnerSessionEnd
where
    RK: lora_phy::mod_traits::RadioKind,
    DLY: lora_phy::DelayNs,
{
    let deadline = Instant::now() + FIRST_OWNER_SESSION_WINDOW;
    let mut deframer = selvage::kiss::Deframer::<FIRST_OWNER_MAX_FRAME>::new();
    let mut read = [0_u8; 64];
    let mut requests = 0_u8;
    let mut parse_failures = 0_u8;
    let mut challenge = None;
    let mut status = initial_status;
    let mut staged = false;

    loop {
        if session_expired(deadline) {
            return no_mutation_session_end(staged);
        }
        let length = match select(rx.read(&mut read), Timer::at(deadline)).await {
            Either::First(Ok(0)) | Either::First(Err(_)) | Either::Second(()) => {
                return no_mutation_session_end(staged);
            }
            Either::First(Ok(length)) => length,
        };
        for &byte in &read[..length] {
            if session_expired(deadline) {
                return no_mutation_session_end(staged);
            }
            if !deframer.push(byte) {
                continue;
            }
            let request = match FirstOwnerRequest::decode(deframer.frame()) {
                Ok(request) => request,
                Err(_) => {
                    parse_failures = parse_failures.saturating_add(1);
                    if parse_failures >= FIRST_OWNER_PARSE_FAILURE_LIMIT {
                        return no_mutation_session_end(staged);
                    }
                    continue;
                }
            };
            requests = requests.saturating_add(1);
            if requests > FIRST_OWNER_REQUEST_LIMIT {
                return no_mutation_session_end(staged);
            }

            match request {
                FirstOwnerRequest::Inspect => {
                    let inspected = match inspect_first_write(owner, scratch, node, facts) {
                        Ok(status) => status,
                        Err(_) => return FirstOwnerSessionEnd::StorageFault,
                    };
                    status = inspected;
                    let mut nonce = [0_u8; 32];
                    if owner.fill_commissioning_entropy(&mut nonce).is_err() {
                        return if staged {
                            FirstOwnerSessionEnd::StagedPending
                        } else {
                            FirstOwnerSessionEnd::EntropyUnavailable
                        };
                    }
                    challenge = Some(ClaimChallenge::from_fresh_entropy(nonce));
                    if !send_first_owner_response(
                        tx,
                        FirstOwnerResponse::Inspect {
                            status,
                            node,
                            nonce,
                        },
                        deadline,
                    )
                    .await
                    {
                        return no_mutation_session_end(staged);
                    }
                }
                FirstOwnerRequest::Claim(request) => {
                    let response = match challenge.take() {
                        _ if !status.claim_eligible() || staged => ClaimResponse::Rejected,
                        Some(challenge) => match challenge.verify(&request, node) {
                            Ok(claim) => match DurableState::from_owner_claim(node, claim, facts) {
                                Ok(state)
                                    if crate::radio_owner::first_write_configuration_feasible(
                                        &state.known_good().configuration,
                                    )
                                    .is_ok() =>
                                {
                                    match stage_first_write(owner, scratch, &state, node, facts) {
                                        Ok(_) => {
                                            staged = true;
                                            status = match inspect_first_write(
                                                owner, scratch, node, facts,
                                            ) {
                                                Ok(status) => status,
                                                Err(_) => {
                                                    return FirstOwnerSessionEnd::StorageFault;
                                                }
                                            };
                                            ClaimResponse::Staged
                                        }
                                        Err(_) => return FirstOwnerSessionEnd::StorageFault,
                                    }
                                }
                                Ok(_) | Err(_) => ClaimResponse::Rejected,
                            },
                            Err(_) => ClaimResponse::Rejected,
                        },
                        None => ClaimResponse::Rejected,
                    };
                    if !send_first_owner_response(tx, FirstOwnerResponse::Claim(response), deadline)
                        .await
                    {
                        return no_mutation_session_end(staged);
                    }
                }
                FirstOwnerRequest::Resume => {
                    if !status.actions().permits_resume() {
                        let _ = send_first_owner_response(
                            tx,
                            FirstOwnerResponse::Resume(ResumeResponse::Rejected),
                            deadline,
                        )
                        .await;
                        continue;
                    }
                    match v4_pending_resume_admissible(
                        owner,
                        resume_pending_a,
                        resume_pending_b,
                        node,
                        facts,
                        status,
                    ) {
                        Ok(true) => {}
                        // This remains a recoverable pending image. When its control pair is
                        // blank, the portable action bits still permit Abandon; otherwise the
                        // original portable recovery shape remains authoritative.
                        Ok(false) => {
                            let _ = send_first_owner_response(
                                tx,
                                FirstOwnerResponse::Resume(ResumeResponse::Rejected),
                                deadline,
                            )
                            .await;
                            continue;
                        }
                        Err(_) => return FirstOwnerSessionEnd::StorageFault,
                    }
                    match resume_first_write(owner, scratch, node, facts) {
                        Ok(ResumeOutcome::Committed) => {
                            respond_then_reset(
                                tx,
                                FirstOwnerResponse::Resume(ResumeResponse::Committed),
                            )
                            .await;
                        }
                        Ok(ResumeOutcome::CommittedWithCleanupFailure(_)) => {
                            respond_then_reset(
                                tx,
                                FirstOwnerResponse::Resume(ResumeResponse::CommittedCleanupPending),
                            )
                            .await;
                        }
                        Ok(ResumeOutcome::AlreadyControlPresent) => {
                            let _ = send_first_owner_response(
                                tx,
                                FirstOwnerResponse::Resume(ResumeResponse::Rejected),
                                deadline,
                            )
                            .await;
                        }
                        Err(_) => return FirstOwnerSessionEnd::StorageFault,
                    }
                }
                FirstOwnerRequest::Abandon => {
                    if !status.actions().permits_abandon() {
                        let _ = send_first_owner_response(
                            tx,
                            FirstOwnerResponse::Abandon(AbandonResponse::Rejected),
                            deadline,
                        )
                        .await;
                        continue;
                    }
                    match abandon_first_write(owner, scratch, node, facts) {
                        Ok(AbandonOutcome::Abandoned) => {
                            respond_then_reset(
                                tx,
                                FirstOwnerResponse::Abandon(AbandonResponse::Abandoned),
                            )
                            .await;
                        }
                        Ok(AbandonOutcome::NothingStaged) => {
                            let _ = send_first_owner_response(
                                tx,
                                FirstOwnerResponse::Abandon(AbandonResponse::Rejected),
                                deadline,
                            )
                            .await;
                        }
                        Err(_) => return FirstOwnerSessionEnd::StorageFault,
                    }
                }
            }
        }
    }
}

/// Everything ordinary service keeps from a successful durable control boot.
///
/// The runtime and its fixed scratch stay resident so a normal-runtime carrier can journal
/// verified commands through the live [`radio_hand::control::QuietWindow`]; the boot snapshot
/// still answers the unauthenticated diagnostic without touching flash.
// The UART low-power image has no signed carrier, so only the snapshot is read there.
#[cfg_attr(feature = "host-uart-low-power", allow(dead_code))]
pub(crate) struct ControlReady {
    pub(crate) snapshot: ControlStatusV1,
    pub(crate) first_write: FirstWriteStatus,
    pub(crate) runtime: ControlRuntime,
    pub(crate) scratch: DurableScratch<'static>,
}

/// Pre-radio result after first-write arbitration and permitted control recovery.
pub(crate) enum ControlBootOutcome {
    /// An ordinary control journal was present and completed runtime recovery.
    ControlReady(ControlReady),
    /// Both journals were blank. This creates no authority.
    BlankUncommissioned,
    /// A first-write pair needs physical USB recovery before normal service may start.
    FirstWritePending,
}

/// Run durable WN1 recovery through the one-shot pre-radio owner.
///
/// The consumed reset token is the sole proof for constructing and dropping the short-lived
/// runtime here. This module owns opaque derivation, board facts, fixed scratch, the sustained
/// GPIO0 witness, and the USB-only pre-host first-owner session.
pub(crate) async fn boot_pre_radio_owner<RK, DLY, R, W>(
    _reset: HardwareResetToken,
    owner: &mut V4RadioOwner<RK, DLY>,
    identity: &[u8; IDENTITY_LEN],
    button: Input<'static>,
    rx: &mut R,
    tx: &mut W,
) -> (Input<'static>, Result<ControlBootOutcome, ControlBootError>)
where
    RK: lora_phy::mod_traits::RadioKind,
    DLY: lora_phy::DelayNs,
    R: Read,
    W: Write,
{
    let Some(mut boot_owner): Option<V4BootOwner<'_, RK, DLY>> = owner.take_boot_owner() else {
        return (button, Err(ControlBootError::OwnerUnavailable));
    };

    // These buffers stay outside `main`'s async future. They are initialized only after the
    // owner handoff succeeds, so a repeated call returns before any StaticCell is touched.
    let control_a = CONTROL_SLOT_A.init([0; CONTROL_SLOT_LEN]);
    let control_b = CONTROL_SLOT_B.init([0; CONTROL_SLOT_LEN]);
    let body = CONTROL_BODY.init([0; MAX_DURABLE_BODY]);
    let page = CONTROL_PAGE.init([0; CONTROL_SLOT_LEN]);

    let (node, semantic_key) = derive_control_material(identity);
    let facts = recovery_facts();
    let mut first_write_scratch = first_write_scratch();
    #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
    let resume_pending_a = RESUME_PENDING_A.init([0; CONTROL_SLOT_LEN]);
    #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
    let resume_pending_b = RESUME_PENDING_B.init([0; CONTROL_SLOT_LEN]);
    let first_write_status =
        match inspect_first_write(&mut boot_owner, &mut first_write_scratch, node, &facts) {
            Ok(status) => status,
            Err(error) => {
                let error = match error {
                    radio_hand::control::FirstWriteStorageError::Store { error, .. } => {
                        ControlBootError::FirstWriteStore(error)
                    }
                    _ => ControlBootError::FirstWriteStore(V4FirstWriteStoreError::Pending(
                        crate::commissioning_store::PendingStoreError::Read,
                    )),
                };
                return (button, Err(error));
            }
        };

    match first_write_boot_gate(first_write_status) {
        // A valid ordinary control journal wins stale/corrupt pending state and
        // never opens a physical first-owner carrier.
        FirstWriteBootGate::ControlPresent => {}
        FirstWriteBootGate::StatusOnly => {
            return (button, Ok(ControlBootOutcome::FirstWritePending));
        }
        FirstWriteBootGate::ClaimOnly => {
            #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
            {
                let (button, presence) = crate::physical_presence::observe(button).await;
                let Some(presence) = presence else {
                    return match first_write_status.ordinary_service_eligible() {
                        true => (button, Ok(ControlBootOutcome::BlankUncommissioned)),
                        false => (button, Ok(ControlBootOutcome::FirstWritePending)),
                    };
                };
                return match first_owner_usb_session(
                    presence,
                    &mut boot_owner,
                    rx,
                    tx,
                    node,
                    &facts,
                    first_write_status,
                    &mut first_write_scratch,
                    resume_pending_a,
                    resume_pending_b,
                )
                .await
                {
                    FirstOwnerSessionEnd::NoMutation
                        if first_write_status.ordinary_service_eligible() =>
                    {
                        (button, Ok(ControlBootOutcome::BlankUncommissioned))
                    }
                    FirstOwnerSessionEnd::NoMutation
                    | FirstOwnerSessionEnd::StagedPending
                    | FirstOwnerSessionEnd::StorageFault => {
                        (button, Ok(ControlBootOutcome::FirstWritePending))
                    }
                    FirstOwnerSessionEnd::EntropyUnavailable => {
                        (button, Err(ControlBootError::EntropyUnavailable))
                    }
                };
            }
            #[cfg(feature = "host-uart-low-power")]
            {
                let _ = (rx, tx);
                return match first_write_status.ordinary_service_eligible() {
                    true => (button, Ok(ControlBootOutcome::BlankUncommissioned)),
                    false => (button, Ok(ControlBootOutcome::FirstWritePending)),
                };
            }
        }
    }

    // Safety: the consumed token can only come from the documented board-startup reset path.
    let mut runtime =
        unsafe { ControlRuntime::new_after_hardware_reset(node, semantic_key, facts) };
    let mut scratch = durable_scratch(control_a, control_b, body, page);
    let result = match runtime
        .boot_pre_radio_owner(&mut boot_owner, &mut scratch)
        .await
    {
        Err(error) => Err(ControlBootError::Runtime(error)),
        Ok(BootState::Ready) => {
            let snapshot = ControlStatusV1::from_recovered_state(
                first_write_status,
                runtime
                    .state()
                    .expect("a ready control runtime retains its durable state"),
                runtime.recovered_rollback(),
            );
            Ok(ControlBootOutcome::ControlReady(ControlReady {
                snapshot,
                first_write: first_write_status,
                runtime,
                scratch,
            }))
        }
        // A control pair was valid a few instructions ago. With the exclusive boot owner,
        // a later blank result cannot be an ordinary concurrent update.
        Ok(BootState::Blank) => Err(ControlBootError::FirstWriteStore(
            V4FirstWriteStoreError::Control(crate::control_store::ControlError::Blank),
        )),
    };
    (button, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
    use heapless::Vec;
    #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
    use radio_hand::control::{
        DurableConfig, ManagementCarrierSet, PublicConfigurationV1, ReticulumTransportPolicy,
    };
    #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
    use radio_hand::region::Region;

    #[allow(dead_code)]
    fn boot_owner_trait_shape<RK, DLY>()
    where
        RK: lora_phy::mod_traits::RadioKind + 'static,
        DLY: lora_phy::DelayNs + 'static,
    {
        fn require<
            T: radio_hand::control::AbSlotStore<Error = crate::control_store::ControlError>
                + radio_hand::control::ConfigApplier<Error = V4ConfigError>
                + radio_hand::control::FirstWriteStore<Error = V4FirstWriteStoreError>,
        >() {
        }
        require::<V4BootOwner<'static, RK, DLY>>();
    }

    #[test]
    fn derivation_is_reboot_stable_and_domain_separated() {
        let identity = [0xA5; 64];
        let mut changed_identity = identity;
        changed_identity[0] ^= 1;
        let node_first = derive_hmac(&identity, NODE_ID_DOMAIN);
        let node_second = derive_hmac(&identity, NODE_ID_DOMAIN);
        let semantic_first = derive_hmac(&identity, SEMANTIC_KEY_DOMAIN);
        let semantic_second = derive_hmac(&identity, SEMANTIC_KEY_DOMAIN);
        let (first_node, _) = derive_control_material(&identity);
        let (second_node, _) = derive_control_material(&identity);

        assert_eq!(node_first, node_second);
        assert_eq!(semantic_first, semantic_second);
        assert_ne!(node_first, semantic_first);
        assert_ne!(node_first, derive_hmac(&changed_identity, NODE_ID_DOMAIN));
        assert_ne!(
            semantic_first,
            derive_hmac(&changed_identity, SEMANTIC_KEY_DOMAIN)
        );
        assert_eq!(first_node, second_node);
        assert_eq!(first_node.0, node_first[..16]);
    }

    #[test]
    fn blank_pairs_create_no_control_authority() {
        let blank = [0xFF; CONTROL_SLOT_LEN];
        let identity = [0xA5; IDENTITY_LEN];
        let (node, _) = derive_control_material(&identity);

        let status = radio_hand::control::first_write_status(
            &blank,
            &blank,
            &blank,
            &blank,
            node,
            &recovery_facts(),
        );
        assert!(status.ordinary_service_eligible());
        assert_eq!(first_write_boot_gate(status), FirstWriteBootGate::ClaimOnly);
    }

    #[test]
    fn control_present_never_opens_a_first_owner_session() {
        let status = FirstWriteStatus {
            control: radio_hand::control::PairEvidence::Valid,
            pending: radio_hand::control::PairEvidence::Blank,
        };
        assert_eq!(
            first_write_boot_gate(status),
            FirstWriteBootGate::ControlPresent
        );
    }

    #[test]
    fn corrupt_pending_can_only_reach_the_physical_abandon_session() {
        let status = FirstWriteStatus {
            control: radio_hand::control::PairEvidence::Blank,
            pending: radio_hand::control::PairEvidence::Corrupt,
        };
        assert!(status.actions().permits_abandon());
        assert!(!status.actions().permits_claim());
        assert!(!status.actions().permits_resume());
        assert_eq!(first_write_boot_gate(status), FirstWriteBootGate::ClaimOnly);
    }

    #[test]
    #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
    fn unsupported_pending_profile_is_rejected_without_losing_abandon() {
        let public = PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(906_875_000),
            ReticulumTransportPolicy::new(false, false, 0).unwrap(),
            // This remains portable-valid pending data, but V4 implements only
            // the literal USB carrier and must not make it durable control.
            ManagementCarrierSet::from_mask(0b11).unwrap(),
        )
        .unwrap();
        let configuration = DurableConfig {
            public,
            sealed_credentials: Vec::new(),
        };
        let status = FirstWriteStatus {
            control: radio_hand::control::PairEvidence::Blank,
            pending: radio_hand::control::PairEvidence::Valid,
        };

        assert!(status.actions().permits_resume());
        assert!(status.actions().permits_abandon());
        assert!(!v4_resume_admissible(status, &configuration));
    }

    #[test]
    #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
    fn timeout_or_disconnected_session_preserves_staged_gating() {
        assert_eq!(
            no_mutation_session_end(false),
            FirstOwnerSessionEnd::NoMutation
        );
        assert_eq!(
            no_mutation_session_end(true),
            FirstOwnerSessionEnd::StagedPending
        );
    }

    #[test]
    #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
    fn recovery_facts_are_usb_only_and_local() {
        let facts = recovery_facts();
        assert_eq!(facts.paths().len(), 1);
        let usb = facts.paths()[0];
        assert_eq!(usb.carrier(), ManagementCarrier::Usb);
        assert!(usb.supports_physical_presence());
        assert!(!usb.supports_remote());
        assert!(!usb.remote_is_authenticated());
    }

    #[test]
    #[cfg(feature = "host-uart-low-power")]
    fn recovery_facts_do_not_alias_uart_to_usb() {
        let facts = recovery_facts();
        assert!(facts.paths().is_empty());
    }
}
