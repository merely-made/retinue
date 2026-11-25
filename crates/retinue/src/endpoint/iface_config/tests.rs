use alloc::format;
use alloc::vec::Vec;
use std::time::Duration;

use super::*;
use crate::endpoint::Endpoint;
use crate::identity::PrivateIdentity;
use crate::node::InterfaceMode;

/// Sections of stock's example config (`rnsd.py` 327-700), comments dropped.
const STOCK_EXAMPLE: &str = "
[reticulum]
  enable_transport = False
  share_instance = Yes
  instance_name = default

[logging]
  loglevel = 4

[interfaces]
  [[Default Interface]]
    type = AutoInterface
    enabled = yes
  [[UDP Interface]]
    type = UDPInterface
    enabled = no
    listen_ip = 0.0.0.0
    listen_port = 4242
    forward_ip = 255.255.255.255
    forward_port = 4242
  [[TCP Server Interface]]
    type = TCPServerInterface
    enabled = no
    listen_ip = 0.0.0.0
    listen_port = 4242
  [[TCP Client Interface]]
    type = TCPClientInterface
    enabled = no
    target_host = 127.0.0.1
    target_port = 4242
  [[I2P]]
    type = I2PInterface
    enabled = no
    connectable = yes
  [[RNode LoRa Interface]]
    type = RNodeInterface
    enabled = no
    port = /dev/ttyUSB0
    frequency = 867200000
    bandwidth = 125000
    txpower = 7
    spreadingfactor = 8
    codingrate = 5
    flow_control = False
  [[Packet Radio KISS Interface]]
    type = KISSInterface
    enabled = no
    port = /dev/ttyUSB1
    speed = 115200
    databits = 8
    parity = none
    stopbits = 1
    preamble = 150
    txtail = 10
    persistence = 200
    slottime = 20
    flow_control = false
  [[Packet Radio AX.25 KISS Interface]]
    type = AX25KISSInterface
    callsign = NO1CLL
    ssid = 0
    enabled = no
    port = /dev/ttyUSB2
    speed = 115200
    parity = none
    preamble = 150
";

fn section(body: &str) -> Result<RnsInterface, RnsConfigError> {
    let text = format!("[interfaces]\n  [[it]]\n{body}");
    RnsConfig::parse(&text)
        .unwrap()
        .interfaces
        .pop()
        .expect("one section")
}

fn tcp_client(extra: &str) -> RnsInterface {
    section(&format!(
        "    type = TCPClientInterface\n    target_host = hub\n    target_port = 4242\n{extra}"
    ))
    .unwrap()
}

#[test]
fn the_stock_example_maps() {
    let config = RnsConfig::parse(STOCK_EXAMPLE).unwrap();
    assert_eq!(config.shared_instance.instance_name, "default");
    let names: Vec<_> = config
        .interfaces
        .iter()
        .map(|i| i.as_ref().map(|i| (i.name.as_str(), i.enabled)))
        .collect();
    assert_eq!(names[2], Ok(("TCP Server Interface", false)));
    assert!(matches!(
        &config.interfaces[4],
        Err(RnsConfigError::UnsupportedType { kind, .. }) if kind == "I2PInterface"
    ));
    let udp = config.interfaces[1].as_ref().unwrap();
    let Carrier::Udp(udp) = &udp.carrier else {
        panic!("UDP")
    };
    let cfg = udp.config().unwrap();
    assert_eq!(cfg.listen, Some("0.0.0.0:4242".parse().unwrap()));
    assert_eq!(cfg.forward, Some("255.255.255.255:4242".parse().unwrap()));
    let server = config.interfaces[2].as_ref().unwrap();
    let Carrier::TcpServer { addr, policy, .. } = &server.carrier else {
        panic!("TCP server")
    };
    assert_eq!(
        *addr,
        crate::endpoint::ListenAddr::Socket("0.0.0.0:4242".parse().unwrap())
    );
    assert!(policy.flap.is_none());
    let client = config.interfaces[3].as_ref().unwrap();
    let Carrier::TcpClient { client, .. } = &client.carrier else {
        panic!("TCP client")
    };
    assert_eq!((client.host.as_str(), client.port), ("127.0.0.1", 4242));
    #[cfg(feature = "serial")]
    {
        let Carrier::Kiss { port, tnc, beacon } = &config.interfaces[6].as_ref().unwrap().carrier
        else {
            panic!("KISS")
        };
        assert_eq!(
            (port.speed, tnc.preamble_ms, tnc.persistence),
            (115200, 150, 200)
        );
        assert!(beacon.is_none());
        let Carrier::Ax25Kiss { header, .. } = &config.interfaces[7].as_ref().unwrap().carrier
        else {
            panic!("AX.25")
        };
        assert_eq!(
            *header,
            crate::iface::ax25::Ax25Header::new("NO1CLL", 0).unwrap()
        );
        let ax25 = |extra: &str| {
            section(&format!(
                "    type = AX25KISSInterface\n    port = /dev/null\n{extra}"
            ))
        };
        assert!(matches!(
            ax25("    callsign = N0CALL\n"),
            Err(RnsConfigError::Value { key, .. }) if key == "ssid"
        ));
        assert!(matches!(
            ax25("    callsign = NO\n    ssid = 1\n"),
            Err(RnsConfigError::Value { key, .. }) if key == "callsign"
        ));
    }
    #[cfg(feature = "tulle-radio")]
    {
        let Carrier::RNode { config, .. } = &config.interfaces[5].as_ref().unwrap().carrier else {
            panic!("RNode")
        };
        assert_eq!(
            (config.params.frequency_hz, config.params.spreading_factor),
            (867_200_000, 8)
        );
        let tcp = section(concat!(
            "    type = RNodeInterface\n    port = TCP://rnode.local\n    frequency = 867200000\n",
            "    bandwidth = 125000\n    spreadingfactor = 8\n    codingrate = 5\n",
        ))
        .unwrap();
        assert!(matches!(
            tcp.carrier,
            Carrier::RNode { port: tulle::serial::RNodePort::Tcp { host, port }, .. }
                if host == "rnode.local" && port == tulle::serial::RNODE_TCP_PORT
        ));
    }
}

#[test]
fn every_mode_alias_maps_and_interface_mode_wins() {
    for (value, mode) in [
        ("full", InterfaceMode::Full),
        ("AP", InterfaceMode::AccessPoint),
        ("access_point", InterfaceMode::AccessPoint),
        ("accesspoint", InterfaceMode::AccessPoint),
        ("ptp", InterfaceMode::PointToPoint),
        ("pointtopoint", InterfaceMode::PointToPoint),
        ("roaming", InterfaceMode::Roaming),
        ("boundary", InterfaceMode::Boundary),
        ("gw", InterfaceMode::Gateway),
        ("gateway", InterfaceMode::Gateway),
        ("internal", InterfaceMode::Internal),
    ] {
        assert_eq!(
            tcp_client(&format!("    mode = {value}\n")).settings.mode,
            mode
        );
        // RNS raises KeyError here without a `mode` key (`Reticulum.py` 859-862).
        let line = format!("    interface_mode = {value}\n    mode = roaming\n");
        assert_eq!(tcp_client(&line).settings.mode, mode);
    }
}

#[test]
fn ifac_takes_bits_and_ignores_none_and_empty() {
    let ifac = |extra| tcp_client(extra).settings.ifac.map(|i| i.size());
    assert_eq!(ifac("    networkname = net\n"), Some(16));
    assert_eq!(
        ifac("    network_name = net\n    ifac_size = 64\n"),
        Some(8)
    );
    assert_eq!(ifac("    passphrase = p\n    ifac_size = 7\n"), Some(16));
    assert_eq!(
        ifac("    networkname = None\n    pass_phrase = \"\"\n"),
        None
    );
    // The later spelling wins only when set.
    let named = tcp_client("    networkname = a\n    network_name = None\n");
    let expected = crate::ifac::Ifac::for_stream(Some("a"), None).unwrap();
    let packet = [0x00, 0x00, 1, 2, 3];
    assert_eq!(
        named.settings.ifac.unwrap().seal(&packet),
        expected.seal(&packet)
    );
    // Serial-family carriers default to 8 bytes.
    #[cfg(feature = "serial")]
    {
        let serial =
            section("    type = SerialInterface\n    port = /dev/null\n    passphrase = p\n");
        assert_eq!(serial.unwrap().settings.ifac.map(|i| i.size()), Some(8));
    }
}

#[test]
fn policy_keys_map_and_a_configured_zero_is_honoured() {
    let s = tcp_client(concat!(
        "    bitrate = 4\n    announce_cap = 0.5\n    outgoing = off\n    gravity = -3\n",
        "    announce_rate_target = 60\n    ic_burst_penalty = 0\n    ic_held_release_interval = 0\n",
        "    ic_burst_freq = 2.6\n    recursive_prs = yes\n    ec_pr_freq = 0\n",
        "    announces_to_internal = 1\n    bootstrap_only = yes\n",
    ))
    .settings;
    assert_eq!(
        (s.bitrate, s.announce_cap, s.outgoing),
        (None, Some(1), false)
    );
    assert_eq!(s.gravity, -3);
    let rate = s.announce_rate.unwrap();
    assert_eq!(
        (rate.target, rate.grace, rate.penalty),
        (Duration::from_secs(60), 0, Duration::ZERO)
    );
    let ingress = s.ingress.unwrap();
    assert_eq!(ingress.burst_penalty, Duration::ZERO);
    assert_eq!(ingress.held_release_interval, Duration::ZERO);
    assert_eq!(ingress.established_interface_hz, 3);
    assert!(s.path_requests.recursive && s.path_requests.egress_hz == 0);
    assert!(s.announces_to_internal && s.bootstrap_only);
    let plain = tcp_client("").settings;
    assert!(plain.ingress.is_none() && plain.announce_rate.is_none() && plain.outgoing);
    assert_eq!(plain.announce_cap, None);
}

#[test]
fn backbone_reads_its_aliases_and_flap_keys() {
    let server = section(concat!(
        "    type = BackboneInterface\n    listen_on = 127.0.0.1\n    port = 4243\n",
        "    fast_flapping_block_time = 2\n    fast_flapping_grace = 0\n",
    ))
    .unwrap();
    let Carrier::TcpServer { addr, policy, .. } = server.carrier else {
        panic!("listener")
    };
    assert_eq!(
        addr,
        crate::endpoint::ListenAddr::Socket("127.0.0.1:4243".parse().unwrap())
    );
    let flap = policy.flap.unwrap();
    assert_eq!((flap.block, flap.grace), (Duration::from_secs(120), 0));
    let client =
        section("    type = BackboneInterface\n    remote = hub.example\n    port = 4243\n")
            .unwrap();
    assert!(matches!(
        client.carrier,
        Carrier::TcpClient { client, backbone: true, .. } if client.host == "hub.example"
    ));
    let both = section(concat!(
        "    type = BackboneInterface\n    target_host = a\n    remote = b\n",
        "    target_port = 1\n    port = 2\n",
    ))
    .unwrap();
    assert!(matches!(
        both.carrier,
        Carrier::TcpClient { client, .. } if client.host == "b" && client.port == 2
    ));
}

#[test]
fn tcp_hardware_mtus_follow_rns() {
    use crate::endpoint::HwMtu;
    let client = |extra: &str| {
        let text = format!(
            "    type = TCPClientInterface\n    target_host = h\n    target_port = 1\n{extra}"
        );
        match section(&text).unwrap().carrier {
            Carrier::TcpClient { client, .. } => client.hw_mtu,
            _ => panic!("TCP client"),
        }
    };
    assert_eq!(
        client(""),
        HwMtu::Auto {
            bitrate: 10_000_000
        }
    );
    assert_eq!(
        client("    bitrate = 1000000\n"),
        HwMtu::Auto { bitrate: 1_000_000 }
    );
    assert_eq!(
        client("    fixed_mtu = 1064\n    bitrate = 1000000\n"),
        HwMtu::Fixed(1064)
    );
    let backbone = |extra: &str| {
        section(&format!("    type = BackboneInterface\n{extra}"))
            .unwrap()
            .carrier
    };
    assert!(matches!(
        backbone("    remote = hub\n    port = 1\n"),
        Carrier::TcpClient { client, .. } if client.hw_mtu == HwMtu::Auto { bitrate: 100_000_000 }
    ));
    assert!(matches!(
        backbone("    listen_on = 127.0.0.1\n    port = 1\n"),
        Carrier::TcpServer { policy, .. } if policy.hw_mtu == Some(HwMtu::Auto { bitrate: 100_000_000 })
    ));
}

#[test]
fn out_of_range_values_are_ignored_as_rns_does() {
    let settings = tcp_client(concat!(
        "    bitrate = -1\n    ifac_size = -8\n    networkname = n\n",
        "    announce_rate_target = -5\n    announce_rate_grace = -1\n",
        "    announce_rate_penalty = -1\n",
    ))
    .settings;
    assert_eq!(settings.bitrate, None);
    assert!(settings.announce_rate.is_none());
    assert_eq!(settings.ifac.unwrap().size(), 16);
}

#[test]
fn bad_values_and_syntax_are_typed_errors() {
    assert!(matches!(
        section("    type = TCPClientInterface\n    target_host = h\n"),
        Err(RnsConfigError::Missing { key, .. }) if key == "target_port"
    ));
    assert!(matches!(
        section("    type = TCPClientInterface\n    target_host = h\n    target_port = x\n"),
        Err(RnsConfigError::Value { .. })
    ));
    assert!(matches!(
        section("    type = TCPClientInterface\n    target_host = h\n    target_port = 1\n    fixed_mtu = 400\n"),
        Err(RnsConfigError::Value { key, .. }) if key == "fixed_mtu"
    ));
    for kind in ["WeaveInterface", "RNodeMultiInterface", "Nonsense"] {
        assert!(matches!(
            section(&format!("    type = {kind}\n")),
            Err(RnsConfigError::UnsupportedType { .. })
        ));
    }
    assert_eq!(
        RnsConfig::parse("[interfaces]\n  [[a]]\n  not a pair\n").unwrap_err(),
        RnsConfigError::Syntax { line: 3 }
    );
}

#[test]
fn values_unquote_and_commands_split_as_shlex() {
    let doc = syntax::parse("[interfaces]\n[[x]]\na = \"q # kept\" # dropped\nb = plain # gone\n")
        .unwrap();
    assert_eq!(doc.interfaces[0].keys[0].1, "q # kept");
    assert_eq!(doc.interfaces[0].keys[1].1, "plain");
    assert_eq!(
        syntax::shell_words(r#"cat 'a b' "c \"d\"" e\ f"#).unwrap(),
        ["cat", "a b", "c \"d\"", "e f"]
    );
    assert!(syntax::shell_words("cat 'open").is_none());
    for (word, value) in [("Yes", true), ("on", true), ("0", false), ("FALSE", false)] {
        assert_eq!(syntax::boolean(word), Some(value));
    }
    assert_eq!(syntax::boolean("maybe"), None);
}

#[test]
fn shared_instance_keys_map() {
    let config = RnsConfig::parse(
        "[reticulum]\n  instance_name = lab\n  shared_instance_port = 40000\n  shared_instance_type = TCP\n",
    )
    .unwrap();
    assert_eq!(config.shared_instance.instance_name, "lab");
    assert_eq!(config.shared_instance.port, 40000);
    assert_eq!(
        config.shared_instance.kind,
        crate::endpoint::SharedInstanceKind::Tcp
    );
}

#[tokio::test]
async fn settings_apply_through_the_public_setters() {
    let ep = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x3c; 64]),
        crate::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    let id = ep.attach_interface().id();
    let s = tcp_client("    mode = gateway\n    announce_cap = 5\n    gravity = 2\n").settings;
    assert!(ep.apply_interface_settings(id, &s));
    assert_eq!(ep.shared.interface_mode(id), InterfaceMode::Gateway);
    let policy = ep.shared.iface_policy(id);
    assert_eq!((policy.cap_percent, policy.gravity), (5, 2));
    ep.detach_interface(id);
    assert!(!ep.apply_interface_settings(id, &s));
}

#[cfg(feature = "pipe")]
#[test]
fn a_pipe_command_is_split() {
    let pipe =
        section("    type = PipeInterface\n    command = netcat -l 'a b'\n    respawn_delay = 0\n")
            .unwrap();
    let Carrier::Pipe(cfg) = pipe.carrier else {
        panic!("pipe")
    };
    assert_eq!(cfg.argv, ["netcat", "-l", "a b"]);
    assert_eq!(cfg.respawn_delay, Duration::ZERO);
}
