use std::{
    hint::spin_loop,
    time::{Duration, Instant},
};

use monty_types::{ResourceLimits, ResourceTracker};

use super::copy_format_template;
use crate::exception_private::RunError;

#[test]
fn format_template_copy_checks_an_exhausted_budget() {
    let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::ZERO));
    tracker.on_execution_start();
    let wall = Instant::now();
    while tracker.check_time().is_ok() {
        assert!(
            wall.elapsed() < Duration::from_secs(10),
            "execution clock did not advance"
        );
        spin_loop();
    }
    tracker.on_execution_stop();
    assert!(matches!(
        copy_format_template("{missing}", &tracker),
        Err(RunError::UncatchableExc(_))
    ));
}
