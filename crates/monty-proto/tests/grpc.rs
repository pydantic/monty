//! The gRPC vocabulary: the cause trailer's wire strings, which servers in
//! other repositories and the bindings' tests pin, and its status round trip.

use monty_proto::grpc::{CAUSE_METADATA_KEY, Cause};
use tonic::{Code, Status};

/// Every cause, with the trailer value and code the wire has carried since
/// the transport was introduced.
const CAUSES: [(Cause, &str, Code); 11] = [
    (Cause::IdleTimeout, "idle_timeout", Code::DeadlineExceeded),
    (Cause::SessionTimeout, "session_timeout", Code::DeadlineExceeded),
    (Cause::TurnTimeout, "turn_timeout", Code::DeadlineExceeded),
    (Cause::OutOfMemory, "out_of_memory", Code::ResourceExhausted),
    (Cause::ServerShutdown, "server_shutdown", Code::Unavailable),
    (Cause::Capacity, "capacity", Code::ResourceExhausted),
    (Cause::CallerCap, "caller_cap", Code::ResourceExhausted),
    (Cause::Unauthenticated, "unauthenticated", Code::Unauthenticated),
    (Cause::PermissionDenied, "permission_denied", Code::PermissionDenied),
    (Cause::WorkerUnavailable, "worker_unavailable", Code::Unavailable),
    (Cause::ProtocolViolation, "protocol_violation", Code::InvalidArgument),
];

#[test]
fn causes_keep_their_wire_strings_and_codes() {
    for (cause, wire, code) in CAUSES {
        assert_eq!(cause.as_str(), wire);
        assert_eq!(cause.to_string(), wire);
        assert_eq!(wire.parse::<Cause>(), Ok(cause));
        assert_eq!(cause.code(), code);
    }
    assert!("IdleTimeout".parse::<Cause>().is_err());
}

#[test]
fn a_status_carries_its_cause_as_a_trailer() {
    for (cause, wire, code) in CAUSES {
        let status = cause.status("why");
        assert_eq!(status.code(), code);
        assert_eq!(status.message(), "why");
        assert_eq!(status.metadata().get(CAUSE_METADATA_KEY).unwrap(), wire);
        assert_eq!(Cause::of(&status), Some(cause));
    }
    assert_eq!(Cause::of(&Status::new(Code::Unavailable, "plain")), None);
    let mut unknown = Status::new(Code::Unavailable, "unknown cause");
    unknown.metadata_mut().insert(CAUSE_METADATA_KEY, "flood".parse().unwrap());
    assert_eq!(Cause::of(&unknown), None);
}
