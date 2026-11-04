//! The remote transport's connection window sizing.

use super::{MAX_WINDOW_BYTES, MIN_CONNECTION_WINDOW_BYTES, STREAM_WINDOW_BYTES, connection_window};

/// Every stream a pool can hold, live and closing, stalled on a full stream
/// window still leaves a stream window of credit for a neighbour's reply.
#[test]
fn stalled_streams_never_exhaust_the_connection_window() {
    for max_processes in [1, 16, 32, 100] {
        let held = 2 * max_processes as u64 * u64::from(STREAM_WINDOW_BYTES);
        let spare = u64::from(connection_window(max_processes)) - held;
        assert!(
            spare >= u64::from(STREAM_WINDOW_BYTES),
            "max_processes = {max_processes}"
        );
    }
}

#[test]
fn the_connection_window_stays_within_http2_bounds() {
    assert_eq!(connection_window(0), MIN_CONNECTION_WINDOW_BYTES);
    assert_eq!(connection_window(1), MIN_CONNECTION_WINDOW_BYTES);
    assert_eq!(connection_window(usize::MAX), MAX_WINDOW_BYTES);
}
