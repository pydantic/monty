//! Tests for `CloseCause`: the close codes are a wire contract, so they are
//! pinned as literal numbers, and every conversion round-trips.

use monty_types::CloseCause;

// === codes are literal wire constants ===

#[test]
fn codes_are_pinned() {
    assert_eq!(CloseCause::IdleTimeout.code(), 4000);
    assert_eq!(CloseCause::SessionTimeout.code(), 4001);
    assert_eq!(CloseCause::TurnTimeout.code(), 4002);
    assert_eq!(CloseCause::OutOfMemory.code(), 4003);
    assert_eq!(CloseCause::RequestTooLarge.code(), 4004);
    assert_eq!(CloseCause::Evicted.code(), 4005);
}

#[test]
fn all_lists_every_cause_in_code_order() {
    let codes: Vec<u16> = CloseCause::ALL.iter().map(|cause| cause.code()).collect();
    assert_eq!(codes, vec![4000, 4001, 4002, 4003, 4004, 4005]);
}

// === round trips ===

#[test]
fn every_cause_round_trips_through_its_code() {
    for cause in CloseCause::ALL {
        assert_eq!(CloseCause::from_code(cause.code()), Some(cause));
    }
}

#[test]
fn names_are_distinct_snake_case() {
    let names: Vec<&str> = CloseCause::ALL.iter().map(|cause| cause.name()).collect();
    assert_eq!(
        names,
        vec![
            "idle_timeout",
            "session_timeout",
            "turn_timeout",
            "out_of_memory",
            "request_too_large",
            "evicted"
        ]
    );
}

#[test]
fn display_is_the_description() {
    assert_eq!(
        CloseCause::OutOfMemory.to_string(),
        "the worker exceeded its memory limit and was terminated"
    );
}

// === unknown codes ===

#[test]
fn unknown_codes_are_none() {
    // registered codes, and a private-use code a newer server might define
    assert_eq!(CloseCause::from_code(1000), None);
    assert_eq!(CloseCause::from_code(1011), None);
    assert_eq!(CloseCause::from_code(4006), None);
    assert_eq!(CloseCause::from_code(4999), None);
}
