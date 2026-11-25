//! Attaching to a stock shared instance (rnsd) as a local client (`LocalInterface.py` 53-202;
//! `Transport.py` 1405-1425, 1936-1939, 3690-3709).

use alloc::string::String;
use alloc::vec::Vec;

use std::collections::HashSet;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::net::TcpStream;

use crate::announce_admission::AnnounceIngressPolicy;
use crate::iface::hdlc;
use crate::node::InterfaceMode;
use crate::packet::{HeaderType, Packet, PacketType};
use crate::transport;

use super::interface::InterfaceId;
use super::mtu::HwMtu;
use super::pump::{self, Framing, PumpEnd};
use super::queue::{OutboundPackets, TrafficClass};
use super::runtime::{Endpoint, endpoint_closed, track};
use super::shared::Shared;

/// RNS's default shared-instance TCP port (`Reticulum.py` 296-305).
pub const SHARED_INSTANCE_PORT: u16 = 37428;

/// Wait between reconnect attempts (`RECONNECT_WAIT`, `LocalInterface.py` 54).
const RECONNECT_WAIT: Duration = Duration::from_secs(8);

/// Wait after reconnecting before answering for our destinations (`LocalInterface.py`
/// 178-181), so the restarted instance's other clients are back too.
const REAPPEAR_WAIT: Duration = Duration::from_secs(10);

/// A local interface's bitrate, which sets its hardware MTU (`LocalInterface.py` 62).
const LOCAL_BITRATE: u64 = 1_000_000_000;

/// Bound on one connect attempt to the instance.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How to reach the shared instance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SharedInstanceKind {
    /// The abstract unix socket `\0rns/<name>` on Linux and Android, falling back to TCP, and
    /// TCP elsewhere, as RNS chooses (`Reticulum.py` 350-362; `platformutils.py` 55-61).
    #[default]
    Auto,
    /// TCP on 127.0.0.1 (`shared_instance_type = tcp`).
    Tcp,
    /// The abstract unix socket only; unsupported off Linux and Android.
    Unix,
}

/// A shared instance to attach to: RNS's `instance_name`, `shared_instance_port` and
/// `shared_instance_type`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedInstance {
    /// Names the abstract socket `\0rns/<instance_name>`.
    pub instance_name: String,
    /// TCP port on 127.0.0.1.
    pub port: u16,
    pub kind: SharedInstanceKind,
}

impl Default for SharedInstance {
    /// RNS's defaults: instance `default`, port 37428, the platform's socket.
    fn default() -> Self {
        Self {
            instance_name: String::from("default"),
            port: SHARED_INSTANCE_PORT,
            kind: SharedInstanceKind::Auto,
        }
    }
}

/// Shared-instance state: which interfaces lead to one.
#[derive(Debug, Default)]
pub(super) struct LocalState {
    /// `ifaces.len()`, so ingress on an endpoint without one takes no lock.
    count: AtomicUsize,
    ifaces: RwLock<HashSet<InterfaceId>>,
}

impl LocalState {
    fn contains(&self, id: InterfaceId) -> bool {
        self.count.load(Ordering::Acquire) > 0 && self.ifaces.read().unwrap().contains(&id)
    }

    /// Whether any interface leads to a shared instance, so this endpoint is its client.
    pub(super) fn attached(&self) -> bool {
        self.count.load(Ordering::Acquire) > 0
    }

    fn insert(&self, id: InterfaceId) {
        let mut ifaces = self.ifaces.write().unwrap();
        ifaces.insert(id);
        self.count.store(ifaces.len(), Ordering::Release);
    }

    fn remove(&self, id: InterfaceId) {
        let mut ifaces = self.ifaces.write().unwrap();
        ifaces.remove(&id);
        self.count.store(ifaces.len(), Ordering::Release);
    }
}

impl Shared {
    /// Adjust a packet received on `iface` before routing: a shared instance's link counts no
    /// hop (`Transport.py` 1936-1939). Retinue keeps wire hops, one fewer than RNS's, so a
    /// destination one hop past the instance reads 0 here as it reads 1 in a stock client.
    ///
    /// An announce the instance re-emits at hops 0 is another of its local clients
    /// (`Transport.py` 2398-2425). A stock client reaches that one with header type 1, which
    /// the instance carries in (1428-1437, 2000-2001): the other client's transport id is
    /// ephemeral, so it drops a header-type-2 packet naming the instance. Its route therefore
    /// keeps no transport node; one past the instance keeps it, as in 1416-1425.
    pub(super) fn local_ingress(&self, iface: InterfaceId, mut pkt: Packet) -> Packet {
        if self.local.contains(iface) {
            if pkt.packet_type == PacketType::Announce && pkt.hops == 0 {
                pkt.header_type = HeaderType::Type1;
                pkt.transport = None;
            }
            pkt.hops = pkt.hops.saturating_sub(1);
        }
        pkt
    }

    /// The instance on `id` went away (`shared_connection_disappeared`, `Transport.py`
    /// 3690-3703): tear down its links as lost and drop the paths, bridges, return paths,
    /// pending relays, held announces and tunnels learned through it. Scoped to `id`, since
    /// this endpoint may hold other carriers. Links still awaiting a proof record no
    /// interface; they lapse at their setup deadline.
    fn shared_connection_lost(&self, id: InterfaceId) {
        let lost: Vec<_> = self.write_diagnostic(|| {
            let mut links = self.links.lock().unwrap();
            let lost: Vec<_> = links
                .iter()
                .filter(|(_, entry)| entry.iface == id)
                .map(|(link, _)| *link)
                .collect();
            for link in &lost {
                if let Some(entry) = links.remove(link) {
                    entry.lost.store(true, Ordering::Release);
                }
            }
            drop(links);
            let mut paths = self.path_table.lock().unwrap();
            let before = paths.len();
            transport::forget_interface(&mut *paths, id, |entry| [entry.iface, entry.iface]);
            let culled = paths.len() != before;
            drop(paths);
            let mut bridges = self.link_transport.lock().unwrap();
            transport::forget_interface(&mut *bridges, id, |bridge| [bridge.from, bridge.out]);
            drop(bridges);
            let changed = culled || !lost.is_empty();
            (lost, changed)
        });
        for link in &lost {
            self.inbound.lock().unwrap().slots.remove(link);
            self.resource_proofs.lock().unwrap().remove(link);
        }
        self.held_announces.lock().unwrap().purge(id);
        self.held_release_wake.notify_waiters();
        self.forget_reverse_via(id);
        self.forget_rebroadcasts_on(id);
        self.drop_tunnels_on(id);
    }

    /// The instance on `id` is back: answer for every registered destination there, as a
    /// path response (`shared_connection_reappeared`, `Transport.py` 3705-3709). Every
    /// registered destination is SINGLE.
    fn shared_connection_reappeared(&self, id: InterfaceId) {
        let online = self.with_iface(id, |i| i.online.load(Ordering::Acquire));
        if online != Some(true) {
            return;
        }
        let dests: Vec<_> = self
            .registered
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.dest)
            .collect();
        for dest in dests {
            if let Some(pkt) = self.path_response(dest) {
                self.send_on_class(id, pkt, TrafficClass::Control);
            }
        }
    }
}

/// A connection to the instance.
enum LocalStream {
    Tcp(TcpStream),
    #[cfg(any(target_os = "linux", target_os = "android"))]
    Unix(tokio::net::UnixStream),
}

/// Connect once, by `instance.kind` (`LocalInterface.py` 133-151).
async fn connect(instance: &SharedInstance) -> io::Result<LocalStream> {
    let attempt = async {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        match instance.kind {
            SharedInstanceKind::Unix => return connect_abstract(&instance.instance_name).await,
            SharedInstanceKind::Auto => {
                if let Ok(stream) = connect_abstract(&instance.instance_name).await {
                    return Ok(stream);
                }
            }
            SharedInstanceKind::Tcp => {}
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        if instance.kind == SharedInstanceKind::Unix {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let stream = TcpStream::connect(("127.0.0.1", instance.port)).await?;
        stream.set_nodelay(true)?;
        Ok(LocalStream::Tcp(stream))
    };
    tokio::time::timeout(CONNECT_TIMEOUT, attempt)
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
}

/// Connect to the abstract socket `\0rns/<name>` (`LocalInterface.py` 68, 135-137). std's
/// connect blocks while the listener's backlog is full, so it runs off the runtime.
#[cfg(any(target_os = "linux", target_os = "android"))]
async fn connect_abstract(name: &str) -> io::Result<LocalStream> {
    #[cfg(target_os = "android")]
    use std::os::android::net::SocketAddrExt;
    #[cfg(target_os = "linux")]
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixStream};

    let addr = SocketAddr::from_abstract_name(alloc::format!("rns/{name}"))?;
    let stream = tokio::task::spawn_blocking(move || UnixStream::connect_addr(&addr))
        .await
        .map_err(io::Error::other)??;
    stream.set_nonblocking(true)?;
    tokio::net::UnixStream::from_std(stream).map(LocalStream::Unix)
}

/// Carry one connection: online while it lasts, then offline with the backlog discarded, as
/// `carry_tcp` does.
async fn carry(
    shared: &Shared,
    id: InterfaceId,
    online: &AtomicBool,
    out: &mut OutboundPackets,
    stream: LocalStream,
) -> PumpEnd {
    if !online.swap(true, Ordering::AcqRel) {
        out.discard();
    }
    let end = match stream {
        LocalStream::Tcp(stream) => {
            let (reader, writer) = stream.into_split();
            pump::run(shared, id, out, Framing::Hdlc, reader, writer).await
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        LocalStream::Unix(stream) => {
            let (reader, writer) = stream.into_split();
            pump::run(shared, id, out, Framing::Hdlc, reader, writer).await
        }
    };
    online.store(false, Ordering::Release);
    out.discard();
    end
}

impl Endpoint {
    /// Attach to a shared instance as a local client, as a stock program does when rnsd is
    /// running. The id is stable: on loss the interface goes offline, its links are torn
    /// down and its routes dropped, and it redials every 8 s; 10 s after reconnecting it
    /// answers for every registered destination (`LocalInterface.py` 54, 153-185).
    ///
    /// Fails if the first connect fails, or with [`io::ErrorKind::Unsupported`] for
    /// [`SharedInstanceKind::Unix`] off Linux and Android.
    pub async fn attach_shared_instance(
        &self,
        instance: SharedInstance,
    ) -> io::Result<InterfaceId> {
        if !self.shared.is_running() {
            return Err(endpoint_closed());
        }
        let mut stream = Some(connect(&instance).await?);
        let attached = self.shared.add_interface(
            hdlc::MAX_FRAME,
            None,
            InterfaceMode::Full,
            true,
            HwMtu::Auto {
                bitrate: LOCAL_BITRATE,
            },
        );
        if !attached.registered {
            return Err(endpoint_closed());
        }
        let (id, online, mut outbound) = (attached.id, attached.online, attached.outbound);
        self.shared.local.insert(id);
        // A local interface is never ingress-limited (`LocalInterface.py` 130).
        let exempt = AnnounceIngressPolicy {
            enabled: false,
            ..AnnounceIngressPolicy::default()
        };
        self.set_interface_ingress_policy(id, Some(exempt));
        let shared = Arc::clone(&self.shared);
        let started = track(&self.shared, async move {
            loop {
                if let Some(connected) = stream.take() {
                    if carry(&shared, id, &online, &mut outbound, connected).await
                        == PumpEnd::Closed
                    {
                        break;
                    }
                    shared.shared_connection_lost(id);
                }
                tokio::time::sleep(RECONNECT_WAIT).await;
                if shared.with_iface(id, |_| ()).is_none() {
                    break;
                }
                stream = connect(&instance).await.ok();
                if stream.is_some() {
                    let later = Arc::clone(&shared);
                    track(&shared, async move {
                        tokio::time::sleep(REAPPEAR_WAIT).await;
                        later.shared_connection_reappeared(id);
                    });
                }
            }
            shared.local.remove(id);
        });
        if !started {
            self.shared.local.remove(id);
            self.shared.forget_interface(id);
            return Err(endpoint_closed());
        }
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destination::DestinationName;
    use crate::hash::AddressHash;
    use crate::identity::PrivateIdentity;

    fn relayed(packet_type: PacketType, hops: u8) -> Packet {
        let mut pkt = crate::path::path_request(AddressHash::from_bytes([7; 16]), &[0; 16]);
        pkt.packet_type = packet_type;
        pkt.address_via(Some(AddressHash::from_bytes([0x5d; 16])));
        pkt.hops = hops;
        pkt
    }

    /// A client of a shared instance keeps never-used routes as a leaf does, 12 minutes, even
    /// when it forwards (`Transport.py` 451-454).
    #[tokio::test]
    async fn a_shared_instance_client_lingers_as_a_leaf() {
        let ep = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[5; 64]),
            crate::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let local = ep.attach_interface().id();
        ep.shared.local.insert(local);
        ep.enable_routing();
        let dest = AddressHash::from_bytes([0x51; 16]);
        let learned = std::time::Instant::now();
        ep.shared.learn_path_at(dest, local, 1, None, learned);
        let leaf = Duration::from_secs(12 * 60);
        assert!(
            ep.route_to_at(dest, learned + leaf - Duration::from_millis(1))
                .is_some()
        );
        assert!(ep.route_to_at(dest, learned + leaf).is_none());
    }

    #[tokio::test]
    async fn shared_instance_hops_are_transparent_and_saturate() {
        let ep = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[3; 64]),
            crate::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let plain = ep.attach_interface().id();
        let local = ep.attach_interface().id();
        ep.shared.local.insert(local);
        let ingress = |iface, hops| {
            ep.shared
                .local_ingress(iface, relayed(PacketType::Data, hops))
        };
        assert_eq!(ingress(plain, 2).hops, 2);
        assert_eq!(ingress(local, 2).hops, 1);
        assert_eq!(ingress(local, 0).hops, 0);
    }

    /// Past the instance, a route keeps the instance as its transport node and leaves as
    /// header type 2 (`Transport.py` 1416-1425); another local client of the instance is
    /// reached with header type 1 (1428-1437).
    #[tokio::test]
    async fn routes_through_the_instance_are_addressed_as_a_stock_client_does() {
        let ep = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[4; 64]),
            crate::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let local = ep.attach_interface().id();
        ep.shared.local.insert(local);
        let beyond = ep
            .shared
            .local_ingress(local, relayed(PacketType::Announce, 1));
        assert_eq!((beyond.hops, beyond.header_type), (0, HeaderType::Type2));
        let sibling = ep
            .shared
            .local_ingress(local, relayed(PacketType::Announce, 0));
        assert_eq!((sibling.hops, sibling.transport), (0, None));
        assert_eq!(sibling.header_type, HeaderType::Type1);

        let dest = DestinationName::new("x", ["y"]).plain_hash();
        ep.shared
            .learn_path(dest, local, beyond.hops, beyond.transport);
        let mut data = relayed(PacketType::Data, 0);
        data.header_type = HeaderType::Type1;
        data.transport = None;
        data.destination = dest;
        let out = ep.shared.address_for(local, data);
        assert_eq!(
            (out.header_type, out.transport),
            (HeaderType::Type2, beyond.transport)
        );
    }

    #[tokio::test]
    async fn loss_drops_routes_and_links_on_that_interface_only() {
        let ep = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[5; 64]),
            crate::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let local = ep.attach_interface().id();
        let other = ep.attach_interface().id();
        let (a, b) = (
            AddressHash::from_bytes([0xa; 16]),
            AddressHash::from_bytes([0xb; 16]),
        );
        ep.shared.learn_path(a, local, 1, None);
        ep.shared.learn_path(b, other, 1, None);
        ep.shared.remember_reverse(a, local, other);
        ep.shared.remember_reverse(b, other, other);
        ep.shared.shared_connection_lost(local);
        assert!(ep.route_to(a).is_none());
        assert_eq!(ep.route_to(b), Some((other, 1)));
        assert_eq!(ep.shared.take_reverse(a, other), None);
        assert_eq!(ep.shared.take_reverse(b, other), Some(other));
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    #[tokio::test]
    async fn unix_kind_is_unsupported_off_linux() {
        let ep = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[6; 64]),
            crate::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let instance = SharedInstance {
            kind: SharedInstanceKind::Unix,
            ..SharedInstance::default()
        };
        let err = ep.attach_shared_instance(instance).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert_eq!(ep.interface_count(), 0);
    }
}
