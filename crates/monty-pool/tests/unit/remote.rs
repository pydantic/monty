//! The remote transport's flow-control window sizing.

use super::{
    MAX_WINDOW_BYTES, MIN_CONNECTION_WINDOW_BYTES, MIN_STREAM_WINDOW_BYTES, STREAM_WINDOW_BYTES, connection_window,
    stream_window,
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
