use std::time::Duration;

#[cfg(target_os = "linux")]
use tokio::process::Command;

use super::allowance;
#[cfg(target_os = "linux")]
use crate::cpu_clock::Clock;

#[test]
fn feed_consumption_and_grace_survive_resumes() {
    let budget = Duration::from_millis(50);
    let grace = Duration::from_millis(10);
    assert_eq!(
        allowance(Some(budget), None, Duration::from_millis(45), Some(grace), Some(grace)),
        Some(Duration::from_millis(15))
    );
    assert_eq!(
        allowance(Some(budget), None, Duration::from_millis(65), Some(grace), Some(grace)),
        Some(Duration::ZERO)
    );
}

#[test]
fn tighter_scope_wins_and_unlimited_scopes_stay_unlimited() {
    let feed = Duration::from_millis(100);
    let turn = Duration::from_millis(20);
    assert_eq!(
        allowance(
            Some(feed),
            Some(turn),
            Duration::ZERO,
            Some(Duration::ZERO),
            Some(Duration::ZERO)
        ),
        Some(turn)
    );
    assert_eq!(
        allowance(
            Some(feed),
            Some(turn),
            Duration::from_millis(90),
            Some(Duration::ZERO),
            Some(Duration::ZERO)
        ),
        Some(Duration::from_millis(10))
    );
    assert_eq!(
        allowance(None, None, Duration::MAX, Some(Duration::MAX), Some(Duration::MAX)),
        None
    );
    assert_eq!(
        allowance(
            None,
            Some(turn),
            Duration::MAX,
            Some(Duration::ZERO),
            Some(Duration::ZERO)
        ),
        Some(turn)
    );
}

#[test]
fn arithmetic_saturates_instead_of_wrapping() {
    assert_eq!(
        allowance(
            Some(Duration::MAX),
            None,
            Duration::ZERO,
            Some(Duration::MAX),
            Some(Duration::MAX)
        ),
        Some(Duration::MAX)
    );
}
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_dead_clock_fails_before_the_request_is_polled() {
    let mut child = Command::new("sleep").arg("60").kill_on_drop(true).spawn().unwrap();
    let monitor = super::Monitor::new(Clock::new(&child).unwrap(), Duration::from_millis(50)).unwrap();
    child.kill().await.unwrap();
    let outcome = super::run(Some(&monitor), None, None, async {
        panic!("request must not run after a clock failure")
    })
    .await;
    assert!(matches!(outcome, Err(super::Failure::Clock(_))));
}

#[test]
fn disabled_graces_disable_only_their_own_backstop() {
    let budget = Duration::from_millis(50);
    assert_eq!(allowance(Some(budget), Some(budget), Duration::ZERO, None, None), None);
    assert_eq!(
        allowance(Some(budget), Some(budget), budget, None, Some(Duration::ZERO)),
        Some(budget)
    );
    assert_eq!(
        allowance(Some(budget), Some(budget), budget, Some(Duration::ZERO), None),
        Some(Duration::ZERO)
    );
}
