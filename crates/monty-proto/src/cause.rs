//! Why a remote server ended a session or refused to open one, shared by both
//! remote transports.
//!
//! gRPC carries a [`Cause`] as the [`CAUSE_METADATA_KEY`] trailer beside its
//! status code (see [`crate::grpc`]). WebSocket carries it as a refused
//! upgrade's [`CAUSE_METADATA_KEY`] header, or as the Close frame's code
//! ([`Cause::close_code`]), so a client never parses free-text reasons.

use strum::{Display, EnumIter, EnumString, FromRepr, IntoStaticStr};

/// Metadata key naming a [`Cause`]: a gRPC trailer, or a header on a refused
/// WebSocket upgrade.
pub const CAUSE_METADATA_KEY: &str = "monty-cause";

/// Why a server ended a session, or refused to open one. Its string form, the
/// variant in `snake_case`, is the metadata value; its discriminant is the
/// WebSocket close code.
///
/// Both are wire contract shared with servers in other repositories: never
/// rename or renumber a variant, only append.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Display, EnumIter, EnumString, FromRepr, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
#[repr(u16)]
pub enum Cause {
    /// No request arrived within the server's idle timeout.
    IdleTimeout = 4000,
    /// The session outlived the server's lifetime limit.
    SessionTimeout = 4001,
    /// One turn exceeded the server's wall-clock cap.
    TurnTimeout = 4002,
    /// The worker exceeded its memory limit; the server ended the session.
    OutOfMemory = 4003,
    /// The server is draining and the idle session had nothing to hand back.
    ServerShutdown = 4004,
    /// The server's session capacity is full.
    Capacity = 4005,
    /// The caller already holds its maximum number of sessions.
    CallerCap = 4006,
    /// The caller presented no acceptable credentials.
    Unauthenticated = 4007,
    /// The caller's credential is valid but not allowed this project or profile.
    PermissionDenied = 4008,
    /// No worker could be reached to serve the session.
    WorkerUnavailable = 4009,
    /// The client broke the protocol (a malformed frame, a second request mid-turn).
    ProtocolViolation = 4010,
}

impl Cause {
    /// The metadata value: the variant in `snake_case`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// The WebSocket close code a server ends a session with for this cause,
    /// in RFC 6455's private-use range (4000–4999).
    #[must_use]
    pub fn close_code(self) -> u16 {
        self as u16
    }

    /// The cause a WebSocket close code stands for; `None` for any other
    /// code, including one a newer server defined.
    #[must_use]
    pub fn from_close_code(code: u16) -> Option<Self> {
        Self::from_repr(code)
    }
}
