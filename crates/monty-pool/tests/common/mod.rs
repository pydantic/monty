//! Helpers shared by the pool integration test crates.

use std::{process::Command, time::Duration};

use monty_pool::Pool;
use tokio::time::{Instant, sleep};

/// Kills a process by pid with SIGKILL — simulates a hard crash.
pub fn kill_pid(pid: u32) {
    #[cfg(unix)]
    {
        let status = Command::new("kill").args(["-9", &pid.to_string()]).status().unwrap();
        assert!(status.success());
    }
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status()
            .unwrap();
        assert!(status.success());
    }
}

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
