use std::time::Duration;

use super::ticks_to_duration;

#[test]
fn converts_mach_ticks_and_windows_filetime_units() {
    assert_eq!(ticks_to_duration(1_440_000, 125, 3).unwrap(), Duration::from_millis(60));
    assert_eq!(ticks_to_duration(600_000, 100, 1).unwrap(), Duration::from_millis(60));
    assert_eq!(ticks_to_duration(1, 125, 3).unwrap(), Duration::from_nanos(41));
    assert_eq!(
        ticks_to_duration(u128::from(u64::MAX) * 2, 100, 1).unwrap().as_secs(),
        3_689_348_814_741
    );
}

#[test]
fn conversion_rejects_overflow_and_invalid_timebase() {
    assert!(ticks_to_duration(1, 1, 0).is_err());
    assert!(ticks_to_duration(u128::MAX, 2, 1).is_err());
    assert!(ticks_to_duration(u128::MAX, 1, 1).is_err());
}
