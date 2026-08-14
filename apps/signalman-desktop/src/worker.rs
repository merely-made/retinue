//! Compatibility names for Signalman's owned installer worker.
//!
//! The desktop receives updates and drains them on Cambium's host thread, but
//! Signalman owns the blocking thread, helper processes, and Linkboy executor
//! call. Keeping the alias here lets the face describe its small integration
//! seam without recreating an execution layer of its own.

pub use signalman::FirmwareInstallWorker as Worker;
