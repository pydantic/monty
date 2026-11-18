//! `Cause`'s WebSocket close codes: wire contract with servers in other
//! repositories, so each is pinned as a literal number.

use monty_proto::Cause;
use strum::IntoEnumIterator as _;

/// Every cause with the close code a WebSocket server ends a session with.
const CLOSE_CODES: [(Cause, u16); 11] = [
    (Cause::IdleTimeout, 4000),
    (Cause::SessionTimeout, 4001),
    (Cause::TurnTimeout, 4002),
    (Cause::OutOfMemory, 4003),
    (Cause::ServerShutdown, 4004),
    (Cause::Capacity, 4005),
    (Cause::CallerCap, 4006),
    (Cause::Unauthenticated, 4007),
    (Cause::PermissionDenied, 4008),
    (Cause::WorkerUnavailable, 4009),
    (Cause::ProtocolViolation, 4010),
];

#[test]
fn causes_keep_their_close_codes() {
    // every variant has a row: a new cause must be pinned here
    for cause in Cause::iter() {
        assert!(
            CLOSE_CODES.iter().any(|(listed, _)| *listed == cause),
            "{cause:?} has no row"
        );
    }
    for (cause, code) in CLOSE_CODES {
        assert_eq!(cause.close_code(), code);
        assert_eq!(Cause::from_close_code(code), Some(cause));
    }
}

#[test]
fn other_close_codes_have_no_cause() {
    for code in [1000, 1001, 1006, 1011, 3999, 4011, 4999] {
        assert_eq!(Cause::from_close_code(code), None);
    }
}
