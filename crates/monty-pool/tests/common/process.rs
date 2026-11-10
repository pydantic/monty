use std::process::Command;

/// Forces a worker to exit without a protocol reply on either host platform.
pub fn kill_pid(pid: u32) {
    #[cfg(unix)]
    let status = Command::new("kill").args(["-9", &pid.to_string()]).status().unwrap();

    #[cfg(windows)]
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .status()
        .unwrap();

    assert!(status.success());
}
