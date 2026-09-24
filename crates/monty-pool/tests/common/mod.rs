//! Helpers shared by the pool integration test crates.

use std::time::Duration;

use monty_pool::Pool;
use tokio::time::{Instant, sleep};

/// Polls until the pool holds exactly `count` idle workers, panicking after a
/// few seconds. Background refills land asynchronously, so tests wait rather
/// than sleeping a fixed time.
pub async fn wait_for_idle(pool: &Pool, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while pool.idle_workers() != count {
        assert!(
            Instant::now() < deadline,
            "idle workers stuck at {}, expected {count}",
            pool.idle_workers()
        );
        sleep(Duration::from_millis(10)).await;
    }
}
