//! The WebSocket close codes a server uses to say why it ended a session.

use std::fmt;

/// Why a WebSocket server ended a session, as the status code of the Close
/// frame it sent.
///
/// The codes are wire contract: a client maps them back to causes without
/// parsing the frame's free-text reason, so renumbering one makes older
/// clients misreport. They sit in RFC 6455's private-use range (4000–4999);
/// no registered code describes a policy limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum CloseCause {
    /// No request arrived within the server's idle timeout.
    IdleTimeout = 4000,
    /// The session outlived the server's session lifetime limit.
    SessionTimeout = 4001,
    /// One request outlived the server's turn timeout.
    TurnTimeout = 4002,
    /// The worker exceeded its memory limit and was terminated.
    OutOfMemory = 4003,
    /// A request frame exceeded the server's size cap.
    RequestTooLarge = 4004,
    /// The server reclaimed the session's capacity.
    Evicted = 4005,
}

impl CloseCause {
    /// Every cause, in code order.
    pub const ALL: [Self; 6] = [
        Self::IdleTimeout,
        Self::SessionTimeout,
        Self::TurnTimeout,
        Self::OutOfMemory,
        Self::RequestTooLarge,
        Self::Evicted,
    ];

    /// The close status code standing for this cause.
    #[must_use]
    pub fn code(self) -> u16 {
        self as u16
    }

    /// The cause a close status code stands for; `None` for any other code,
    /// including one a newer server defined.
    #[must_use]
    pub fn from_code(code: u16) -> Option<Self> {
        Self::ALL.into_iter().find(|cause| cause.code() == code)
    }

    /// The cause's stable snake_case name, e.g. `idle_timeout`, for bindings
    /// that expose it as a string.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::IdleTimeout => "idle_timeout",
            Self::SessionTimeout => "session_timeout",
            Self::TurnTimeout => "turn_timeout",
            Self::OutOfMemory => "out_of_memory",
            Self::RequestTooLarge => "request_too_large",
            Self::Evicted => "evicted",
        }
    }

    /// A one-line description, suitable as the Close frame's reason text.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::IdleTimeout => "the session was idle for longer than the server allows",
            Self::SessionTimeout => "the session outlived the server's session lifetime limit",
            Self::TurnTimeout => "the request outlived the server's turn timeout",
            Self::OutOfMemory => "the worker exceeded its memory limit and was terminated",
            Self::RequestTooLarge => "the request frame exceeded the server's size cap",
            Self::Evicted => "the server reclaimed the session's capacity",
        }
    }
}

impl fmt::Display for CloseCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.description())
    }
}
