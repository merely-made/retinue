#![forbid(unsafe_code)]

//! Postilion: the shared radio-host library of the retinue family.
//!
//! A postilion rides mounted on the lead horse, guiding the team from inside the motive
//! apparatus — read against [`outrider`](https://crates.io/crates/outrider), who escorts from
//! alongside. This crate is that riding position: the host-side work every radio-driving
//! application repeats, held once so a face does not have to reimplement it.
//!
//! A [`Station`] is one operator on one radio: a caller-supplied identity, a board on a serial
//! port in either personality, an announce cadence, a table of peers heard, and a stream of
//! [`Event`]s. What it deliberately does **not** have is a user interface or an identity
//! store. It prints nothing, prompts for nothing, reads no private-key files, and decides no
//! policy about how a person is shown a message; those are the application's business, and
//! keeping them out is what lets a terminal, a GUI and a test harness share one implementation.
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use postilion::{Event, Radio, Station, StationConfig};
//! use retinue::identity::PrivateIdentity;
//!
//! let identity = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
//! let mut station = Station::open(StationConfig::new("COM6", "alice", identity))
//! .await?;
//!
//! println!("you are {}", station.address());
//! while let Some(event) = station.next_event().await {
//!     if let Event::Message { from, payload, .. } = event {
//!         println!("[{from}] {}", String::from_utf8_lossy(&payload.content));
//!     }
//! }
//! # Ok(()) }
//! ```

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub mod control;
pub mod management;

use management::{DEFAULT_ANNOUNCE_HISTORY_BOUND, ManagementState};

/// Nonces a stamp search may try before giving up.
///
/// Sixteen times the expected count for LXMF's usual cost of 8 (2^8 trials), so an
/// unlucky-but-honest search still completes while a peer demanding an unreasonable cost is
/// refused rather than hanging the station. Each trial is one SHA-256 compression once the
/// derivation is done, so this bound is milliseconds on a host.
const STAMP_ATTEMPT_BUDGET: u64 = 1 << 12;

/// Qualified whole-transfer deadline for a station's direct radio carriage.
///
/// The two-board Resource receipt uses this deadline. Callers can narrow or widen it through
/// [`StationConfig::resource_timeout`] without replacing the profile-derived retry policy.
pub const DEFAULT_RESOURCE_TIMEOUT: Duration = Duration::from_secs(120);

/// Strict half-duplex radios request one Resource part per turn. A broad window is useful on a
/// fast stream but makes both ends transmit over one another on this family's shared channel.
const RADIO_RESOURCE_REQUEST_WINDOW: usize = 1;

use outrider::{
    DEFAULT_MAX_MESSAGE_BYTES, DeliveryAnnounce, LxmfPayload, announce_delivery,
    delivery_destination, receive_direct_with_stamp_cost_and_resource_config, register_delivery,
    send_direct_stamped_with_resource_config,
};
use retinue::endpoint::{Endpoint, PeerAnnounce, ResourceTransferConfig};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use retinue::iface::tulle::drive;
use tokio::sync::mpsc;
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};
use tulle::serial::{RNodeSerialLink, SerialPumpConfig};

/// Which board personality is on the other end of the cable.
///
/// The same PHY either way: the RNode channel programs the sync word and preamble every other
/// personality in this family uses, so the two are on one another's air. What differs is only
/// which host protocol the board speaks, which is why one library serves both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Radio {
    /// The direct-PHY modem, this family's own host protocol.
    #[default]
    Phy,
    /// The RNode channel, the protocol stock Reticulum clients speak.
    Rnode,
}

impl Radio {
    /// Parse a mode name, for a command line or a config file.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "phy" => Some(Radio::Phy),
            "rnode" => Some(Radio::Rnode),
            _ => None,
        }
    }
}

/// How a station is brought up.
#[derive(Clone, Debug)]
pub struct StationConfig {
    /// Serial port the board is on.
    pub port: String,
    /// Display name, carried in the delivery announce and used as a message title.
    pub name: String,
    /// Channel bandwidth. The rest of the profile is this family's trunk shape.
    pub bandwidth_hz: u32,
    /// Which host protocol the board speaks.
    pub radio: Radio,
    /// How often to re-announce.
    ///
    /// Announcing is how strangers find you and, just as importantly, how they become able to
    /// *verify* you: a receiver cannot check a signature from an identity it has never heard
    /// announced. Thirty seconds is chatty for a shared band and right for a handful of
    /// people in a park; a real deployment wants far less.
    pub announce_interval: Duration,
    /// Maximum announce observations retained for management history.
    pub announce_history_bound: usize,
    /// Maximum time allowed for one complete direct-message Resource transfer.
    ///
    /// Retries and request turns are derived from the selected radio profile; this is the
    /// owner-controlled failure horizon around that mechanism.
    pub resource_timeout: Duration,
    /// The station's Reticulum identity.
    ///
    /// This is supplied by the host. Postilion deliberately neither persists
    /// it nor creates one: a radio application must obtain a scoped credential
    /// from its own authority boundary rather than treating a local file as an
    /// account.
    pub identity: PrivateIdentity,
}

impl StationConfig {
    /// Build a station configuration with this family's ordinary radio defaults.
    pub fn new(
        port: impl Into<String>,
        name: impl Into<String>,
        identity: PrivateIdentity,
    ) -> Self {
        Self {
            port: port.into(),
            name: name.into(),
            bandwidth_hz: 250_000,
            radio: Radio::Phy,
            announce_interval: Duration::from_secs(30),
            announce_history_bound: DEFAULT_ANNOUNCE_HISTORY_BOUND,
            resource_timeout: DEFAULT_RESOURCE_TIMEOUT,
            identity,
        }
    }
}

/// Non-secret radio configuration retained for management snapshots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationRadioConfig {
    pub port: String,
    pub bandwidth_hz: u32,
    pub radio: Radio,
    pub announce_interval: Duration,
    pub announce_history_bound: usize,
}

impl From<&StationConfig> for StationRadioConfig {
    fn from(config: &StationConfig) -> Self {
        Self {
            port: config.port.clone(),
            bandwidth_hz: config.bandwidth_hz,
            radio: config.radio,
            announce_interval: config.announce_interval,
            announce_history_bound: config.announce_history_bound,
        }
    }
}

/// Someone heard announcing.
#[derive(Clone, Debug)]
pub struct Peer {
    /// The peer's delivery destination: the address a person is told and `/peers` lists.
    pub destination: AddressHash,
    /// The stamp cost it advertises, if any.
    pub stamp_cost: Option<u8>,
    /// The display name from its announce, if it carried one.
    pub name: Option<String>,
    /// The whole announce, because that is what sending takes.
    pub announce: PeerAnnounce,
}

impl Peer {
    fn from_announce(announce: PeerAnnounce) -> Self {
        let decoded = DeliveryAnnounce::decode(&announce.app_data).ok();
        Self {
            destination: announce.destination,
            stamp_cost: decoded.as_ref().and_then(|delivery| delivery.stamp_cost),
            name: decoded
                .and_then(|delivery| delivery.display_name)
                .and_then(|bytes| String::from_utf8(bytes).ok()),
            announce,
        }
    }
}

/// Something that happened, for an application to render however it likes.
///
/// The variants differ in size, which clippy notices: a `Peer` carries a whole announce and
/// dwarfs a `Dropped` string. Boxing to even them out would trade a fixed few hundred bytes
/// on a channel that sees a handful of events a minute for a heap allocation on every one,
/// on a station whose whole point is running where memory is scarce. Kept flat on purpose.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum Event {
    /// A peer was heard for the first time.
    PeerAppeared(Peer),
    /// An authenticated message arrived.
    Message {
        /// The authenticated LXMF object identity. Replaying this value is how
        /// an application suppresses the same object after reconnect or restart.
        message_id: [u8; 32],
        /// The sender's delivery destination, not its identity hash: that is what a person
        /// was told and what the peer table lists, so reporting the identity hash would
        /// leave nobody able to match a message to a peer they know.
        from: AddressHash,
        /// The public signing key proven by the LXMF signature and Retinue link.
        /// Applications may address this sender without silently adding it to a contact book.
        sender_identity: [u8; 32],
        /// Whether the authenticated object arrived inline or through a Resource transfer.
        mode: retinue::endpoint::PayloadMode,
        /// The complete authenticated LXMF payload. Applications that own a
        /// typed field, such as Signalman's field-7 voice clip, receive it
        /// without copying field bytes into the title or content body.
        payload: LxmfPayload,
    },
    /// Something arrived and was refused. Surfaced rather than swallowed, because the
    /// commonest cause is a sender this station has never heard announce, and a silent drop
    /// makes that indistinguishable from a dead radio.
    Dropped(String),
}

impl Event {
    /// Preserve every authenticated fact Outrider proved at the host boundary.
    pub fn authenticated_message(received: outrider::ReceivedDirect) -> Self {
        Self::Message {
            message_id: received.message.message_id,
            from: delivery_destination(&received.source_identity),
            sender_identity: *received.source_identity.ed25519_bytes(),
            mode: received.mode,
            payload: received.message.payload,
        }
    }
}

/// What became of a send.
#[derive(Clone, Debug)]
pub enum Sent {
    /// Retinue accepted the object for carriage. This is not an end-recipient
    /// delivery receipt.
    HandedToRadio {
        message_id: [u8; 32],
        mode: retinue::endpoint::PayloadMode,
    },
    /// Nobody matching the prefix announced inside the wait.
    NoSuchPeer,
}

impl Sent {
    /// Report precisely the acceptance Outrider returned, without promoting it
    /// to an end-recipient delivery claim.
    pub fn handed_to_radio(receipt: outrider::DirectReceipt) -> Self {
        Self::HandedToRadio {
            message_id: receipt.message_id,
            mode: receipt.mode,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("radio: {0}")]
    Radio(String),
    #[error("the radio did not come online in time")]
    RadioTimeout,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("lxmf: {0}")]
    Lxmf(String),
    #[error("the requested profile is not a valid LoRa configuration")]
    Profile,
}

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
            async move {
                while let Ok(heard) = endpoint.next_announcement().await {
                    let peer = Peer::from_announce(heard);
                    let fresh = management
                        .lock()
                        .unwrap()
                        .observe(peer.clone(), Instant::now());
                    if fresh && events_tx.send(Event::PeerAppeared(peer)).is_err() {
                        return;
                    }
                }
            }
        }));

        tasks.push(tokio::spawn({
            let endpoint = Arc::clone(&endpoint);
            async move {
                loop {
                    let Ok(accepted) = endpoint.accept_resource().await else {
                        return;
                    };
                    let event = match receive_direct_with_stamp_cost_and_resource_config(
                        &endpoint,
                        accepted,
                        DEFAULT_MAX_MESSAGE_BYTES,
                        None,
                        resource_config,
                    )
                    .await
                    {
                        Ok(received) => Event::authenticated_message(received),
                        Err(error) => Event::Dropped(error.to_string()),
                    };
                    if events_tx.send(event).is_err() {
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
    /// them to announce if they are not yet known.
    ///
    /// Waiting rather than failing is the ordinary park case: an address is something you
    /// were told or met last week, and the owner may simply be between announces.
    pub async fn send_text(
        &self,
        prefix: &str,
        body: &str,
        patience: Duration,
    ) -> Result<Sent, Error> {
        self.send_bytes(prefix, self.name.as_bytes(), body.as_bytes(), patience)
            .await
    }

    /// Send one arbitrary LXMF title and byte body to a known peer.
    ///
    /// Text chat is one consumer of this primitive. Other host applications
    /// can carry their own bounded, authenticated application frames without
    /// teaching this radio boundary their message semantics.
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

        // A budget, not zero. Zero meant a peer that advertises any stamp cost was
        // unreachable: the search was asked for no attempts and failed on the first one, so
        // the only peers this station could talk to were the ones asking nothing. Stamp
        // work is skipped entirely when a peer advertises no cost, so a bench of our own
        // stations still pays nothing for it.
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

fn radio_resource_config(
    params: &tulle::lora::LoRaParams,
    timeout: Duration,
) -> ResourceTransferConfig {
    ResourceTransferConfig {
        timeout,
        retry_interval: tulle::pacing::resource_retry(params, false),
        request_window: RADIO_RESOURCE_REQUEST_WINDOW,
    }
}

/// This family's trunk profile at a chosen bandwidth.
pub fn profile(bandwidth_hz: u32) -> PhyProfile {
    PhyProfile {
        frequency_hz: 906_875_000,
        bandwidth_hz,
        spreading_factor: 8,
        coding_rate_denominator: 5,
        preamble_symbols: 16,
        sync_word: 0x12,
        explicit_header: true,
        crc: true,
        invert_iq: false,
        tx_power_dbm: 17,
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn station_config_requires_a_caller_supplied_identity() {
        let identity = PrivateIdentity::from_secret_bytes(&[0x41; 64]);
        let config = StationConfig::new("COM6", "bench", identity.clone());

        assert_eq!(config.port, "COM6");
        assert_eq!(config.name, "bench");
        assert_eq!(config.resource_timeout, DEFAULT_RESOURCE_TIMEOUT);
        assert_eq!(config.identity.public().hash(), identity.public().hash());
    }

    /// Station carriage must use the same strict half-duplex policy as the qualified
    /// two-board Resource receipt. The fast-link default reproduces collisions on hardware.
    #[test]
    fn radio_resource_policy_is_profile_derived_and_single_turn() {
        let params = tulle::lora::LoRaParams::try_from(profile(250_000)).unwrap();
        let timeout = Duration::from_secs(75);
        let config = radio_resource_config(&params, timeout);

        assert_eq!(config.timeout, timeout);
        assert_eq!(
            config.retry_interval,
            tulle::pacing::resource_retry(&params, false)
        );
        assert_eq!(config.request_window, 1);
    }

    #[test]
    fn radio_modes_parse_and_default_to_the_family_protocol() {
        assert_eq!(Radio::parse("phy"), Some(Radio::Phy));
        assert_eq!(Radio::parse("rnode"), Some(Radio::Rnode));
        assert_eq!(Radio::parse("meshtastic"), None);
        assert_eq!(Radio::default(), Radio::Phy);
    }

    /// The on-air settings a host protocol cannot reach are this family's own, and both
    /// personalities must agree on them or two of our own boards cannot hear each other.
    #[test]
    fn the_trunk_profile_is_the_familys_own_air() {
        let profile = profile(250_000);
        assert_eq!(profile.sync_word, 0x12);
        assert_eq!(profile.preamble_symbols, 16);
        assert_eq!(profile.spreading_factor, 8);
        assert_eq!(profile.coding_rate_denominator, 5);
        assert!(profile.explicit_header && profile.crc && !profile.invert_iq);
    }
}
