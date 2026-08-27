//! The retinue half of the R3 request/response gate.
//!
//! retinue initiates a link to an RNS destination, sends a request to its `/echo` handler,
//! and checks the response ties back by request id. Then, over the same connection, it acts
//! as a responder for a request RNS sends the other way.
//!
//! Driven by `oracle/interop_reqresp.py`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use retinue::announce::{self, AnnounceBlob, TimebaseGenerator};
use retinue::destination::DestinationName;
use retinue::identity::PrivateIdentity;
use retinue::iface::tcp::{RecvError, TcpInterface, TcpInterfaceListener};
use retinue::link::{self, Inbound, LinkMode, LinkTrailer, PendingLink};
use retinue::packet::{Packet, PacketType};
use retinue::request::{Request, Response};

/// The RNS destination retinue calls into (fixed seed, known to both).
const DEST_SEED: [u8; 64] = [0x11; 64];
const EPHEMERAL_SEED: [u8; 64] = [0x33; 64];
/// retinue's own responder identity, for the reverse direction.
const OUR_SEED: [u8; 64] = [0x55; 64];
const OUR_EPHEMERAL: [u8; 64] = [0x77; 64];

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}
fn iv(n: u8) -> [u8; 16] {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_le_bytes();
    let mut v = [0u8; 16];
    v[..8].copy_from_slice(&t[..8]);
    v[15] = n;
    v
}
fn next_blob(generator: &mut TimebaseGenerator) -> AnnounceBlob {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let ordinal = generator.next(seconds).expect("announce timebase");
    AnnounceBlob::mint([0x55; 5], ordinal).expect("announce timebase fits")
}
async fn send(iface: &mut TcpInterface, p: &Packet) {
    iface.send(p).await.expect("send");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpInterfaceListener::bind("127.0.0.1:0".parse()?).await?;
    println!("LISTENING {}", listener.local_addr()?.port());
    let mut iface = listener.accept().await?;

    // `accept` returns when the TCP connection lands, which is earlier than RNS being able to
    // use it: its `TCPClientInterface` drops a peer whose first frame arrives before it has
    // finished connecting, and a peer dropped that way stays dropped -- no amount of resending
    // below revives it, because the frames are being discarded rather than lost. This wait is
    // the only lever for that failure, and it is why it is longer than the 250 ms carried here
    // previously: at 250 ms a 120-run census still lost about one run in sixty to it, with RNS
    // proving nothing and seeing no announce for the whole ten-second retry window.
    //
    // Treat 750 as provisional. It was fitted on a box running 54 rustc and 14 cargo processes
    // across 16 cores -- a 3.4x oversubscription that inflates every latency this wait is
    // racing against. It is therefore probably generous on an idle machine and may still be
    // short on a busier one, which is the same superstition the 250 ms in `interop_r1` was
    // carried as for months. The honest fix is to wait on evidence the peer is usable rather
    // than on a constant; until something can supply that signal, this number should be
    // re-measured on a quiet machine before anyone trusts it.
    tokio::time::sleep(Duration::from_millis(750)).await;

    // Announce our responder destination so RNS can call us back.
    let our_id = PrivateIdentity::from_secret_bytes(&OUR_SEED);
    let our_name = DestinationName::new("retinue", ["svc"]);
    let our_dest = our_name.destination_hash(our_id.public());
    // Each announce gets a fresh typed blob. Byte-identical repeats are the one thing
    // that cannot be used as evidence here: this repo has already observed
    // RNS suppressing repeats from a destination it already knows (see the small
    // plan's 2026-08-06 entry), and `interop_tcp.rs` says so outright. Every other
    // example that re-announces varies it; this one did not, which silently
    // confounded the conclusion that a dropped peer "stays dropped".
    let mut timebase = TimebaseGenerator::host(0).expect("valid host timebase");
    let mut fresh_announce = || {
        let blob = next_blob(&mut timebase);
        announce::build(&our_id, our_name.name_hash(), &blob, None, b"svc")
    };

    // --- Direction 1: retinue -> RNS request.
    let peer = *PrivateIdentity::from_secret_bytes(&DEST_SEED).public();
    let dest = DestinationName::new("retinue", ["reqresp"]).destination_hash(&peer);
    let (pending, request) = PendingLink::open(
        dest,
        peer,
        &EPHEMERAL_SEED,
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
    );

    // Neither the announce nor the link request is acknowledged, and `accept` returning does
    // not mean the peer can receive them: the gate builds its Reticulum instance, its
    // destination and its announce handler only *after* the TCP connection lands. Sending
    // each once behind a fixed 250 ms guess loses that race often enough to matter. A 120-run
    // census of this gate put every failure into one of two shapes, and both are that race:
    // RNS never saw our /svc announce, so it never opened direction 2; or our link request
    // drew no proof and we gave up at the 10 s mark having sent it exactly once.
    //
    // So resend both until the proof lands, which is what any stack does with an
    // unacknowledged request, rather than guessing a delay long enough for the worst case.
    //
    // The wait also has to answer RNS while it runs. The previous loop discarded every packet
    // that was not a proof, so an inbound link request for our own destination -- which RNS
    // sends as soon as it sees the announce, quite possibly before it proves ours -- was
    // dropped on the floor, and direction 2 then had no link to arrive on.
    let mut resp_link: Option<retinue::link::Link> = None;
    let proof_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut established: Option<retinue::link::Link> = None;

    while established.is_none() && tokio::time::Instant::now() < proof_deadline {
        send(&mut iface, &fresh_announce()).await;
        send(&mut iface, &request).await;

        let attempt_until =
            (tokio::time::Instant::now() + Duration::from_millis(1000)).min(proof_deadline);
        while tokio::time::Instant::now() < attempt_until {
            let left = attempt_until.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(left, iface.recv()).await {
                Err(_) => break,
                // Distinguish a dead socket from a malformed frame. Both used to
                // fall through to `TIMEOUT proof`, so a census could not tell
                // "RNS discarded our frames" from "the connection was gone".
                Ok(Err(RecvError::Io(e))) => {
                    println!("PROOF_WAIT_IO {e}");
                    break;
                }
                Ok(Err(_)) => continue,
                Ok(Ok(p)) if p.packet_type == PacketType::Proof => {
                    established = Some(pending.prove(&p)?);
                    break;
                }
                Ok(Ok(p))
                    if resp_link.is_none()
                        && p.packet_type == PacketType::LinkRequest
                        && p.destination == our_dest =>
                {
                    let (l, proof) = link::accept(
                        &p,
                        &our_id,
                        &OUR_EPHEMERAL,
                        LinkTrailer {
                            mode: LinkMode::Aes256Cbc,
                            mtu: 500,
                        },
                    )?;
                    send(&mut iface, &proof).await;
                    resp_link = Some(l);
                }
                Ok(Ok(_)) => continue,
            }
        }
    }

    let Some(out_link) = established else {
        println!("TIMEOUT proof");
        return Ok(());
    };
    send(&mut iface, &out_link.rtt_packet(0.05, &iv(1))).await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    let req = Request::new(b"/echo", b"ping-from-retinue".to_vec(), now());
    let req_pkt = out_link.request_packet(&req.pack(), &iv(2));
    let request_id = req_pkt.hash(); // what the response must reference
    send(&mut iface, &req_pkt).await;
    println!("SENT_REQUEST id={request_id}");

    // Meanwhile, act as responder for RNS's inbound link + request.
    let mut direction1_done = false;
    let mut direction2_done = false;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline && !(direction1_done && direction2_done) {
        // Keep announcing until RNS acts on it. The retry loop above stops the moment our own
        // proof arrives, but RNS registers its announce handler *after* the destination that
        // proved us, so an announce sent in that window is processed by its Transport with
        // nothing listening and is simply never seen. Direction 1 then passes while direction 2
        // never begins -- the one failure shape a census of 120 runs could not otherwise
        // account for. Re-announcing costs a packet a second and closes it.
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let slice = remaining.min(Duration::from_secs(1));
        let packet = match tokio::time::timeout(slice, iface.recv()).await {
            Err(_) => {
                if resp_link.is_none() {
                    send(&mut iface, &fresh_announce()).await;
                }
                continue;
            }
            Ok(Err(RecvError::Wire(_))) => continue,
            Ok(Err(RecvError::Io(_))) => break,
            Ok(Ok(p)) => p,
        };

        // Direction 1: our outbound link carries the response.
        if let Some(Inbound::Response(bytes)) = out_link.receive(&packet) {
            match Response::unpack(&bytes) {
                Ok(resp) => {
                    let matched = resp.request_id == request_id;
                    println!(
                        "RECV_RESPONSE data={} id_match={}",
                        String::from_utf8_lossy(&resp.data),
                        matched,
                    );
                    direction1_done = true;
                }
                Err(e) => println!("RESPONSE_PARSE_FAILED {e}"),
            }
            continue;
        }

        // Direction 2: RNS opens a link to our /svc destination and sends a request.
        match &resp_link {
            None if packet.packet_type == PacketType::LinkRequest
                && packet.destination == our_dest =>
            {
                let (l, proof) = link::accept(
                    &packet,
                    &our_id,
                    &OUR_EPHEMERAL,
                    LinkTrailer {
                        mode: LinkMode::Aes256Cbc,
                        mtu: 500,
                    },
                )?;
                send(&mut iface, &proof).await;
                resp_link = Some(l);
            }
            Some(l) => {
                if let Some(Inbound::Request(bytes)) = l.receive(&packet) {
                    let incoming = Request::unpack(&bytes)?;
                    // request_id is the hash of the received request packet.
                    let id = packet.hash();
                    let mut data = b"retinue-echo:".to_vec();
                    data.extend_from_slice(&incoming.data);
                    let response = Response::new(id, data);
                    send(&mut iface, &l.response_packet(&response.pack(), &iv(3))).await;
                    println!("ANSWERED_REQUEST");
                    direction2_done = true;
                }
            }
            None => {}
        }
    }

    println!("DONE d1={direction1_done} d2={direction2_done}");

    // Hold the connection open briefly before `iface` drops.
    //
    // `send` writes the response through to the kernel and returns, so the bytes are on
    // their way -- but this example exits the instant its own done-conditions are met, and
    // dropping the interface closes the socket underneath a peer that has not read yet.
    // The race is directly observed, not inferred: in runs that pass, RNS logs receipt of
    // the response *after* our socket has already closed; in runs that fail it reports
    // `None` while our own log already says ANSWERED_REQUEST and d2=true. The 250 ms wait
    // after `accept` above is the same shape of concession at the other end of the
    // connection's life, and this is its bookend. A real responder does not exit here,
    // which is why this belongs to the example and not to `TcpInterface`.
    //
    // It does NOT make the gate reliable. Measured against RNS 1.5.0: 4 failures in 30 runs
    // before this wait, 4 in 60 after -- indistinguishable at these sample sizes. What did
    // change is which mode fails. The teardown signature above stopped appearing, and the
    // residue is two other modes this wait cannot touch: `d2=false`, where we break out of
    // the receive loop before RNS's request arrives at all, and a collapse of the whole
    // exchange in which direction 1 fails too. Both are unexplained. Do not read a passing
    // run of this gate as strong evidence; see the 2026-08-23 re-pin receipt.
    tokio::time::sleep(Duration::from_millis(250)).await;
    Ok(())
}
