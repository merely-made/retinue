//! Construction boundary for the optional retained resident protocols.
//!
//! Flash reservations happen before this function returns a runtime.  A bad
//! command therefore cannot leave partially constructed protocol state alive.

use core::{fmt::Write, time::Duration};

use embassy_futures::select::{Either3, select3};
use embassy_time::{Duration as EmbassyDuration, Instant, Timer, with_timeout};
use lora_phy::{DelayNs, mod_traits::RadioKind};
use radio_hand::{link::HostLink, resident_command};

use radio_hand::{
    instances::{Config, Runtime},
    resident_wire::{ResidentSetup, SennetKey},
};
use retinue::{
    hash::NameHash,
    identity::PrivateIdentity,
    node::{FreshnessPolicy, Node, PayloadLimits},
};
use selvage::personality::{
    ControllerConfig, CoveragePolicy, InstalledPersonalitySet, PersonalityId,
};
use sennet::{
    flood::{ManagedFloodConfig, RelayDelayWindow},
    instance::{PacketIdLease, SennetInstance, SennetInstanceConfig},
    node::Channel,
    node_info::NodeDirectoryConfig,
    transport::ChannelKey,
};
use tucket::{
    identity::LocalIdentity,
    instance::{Instance, InstanceConfig},
    node::{Node as TucketNode, NodeCapacity},
};

use crate::{
    radio_owner::{V4QuietPreflight, V4RadioOwner},
    store::{ReservationError, SettingsStore},
};

const ANNOUNCE_LEASE: u64 = 65_536;

pub(crate) struct ResidentState {
    pub runtime: Runtime,
    pub announce: retinue::announce::TimebaseGenerator,
}

const HOST_CHUNK: usize = 32;

/// Serve the retained runtime as the only post-setup radio loop.
///
/// The select races only an armed IRQ waiter, a small host read, and the
/// runtime's next deadline.  Collection happens after the IRQ wins and before
/// any profile transition, so no command can discard a completed RF frame.
pub(crate) async fn serve<RK, DLY, L>(
    mut state: ResidentState,
    owner: &mut V4RadioOwner<RK, DLY>,
    host: &mut L,
    initial_host_bytes: &[u8],
) -> !
where
    RK: RadioKind,
    DLY: DelayNs,
    L: HostLink,
{
    let mut stream = resident_command::Stream::new();
    let mut host_bytes = [0_u8; HOST_CHUNK];
    let mut radio_bytes = [0_u8; 255];
    let mut initial_cursor = 0;
    loop {
        match owner.ensure_rx().await {
            Ok(_) => owner.radio_online(),
            Err(_) => esp_hal::system::software_reset(),
        }

        // Polling is bounded even at home: Retinue's scheduled work and announce
        // cadence have no controller deadline while USB is silent.
        let now = Instant::now().as_millis();
        let deadline = Timer::at(Instant::from_millis(
            state
                .runtime
                .next_deadline()
                .unwrap_or(u64::MAX)
                .min(now.saturating_add(25)),
        ));
        let radio = async {
            if crate::wake_input::radio_is_high() {
                Ok(())
            } else {
                owner.wait_rx_irq().await
            }
        };
        let selected = if initial_cursor < initial_host_bytes.len() {
            let count = HOST_CHUNK.min(initial_host_bytes.len() - initial_cursor);
            host_bytes[..count]
                .copy_from_slice(&initial_host_bytes[initial_cursor..initial_cursor + count]);
            initial_cursor += count;
            drop(radio);
            Either3::First(Ok(count))
        } else {
            select3(host.read(&mut host_bytes), radio, deadline).await
        };
        match selected {
            Either3::Second(Ok(())) => {
                match owner.collect(&mut radio_bytes).await {
                    Ok(Some(frame)) => {
                        owner.note_radio_frame(&frame);
                        let now = Instant::now().as_millis();
                        // Malformed RF is a protocol report, not a radio-custody fault.
                        match state.runtime.ingest(now, &radio_bytes[..frame.len]) {
                            Ok(report) => {
                                report_to_host(host, &report, now, state.runtime.next_deadline())
                                    .await
                            }
                            Err(
                                radio_hand::instances::Error::ClockRegression
                                | radio_hand::instances::Error::RecoveryRequired,
                            ) => esp_hal::system::software_reset(),
                            // RF is untrusted input. Codec/protocol refusals never imply uncertain PHY custody.
                            Err(_) => {
                                let report = state.runtime.take_report();
                                report_to_host(host, &report, now, state.runtime.next_deadline())
                                    .await;
                                diagnostic(
                                    host,
                                    "resident rf refused",
                                    state.runtime.next_deadline(),
                                )
                                .await
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(_) => esp_hal::system::software_reset(),
                }
            }
            Either3::Second(Err(_)) => esp_hal::system::software_reset(),
            Either3::First(Ok(length)) => {
                owner.note_host_activity();
                for byte in host_bytes[..length].iter().copied() {
                    match stream.push(byte) {
                        resident_command::Event::Pending => {}
                        resident_command::Event::Rejected => {
                            diagnostic(host, "resident refused", state.runtime.next_deadline())
                                .await;
                        }
                        resident_command::Event::Complete(command) => {
                            drain_active_rx(&mut state, owner, host, &mut radio_bytes).await;
                            let now = Instant::now().as_millis();
                            let mut iv = [0_u8; 16];
                            owner
                                .resident_random(&mut iv)
                                .unwrap_or_else(|_| esp_hal::system::software_reset());
                            let result = command_into_runtime(&mut state, now, command, iv);
                            match result {
                                Ok(Some(step)) => {
                                    apply_step(&mut state, owner, host, step, now).await;
                                }
                                Ok(None) => {
                                    status_to_host(host, &state, now).await;
                                }
                                Err(()) => {
                                    let report = state.runtime.take_report();
                                    report_to_host(
                                        host,
                                        &report,
                                        now,
                                        state.runtime.next_deadline(),
                                    )
                                    .await;
                                    diagnostic(
                                        host,
                                        "resident refused",
                                        state.runtime.next_deadline(),
                                    )
                                    .await
                                }
                            }
                        }
                    }
                }
            }
            Either3::First(Err(_)) => {}
            Either3::Third(()) => {}
        }

        drain_active_rx(&mut state, owner, host, &mut radio_bytes).await;
        let now = Instant::now().as_millis();
        let mut iv = [0_u8; 16];
        owner
            .resident_random(&mut iv)
            .unwrap_or_else(|_| esp_hal::system::software_reset());
        let step = state
            .runtime
            .tick(now, || iv)
            .unwrap_or_else(|_| esp_hal::system::software_reset());
        apply_step(&mut state, owner, host, step, now).await;

        let now = Instant::now().as_millis();
        let announce = if state
            .runtime
            .active()
            .is_some_and(|a| a.instance == PersonalityId(0))
            && state.runtime.retinue().node().announce_due(now)
        {
            let mut nonce = [0_u8; 5];
            owner
                .resident_random(&mut nonce)
                .unwrap_or_else(|_| esp_hal::system::software_reset());
            let ordinal = state
                .announce
                .next(now / 1_000)
                .unwrap_or_else(|_| esp_hal::system::software_reset());
            Some(
                retinue::announce::AnnounceBlob::mint(nonce, ordinal)
                    .unwrap_or_else(|_| esp_hal::system::software_reset()),
            )
        } else {
            None
        };
        let report = state
            .runtime
            .poll(now, announce.as_ref())
            .unwrap_or_else(|_| esp_hal::system::software_reset());
        report_to_host(host, &report, now, state.runtime.next_deadline()).await;
        // A high edge after poll belongs to the current activation. Drain and
        // rearm it before asking Runtime whether a physical TX may begin.
        drain_active_rx(&mut state, owner, host, &mut radio_bytes).await;
        owner
            .ensure_rx()
            .await
            .unwrap_or_else(|_| esp_hal::system::software_reset());
        if let Some(tx) = state
            .runtime
            .begin_tx(Instant::now().as_millis())
            .unwrap_or_else(|_| esp_hal::system::software_reset())
        {
            let now = Instant::now().as_millis();
            let remaining = tx
                .deadline
                .checked_sub(now)
                .unwrap_or_else(|| esp_hal::system::software_reset());
            let budget = remaining.min(state.runtime.tx_budget_ms());
            let sent = match with_timeout(
                EmbassyDuration::from_millis(budget),
                owner.executive().transmit(&tx.frame),
            )
            .await
            {
                Ok(code) => code == selvage::TX_ACCEPTED,
                Err(_) => esp_hal::system::software_reset(),
            };
            let report = state
                .runtime
                .complete_tx(Instant::now().as_millis(), tx.id, sent)
                .unwrap_or_else(|_| esp_hal::system::software_reset());
            report_to_host(host, &report, now, state.runtime.next_deadline()).await;
        }
    }
}

fn command_into_runtime(
    state: &mut ResidentState,
    now: u64,
    command: resident_command::Command,
    iv: [u8; 16],
) -> Result<Option<radio_hand::instances::Step>, ()> {
    use resident_command::Command;
    match command {
        Command::Status => Ok(None),
        Command::Excursion {
            target,
            duration_ms,
            allow_loss,
        } => {
            let step = state
                .runtime
                .request(
                    now,
                    selvage::personality::Excursion {
                        target: PersonalityId(target),
                        duration_ms,
                        interruption: if allow_loss {
                            selvage::personality::InterruptionPolicy::AllowSessionLoss
                        } else {
                            selvage::personality::InterruptionPolicy::ResumableOnly
                        },
                    },
                    || iv,
                )
                .map_err(|_| ())?;
            Ok(Some(step))
        }
        Command::Cancel => {
            let step = state.runtime.cancel(now, || iv).map_err(|_| ())?;
            Ok(Some(step))
        }
        Command::SennetText {
            destination,
            hops,
            want_ack,
            text,
        } => {
            let report = state
                .runtime
                .send_sennet(
                    now,
                    sennet::transport::Header {
                        destination,
                        source: 0,
                        packet_id: 0,
                        hop_limit: hops,
                        want_ack,
                        via_mqtt: false,
                        hop_start: hops,
                        channel_hash: 0,
                        next_hop: 0,
                        relay_node: 0,
                    },
                    &text,
                )
                .map_err(|_| ())?;
            Ok(Some(radio_hand::instances::Step {
                transition: None,
                report,
            }))
        }
        Command::TucketText {
            to,
            timestamp,
            ttl_ms,
            attempts,
            flood_last,
            text,
        } => {
            let until = now.checked_add(ttl_ms).ok_or(())?;
            state
                .runtime
                .send_tucket(
                    now,
                    to,
                    &text,
                    tucket::node::TextRetryPolicy::new(attempts, flood_last).ok_or(())?,
                    tucket::instance::SendTiming {
                        timestamp,
                        expires_at: until,
                        allowed_until: until,
                    },
                )
                .map_err(|_| ())?;
            Ok(Some(radio_hand::instances::Step {
                transition: None,
                report: state.runtime.take_report(),
            }))
        }
        Command::TucketAdvert { timestamp, data } => {
            let report = state
                .runtime
                .advertise_tucket(now, timestamp, &data)
                .map_err(|_| ())?;
            Ok(Some(radio_hand::instances::Step {
                transition: None,
                report,
            }))
        }
    }
}

async fn drain_active_rx<RK: RadioKind, DLY: DelayNs, L: HostLink>(
    state: &mut ResidentState,
    owner: &mut V4RadioOwner<RK, DLY>,
    host: &mut L,
    bytes: &mut [u8],
) {
    if !crate::wake_input::radio_is_high() {
        return;
    }
    let Some(frame) = owner
        .collect(bytes)
        .await
        .unwrap_or_else(|_| esp_hal::system::software_reset())
    else {
        return;
    };
    owner.note_radio_frame(&frame);
    let now = Instant::now().as_millis();
    match state.runtime.ingest(now, &bytes[..frame.len]) {
        Ok(report) => report_to_host(host, &report, now, state.runtime.next_deadline()).await,
        Err(
            radio_hand::instances::Error::ClockRegression
            | radio_hand::instances::Error::RecoveryRequired,
        ) => esp_hal::system::software_reset(),
        Err(_) => {
            let report = state.runtime.take_report();
            report_to_host(host, &report, now, state.runtime.next_deadline()).await;
            diagnostic(host, "resident rf refused", state.runtime.next_deadline()).await
        }
    }
}

async fn apply_step<RK: RadioKind, DLY: DelayNs, L: HostLink>(
    state: &mut ResidentState,
    owner: &mut V4RadioOwner<RK, DLY>,
    host: &mut L,
    step: radio_hand::instances::Step,
    now: u64,
) {
    if let Some(transition) = step.transition {
        if owner.quiet_preflight() == V4QuietPreflight::CompletedFramePending {
            // Runtime has paused the old activation. A late edge belongs to the
            // transition blackout, so collect it before quiet entry but never
            // hand it to the newly selected protocol.
            let mut dropped = [0_u8; 255];
            if let Some(frame) = owner
                .collect(&mut dropped)
                .await
                .unwrap_or_else(|_| esp_hal::system::software_reset())
            {
                owner.note_radio_frame(&frame);
                diagnostic(
                    host,
                    "resident transition rf dropped",
                    Some(transition.deadline),
                )
                .await;
            }
        }
        owner
            .ensure_rx()
            .await
            .unwrap_or_else(|_| esp_hal::system::software_reset());
        if owner.quiet_preflight() != V4QuietPreflight::Ready {
            esp_hal::system::software_reset();
        }
        let remaining = transition
            .deadline
            .checked_sub(Instant::now().as_millis())
            .unwrap_or_else(|| esp_hal::system::software_reset());
        let profile = state
            .runtime
            .profile(transition.to)
            .unwrap_or_else(|_| esp_hal::system::software_reset());
        if !matches!(
            with_timeout(
                EmbassyDuration::from_millis(remaining),
                owner.apply_excursion_profile(&profile)
            )
            .await,
            Ok(Ok(()))
        ) {
            esp_hal::system::software_reset();
        }
        let report = state
            .runtime
            .acknowledge(
                Instant::now().as_millis(),
                transition.id,
                selvage::personality::Acknowledgement::Completed,
            )
            .unwrap_or_else(|_| esp_hal::system::software_reset());
        // USB reporting is deliberately after confirmed RX and cannot postpone acknowledgement.
        report_to_host(host, &report, now, state.runtime.next_deadline()).await;
    }
    report_to_host(host, &step.report, now, state.runtime.next_deadline()).await;
}

async fn report_to_host<L: HostLink>(
    host: &mut L,
    report: &radio_hand::instances::Report,
    now: u64,
    deadline: Option<u64>,
) {
    if report.events.is_empty() && report.overflowed == 0 {
        return;
    }
    let line = report.format(now);
    diagnostic(host, &line, deadline).await;
}

async fn status_to_host<L: HostLink>(host: &mut L, state: &ResidentState, now: u64) {
    let mut line = heapless::String::<512>::new();
    let (stack_sampled_bytes, stack_capacity, stack_sample_valid) =
        crate::heap::sampled_stack_usage();
    let formatted = writeln!(
        line,
        "resident status now={} state={:?} active={:?} deadline={:?}",
        now,
        state.runtime.state(),
        state.runtime.active(),
        state.runtime.next_deadline(),
    );
    if formatted.is_ok() {
        diagnostic(host, &line, state.runtime.next_deadline()).await;
    }
    line.clear();
    let formatted = writeln!(
        line,
        "resident memory now={} heap_capacity={} heap_allocator_used={} heap_allocator_used_peak={} heap_requested={} heap_requested_peak={} heap_alloc_failures={} stack_measurement=sampled_sp_not_high_water stack_sp=0x{:x} stack_sampled_bytes={} stack_capacity={} stack_sample_valid={}",
        now,
        crate::heap::capacity(),
        crate::heap::allocator_used(),
        crate::heap::allocator_used_peak(),
        crate::heap::requested(),
        crate::heap::requested_peak(),
        crate::heap::allocation_failures(),
        crate::heap::sampled_stack_pointer(),
        stack_sampled_bytes,
        stack_capacity,
        stack_sample_valid,
    );
    if formatted.is_ok() {
        diagnostic(host, &line, state.runtime.next_deadline()).await;
    }
}

async fn diagnostic<L: HostLink>(host: &mut L, line: &str, deadline: Option<u64>) {
    let now = Instant::now().as_millis();
    let available = deadline
        .map(|at| at.saturating_sub(now).saturating_sub(1))
        .unwrap_or(20)
        .min(20);
    if available != 0 {
        let _ = with_timeout(
            EmbassyDuration::from_millis(available),
            host.write_diagnostic(line.as_bytes()),
        )
        .await;
    }
}

#[derive(Debug)]
pub(crate) enum BuildError {
    MissingIdentity,
    Storage(ReservationError),
    Configuration,
    Quiet,
}

/// Construct the three cores only from a verified USB setup and already-durable
/// reservations. Identity comes from stored board settings, never USB bytes.
pub(crate) fn build(
    now: u64,
    setup: ResidentSetup,
    stored_identity: Option<[u8; 64]>,
    store: &mut SettingsStore,
) -> Result<ResidentState, BuildError> {
    let identity = stored_identity.ok_or(BuildError::MissingIdentity)?;
    let packet = store
        .reserve_packet_ids(setup.packet_lease_count)
        .map_err(BuildError::Storage)?;
    let announce = store
        .reserve_announce_lease(ANNOUNCE_LEASE)
        .map_err(BuildError::Storage)?;

    let key = match setup.sennet_key {
        SennetKey::Aes128(key) => ChannelKey::Aes128(key),
        SennetKey::Aes256(key) => ChannelKey::Aes256(key),
    };
    let sennet = SennetInstance::new(
        SennetInstanceConfig {
            channel: Channel {
                hash: setup.sennet_channel,
                key,
            },
            flood: ManagedFloodConfig {
                channel_hash: setup.sennet_channel,
                relay_node: (setup.sennet_source & 0xff) as u8,
                seen_capacity: 16,
                delay: RelayDelayWindow::new(Duration::ZERO, Duration::ZERO)
                    .map_err(|_| BuildError::Configuration)?,
            },
            directory: NodeDirectoryConfig {
                capacity: 8,
                id_limit: 32,
                long_name_limit: 64,
                short_name_limit: 16,
            },
            pending_ttl: setup.frame_ttl_ms,
        },
        PacketIdLease::new(
            setup.sennet_source,
            packet.start(),
            packet.end(),
            packet.start(),
        )
        .map_err(|_| BuildError::Configuration)?,
    )
    .map_err(|_| BuildError::Configuration)?;
    let tucket = Instance::new(
        TucketNode::with_capacity(
            LocalIdentity::from_seed(setup.tucket_identity_seed),
            false,
            NodeCapacity::new(8, 32).map_err(|_| BuildError::Configuration)?,
        )
        .map_err(|_| BuildError::Configuration)?,
        InstanceConfig::new(2, 1_000).ok_or(BuildError::Configuration)?,
    );
    let ids = [PersonalityId(0), PersonalityId(1), PersonalityId(2)];
    let controller = ControllerConfig {
        home: PersonalityId(setup.home),
        pin: setup.pin.map(PersonalityId),
        installed: InstalledPersonalitySet::new(&ids).map_err(|_| BuildError::Configuration)?,
        coverage: if setup.require_coverage {
            CoveragePolicy::RequireCoverage
        } else {
            CoveragePolicy::AllowGap
        },
        max_excursion_ms: setup.max_excursion_ms,
        return_budget_ms: setup.return_budget_ms,
        max_defer_ms: setup.max_defer_ms,
        transition_timeout_ms: setup.transition_timeout_ms,
    };
    let name = NameHash::from_bytes(setup.retinue_name_hash);
    let retinue = Node::<8, 4, 1, 4>::new_with_payload_limits(
        PrivateIdentity::from_secret_bytes(&identity),
        name,
        PayloadLimits {
            max_ingress_bytes: 255,
            max_app_data: 32,
            max_link_payload: 160,
            max_outbound_resource: 1_024,
            max_resource_parts: 8,
        },
    )
    .with_freshness_policy(FreshnessPolicy {
        max_destinations: 8,
        max_blobs_per_destination: 2,
        retention: 604_800_000,
    })
    .map_err(|_| BuildError::Configuration)?;
    let runtime = Runtime::new(
        now,
        Config {
            controller,
            profiles: setup.profiles,
            tx_budget_ms: setup.tx_budget_ms,
            frame_ttl_ms: setup.frame_ttl_ms,
        },
        retinue,
        sennet,
        tucket,
    )
    .map_err(|_| BuildError::Configuration)?;
    let announce = retinue::announce::TimebaseGenerator::firmware_lease(
        announce.floor(),
        announce.reserved_through(),
    )
    .map_err(|_| BuildError::Configuration)?;
    Ok(ResidentState { runtime, announce })
}
