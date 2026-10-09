//! Black-box acceptance for the radio-free personality controller.
//!
//! The fake adapters retain local state and expose pause outcomes. They do not
//! model a radio, a remote session, or protocol compatibility.

mod admission;
mod coverage;
mod recovery;
mod returns;

use tulle::personality::{
    Controller, ControllerConfig, CoveragePolicy, Excursion, InstalledPersonalitySet,
    InterruptionPolicy, PauseOutcome, PersonalityId, StopCapability,
};

const HOME: PersonalityId = PersonalityId(1);
const OTHER: PersonalityId = PersonalityId(2);

#[derive(Debug)]
struct FakeAdapter {
    id: PersonalityId,
    local_state: u32,
    pause: PauseOutcome,
    stop: StopCapability,
}

impl FakeAdapter {
    fn new(id: PersonalityId) -> Self {
        Self {
            id,
            local_state: 0,
            pause: PauseOutcome::Ready,
            stop: StopCapability::Resumable,
        }
    }
    fn pause(&self) -> PauseOutcome {
        self.pause
    }
    fn stop(&self) -> StopCapability {
        self.stop
    }
    fn visit(&mut self) {
        self.local_state += 1;
    }
}

fn controller(coverage: CoveragePolicy) -> Controller {
    let installed = InstalledPersonalitySet::new(&[HOME, OTHER]).unwrap();
    Controller::new(
        ControllerConfig {
            home: HOME,
            pin: None,
            installed,
            coverage,
            max_excursion_ms: 100,
            return_budget_ms: 20,
            max_defer_ms: 10,
            transition_timeout_ms: 5,
        },
        0,
    )
    .unwrap()
}

fn request(policy: InterruptionPolicy) -> Excursion {
    Excursion {
        target: OTHER,
        duration_ms: 30,
        interruption: policy,
    }
}
