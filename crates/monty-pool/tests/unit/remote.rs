//! Remote transport window sizing, connection diagnostics, and how WebSocket
//! refusals and Close frames map onto `RemoteDisconnect`.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use h2::{Error as H2Error, Reason};
use http::Response;
use monty_proto::Cause;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, frame::coding::CloseCode};
use tonic::Status;

use super::{
    RemoteDisconnect,
    grpc::{
        MAX_WINDOW_BYTES, MIN_CONNECTION_WINDOW_BYTES, MIN_STREAM_WINDOW_BYTES, STREAM_WINDOW_BYTES, connection_window,
        stream_window,
    },
    retry_after,
    websocket::{close_disconnect, refusal},
};
use crate::PoolError;

/// Every stream a pool can hold, live and closing, stalled on a full stream
/// window still leaves a stream window of credit for a neighbour's reply, up
/// to and past the size where the stream window has to shrink.
#[test]
fn stalled_streams_never_exhaust_the_connection_window() {
    for max_processes in [1, 16, 32, 100, 511, 512, 4_000, 16_000] {
        let stream = u64::from(stream_window(max_processes));
        let held = 2 * max_processes as u64 * stream;
        let spare = u64::from(connection_window(max_processes)) - held;
        assert!(spare >= stream, "max_processes = {max_processes}");
    }
}

#[test]
fn the_windows_stay_within_http2_bounds() {
    assert_eq!(stream_window(1), STREAM_WINDOW_BYTES);
    assert_eq!(connection_window(0), MIN_CONNECTION_WINDOW_BYTES);
    assert_eq!(connection_window(1), MIN_CONNECTION_WINDOW_BYTES);
    assert_eq!(stream_window(usize::MAX), MIN_STREAM_WINDOW_BYTES);
    assert_eq!(connection_window(usize::MAX), MAX_WINDOW_BYTES);
}

#[test]
fn retry_after_accepts_delay_seconds_and_http_dates() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_445_412_478);
    for (value, expected) in [
        ("0", Some(0)),
        ("120", Some(120)),
        ("Wed, 21 Oct 2015 07:28:00 GMT", Some(2)),
        ("Wed, 21 Oct 2015 07:27:00 GMT", Some(0)),
        ("", None),
        ("-1", None),
        ("+1", None),
        ("1.5", None),
        ("3600", Some(3600)),
        ("3601", Some(3600)),
        ("18446744073709551616", Some(3600)),
        ("Wed, 21 Oct 2099 07:28:00 GMT", Some(3600)),
        ("not a date", None),
    ] {
        assert_eq!(retry_after(value, now), expected.map(Duration::from_secs));
    }
}

#[test]
fn plaintext_framing_failure_suggests_https() {
    let mut status = Status::unknown("transport error");
    status.set_source(Arc::new(H2Error::from(Reason::FRAME_SIZE_ERROR)));
    for url in ["http://example.com", "HTTP://example.com"] {
        let details = RemoteDisconnect::from_connect_status(url, &status);
        insta::allow_duplicates! {
            insta::assert_snapshot!(details.to_string(), @"UNKNOWN: transport error: protocol error: frame with invalid size; the server may require https:// instead of http://");
        }
    }
    let details = RemoteDisconnect::from_connect_status("https://example.com", &status);
    insta::assert_snapshot!(details.to_string(), @"UNKNOWN: transport error: protocol error: frame with invalid size");
    let details =
        RemoteDisconnect::from_connect_status("http://example.com", &Status::unknown("frame with invalid size"));
    insta::assert_snapshot!(details.to_string(), @"UNKNOWN: frame with invalid size");
}

/// Renders a disconnect with every field, so a snapshot pins the mapping.
fn describe(details: &RemoteDisconnect) -> String {
    format!(
        "{} | code={:?} cause={:?} message={:?} retry_after={:?}",
        details, details.status_code, details.cause, details.status_message, details.retry_after
    )
}

fn close(code: u16, reason: &str) -> RemoteDisconnect {
    close_disconnect(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: reason.into(),
    }))
}

#[test]
fn every_cause_close_code_reports_its_cause() {
    for cause in [
        Cause::IdleTimeout,
        Cause::OutOfMemory,
        Cause::ServerShutdown,
        Cause::ProtocolViolation,
    ] {
        let details = close(cause.close_code(), "why");
        assert_eq!(details.cause, Some(cause));
        assert_eq!(details.status_message.as_deref(), Some("why"));
        assert_eq!(details.transport_error, None);
    }
}

#[test]
fn close_frames_map_onto_status_fields() {
    let cases = [
        describe(&close_disconnect(None)),
        describe(&close(1000, "")),
        describe(&close(1000, "done")),
        describe(&close(1001, "")),
        describe(&close(1008, "no")),
        describe(&close(1011, "")),
        describe(&close(4002, "turn exceeded 30s")),
        describe(&close(4003, "")),
        describe(&close(4999, "")),
        describe(&close(3000, "custom")),
    ];
    insta::assert_snapshot!(cases.join("\n"), @r#"
    OK | code=Some("OK") cause=None message=None retry_after=None
    OK | code=Some("OK") cause=None message=None retry_after=None
    OK: done | code=Some("OK") cause=None message=Some("done") retry_after=None
    UNAVAILABLE: close code 1001 | code=Some("UNAVAILABLE") cause=None message=Some("close code 1001") retry_after=None
    PERMISSION_DENIED: no | code=Some("PERMISSION_DENIED") cause=None message=Some("no") retry_after=None
    INTERNAL: close code 1011 | code=Some("INTERNAL") cause=None message=Some("close code 1011") retry_after=None
    DEADLINE_EXCEEDED (turn_timeout): turn exceeded 30s | code=Some("DEADLINE_EXCEEDED") cause=Some(TurnTimeout) message=Some("turn exceeded 30s") retry_after=None
    RESOURCE_EXHAUSTED (out_of_memory): close code 4003 | code=Some("RESOURCE_EXHAUSTED") cause=Some(OutOfMemory) message=Some("close code 4003") retry_after=None
    UNKNOWN: close code 4999 | code=Some("UNKNOWN") cause=None message=Some("close code 4999") retry_after=None
    UNKNOWN: custom | code=Some("UNKNOWN") cause=None message=Some("custom") retry_after=None
    "#);
}

/// A refused upgrade's status, headers and body, as tungstenite reports it.
fn refused(status: u16, headers: &[(&str, &str)], body: &str) -> PoolError {
    let mut response = Response::builder().status(status);
    for (name, value) in headers {
        response = response.header(*name, *value);
    }
    let body = (!body.is_empty()).then(|| body.as_bytes().to_vec());
    refusal("ws://example.com/", &response.body(body).unwrap())
}

#[test]
fn refused_upgrades_map_like_grpc_http_statuses() {
    let cases = [
        refused(401, &[], "bad token"),
        refused(403, &[("monty-cause", "permission_denied")], ""),
        refused(429, &[("retry-after", "7")], ""),
        refused(
            503,
            &[("monty-cause", "capacity"), ("retry-after", "2")],
            "full\nmore detail",
        ),
        refused(400, &[("monty-cause", "caller_cap")], ""),
        refused(404, &[], "<html>Not Found</html>"),
        refused(200, &[], ""),
        refused(503, &[("monty-cause", "flood")], ""),
    ];
    let rendered: Vec<String> = cases
        .iter()
        .map(|err| match err {
            PoolError::Refused { refusal, .. } => format!("Refused: {}", describe(refusal)),
            other => format!("{other:?}"),
        })
        .collect();
    insta::assert_snapshot!(rendered.join("\n"), @r#"
    Refused: UNAUTHENTICATED: HTTP 401 Unauthorized: bad token | code=Some("UNAUTHENTICATED") cause=None message=Some("HTTP 401 Unauthorized: bad token") retry_after=None
    Refused: PERMISSION_DENIED (permission_denied): HTTP 403 Forbidden | code=Some("PERMISSION_DENIED") cause=Some(PermissionDenied) message=Some("HTTP 403 Forbidden") retry_after=None
    Refused: UNAVAILABLE: HTTP 429 Too Many Requests | code=Some("UNAVAILABLE") cause=None message=Some("HTTP 429 Too Many Requests") retry_after=Some(7s)
    Refused: RESOURCE_EXHAUSTED (capacity): HTTP 503 Service Unavailable: full | code=Some("RESOURCE_EXHAUSTED") cause=Some(Capacity) message=Some("HTTP 503 Service Unavailable: full") retry_after=Some(2s)
    Refused: RESOURCE_EXHAUSTED (caller_cap): HTTP 400 Bad Request | code=Some("RESOURCE_EXHAUSTED") cause=Some(CallerCap) message=Some("HTTP 400 Bad Request") retry_after=None
    Connect { url: "ws://example.com/", details: RemoteDisconnect { status_code: None, status_message: None, cause: None, transport_error: Some("the server refused the WebSocket upgrade: HTTP 404 Not Found: <html>Not Found</html>"), retry_after: None } }
    Connect { url: "ws://example.com/", details: RemoteDisconnect { status_code: None, status_message: None, cause: None, transport_error: Some("the server refused the WebSocket upgrade: HTTP 200 OK"), retry_after: None } }
    Refused: UNAVAILABLE: HTTP 503 Service Unavailable | code=Some("UNAVAILABLE") cause=None message=Some("HTTP 503 Service Unavailable") retry_after=None
    "#);
}

#[test]
fn a_refusal_body_is_capped() {
    let PoolError::Refused { refusal, .. } = refused(503, &[], &"x".repeat(500)) else {
        panic!("expected a refusal");
    };
    let message = refusal.status_message.unwrap();
    assert_eq!(message.chars().count(), "HTTP 503 Service Unavailable: ".len() + 201);
}
