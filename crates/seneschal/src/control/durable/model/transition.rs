use core::fmt;

use crate::control::Response;

/// Whether a durable request changed state or was answered from its retained receipt.
///
/// Runtimes must journal and apply only [`Self::Changed`]. Replaying a successful request is
/// deliberately side-effect free.
#[derive(Clone, PartialEq, Eq)]
pub enum Transition {
    Changed(Response),
    Replayed(Response),
}

impl Transition {
    pub(super) const fn changed(response: Response) -> Self {
        Self::Changed(response)
    }

    pub(super) const fn replayed(response: Response) -> Self {
        Self::Replayed(response)
    }

    /// Whether the durable state changed and still requires runtime persistence.
    pub const fn is_changed(&self) -> bool {
        matches!(self, Self::Changed(_))
    }

    /// The protocol response, independent of whether it was newly produced or replayed.
    pub const fn response(&self) -> &Response {
        match self {
            Self::Changed(response) | Self::Replayed(response) => response,
        }
    }

    /// Consume the transition and return its response.
    pub fn into_response(self) -> Response {
        match self {
            Self::Changed(response) | Self::Replayed(response) => response,
        }
    }
}

impl fmt::Debug for Transition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Changed(response) => formatter.debug_tuple("Changed").field(response).finish(),
            Self::Replayed(response) => formatter.debug_tuple("Replayed").field(response).finish(),
        }
    }
}
