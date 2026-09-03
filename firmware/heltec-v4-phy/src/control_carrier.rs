//! Normal-runtime signed control carrier on the literal USB Serial/JTAG host stream.
//!
//! This is the V4's first authenticated management path. Every frame is a complete
//! `retinue::command` envelope; the verifier restored from the durable owner grants checks
//! target, allowlist, counter window, and signature, and only then does the request reach
//! the resident control runtime, which journals the accepted counter inside a live quiet
//! window before any response exists. The slice observes `Status` only: a verified mutation
//! is journaled and refused as unsupported rather than applied.
//!
//! Refusals are silent by design. An unverified frame earns no reply, matching the FS2
//! posture that a refusal is counted rather than answered; a bench that hears nothing has
//! either the wrong key, a stale counter, or a board that could not quiet its radio.

use lora_phy::DelayNs;
use lora_phy::mod_traits::RadioKind;
use radio_hand::control::{
    ControlVerifier, MAX_CONTROL_RESPONSE_FRAME_LEN, Response, decode_command_frame,
    decode_verified_command, encode_response_frame, restore_control_verifier,
};
use radio_hand::link::HostLink;

use crate::control_boot::ControlReady;
use crate::radio_owner::{V4QuietPreflight, V4RadioOwner};

/// Largest KISS-escaped response the carrier writes.
const MAX_RESPONSE_WIRE: usize = 2 + MAX_CONTROL_RESPONSE_FRAME_LEN * 2;

/// The resident verifier and the rule for when it must be rebuilt.
pub(crate) struct ControlCarrier {
    /// `None` means the in-memory verifier may be ahead of the journal and must be rebuilt
    /// from durable grants and counters before it accepts another envelope.
    verifier: Option<ControlVerifier>,
}

impl ControlCarrier {
    pub(crate) const fn new() -> Self {
        Self { verifier: None }
    }

    /// Serve one complete, tagged command frame from the ordinary modem stream.
    ///
    /// Silence on every refusal. The quiet preflight runs before the verifier advances, so a
    /// board that cannot stop its radio right now never desynchronizes an operator's counter.
    pub(crate) async fn serve<RK, DLY, L>(
        &mut self,
        frame: &[u8],
        owner: &mut V4RadioOwner<RK, DLY>,
        ready: &mut ControlReady,
        host: &mut L,
    ) where
        RK: RadioKind,
        DLY: DelayNs,
        L: HostLink,
    {
        let Ok(command) = decode_command_frame(frame) else {
            return;
        };
        if ready.runtime.is_poisoned() || ready.runtime.reset_pending() {
            return;
        }
        if owner.quiet_preflight() != V4QuietPreflight::Ready {
            return;
        }
        let verifier = match self.verifier.as_mut() {
            Some(verifier) => verifier,
            None => {
                let Some(restored) = ready
                    .runtime
                    .state()
                    .and_then(|state| restore_control_verifier(state).ok())
                else {
                    return;
                };
                self.verifier.insert(restored)
            }
        };
        let Ok(verified) = verifier.verify(command) else {
            return;
        };
        let outcome = match decode_verified_command(&verified) {
            Ok(inbound) => ready
                .runtime
                .observe_status_inbound(owner, &mut ready.scratch, &inbound, ready.first_write)
                .await
                .map(|outcome| Some(outcome.into_value())),
            // Verified but not a node-addressed WN0 request: the counter still becomes
            // durable, and the sender learns nothing beyond silence.
            Err(_) => ready
                .runtime
                .record_verified_command(owner, &mut ready.scratch, &verified)
                .await
                .map(|_| None),
        };
        match outcome {
            Ok(Some(response)) => send_response(&response, host).await,
            Ok(None) => {}
            // The verifier advanced but the journal may not have. Discard it; the next frame
            // rebuilds from whatever the durable state actually says.
            Err(_) => self.verifier = None,
        }
    }
}

/// Writes one tagged response. `HostLink::write_all` flushes the peripheral itself.
async fn send_response<L: HostLink>(response: &Response, host: &mut L) {
    let mut frame = [0_u8; MAX_CONTROL_RESPONSE_FRAME_LEN];
    let mut wire = [0_u8; MAX_RESPONSE_WIRE];
    if let Ok(len) = encode_response_frame(response, &mut frame)
        && let Some(wire_len) = selvage::kiss::encode_into(&frame[..len], &mut wire)
    {
        let _ = host.write_all(&wire[..wire_len]).await;
    }
}
