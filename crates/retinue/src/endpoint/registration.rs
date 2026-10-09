//! Registered destinations, their ratchets, and the announces that advertise them.

use alloc::vec::Vec;

use std::io;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::announce::{self, ANNOUNCE_NONCE_LEN, AnnounceBlob, TimebaseGenerator};
use crate::destination::DestinationName;
use crate::hash::{AddressHash, NameHash};
use crate::identity::KEY_LEN;
use crate::packet::Packet;
use crate::ratchet::RatchetStore;

use super::entropy::fill_random;
use super::runtime::Endpoint;
use super::shared::Shared;
use super::single::ProofStrategy;

/// A destination this endpoint accepts links on.
pub(super) struct Registered {
    pub(super) dest: AddressHash,
    pub(super) kind: RegistrationKind,
    /// The name and app data this destination announced with, kept to answer path requests.
    pub(super) name: DestinationName,
    pub(super) app_data: Vec<u8>,
    /// Builds the app data afresh at each path response instead, when set.
    pub(super) app_data_source: Option<AppDataSource>,
    /// Receive ratchets, owned here and rotated at announce. Shared so inbound trial
    /// decryption never copies the store.
    pub(super) ratchets: Option<Arc<RatchetStore>>,
    /// Refuse single packets encrypted to the identity key rather than a ratchet, so an
    /// advertised ratchet cannot be silently downgraded. Off by default, as in RNS.
    pub(super) enforce_ratchets: bool,
    pub(super) proof_strategy: ProofStrategy,
}

/// Builds a destination's app data at a given host time, in Unix seconds.
pub(super) type AppDataSource = Arc<dyn Fn(u64) -> Vec<u8> + Send + Sync>;

/// The host's durable store for a ratcheted destination's signed snapshot.
pub(super) type RatchetPersistence =
    alloc::boxed::Box<dyn FnMut(AddressHash, &[u8]) -> io::Result<()> + Send>;

#[derive(Clone, Copy)]
pub(super) enum RegistrationKind {
    BestEffort,
    Reliable,
    Resource,
}

impl Shared {
    pub(super) fn registered_ratchets(&self, dest: AddressHash) -> Option<Arc<RatchetStore>> {
        self.registered
            .lock()
            .unwrap()
            .iter()
            .find(|registration| registration.dest == dest)?
            .ratchets
            .clone()
    }

    /// The ratchet public key an announce for `dest` carries at `now_seconds`, rotating the
    /// store first when its interval has passed (`Destination.py` 228-242).
    ///
    /// Persist before advertise: a rotated store is handed to the persistence hook before it
    /// is installed. If the hook fails, the previous epoch, which is already persisted, stays
    /// advertised and the next announce retries.
    pub(super) fn advertised_ratchet(
        &self,
        dest: AddressHash,
        now_seconds: u64,
    ) -> Option<[u8; KEY_LEN]> {
        let store = self.registered_ratchets(dest)?;
        if !store.rotation_due(now_seconds as f64) {
            return store.current_public();
        }
        let mut persistence = self.ratchet_persistence.lock().unwrap();
        // Re-read under the rotation lock: a concurrent announce may have rotated already.
        let store = self.registered_ratchets(dest)?;
        if store.rotation_due(now_seconds as f64)
            && let Ok(next) = self.commit_ratchets(
                &mut persistence,
                dest,
                RatchetStore::clone(&store),
                now_seconds,
            )
        {
            let public = next.current_public();
            if let Some(registration) = self
                .registered
                .lock()
                .unwrap()
                .iter_mut()
                .find(|registration| registration.dest == dest)
            {
                registration.ratchets = Some(next);
            }
            return public;
        }
        store.current_public()
    }

    /// Rotate `ratchets` if due, then hand its identity-signed snapshot to the host before the
    /// caller installs it. The caller holds `persistence`'s lock until it has installed the
    /// result, so the installed store is always the one most recently persisted.
    fn commit_ratchets(
        &self,
        persistence: &mut Option<RatchetPersistence>,
        dest: AddressHash,
        mut ratchets: RatchetStore,
        now_seconds: u64,
    ) -> io::Result<Arc<RatchetStore>> {
        let now = now_seconds as f64;
        if ratchets.rotation_due(now) {
            let mut secret = [0; KEY_LEN];
            fill_random(&mut secret);
            ratchets
                .rotate_if_due(secret, now)
                .expect("whole host seconds are a finite timestamp");
        }
        if let Some(persist) = persistence {
            persist(dest, &ratchets.encode_snapshot(&self.identity))?;
        }
        Ok(Arc::new(ratchets))
    }

    /// Build one locally owned announce from a typed blob. The generator is keyed by the
    /// derived destination rather than the endpoint identity, so two registered names do not
    /// consume one another's ordinal space.
    pub(super) fn build_announce_at(
        &self,
        name: &DestinationName,
        ratchet: Option<&[u8; crate::announce::RATCHET_LEN]>,
        app_data: &[u8],
        source_seconds: u64,
    ) -> Packet {
        let destination = name.destination_hash(self.identity.public());
        let blob = self.next_announce_blob(destination, source_seconds);
        announce::build(&self.identity, name.name_hash(), &blob, ratchet, app_data)
    }

    fn next_announce_blob(&self, destination: AddressHash, source_seconds: u64) -> AnnounceBlob {
        let ordinal = self
            .announce_timebases
            .lock()
            .unwrap()
            .entry(destination)
            .or_insert_with(|| {
                TimebaseGenerator::host(0)
                    .expect("the host announce timebase starts within its wire range")
            })
            .next(source_seconds)
            .expect("host announce timebase is representable and not exhausted");
        let mut nonce = [0_u8; ANNOUNCE_NONCE_LEN];
        fill_random(&mut nonce);
        AnnounceBlob::mint(nonce, ordinal)
            .expect("TimebaseGenerator only returns timebases representable on the announce wire")
    }
}

impl Endpoint {
    /// Register a destination to accept best-effort links on, and announce it. Accept these
    /// with [`accept`](Self::accept).
    pub fn register(&self, name: DestinationName, app_data: &[u8]) {
        self.register_with(name, app_data, RegistrationKind::BestEffort, None);
    }

    /// Register a best-effort-link destination that also receives ratcheted single packets.
    ///
    /// The endpoint owns `ratchets` (empty or restored) and rotates it when an announce finds
    /// the epoch due, persisting each snapshot through
    /// [`set_ratchet_persistence`](Self::set_ratchet_persistence) before advertising it.
    /// Fails if that hook refuses the initial snapshot.
    pub fn register_with_ratchets(
        &self,
        name: DestinationName,
        app_data: &[u8],
        ratchets: RatchetStore,
    ) -> io::Result<()> {
        self.register_ratcheted(name, app_data, RegistrationKind::BestEffort, ratchets)
    }

    /// Register a destination to accept **reliable** links on (Channel/Buffer with proof
    /// acks, for lossy interfaces) and announce it. Accept these with
    /// [`accept_reliable`](Self::accept_reliable).
    pub fn register_reliable(&self, name: DestinationName, app_data: &[u8]) {
        self.register_with(name, app_data, RegistrationKind::Reliable, None);
    }

    /// Register a destination that accepts resource sessions, then announce it.
    pub fn register_resource(&self, name: DestinationName, app_data: &[u8]) {
        self.register_with(name, app_data, RegistrationKind::Resource, None);
    }

    /// Register a resource destination that also receives ratcheted single packets. Ratchet
    /// ownership and persistence are as for
    /// [`register_with_ratchets`](Self::register_with_ratchets).
    pub fn register_resource_with_ratchets(
        &self,
        name: DestinationName,
        app_data: &[u8],
        ratchets: RatchetStore,
    ) -> io::Result<()> {
        self.register_ratcheted(name, app_data, RegistrationKind::Resource, ratchets)
    }

    /// Install the host's durable store for ratchet snapshots, `(destination, snapshot)`.
    /// Install it before registering ratcheted destinations.
    ///
    /// It receives the identity-signed snapshot (see [`RatchetStore::restore`]) on register,
    /// update, and every rotation, before any announce carries the new ratchet; an error keeps
    /// the previous one advertised. It runs with rotation locked, so it must not call back
    /// into this endpoint. Without a hook, ratchets are lost on restart.
    pub fn set_ratchet_persistence(
        &self,
        persist: impl FnMut(AddressHash, &[u8]) -> io::Result<()> + Send + 'static,
    ) {
        *self.shared.ratchet_persistence.lock().unwrap() = Some(alloc::boxed::Box::new(persist));
    }

    /// The id of the ratchet a registered destination currently advertises, if it has
    /// ratchets.
    pub fn current_ratchet_id(&self, name: &DestinationName) -> Option<NameHash> {
        self.shared
            .registered_ratchets(name.destination_hash(self.shared.identity.public()))?
            .current_id()
    }

    fn register_ratcheted(
        &self,
        name: DestinationName,
        app_data: &[u8],
        kind: RegistrationKind,
        ratchets: RatchetStore,
    ) -> io::Result<()> {
        let dest = name.destination_hash(self.shared.identity.public());
        let ratchets = {
            let mut persistence = self.shared.ratchet_persistence.lock().unwrap();
            self.shared.commit_ratchets(
                &mut persistence,
                dest,
                ratchets,
                host_announce_seconds(),
            )?
        };
        self.register_with(name, app_data, kind, Some(ratchets));
        Ok(())
    }

    fn register_with(
        &self,
        name: DestinationName,
        app_data: &[u8],
        kind: RegistrationKind,
        ratchets: Option<Arc<RatchetStore>>,
    ) {
        let dest = name.destination_hash(self.shared.identity.public());
        self.shared.registered.lock().unwrap().push(Registered {
            dest,
            kind,
            name: name.clone(),
            app_data: app_data.to_vec(),
            app_data_source: None,
            ratchets,
            enforce_ratchets: false,
            proof_strategy: ProofStrategy::None,
        });
        self.announce(&name, app_data);
    }

    /// Replace a registered destination's receive-ratchet state, persist it through the
    /// [`set_ratchet_persistence`](Self::set_ratchet_persistence) hook, and announce its
    /// current public key. The endpoint owns the store from here on.
    pub fn update_ratchets(
        &self,
        name: &DestinationName,
        ratchets: RatchetStore,
    ) -> io::Result<()> {
        let dest = name.destination_hash(self.shared.identity.public());
        let not_registered =
            || io::Error::new(io::ErrorKind::NotFound, "destination is not registered");
        let app_data = {
            let mut persistence = self.shared.ratchet_persistence.lock().unwrap();
            if !self
                .shared
                .registered
                .lock()
                .unwrap()
                .iter()
                .any(|registration| registration.dest == dest)
            {
                return Err(not_registered());
            }
            let ratchets = self.shared.commit_ratchets(
                &mut persistence,
                dest,
                ratchets,
                host_announce_seconds(),
            )?;
            let mut registered = self.shared.registered.lock().unwrap();
            let registration = registered
                .iter_mut()
                .find(|registration| registration.dest == dest)
                .ok_or_else(not_registered)?;
            registration.ratchets = Some(ratchets);
            registration.app_data.clone()
        };
        self.announce(name, &app_data);
        Ok(())
    }

    /// Build a registered destination's app data afresh for every path response, from the
    /// host clock in Unix seconds, in place of the bytes it was registered with: RNS's
    /// callable default app data (`Destination.py` 290-296, 678-686). Announces still carry
    /// what the caller passes to [`announce`](Self::announce).
    pub fn set_app_data_source(
        &self,
        name: &DestinationName,
        source: impl Fn(u64) -> Vec<u8> + Send + Sync + 'static,
    ) -> io::Result<()> {
        let source: AppDataSource = Arc::new(source);
        self.with_registration(name, |registration| {
            registration.app_data_source = Some(source);
            Ok(())
        })
    }

    /// Emit an announce for a destination on every interface.
    pub fn announce(&self, name: &DestinationName, app_data: &[u8]) {
        let pkt = self.build_announce_at(name, app_data, host_announce_seconds());
        self.shared.broadcast(pkt);
    }

    /// The deterministic half of [`Self::announce`]. Private: firmware must supply its own
    /// reservation-backed ordinal rather than inherit this unbounded host generator.
    pub(super) fn build_announce_at(
        &self,
        name: &DestinationName,
        app_data: &[u8],
        source_seconds: u64,
    ) -> Packet {
        let dest = name.destination_hash(self.shared.identity.public());
        let ratchet = self.shared.advertised_ratchet(dest, source_seconds);
        self.shared
            .build_announce_at(name, ratchet.as_ref(), app_data, source_seconds)
    }

    pub(super) fn with_registration(
        &self,
        name: &DestinationName,
        change: impl FnOnce(&mut Registered) -> io::Result<()>,
    ) -> io::Result<()> {
        let dest = name.destination_hash(self.shared.identity.public());
        let mut registered = self.shared.registered.lock().unwrap();
        let registration = registered
            .iter_mut()
            .find(|registration| registration.dest == dest)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "destination is not registered")
            })?;
        change(registration)
    }
}

/// Whole seconds from the host clock for a local announce ordinal.
///
/// [`TimebaseGenerator`] prevents a backward or repeated source clock from reusing an ordinal.
/// A host clock before the Unix epoch cannot supply the required non-negative wire value.
pub(super) fn host_announce_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("host clock is before the Unix epoch")
        .as_secs()
}
