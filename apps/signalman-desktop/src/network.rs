//! Off-UI-thread force layout and the shared Network projection.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::Duration;

use cambium::{
    GraphCanvasEdge, GraphCanvasNode, GraphCanvasRelation, GraphCanvasSubgraph, GraphCanvasSwatch,
    GraphViewport,
};
use euclid::default::Point2D;
use seiche::{Boundary, EdgeSpring, LayoutSnapshot, NodeExclusion, NodeKey, Simulation};
use signalman::management::{ManagementNodeId, ManagementPresence};

use crate::device_mere::DeviceProjection;

pub type LayoutWake = Arc<dyn Fn() + Send + Sync>;

pub const NETWORK_LEAF_KEY: u64 = 0x5349_474e_4554;
const NETWORK_WIDTH: f32 = 760.0;
const NETWORK_HEIGHT: f32 = 360.0;
#[cfg(not(test))]
const SETTLE_TICKS: usize = 180;
#[cfg(test)]
const SETTLE_TICKS: usize = 4;

/// Build the Sprigging paint half of Signalman's Network scene.
///
/// The matching semantic targets live in Cambium's retained `graph_canvas`
/// view. Keeping this builder in the library lets the desktop binary and its
/// mixed-realization receipt exercise the same product palette and projection.
pub fn paint_network_leaf(
    swatch: &GraphCanvasSwatch<ManagementNodeId, ManagementPresence>,
) -> sprigging::GraphCanvas {
    swatch.paint_leaf(|presence| match presence {
        ManagementPresence::Live => sprigging::ColorF {
            r: 0.35,
            g: 0.72,
            b: 0.56,
            a: 1.0,
        },
        ManagementPresence::Stale => sprigging::ColorF {
            r: 0.43,
            g: 0.46,
            b: 0.52,
            a: 1.0,
        },
    })
}

/// Owner-selected physics values. `force_strength` scales the three public
/// Seiche force defaults together; damping is applied directly to every body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NetworkPhysics {
    pub force_strength: f32,
    pub linear_damping: f32,
}

impl Default for NetworkPhysics {
    fn default() -> Self {
        Self {
            force_strength: 1.0,
            linear_damping: 2.5,
        }
    }
}

/// Topology handed to the physics actor. Existing keys retain their positions;
/// the supplied points seed only new bodies unless physics itself changed.
#[derive(Clone, Debug, PartialEq)]
pub struct NetworkInput {
    pub epoch: u64,
    pub nodes: Vec<(NodeKey, Point2D<f32>)>,
    pub edges: Vec<(NodeKey, NodeKey)>,
    pub physics: NetworkPhysics,
}

/// Latest actor result. `epoch` rejects a result from superseded presentation
/// topology or settings.
#[derive(Clone, Debug, PartialEq)]
pub struct NetworkLayout {
    pub epoch: u64,
    pub snapshot: LayoutSnapshot,
    pub worker_thread: ThreadId,
}

enum Command {
    Reconcile(NetworkInput),
    Pin(NodeKey, Point2D<f32>),
    Unpin(NodeKey),
    Stop,
}

type Latest = Arc<(Mutex<Option<NetworkLayout>>, Condvar)>;

/// One Seiche simulation owned by one ordinary worker thread.
pub struct NetworkWorker {
    commands: Sender<Command>,
    latest: Latest,
    worker_thread: ThreadId,
    join: Option<JoinHandle<()>>,
}

impl NetworkWorker {
    pub fn spawn(wake: LayoutWake) -> Self {
        let (commands, receiver) = mpsc::channel();
        let latest = Arc::new((Mutex::new(None), Condvar::new()));
        let actor_latest = Arc::clone(&latest);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("signalman-network-layout".to_owned())
            .spawn(move || {
                let worker_thread = thread::current().id();
                let _ = ready_tx.send(worker_thread);
                let mut physics = NetworkPhysics::default();
                let mut simulation = configured_simulation(physics);
                let mut active_epoch = None;
                let mut ticks_remaining = 0usize;
                let mut running = true;

                while running {
                    let command = if ticks_remaining == 0 {
                        receiver.recv().ok()
                    } else {
                        match receiver.recv_timeout(Duration::from_millis(16)) {
                            Ok(command) => Some(command),
                            Err(RecvTimeoutError::Timeout) => None,
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    };

                    match command {
                        Some(Command::Reconcile(input)) => {
                            if active_epoch.is_none_or(|epoch| input.epoch > epoch) {
                                if input.physics != physics {
                                    physics = input.physics;
                                    simulation = configured_simulation(physics);
                                }
                                simulation.sync_nodes(input.nodes);
                                simulation.sync_edges(input.edges);
                                active_epoch = Some(input.epoch);
                                ticks_remaining = SETTLE_TICKS;
                            }
                        }
                        Some(Command::Pin(node, position)) => {
                            simulation.pin(node, position);
                            ticks_remaining = SETTLE_TICKS;
                        }
                        Some(Command::Unpin(node)) => {
                            simulation.unpin(node);
                            ticks_remaining = SETTLE_TICKS;
                        }
                        Some(Command::Stop) => running = false,
                        None => {}
                    }

                    if running && ticks_remaining > 0 {
                        if let Some(epoch) = active_epoch {
                            simulation.tick(1.0 / 60.0);
                            ticks_remaining -= 1;
                            let layout = NetworkLayout {
                                epoch,
                                snapshot: simulation.snapshot(epoch),
                                worker_thread,
                            };
                            let (slot, changed) = &*actor_latest;
                            *slot.lock().unwrap() = Some(layout);
                            changed.notify_all();
                            wake();
                        }
                    }
                }
            })
            .expect("spawn Signalman layout actor");
        let worker_thread = ready_rx
            .recv()
            .expect("layout actor reports its thread before use");
        Self {
            commands,
            latest,
            worker_thread,
            join: Some(join),
        }
    }

    pub fn worker_thread(&self) -> ThreadId {
        self.worker_thread
    }

    pub fn reconcile(&self, input: NetworkInput) -> bool {
        self.commands.send(Command::Reconcile(input)).is_ok()
    }

    pub fn pin(&self, node: NodeKey, position: Point2D<f32>) -> bool {
        self.commands.send(Command::Pin(node, position)).is_ok()
    }

    pub fn unpin(&self, node: NodeKey) -> bool {
        self.commands.send(Command::Unpin(node)).is_ok()
    }

    /// Take the single latest layout, dropping all intermediate ticks.
    pub fn take_latest(&self) -> Option<NetworkLayout> {
        self.latest.0.lock().unwrap().take()
    }

    #[cfg(test)]
    fn wait_latest(&self, timeout: Duration) -> Option<NetworkLayout> {
        let (slot, changed) = &*self.latest;
        let guard = slot.lock().unwrap();
        let (mut guard, _) = changed
            .wait_timeout_while(guard, timeout, |latest| latest.is_none())
            .unwrap();
        guard.take()
    }
}

impl Drop for NetworkWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn configured_simulation(physics: NetworkPhysics) -> Simulation {
    let mut simulation = Simulation::new();
    let mut exclusion = NodeExclusion::default();
    exclusion.strength *= physics.force_strength;
    let mut spring = EdgeSpring::default();
    spring.stiffness *= physics.force_strength;
    let mut boundary = Boundary::default();
    boundary.strength *= physics.force_strength;
    simulation.add_force(exclusion);
    simulation.add_force(spring);
    simulation.add_force(boundary);
    simulation.set_linear_damping(physics.linear_damping);
    simulation
}

/// Build the one actor input from the same projection used by the canvas and
/// companion list. A retained layout seeds a physics-settings rebuild in place.
pub fn input_from_projection(
    projection: &DeviceProjection,
    layout: Option<&LayoutSnapshot>,
    epoch: u64,
    physics: NetworkPhysics,
) -> NetworkInput {
    let retained = layout
        .map(|snapshot| {
            snapshot
                .positions
                .iter()
                .copied()
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let count = projection.nodes.len();
    let nodes = projection
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            (
                node.key,
                retained
                    .get(&node.key)
                    .copied()
                    .unwrap_or_else(|| seed_position(index, count)),
            )
        })
        .collect();
    let edges = projection
        .relations
        .iter()
        .map(|relation| (relation.from, relation.to))
        .collect();
    NetworkInput {
        epoch,
        nodes,
        edges,
        physics,
    }
}

/// Build canvas paint and interaction material from the canonical projection.
pub fn swatch_from_projection(
    projection: &DeviceProjection,
    layout: Option<&LayoutSnapshot>,
    selected: Option<&ManagementNodeId>,
    pan: (f32, f32),
    zoom: f32,
    show_labels: bool,
) -> GraphCanvasSwatch<ManagementNodeId, ManagementPresence> {
    let positions = layout
        .map(|snapshot| {
            snapshot
                .positions
                .iter()
                .copied()
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let count = projection.nodes.len();
    let ids_by_key = projection
        .nodes
        .iter()
        .map(|node| (node.key, node.fact.id.clone()))
        .collect::<BTreeMap<_, _>>();
    let nodes = projection
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let world = positions
                .get(&node.key)
                .copied()
                .unwrap_or_else(|| seed_position(index, count));
            let presence = match node.fact.presence {
                ManagementPresence::Live => "live",
                ManagementPresence::Stale => "stale",
            };
            GraphCanvasNode {
                id: node.fact.id.clone(),
                kind: node.fact.presence,
                position: normalized_from_world(world),
                label: format!("{}; {presence}", node.fact.label),
                key: Some(node.fact.id.as_str().to_owned()),
            }
        })
        .collect::<Vec<_>>();
    let relations = projection
        .relations
        .iter()
        .filter_map(|relation| {
            Some(GraphCanvasRelation {
                id: relation.id.as_str().to_owned(),
                from: ids_by_key.get(&relation.from)?.clone(),
                to: ids_by_key.get(&relation.to)?.clone(),
                kind: relation.fact.kind.vocabulary().to_owned(),
                label: relation.fact.label.clone(),
                // No authored polyline: the canvas derives the straight route
                // between the two endpoints and fans siblings itself.
                route: Vec::new(),
                visible: true,
                emphasized: false,
            })
        })
        .collect::<Vec<_>>();
    let edges = relations
        .iter()
        .map(|relation| GraphCanvasEdge {
            from: relation.from.clone(),
            to: relation.to.clone(),
        })
        .collect();
    let mut swatch = GraphCanvasSwatch::new(NETWORK_LEAF_KEY, GraphCanvasSubgraph { nodes, edges })
        .with_size(NETWORK_WIDTH as u32, NETWORK_HEIGHT as u32)
        .with_label("Observed network")
        .with_expand(false)
        .with_node_labels(show_labels)
        .with_relations(relations);
    swatch.selected = selected.cloned();
    swatch.viewport = GraphViewport { pan, zoom };
    swatch
}

pub fn world_from_normalized(position: (f32, f32)) -> Point2D<f32> {
    Point2D::new(
        (position.0 - 0.5) * NETWORK_WIDTH,
        (position.1 - 0.5) * NETWORK_HEIGHT,
    )
}

fn normalized_from_world(position: Point2D<f32>) -> (f32, f32) {
    (
        (position.x / NETWORK_WIDTH + 0.5).clamp(0.0, 1.0),
        (position.y / NETWORK_HEIGHT + 0.5).clamp(0.0, 1.0),
    )
}

fn seed_position(index: usize, count: usize) -> Point2D<f32> {
    if count <= 1 {
        return Point2D::new(0.0, 0.0);
    }
    let angle = std::f32::consts::TAU * index as f32 / count as f32;
    Point2D::new(angle.cos() * 120.0, angle.sin() * 120.0)
}

/// Apply only layouts produced for the current presentation epoch.
pub fn accept_layout(current_epoch: u64, layout: NetworkLayout) -> Option<LayoutSnapshot> {
    (layout.epoch == current_epoch).then_some(layout.snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn input(epoch: u64) -> NetworkInput {
        NetworkInput {
            epoch,
            nodes: vec![
                (NodeKey::new(0), Point2D::new(-20.0, 0.0)),
                (NodeKey::new(1), Point2D::new(20.0, 0.0)),
            ],
            edges: vec![(NodeKey::new(0), NodeKey::new(1))],
            physics: NetworkPhysics::default(),
        }
    }

    #[test]
    fn actor_runs_off_thread_and_keeps_only_the_latest_snapshot() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let wake_count = Arc::clone(&wakes);
        let worker = NetworkWorker::spawn(Arc::new(move || {
            wake_count.fetch_add(1, Ordering::Relaxed);
        }));
        assert_ne!(worker.worker_thread(), thread::current().id());
        assert!(worker.reconcile(input(1)));
        let first = worker
            .wait_latest(Duration::from_secs(1))
            .expect("actor published a layout");
        assert_eq!(first.epoch, 1);
        assert_eq!(first.worker_thread, worker.worker_thread());
        assert_eq!(first.snapshot.positions.len(), 2);

        let _ = worker.wait_latest(Duration::from_secs(1));
        std::thread::yield_now();
        let latest = worker
            .wait_latest(Duration::from_secs(1))
            .expect("latest tick remains available");
        assert_eq!(latest.epoch, 1);
        assert!(
            worker.take_latest().is_none(),
            "the slot was taken, not queued"
        );
        assert!(wakes.load(Ordering::Relaxed) > 0);

        std::thread::sleep(Duration::from_millis(100));
        let _ = worker.take_latest();
        let settled_wakes = wakes.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            wakes.load(Ordering::Relaxed),
            settled_wakes,
            "the finite settle budget parks the actor"
        );
        assert!(worker.take_latest().is_none());
    }

    #[test]
    fn stale_epochs_are_ignored_and_pin_unpin_use_the_same_actor() {
        let worker = NetworkWorker::spawn(Arc::new(|| {}));
        worker.reconcile(input(2));
        let settled = worker
            .wait_latest(Duration::from_secs(1))
            .expect("epoch two layout");
        assert_eq!(settled.epoch, 2);

        worker.reconcile(input(1));
        let still_two = worker
            .wait_latest(Duration::from_secs(1))
            .expect("actor keeps ticking current topology");
        assert_eq!(still_two.epoch, 2);
        assert!(accept_layout(1, still_two.clone()).is_none());
        assert!(accept_layout(2, still_two).is_some());

        let pinned = Point2D::new(90.0, -30.0);
        assert!(worker.pin(NodeKey::new(0), pinned));
        let pinned_layout = worker
            .wait_latest(Duration::from_secs(1))
            .expect("pin produced a newer layout");
        let position = pinned_layout
            .snapshot
            .positions
            .iter()
            .find(|(key, _)| *key == NodeKey::new(0))
            .map(|(_, position)| *position)
            .unwrap();
        assert_eq!(position, pinned);
        assert!(worker.unpin(NodeKey::new(0)));
    }

    #[test]
    fn physics_change_is_an_explicit_new_epoch() {
        let worker = NetworkWorker::spawn(Arc::new(|| {}));
        worker.reconcile(input(1));
        worker
            .wait_latest(Duration::from_secs(1))
            .expect("first physics layout");
        let mut changed = input(2);
        changed.physics.force_strength = 1.5;
        changed.physics.linear_damping = 4.0;
        assert!(worker.reconcile(changed));
        let layout = worker
            .wait_latest(Duration::from_secs(1))
            .expect("reconfigured layout");
        assert_eq!(layout.epoch, 2);
    }
}
