//! Structured execution beneath CLI and graphical faces.

mod device;
mod event;
mod process;
mod run;
#[cfg(test)]
mod tests;
mod uf2_volume;

use std::time::Duration;

pub use device::{DeviceFailure, DeviceRunner, LiveDeviceRunner};
pub use event::{ExecutionError, ExecutionStage, FlashEvent, RecoveryFacts};
pub use process::{
    ProcessFailure, ProcessOutput, ProcessProgress, ProcessRunner, SystemProcessRunner,
};
pub use run::execute_plan;

pub const DEFAULT_PATIENCE: Duration = Duration::from_secs(12);
