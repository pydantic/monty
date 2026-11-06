//! Fuzz target for the virtual path policy.
//!
//! Feeds arbitrary strings through Monty's path normalization and mount-prefix
//! stripping (with a synthetic mount at `/mnt`, so no host directory is opened)
//! and asserts the confinement invariant on every accepted path: the result is
//! mount-relative, with no `..` component, no backslash, no Windows drive
//! prefix and no null byte, so joining it to the mount root cannot climb out.
#![no_main]

use libfuzzer_sys::fuzz_target;
use monty_fs::test_util::resolve_mounted_path;

fuzz_target!(|data: &[u8]| {
    let raw = String::from_utf8_lossy(data);
    if let Ok(relative) = resolve_mounted_path(&raw) {
        assert_confined(&relative);
    }
    // Route the payload through the mount as well: bare strings mostly fail the
    // mount-point lookup, which leaves normalization and stripping unexplored.
    if let Ok(relative) = resolve_mounted_path(&format!("/mnt/{raw}")) {
        assert_confined(&relative);
    }
});

/// Asserts that an accepted mount-relative path cannot escape the mount.
fn assert_confined(relative: &str) {
    assert!(!relative.contains('\0'), "null byte survived policy: {relative:?}");
    assert!(!relative.contains('\\'), "backslash survived policy: {relative:?}");
    // The mount root itself is the empty relative path.
    if relative.is_empty() {
        return;
    }
    assert!(!relative.starts_with('/'), "path is not mount-relative: {relative:?}");
    for segment in relative.split('/') {
        assert!(
            !matches!(segment, "" | "." | ".."),
            "unnormalized segment {segment:?} in {relative:?}"
        );
        let bytes = segment.as_bytes();
        assert!(
            !(bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'),
            "drive prefix survived policy: {relative:?}"
        );
    }
}
