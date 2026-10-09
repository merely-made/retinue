//! Availability captures, the observation collector and retention settings.

use crate::availability::AvailabilityCapture;

use super::{DesktopState, ObservationRequest};

impl DesktopState {
    pub fn request_observation_load(&mut self) {
        self.pending_observation = Some(ObservationRequest::Load);
    }

    pub fn request_observation_export(&mut self) {
        self.pending_observation = Some(ObservationRequest::Export);
    }

    pub fn request_observation_start(&mut self) {
        if self.observation_collecting {
            self.observation_notice = Some("Observation collection is already running.".into());
            return;
        }
        if self.install_running {
            self.observation_notice =
                Some("Finish the active installation before borrowing its serial device.".into());
            return;
        }
        let Some(device) = self.device() else {
            self.observation_notice = Some("Select a surveyed serial device first.".into());
            return;
        };
        let port = device.port.clone();
        if std::env::var("SIGNALMAN_STATION_PORT")
            .ok()
            .is_some_and(|station| port.eq_ignore_ascii_case(station.trim()))
        {
            self.observation_notice = Some("The live station owns that serial port. Stop the station before collecting observations from it.".into());
            return;
        }
        let association = self.observation_device_association.text().trim().to_owned();
        if association.is_empty() {
            self.observation_notice =
                Some("Enter the stable local device association for this board.".into());
            return;
        }
        self.pending_observation = Some(ObservationRequest::StartCollector {
            port: port.clone(),
            association,
        });
        self.observation_collecting = true;
        self.observation_notice = Some(format!("Collecting read-only observations from {}…", port));
    }

    pub fn request_observation_stop(&mut self) {
        if self.observation_collecting {
            self.pending_observation = Some(ObservationRequest::StopCollector);
            self.observation_notice = Some("Stopping observation collection…".into());
        }
    }

    pub fn observation_collector_stopped(&mut self, notice: String) {
        self.observation_collecting = false;
        self.observation_notice = Some(notice);
    }

    pub fn take_observation_request(&mut self) -> Option<ObservationRequest> {
        self.pending_observation.take()
    }

    pub fn adopt_availability(&mut self, capture: AvailabilityCapture) {
        self.availability.push(capture);
        self.selected_availability = Some(self.availability.len() - 1);
        self.observation_notice = Some("Observation capture loaded and replayed.".into());
    }

    pub fn select_availability(&mut self, index: usize) {
        if index < self.availability.len() {
            self.selected_availability = Some(index);
        }
    }

    pub fn toggle_observation_durable(&mut self) {
        self.observation_durable = !self.observation_durable;
        self.pending_observation = Some(ObservationRequest::SaveSettings);
        self.observation_notice = Some(
            if self.observation_durable {
                "Durable capture requested. Storage status is reported when a capture arrives."
            } else {
                "Automatic durable capture is off. Explicit export remains available."
            }
            .into(),
        );
    }

    pub fn cycle_observation_entry_bound(&mut self) {
        self.observation_retention_entries = match self.observation_retention_entries {
            0..=1_024 => 4_096,
            1_025..=4_096 => 16_384,
            _ => 1_024,
        };
        self.pending_observation = Some(ObservationRequest::SaveSettings);
    }

    pub fn cycle_observation_byte_bound(&mut self) {
        self.observation_retention_bytes = match self.observation_retention_bytes {
            0..=131_072 => 524_288,
            131_073..=524_288 => 2_097_152,
            _ => 131_072,
        };
        self.pending_observation = Some(ObservationRequest::SaveSettings);
    }

    pub fn cycle_observation_age_bound(&mut self) {
        const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
        self.observation_retention_age_ms = match self.observation_retention_age_ms {
            0..=DAY_MS => 7 * DAY_MS,
            value if value <= 7 * DAY_MS => 30 * DAY_MS,
            _ => DAY_MS,
        };
        self.pending_observation = Some(ObservationRequest::SaveSettings);
    }
}
