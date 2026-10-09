//! The management graph, its layout, and owner network policy.

use std::collections::BTreeSet;
use std::time::Duration;

use seiche::NodeKey;
use signalman::management::{
    ManagementMaterial, ManagementNodeId, ManagementPresence, StalePolicy,
};

use crate::device_mere::{DeviceProjection, ReconcileReceipt};
use crate::network::{
    NetworkLayout, NetworkPhysics, accept_layout, input_from_projection, swatch_from_projection,
    world_from_normalized,
};

use super::{DesktopState, LabelDensity, ManagementSettings, NetworkRequest};

impl DesktopState {
    /// Apply one pure management projection to the app-owned Mere. Only a
    /// topology change restarts physics; refreshed payload and stale markings
    /// remain graph edits without disturbing the current layout.
    pub fn apply_management_material(&mut self, material: &ManagementMaterial) -> ReconcileReceipt {
        let previous_projection = self.network_projection();
        let receipt = self.device_mere.reconcile(material);
        let projection = self.network_projection();
        if self.device_mere.selected().is_some_and(|selected| {
            !projection
                .nodes
                .iter()
                .any(|node| &node.fact.id == selected)
        }) {
            self.device_mere.select(None);
        }
        if self.selected_relation.as_ref().is_some_and(|selected| {
            !projection
                .relations
                .iter()
                .any(|relation| &relation.id == selected)
        }) {
            self.selected_relation = None;
        }
        if !same_visible_topology(&previous_projection, &projection) {
            self.queue_network_reconcile(&projection);
        }
        receipt
    }

    /// Apply one live-station observation. Snapshots project under the
    /// owner's current stale policy and enter through the same
    /// `apply_management_material` door as every other source; the other
    /// variants only update the presentation status line.
    pub fn apply_station_event(&mut self, event: crate::station::StationEvent) {
        match event {
            crate::station::StationEvent::Connected {
                name,
                port,
                expires_at_ms: _,
            } => {
                self.station_notice =
                    Some(format!("Station \u{201c}{name}\u{201d} is live on {port}."));
            }
            crate::station::StationEvent::Snapshot {
                snapshot,
                captured_unix_ms,
            } => {
                let material = signalman::management::project_management(
                    &snapshot,
                    captured_unix_ms,
                    self.stale_policy(),
                );
                self.apply_management_material(&material);
            }
            crate::station::StationEvent::Failed { message } => {
                self.station_notice = Some(message);
            }
        }
    }

    pub fn network_projection(&self) -> DeviceProjection {
        let mut projection = self.device_mere.projection();
        if self.management_settings.show_last_known {
            return projection;
        }
        projection
            .nodes
            .retain(|node| node.fact.presence == ManagementPresence::Live);
        let visible = projection
            .nodes
            .iter()
            .map(|node| node.key)
            .collect::<BTreeSet<_>>();
        projection
            .relations
            .retain(|relation| visible.contains(&relation.from) && visible.contains(&relation.to));
        projection
    }

    pub fn network_swatch(
        &self,
    ) -> cambium::GraphCanvasSwatch<ManagementNodeId, signalman::management::ManagementPresence>
    {
        let projection = self.network_projection();
        swatch_from_projection(
            &projection,
            self.network_layout.as_ref(),
            self.device_mere.selected(),
            self.network_pan,
            self.network_zoom,
            self.management_settings.label_density == LabelDensity::Shown,
        )
    }

    pub fn take_network_request(&mut self) -> Option<NetworkRequest> {
        self.pending_network.take()
    }

    pub fn adopt_network_layout(&mut self, layout: NetworkLayout) -> bool {
        let Some(snapshot) = accept_layout(self.network_epoch, layout) else {
            return false;
        };
        self.network_layout = Some(snapshot);
        // An actor snapshot lags the pointer; while a drag is active the
        // echoed position stays authoritative for the dragged node so paint
        // does not flick it back to a stale physics position.
        if let Some((key, position)) = self.network_drag {
            self.echo_drag_position(key, position);
        }
        true
    }

    fn echo_drag_position(&mut self, key: NodeKey, position: euclid::default::Point2D<f32>) {
        let Some(layout) = &mut self.network_layout else {
            return;
        };
        if let Some(entry) = layout
            .positions
            .iter_mut()
            .find(|(existing, _)| *existing == key)
        {
            entry.1 = position;
        } else {
            layout.positions.push((key, position));
        }
    }

    pub fn select_network_node(&mut self, id: ManagementNodeId) {
        self.device_mere.select(Some(id));
        self.selected_relation = None;
    }

    pub fn select_network_relation(&mut self, id: &str) {
        let projection = self.network_projection();
        self.selected_relation = projection
            .relations
            .iter()
            .find(|relation| relation.id.as_str() == id)
            .map(|relation| relation.id.clone());
    }

    pub fn drag_network_node(
        &mut self,
        id: &ManagementNodeId,
        phase: cambium::PointerPhase,
        normalized: (f32, f32),
    ) {
        let Some(key) = self
            .network_projection()
            .nodes
            .iter()
            .find(|node| &node.fact.id == id)
            .map(|node| node.key)
        else {
            return;
        };
        self.pending_network = Some(match phase {
            cambium::PointerPhase::Down | cambium::PointerPhase::Move => {
                let position = world_from_normalized(normalized);
                self.network_drag = Some((key, position));
                self.echo_drag_position(key, position);
                NetworkRequest::Pin(key, position)
            }
            cambium::PointerPhase::Up => {
                self.network_drag = None;
                NetworkRequest::Unpin(key)
            }
        });
    }

    pub fn pan_network(&mut self, dx: f32, dy: f32) {
        self.network_pan.0 = (self.network_pan.0 + dx).clamp(-1.0, 1.0);
        self.network_pan.1 = (self.network_pan.1 + dy).clamp(-1.0, 1.0);
    }

    pub fn zoom_network(&mut self, factor: f32) {
        self.network_zoom = (self.network_zoom * factor).clamp(0.5, 3.0);
    }

    pub fn reset_network_view(&mut self) {
        self.network_pan = (0.0, 0.0);
        self.network_zoom = 1.0;
    }

    /// The stale policy `apply_station_event` passes to `project_management`
    /// at each capture.
    pub fn stale_policy(&self) -> StalePolicy {
        StalePolicy {
            after: Duration::from_secs(u64::from(self.management_settings.stale_age_minutes) * 60),
        }
    }

    /// Postilion reads this bound when the station opens. It has no runtime
    /// setter, so the shell labels this value as applying to the next connection.
    pub fn announce_history_bound(&self) -> usize {
        self.management_settings.announce_history_bound
    }

    pub fn shorten_stale_age(&mut self) {
        self.management_settings.stale_age_minutes = self
            .management_settings
            .stale_age_minutes
            .saturating_sub(5)
            .max(1);
    }

    pub fn lengthen_stale_age(&mut self) {
        self.management_settings.stale_age_minutes = self
            .management_settings
            .stale_age_minutes
            .saturating_add(5)
            .min(10_080);
    }

    pub fn reduce_history_bound(&mut self) {
        self.management_settings.announce_history_bound =
            (self.management_settings.announce_history_bound / 2).max(16);
    }

    pub fn increase_history_bound(&mut self) {
        self.management_settings.announce_history_bound = self
            .management_settings
            .announce_history_bound
            .saturating_mul(2)
            .min(4096);
    }

    pub fn reduce_force_strength(&mut self) {
        self.management_settings.force_strength =
            (self.management_settings.force_strength * 0.8).max(0.25);
        self.reconfigure_network_physics();
    }

    pub fn increase_force_strength(&mut self) {
        self.management_settings.force_strength =
            (self.management_settings.force_strength * 1.25).min(4.0);
        self.reconfigure_network_physics();
    }

    pub fn reduce_layout_damping(&mut self) {
        self.management_settings.layout_damping =
            (self.management_settings.layout_damping - 0.5).max(0.5);
        self.reconfigure_network_physics();
    }

    pub fn increase_layout_damping(&mut self) {
        self.management_settings.layout_damping =
            (self.management_settings.layout_damping + 0.5).min(8.0);
        self.reconfigure_network_physics();
    }

    pub fn toggle_network_labels(&mut self) {
        self.management_settings.label_density = match self.management_settings.label_density {
            LabelDensity::Hidden => LabelDensity::Shown,
            LabelDensity::Shown => LabelDensity::Hidden,
        };
    }

    pub fn toggle_last_known(&mut self) {
        self.management_settings.show_last_known = !self.management_settings.show_last_known;
        if !self.management_settings.show_last_known {
            let projection = self.network_projection();
            if self.device_mere.selected().is_some_and(|selected| {
                !projection
                    .nodes
                    .iter()
                    .any(|node| &node.fact.id == selected)
            }) {
                self.device_mere.select(None);
            }
            if self.selected_relation.as_ref().is_some_and(|selected| {
                !projection
                    .relations
                    .iter()
                    .any(|relation| &relation.id == selected)
            }) {
                self.selected_relation = None;
            }
        }
        let projection = self.network_projection();
        self.queue_network_reconcile(&projection);
    }

    pub fn reset_management_settings(&mut self) {
        let previous = self.management_settings;
        self.management_settings = ManagementSettings::default();
        if previous.force_strength != self.management_settings.force_strength
            || previous.layout_damping != self.management_settings.layout_damping
            || previous.show_last_known != self.management_settings.show_last_known
        {
            let projection = self.network_projection();
            self.queue_network_reconcile(&projection);
        }
    }

    fn reconfigure_network_physics(&mut self) {
        let projection = self.network_projection();
        self.queue_network_reconcile(&projection);
    }

    fn queue_network_reconcile(&mut self, projection: &DeviceProjection) {
        self.network_epoch = self.network_epoch.saturating_add(1);
        let physics = NetworkPhysics {
            force_strength: self.management_settings.force_strength,
            linear_damping: self.management_settings.layout_damping,
        };
        self.pending_network = Some(NetworkRequest::Reconcile(input_from_projection(
            projection,
            self.network_layout.as_ref(),
            self.network_epoch,
            physics,
        )));
    }
}

fn same_visible_topology(left: &DeviceProjection, right: &DeviceProjection) -> bool {
    left.nodes
        .iter()
        .map(|node| node.key)
        .eq(right.nodes.iter().map(|node| node.key))
        && left
            .relations
            .iter()
            .map(|relation| {
                (
                    &relation.id,
                    relation.from,
                    relation.to,
                    &relation.fact.kind,
                )
            })
            .eq(right.relations.iter().map(|relation| {
                (
                    &relation.id,
                    relation.from,
                    relation.to,
                    &relation.fact.kind,
                )
            }))
}
