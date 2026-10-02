//! A five-radio topology with one cuttable shortcut, and its two scenarios.
//!
//! fire–church, church–water, water–ridge, water–garage, and fire–water (the shortcut).
//! A message goes from fire to garage.
//!
//! Fire, the sender, is a leaf: it relays nothing. Its two neighbours hear each other, so no
//! path is shorter through fire, and relaying would only add its rebroadcasts to the air.
//! It still learns routes and addresses its first relay, as every `Node` does.

use retinue_sim::{Cut, Edge, NodeSpec, Scenario, Send, Timing, Topology};

pub fn topology() -> Topology {
    let edge = |a: &str, b: &str| Edge {
        a: a.into(),
        b: b.into(),
    };
    Topology {
        nodes: ["fire", "church", "water", "ridge", "garage"]
            .into_iter()
            .map(|name| NodeSpec {
                name: name.into(),
                transit: name != "fire",
            })
            .collect(),
        edges: vec![
            edge("fire", "church"),
            edge("church", "water"),
            edge("water", "ridge"),
            edge("water", "garage"),
            edge("fire", "water"),
        ],
    }
}

fn shortcut_cut(at: u64) -> Cut {
    Cut {
        at,
        a: "fire".into(),
        b: "water".into(),
    }
}

fn send(at: u64, n: u32) -> Send {
    Send {
        at,
        from: "fire".into(),
        to: "garage".into(),
        payload: format!("message {n}"),
    }
}

/// The shortcut is cut before anyone announces.
pub fn cold() -> Scenario {
    Scenario {
        name: "cold-cut".into(),
        topology: topology(),
        cuts: vec![shortcut_cut(0)],
        sends: vec![send(10_000, 1)],
        timing: Timing::defaults(20_000),
    }
}

/// Routes settle and a message crosses the shortcut; then it is cut. Fire keeps sending
/// every three minutes, through the next announce at ten minutes.
pub fn warm() -> Scenario {
    Scenario {
        name: "warm-cut".into(),
        topology: topology(),
        cuts: vec![shortcut_cut(60_000)],
        sends: vec![
            send(10_000, 1),
            send(180_000, 2),
            send(360_000, 3),
            send(540_000, 4),
            send(720_000, 5),
        ],
        timing: Timing::defaults(760_000),
    }
}
