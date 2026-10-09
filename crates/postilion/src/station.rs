//! One operator on one radio: bring-up, background tasks, and sending.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use outrider::{
    DEFAULT_MAX_MESSAGE_BYTES, DeliveryAnnounce, LxmfPayload, Verification, announce_delivery,
    receive_direct_with_stamp_cost_and_resource_config, register_delivery,
    send_direct_stamped_with_resource_config,
};
use retinue::endpoint::{Endpoint, ResourceTransferConfig};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use retinue::iface::tulle::drive;
use tokio::sync::mpsc;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};
use tulle::serial::{RNodeSerialLink, SerialPumpConfig};

use crate::held::Held;
use crate::management::{self, ManagementState};
use crate::{Error, Event, Peer, Radio, Sent, StationConfig, StationRadioConfig, profile};

/// Nonces a stamp search may try before giving up.
///
/// Sixteen times the expected count for LXMF's usual cost of 8 (2^8 trials), so an
/// unlucky-but-honest search still completes while a peer demanding an unreasonable cost is
/// refused rather than hanging the station. Each trial is one SHA-256 compression once the
/// derivation is done, so this bound is milliseconds on a host.
const STAMP_ATTEMPT_BUDGET: u64 = 1 << 12;

/// Strict half-duplex radios request one Resource part per turn. A broad window is useful on a
/// fast stream but makes both ends transmit over one another on this family's shared channel.
const RADIO_RESOURCE_REQUEST_WINDOW: usize = 1;

/// One operator, one radio.
pub struct Station {
    endpoint: Arc<Endpoint>,
    identity: PrivateIdentity,
    name: String,
    address: AddressHash,
    management: Arc<Mutex<ManagementState>>,
    radio_config: StationRadioConfig,
    resource_config: ResourceTransferConfig,
    events: mpsc::UnboundedReceiver<Event>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    driver: tokio::task::AbortHandle,
}

impl Station {
    /// Bring up a station with its supplied identity, open the radio, register and announce.
    pub async fn open(config: StationConfig) -> Result<Self, Error> {
        let identity = config.identity.clone();
        let radio_config = StationRadioConfig::from(&config);
        let profile = profile(config.bandwidth_hz);
        let params = tulle::lora::LoRaParams::try_from(profile).map_err(|_| Error::Profile)?;
        let resource_config = radio_resource_config(&params, config.resource_timeout);

        let endpoint = Arc::new(Endpoint::new(identity.clone()));
        // The board carries 255 on the air whatever the host protocol claims, so link traffic
        // is bounded here rather than discovered as a refusal later.
        endpoint.set_link_mtu(255);
        // Pacing derived from the profile: constants picked for fast links fire retries into
        // the answers they are waiting for.
        endpoint.set_link_setup_retry(tulle::pacing::link_setup_retry(&params, false));
        let interface = endpoint.attach_interface();
        // RNS allows a link's first hop the time to carry one MTU at the medium's rate.
        endpoint.set_first_hop_airtime(interface.id(), params.time_on_air(500));

        // `drive` is generic over `tulle::radio_io::PacketRadio` and both serial links
        // implement it, so a personality costs one match arm rather than a second stack.
        let driver = match config.radio {
            Radio::Phy => {
                let mut radio = DirectPhySerialLink::open(
                    &config.port,
                    profile,
                    AirtimeBudget::new(60_000, 60_000),
                    DirectPhySerialConfig {
                        online_timeout: Duration::from_secs(10),
                        transmit_timeout: Duration::from_secs(10),
                        ..DirectPhySerialConfig::default()
                    },
                )
                .map_err(|error| Error::Radio(error.to_string()))?;
                tokio::time::timeout(Duration::from_secs(15), radio.wait_online())
                    .await
                    .map_err(|_| Error::RadioTimeout)?
                    .map_err(|error| Error::Radio(error.to_string()))?;
                tokio::spawn(drive(interface, radio))
            }
            Radio::Rnode => {
                let mut radio = RNodeSerialLink::open(
                    &config.port,
                    params,
                    AirtimeBudget::new(60_000, 60_000),
                    SerialPumpConfig::default(),
                )
                .map_err(|error| Error::Radio(error.to_string()))?;
                tokio::time::timeout(Duration::from_secs(25), radio.wait_online())
                    .await
                    .map_err(|_| Error::RadioTimeout)?
                    .map_err(|error| Error::Radio(error.to_string()))?;
                tokio::spawn(drive(interface, radio))
            }
        };

        let announce = DeliveryAnnounce::named(config.name.as_bytes().to_vec());
        let address = register_delivery(&endpoint, &announce)
            .map_err(|error| Error::Lxmf(error.to_string()))?;

        // Registering makes the destination exist; it does not put it on the air.
        let _ = announce_delivery(&endpoint, &announce);

        let (events_tx, events) = mpsc::unbounded_channel();
        let management = Arc::new(Mutex::new(ManagementState::new(
            config.announce_history_bound,
        )));
        let held = Arc::new(Held::default());
        let mut tasks = Vec::new();

        tasks.push(tokio::spawn({
            let endpoint = Arc::clone(&endpoint);
            let announce = announce.clone();
            let interval = config.announce_interval;
            async move {
                loop {
                    tokio::time::sleep(interval).await;
                    let _ = announce_delivery(&endpoint, &announce);
                }
            }
        }));

        tasks.push(tokio::spawn({
            let endpoint = Arc::clone(&endpoint);
            let management = Arc::clone(&management);
            let events_tx = events_tx.clone();
            let held = Arc::clone(&held);
            async move {
                while let Ok(heard) = endpoint.next_announcement().await {
                    let peer = Peer::from_announce(heard);
                    let released = held.release(&endpoint, peer.destination, now_secs());
                    let fresh = management
                        .lock()
                        .unwrap()
                        .observe(peer.clone(), Instant::now());
                    if fresh && events_tx.send(Event::PeerAppeared(peer)).is_err() {
                        return;
                    }
                    if released
                        .into_iter()
                        .any(|event| events_tx.send(event).is_err())
                    {
                        return;
                    }
                }
            }
        }));

        tasks.push(tokio::spawn({
            let endpoint = Arc::clone(&endpoint);
            let held = Arc::clone(&held);
            async move {
                loop {
                    let Ok(accepted) = endpoint.accept_resource().await else {
                        return;
                    };
                    let event = match receive_direct_with_stamp_cost_and_resource_config(
                        &endpoint,
                        accepted,
                        &held.delivered,
                        DEFAULT_MAX_MESSAGE_BYTES,
                        None,
                        resource_config,
                    )
                    .await
                    {
                        Ok(received) if received.verification == Verification::SourceUnknown => {
                            held.hold(received)
                        }
                        Ok(received) => Some(Event::authenticated_message(received)),
                        Err(error) => Some(Event::Dropped(error.to_string())),
                    };
                    if let Some(event) = event
                        && events_tx.send(event).is_err()
                    {
                        return;
                    }
                }
            }
        }));

        Ok(Self {
            endpoint,
            identity,
            name: config.name,
            address,
            management,
            radio_config,
            resource_config,
            events,
            tasks,
            driver: driver.abort_handle(),
        })
    }

    /// This station's delivery address: what to tell somebody so they can write to you.
    pub fn address(&self) -> AddressHash {
        self.address
    }

    /// Every peer heard so far.
    pub fn peers(&self) -> Vec<Peer> {
        self.management.lock().unwrap().peers()
    }

    /// The first known peer whose address starts with `prefix`.
    pub fn find(&self, prefix: &str) -> Option<Peer> {
        self.management
            .lock()
            .unwrap()
            .peers()
            .into_iter()
            .find(|peer| peer.destination.to_string().starts_with(prefix))
    }

    /// Wait for the next thing worth telling a person about.
    pub async fn next_event(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    /// Announce now, in addition to the cadence.
    pub fn announce(&self) {
        let announce = DeliveryAnnounce::named(self.name.as_bytes().to_vec());
        let _ = announce_delivery(&self.endpoint, &announce);
    }

    /// Send text to the peer whose address begins with `prefix`, waiting up to `patience` for
    /// them to announce if they are not yet known (the owner may be between announces).
    pub async fn send_text(
        &self,
        prefix: &str,
        body: &str,
        patience: Duration,
    ) -> Result<Sent, Error> {
        self.send_bytes(prefix, self.name.as_bytes(), body.as_bytes(), patience)
            .await
    }

    /// Send one arbitrary LXMF title and byte body to a known peer, without this radio
    /// boundary knowing the application's message semantics.
    pub async fn send_bytes(
        &self,
        prefix: &str,
        title: &[u8],
        body: &[u8],
        patience: Duration,
    ) -> Result<Sent, Error> {
        let payload = LxmfPayload::text(now_secs(), title, body);
        self.send_payload(prefix, &payload, patience).await
    }

    /// Send one complete LXMF payload to a known peer.
    ///
    /// This is the field-preserving sibling of [`Self::send_bytes`]. The
    /// caller retains ownership of application field semantics; Postilion only
    /// authenticates and carries the bounded LXMF object.
    pub async fn send_payload(
        &self,
        prefix: &str,
        payload: &LxmfPayload,
        patience: Duration,
    ) -> Result<Sent, Error> {
        let deadline = std::time::Instant::now() + patience;
        let peer = loop {
            if let Some(peer) = self.find(prefix) {
                break peer;
            }
            if std::time::Instant::now() >= deadline {
                return Ok(Sent::NoSuchPeer);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        };

        // A zero budget would make every peer advertising a stamp cost unreachable; peers
        // advertising none still cost no stamp work.
        let receipt = send_direct_stamped_with_resource_config(
            &self.endpoint,
            &self.identity,
            &peer.announce,
            payload,
            self.stamp_seed(),
            STAMP_ATTEMPT_BUDGET,
            self.resource_config,
        )
        .await
        .map_err(|error| Error::Lxmf(error.to_string()))?;
        Ok(Sent::handed_to_radio(receipt))
    }

    /// A fresh starting nonce for a stamp search.
    ///
    /// Random rather than zero, so two stations minting against the same message id do not
    /// walk the same nonces in the same order. The seed need not be secret; it only needs to
    /// differ.
    fn stamp_seed(&self) -> [u8; 32] {
        let mut seed = [0_u8; 32];
        // Losing the OS entropy source costs uniqueness, not correctness: zero is still a
        // valid place to start a search, it just may retread a neighbour's path.
        let _ = getrandom::fill(&mut seed);
        seed
    }

    /// The endpoint underneath, for work this library does not cover yet.
    pub fn endpoint(&self) -> &Arc<Endpoint> {
        &self.endpoint
    }

    /// Capture the management read model with one clock sample.
    pub fn management_snapshot(&self) -> management::ManagementSnapshot {
        self.management_snapshot_at(Instant::now())
    }

    /// Capture the management read model against a caller-supplied instant.
    /// This is useful to make route and observation ages deterministic in tests.
    /// Generation ordering assumes successive captures do not move this instant backwards.
    pub fn management_snapshot_at(&self, captured_at: Instant) -> management::ManagementSnapshot {
        management::ManagementSnapshot::capture(
            &self.endpoint,
            *self.identity.public(),
            self.address,
            &self.name,
            &self.radio_config,
            &self.management,
            captured_at,
        )
    }
}

impl Drop for Station {
    fn drop(&mut self) {
        self.driver.abort();
        for task in &self.tasks {
            task.abort();
        }
    }
}

pub(crate) fn radio_resource_config(
    params: &tulle::lora::LoRaParams,
    timeout: Duration,
) -> ResourceTransferConfig {
    ResourceTransferConfig {
        timeout,
        retry_interval: tulle::pacing::resource_retry(params, false),
        request_window: RADIO_RESOURCE_REQUEST_WINDOW,
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}
