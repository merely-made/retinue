//! The Retinue half of the announce-ingress live gates: `oracle/interop_held_path_response.py`,
//! `interop_ingress_burst.py`, `interop_announce_rate.py` and `interop_gravity.py`.
//!
//! Environment:
//! - `RETINUE_CONNECT`: comma-separated `host:port` TCP clients, interfaces 0, 1, ...
//! - `RETINUE_GRAVITY`: comma-separated gravities for those interfaces.
//! - `RETINUE_LISTEN`: a port on 127.0.0.1 to listen on (0 picks one).
//! - `RETINUE_ROUTING=1`: run as a transport.
//! - `RETINUE_INGRESS=hold_ms,penalty_ms,release_ms`: shorter burst timings, as the stock
//!   side's `ic_burst_hold`, `ic_burst_penalty` and `ic_held_release_interval`.
//!
//! Prints `UP <identity>`, `IFACE <index> <id>` and `LISTENING <port>`, then
//! `ANNOUNCE <dest> <iface> <hops> <released>` for each published announce, `released` being
//! the ingress interface's release count when it was published. Commands on stdin, one per
//! line: `request <dest>`, `route <dest>`, `counters`.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    probe::run().await
}

mod probe {
    use std::io::BufRead;
    use std::sync::Arc;
    use std::time::Duration;

    use retinue::announce_admission::AnnounceIngressPolicy;
    use retinue::endpoint::Endpoint;
    use retinue::hash::AddressHash;
    use retinue::identity::PrivateIdentity;
    use tokio::sync::mpsc;

    type Error = Box<dyn std::error::Error>;

    fn env_list(name: &str) -> Vec<String> {
        std::env::var(name)
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn parse_dest(hex_text: &str) -> Option<AddressHash> {
        let bytes: [u8; 16] = hex::decode(hex_text).ok()?.try_into().ok()?;
        Some(AddressHash::from_bytes(bytes))
    }

    fn ingress_policy(spec: &str) -> Result<AnnounceIngressPolicy, Error> {
        let ms: Vec<u64> = spec.split(',').map(str::parse).collect::<Result<_, _>>()?;
        let [hold, penalty, release] = ms[..] else {
            return Err("RETINUE_INGRESS takes hold_ms,penalty_ms,release_ms".into());
        };
        Ok(AnnounceIngressPolicy {
            burst_hold: Duration::from_millis(hold),
            burst_penalty: Duration::from_millis(penalty),
            held_release_interval: Duration::from_millis(release),
            ..AnnounceIngressPolicy::default()
        })
    }

    fn command(ep: &Endpoint, line: &str) {
        let mut words = line.split_whitespace();
        match (words.next(), words.next().and_then(parse_dest)) {
            (Some("request"), Some(dest)) => {
                println!("REQUESTED {dest} {}", ep.request_path(dest));
            }
            (Some("route"), Some(dest)) => match ep.route_to(dest) {
                Some((iface, hops)) => println!("ROUTE {dest} {iface} {hops}"),
                None => println!("ROUTE {dest} none"),
            },
            (Some("counters"), _) => {
                for id in ep.interface_ids() {
                    let c = ep.announce_ingress_counters(id);
                    println!(
                        "COUNTERS {id} {} {} {} {}",
                        c.observed, c.held, c.released, c.held_dropped
                    );
                }
                let r = ep.routing_counters();
                println!(
                    "ROUTING {} {} {}",
                    r.forwarded_announces, r.relay_rate_limited_announces, r.held_announces
                );
            }
            _ => println!("UNKNOWN {line}"),
        }
    }

    pub(super) async fn run() -> Result<(), Error> {
        let ep = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
            &[0x1C; 64],
        )));
        if let Ok(spec) = std::env::var("RETINUE_INGRESS") {
            ep.set_announce_ingress_policy(ingress_policy(&spec)?);
        }
        if std::env::var("RETINUE_ROUTING").is_ok_and(|v| v == "1") {
            ep.enable_routing();
        }
        let gravity = env_list("RETINUE_GRAVITY");
        for (index, addr) in env_list("RETINUE_CONNECT").iter().enumerate() {
            let id = ep.attach_tcp_client(addr.parse()?).await?;
            if let Some(g) = gravity.get(index) {
                assert!(ep.set_interface_gravity(id, g.parse()?));
            }
            println!("IFACE {index} {id}");
        }
        if let Ok(port) = std::env::var("RETINUE_LISTEN") {
            let addr = ep
                .listen_tcp(([127, 0, 0, 1], port.parse()?).into())
                .await?;
            println!("LISTENING {}", addr.port());
        }
        println!("UP {}", ep.identity().hash());

        let announces = Arc::clone(&ep);
        tokio::spawn(async move {
            while let Ok(a) = announces.next_announcement().await {
                let released = announces.announce_ingress_counters(a.interface).released;
                println!(
                    "ANNOUNCE {} {} {} {released}",
                    a.destination, a.interface, a.hops
                );
            }
        });

        let (tx, mut rx) = mpsc::unbounded_channel();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        while let Some(line) = rx.recv().await {
            command(&ep, &line);
        }
        Ok(())
    }
}
