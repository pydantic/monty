//! Remote transport window sizing and connection diagnostics.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use h2::{Error as H2Error, Reason};
use tonic::Status;

use super::{
    MAX_WINDOW_BYTES, MIN_CONNECTION_WINDOW_BYTES, MIN_STREAM_WINDOW_BYTES, RemoteDisconnect, STREAM_WINDOW_BYTES,
    connection_window, retry_after, stream_window,
};

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
    for url in ["http://example.com", "HTTP://example.com", "ws://example.com"] {
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
