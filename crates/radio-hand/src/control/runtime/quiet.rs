use super::{AbSlotStore, ConfigApplier};

/// Board-owned proof that live flash access is safe.
///
/// `enter` must stop the radio at a frame boundary and disarm every RX/TX/IRQ transaction. An
/// `Err` from `enter` is permitted only before stopping begins and must leave ordinary operation
/// unchanged. Once stopping begins, cancellation or failure must leave the board stopped or
/// synchronously force/latch a hardware reset; it must not return a retryable `Err`. A successful
/// guard remains held through erase, program, readback, and apply. T114 and V4 need a combined
/// owner/refactor because their live radio and store are not independently borrowable; this is
/// the portable seam only.
#[allow(async_fn_in_trait)]
pub trait QuietWindow {
    /// Error returned while stopping or resuming the board.
    type Error;
    /// Error returned by the guard's durable A/B store.
    type StoreError;
    /// Error returned by the guard's hardware configuration applier.
    type ApplyError;
    type Guard<'a>: QuietGuard<Error = Self::Error>
        + AbSlotStore<Error = Self::StoreError>
        + ConfigApplier<Error = Self::ApplyError>
    where
        Self: 'a;

    async fn enter(&mut self) -> Result<Self::Guard<'_>, Self::Error>;
}

/// A borrow-scoped live flash quiet window.
#[allow(async_fn_in_trait)]
pub trait QuietGuard {
    type Error;

    /// Synchronously leave the board safe when an entered live operation is abandoned.
    ///
    /// This must not await. It normally latches or forces reset and keeps radio work stopped, so
    /// the board cannot resume with an uncertain flash or radio configuration.
    fn abort(&mut self);

    /// Finish after all slot I/O and hardware application are done.
    ///
    /// A dropped `finish` future is an abort: callers retain the guard until this returns `Ok`.
    async fn finish(&mut self) -> Result<QuietExit, Self::Error>;
}

pub(super) struct ActiveQuietGuard<G: QuietGuard> {
    guard: G,
    armed: bool,
}

impl<G: QuietGuard> ActiveQuietGuard<G> {
    pub(super) fn new(guard: G) -> Self {
        Self { guard, armed: true }
    }

    pub(super) async fn finish(&mut self) -> Result<QuietExit, G::Error> {
        let exit = self.guard.finish().await?;
        self.armed = false;
        Ok(exit)
    }

    pub(super) fn inner_mut(&mut self) -> &mut G {
        &mut self.guard
    }
}

impl<G: QuietGuard> Drop for ActiveQuietGuard<G> {
    fn drop(&mut self) {
        if self.armed {
            self.guard.abort();
        }
    }
}

/// Whether ordinary operation may resume after a live control transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuietExit {
    Resumed,
    ResetRequired,
}

/// A successful live transition and the board action required before another one.
pub struct LiveOutcome<T> {
    pub(super) value: T,
    pub(super) exit: QuietExit,
}

impl<T> LiveOutcome<T> {
    pub const fn value(&self) -> &T {
        &self.value
    }
    pub fn into_value(self) -> T {
        self.value
    }
    pub const fn exit(&self) -> QuietExit {
        self.exit
    }
}
