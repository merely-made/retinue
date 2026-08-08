//! Postilion: the shared radio-host library of the retinue family.
//!
//! A postilion rides mounted on the lead horse, guiding the team from inside the motive
//! apparatus — read against [`outrider`](https://crates.io/crates/outrider), who escorts from
//! alongside. This crate is that riding position: the host-side work every radio-driving
//! application repeats, held once so a face does not have to reimplement it.
//!
//! A [`Station`] is one operator on one radio: an identity that survives restarts, a board on
//! a serial port in either personality, an announce cadence, a table of peers heard, and a
//! stream of [`Event`]s. What it deliberately does **not** have is a user interface. It
//! prints nothing, prompts for nothing, and decides no policy about how a person is shown a
//! message; that is the application's business, and keeping it out is what lets a terminal,
//! a GUI and a test harness share one implementation.
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use postilion::{Event, Radio, Station, StationConfig};
//!
//! let mut station = Station::open(StationConfig {
//!     port: "COM6".into(),
//!     name: "alice".into(),
//!     radio: Radio::Phy,
//!     ..StationConfig::default()
//! })
//! .await?;
//!
//! println!("you are {}", station.address());
//! while let Some(event) = station.next_event().await {
//!     if let Event::Message { from, body, .. } = event {
//!         println!("[{from}] {}", String::from_utf8_lossy(&body));
//!     }
//! }
//! # Ok(()) }
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Nonces a stamp search may try before giving up.
///
/// Sixteen times the expected count for LXMF's usual cost of 8 (2^8 trials), so an
/// unlucky-but-honest search still completes while a peer demanding an unreasonable cost is
/// refused rather than hanging the station. Each trial is one SHA-256 compression once the
/// derivation is done, so this bound is milliseconds on a host.
const STAMP_ATTEMPT_BUDGET: u64 = 1 << 12;

use outrider::{
    DEFAULT_MAX_MESSAGE_BYTES, DeliveryAnnounce, LxmfPayload, announce_delivery,
    delivery_destination, receive_direct_with_stamp_cost, register_delivery, send_direct_stamped,
};
use retinue::endpoint::{Endpoint, PeerAnnounce};
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
    /// Where the operator's private identity lives. The file is the account.
    pub identity_path: PathBuf,
}

impl Default for StationConfig {
    fn default() -> Self {
        Self {
            port: String::new(),
            name: "me".into(),
            bandwidth_hz: 250_000,
            radio: Radio::Phy,
            announce_interval: Duration::from_secs(30),
            identity_path: PathBuf::from("station.id"),
        }
    }
}

impl StationConfig {
    /// The identity file this family's tools use for `name`, beside the working directory.
    pub fn identity_for(name: &str) -> PathBuf {
        PathBuf::from(format!("park-{name}.id"))
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
        /// The sender's delivery destination, not its identity hash: that is what a person
        /// was told and what the peer table lists, so reporting the identity hash would
        /// leave nobody able to match a message to a peer they know.
        from: AddressHash,
        title: Vec<u8>,
        body: Vec<u8>,
    },
    /// Something arrived and was refused. Surfaced rather than swallowed, because the
    /// commonest cause is a sender this station has never heard announce, and a silent drop
    /// makes that indistinguishable from a dead radio.
    Dropped(String),
}

/// What became of a send.
#[derive(Clone, Debug)]
pub enum Sent {
    /// Handed to the radio.
    Delivered {
        mode: retinue::endpoint::PayloadMode,
    },
    /// Nobody matching the prefix announced inside the wait.
    NoSuchPeer,
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
    peers: Arc<Mutex<Vec<Peer>>>,
    events: mpsc::UnboundedReceiver<Event>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    driver: tokio::task::AbortHandle,
}

impl Station {
    /// Bring up a station: load or mint the identity, open the radio, register and announce.
    pub async fn open(config: StationConfig) -> Result<Self, Error> {
        let identity = load_identity(&config.identity_path)?;
        let profile = profile(config.bandwidth_hz);
        let params = tulle::lora::LoRaParams::try_from(profile).map_err(|_| Error::Profile)?;

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
        let peers: Arc<Mutex<Vec<Peer>>> = Arc::new(Mutex::new(Vec::new()));
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
            let peers = Arc::clone(&peers);
            let events_tx = events_tx.clone();
            async move {
                while let Ok(heard) = endpoint.next_announcement().await {
                    let decoded = DeliveryAnnounce::decode(&heard.app_data).ok();
                    let peer = Peer {
                        destination: heard.destination,
                        stamp_cost: decoded.as_ref().and_then(|a| a.stamp_cost),
                        name: decoded
                            .and_then(|a| a.display_name)
                            .and_then(|bytes| String::from_utf8(bytes).ok()),
                        announce: heard,
                    };
                    let fresh = {
                        let mut table = peers.lock().unwrap();
                        if table.iter().any(|p| p.destination == peer.destination) {
                            false
                        } else {
                            table.push(peer.clone());
                            true
                        }
                    };
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
                    let event = match receive_direct_with_stamp_cost(
                        &endpoint,
                        accepted,
                        DEFAULT_MAX_MESSAGE_BYTES,
                        None,
                    )
                    .await
                    {
                        Ok(received) => Event::Message {
                            from: delivery_destination(&received.source_identity),
                            title: received.message.payload.title,
                            body: received.message.payload.content,
                        },
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
            peers,
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
        self.peers.lock().unwrap().clone()
    }

    /// The first known peer whose address starts with `prefix`.
    pub fn find(&self, prefix: &str) -> Option<Peer> {
        self.peers
            .lock()
            .unwrap()
            .iter()
            .find(|peer| peer.destination.to_string().starts_with(prefix))
            .cloned()
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

        let payload = LxmfPayload::text(now_secs(), self.name.as_bytes(), body.as_bytes().to_vec());
        // A budget, not zero. Zero meant a peer that advertises any stamp cost was
        // unreachable: the search was asked for no attempts and failed on the first one, so
        // the only peers this station could talk to were the ones asking nothing. Stamp
        // work is skipped entirely when a peer advertises no cost, so a bench of our own
        // stations still pays nothing for it.
        let receipt = send_direct_stamped(
            &self.endpoint,
            &self.identity,
            &peer.announce,
            &payload,
            self.stamp_seed(),
            STAMP_ATTEMPT_BUDGET,
        )
        .await
        .map_err(|error| Error::Lxmf(error.to_string()))?;
        Ok(Sent::Delivered { mode: receipt.mode })
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
}

impl Drop for Station {
    fn drop(&mut self) {
        self.driver.abort();
        for task in &self.tasks {
            task.abort();
        }
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

/// Load an operator's identity, or mint and save one.
///
/// A file rather than a keyring because the point is that the address is stable and its owner
/// can see where it lives. It is a private key: the file is the account, and losing it means
/// becoming a stranger to everyone who knew you.
pub fn load_identity(path: &std::path::Path) -> std::io::Result<PrivateIdentity> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() == 64 => {
            let mut seed = [0_u8; 64];
            seed.copy_from_slice(&bytes);
            return Ok(PrivateIdentity::from_secret_bytes(&seed));
        }
        // A file that exists but is the wrong size used to be quietly replaced with a fresh
        // identity. That is the worst available outcome: this is a private key, the address
        // everyone knows this station by is derived from it, and a truncated write or a
        // half-copied file would silently become a new station with no way back. Refuse and
        // say so; renaming it is a decision for whoever owns it.
        Ok(bytes) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{} holds {} bytes, not the 64 an identity needs. Refusing to overwrite \
                     it: this is a private key and replacing it silently would mint a new \
                     station under a new address. Move it aside to start fresh.",
                    path.display(),
                    bytes.len(),
                ),
            ));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        Err(_) => {}
    }

    let mut seed = [0_u8; 64];
    getrandom::fill(&mut seed).expect("system entropy");

    // Write beside the target and rename into place. A direct write that is interrupted --
    // power loss on a solar node, a full disk -- leaves a partial key, which the read above
    // would then refuse forever. Rename is atomic on every platform this runs on, so the
    // file either is the old identity or is the whole new one.
    let temporary = path.with_extension("id.new");
    std::fs::write(&temporary, seed)?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(PrivateIdentity::from_secret_bytes(&seed))
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    /// A station's address is derived from this key. Replacing a damaged one silently would
    /// mint a new station under a new address, and nobody would learn why their peers
    /// stopped answering.
    #[test]
    fn a_damaged_identity_is_refused_rather_than_replaced() {
        let dir = std::env::temp_dir().join("postilion-identity-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("short.id");
        std::fs::write(&path, [0x41_u8; 17]).unwrap();

        let error = load_identity(&path).expect_err("a 17-byte identity must be refused");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            std::fs::read(&path).unwrap().len(),
            17,
            "and the file left exactly as it was found",
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Minting is idempotent: the second call must load what the first wrote, or every
    /// restart would be a new station.
    #[test]
    fn a_minted_identity_is_loaded_again_next_time() {
        let dir = std::env::temp_dir().join("postilion-identity-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("mint.id");
        let _ = std::fs::remove_file(&path);

        let first = load_identity(&path).unwrap();
        let again = load_identity(&path).unwrap();
        assert_eq!(
            first.public().hash(),
            again.public().hash(),
            "the same station across restarts",
        );
        // No debris from the atomic write.
        assert!(!path.with_extension("id.new").exists(), "temporary removed");
        let _ = std::fs::remove_file(&path);
    }

    use super::*;

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

    /// An identity survives a restart, because the address is the account.
    #[test]
    fn an_identity_is_minted_once_and_reloaded_after() {
        let dir = std::env::temp_dir().join(format!("postilion-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("station.id");
        let _ = std::fs::remove_file(&path);

        let first = load_identity(&path).unwrap();
        let again = load_identity(&path).unwrap();
        assert_eq!(
            delivery_destination(first.public()),
            delivery_destination(again.public()),
            "a reloaded identity must keep its address",
        );

        std::fs::remove_file(&path).unwrap();
        let fresh = load_identity(&path).unwrap();
        assert_ne!(
            delivery_destination(first.public()),
            delivery_destination(fresh.public()),
            "and a lost file really does make a stranger",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
