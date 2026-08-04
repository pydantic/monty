//! Renaming a real host file inside an `OverlayMemory` mount records a
//! `RealFileRef` rather than copying the bytes, and reads followed that cached
//! host path with no boundary check — it was validated once, at rename time.
//! These tests cover the gaps the rename-time guards (`reject_escaping_symlink`,
//! the symlink skip in `collect_real_descendants`) leave open.

#[cfg(unix)]
use std::os::unix::fs::symlink as unix_symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_file as win_symlink_file;
#[cfg(windows)]
use std::process::Command;
use std::{fs, path::Path};

use monty_fs::{MountCallOutcome, MountError, MountMode, MountTable, OverlayState};
use monty_types::{MontyObject, OsFunctionCall, RenameCallArgs};
use tempfile::TempDir;

/// Mounts `host` at `/mnt` in `OverlayMemory` mode — the only mode that builds
/// `RealFileRef` entries.
fn mount_overlay(host: &Path) -> MountTable {
    let mut mt = MountTable::new();
    mt.mount("/mnt", host, MountMode::OverlayMemory(OverlayState::new()), None)
        .expect("failed to configure mount");
    mt
}

/// Dispatches a call, panicking if the mount table declines to handle it.
fn dispatch(mt: &mut MountTable, call: OsFunctionCall) -> Result<MontyObject, MountError> {
    match mt.handle_os_call(call) {
        MountCallOutcome::Handled(result) => result,
        MountCallOutcome::NotHandled(call) => panic!("mount table returned NotHandled: {call:?}"),
    }
}

/// The OS call `os.rename(src, dst)` produces.
fn rename_call(src: &str, dst: &str) -> OsFunctionCall {
    OsFunctionCall::Rename(RenameCallArgs {
        src: src.into(),
        dst: dst.into(),
    })
}

/// Creates a file symlink, handling platform differences.
#[cfg(unix)]
fn symlink_file(original: &Path, link: &Path) {
    unix_symlink(original, link).expect("failed to create symlink");
}

/// Creates a file symlink, handling platform differences.
#[cfg(windows)]
fn symlink_file(original: &Path, link: &Path) {
    win_symlink_file(original, link).expect("failed to create symlink (enable Windows Developer Mode or run elevated)");
}

/// Variant 1 — the cached host path is re-pointed outside the mount after the
/// rename-time check has already passed. Sandboxed code cannot create that
/// symlink itself, so this models a host process sharing the mount.
#[test]
fn stale_ref_is_revalidated_on_read() {
    let mount_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();

    let secret = outside_dir.path().join("secret.txt");
    fs::write(&secret, "SECRET-HOST-CONTENT").unwrap();

    fs::create_dir_all(mount_dir.path().join("inner")).unwrap();
    let real_file = mount_dir.path().join("inner/file.txt");
    fs::write(&real_file, "public").unwrap();

    let mut mt = mount_overlay(mount_dir.path());

    // Caches a RealFileRef whose host_path is `real_file` — validated, in bounds.
    dispatch(&mut mt, rename_call("/mnt/inner/file.txt", "/mnt/moved.txt")).expect("rename should succeed");

    // The host-side actor re-points it out of the mount, staling the cache.
    fs::remove_file(&real_file).unwrap();
    symlink_file(&secret, &real_file);

    let outcome = dispatch(&mut mt, OsFunctionCall::ReadText("/mnt/moved.txt".into()));
    println!("[variant 1] read of stale ref: {outcome:?}");

    let leaked = matches!(&outcome, Ok(MontyObject::String(s)) if s.contains("SECRET-HOST-CONTENT"));
    println!("[variant 1] LEAKED?          : {leaked}");

    assert!(
        !leaked,
        "HOST FILE DISCLOSURE: reading a stale overlay ref returned the contents of {}",
        secret.display()
    );
}

/// Control — the same escaping symlink read through the normal resolution path
/// is rejected, so variant 1's leak is specific to the `RealFileRef` shortcut.
#[test]
fn direct_read_through_escaping_symlink_is_rejected() {
    let mount_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();

    let secret = outside_dir.path().join("secret.txt");
    fs::write(&secret, "SECRET-HOST-CONTENT").unwrap();
    symlink_file(&secret, &mount_dir.path().join("link.txt"));

    let mut mt = mount_overlay(mount_dir.path());
    let outcome = dispatch(&mut mt, OsFunctionCall::ReadText("/mnt/link.txt".into()));
    println!("[control] direct read via symlink: {outcome:?}");

    let leaked = matches!(&outcome, Ok(MontyObject::String(s)) if s.contains("SECRET-HOST-CONTENT"));
    assert!(!leaked, "control failed: the ordinary resolution path also leaks");
}

/// The premise `collect_real_descendants`' capture-time skip rests on: std must
/// classify a Windows directory junction as a symlink for its `is_symlink()`
/// filter to catch one. If this flips, revisit that skip.
#[cfg(windows)]
#[test]
fn junction_is_classified_as_symlink() {
    let dir = TempDir::new().unwrap();
    let (target, junction) = (dir.path().join("t"), dir.path().join("jn"));
    fs::create_dir(&target).unwrap();
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&target)
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed: {status:?}");
    assert!(fs::symlink_metadata(&junction).unwrap().file_type().is_symlink());
}

/// Variant 1b — renaming an escaping symlink into the overlay must be refused
/// outright, so a regression in `reject_escaping_symlink` is caught here too.
#[test]
fn renaming_escaping_symlink_into_overlay_is_rejected() {
    let mount_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();

    let secret = outside_dir.path().join("secret.txt");
    fs::write(&secret, "SECRET-HOST-CONTENT").unwrap();
    symlink_file(&secret, &mount_dir.path().join("link.txt"));

    let mut mt = mount_overlay(mount_dir.path());
    let rename_outcome = dispatch(&mut mt, rename_call("/mnt/link.txt", "/mnt/captured.txt"));
    println!("[variant 1b] rename outcome: {rename_outcome:?}");

    // A refusal ends the attack here; if it succeeded, the read is what leaks.
    if rename_outcome.is_ok() {
        let read_outcome = dispatch(&mut mt, OsFunctionCall::ReadText("/mnt/captured.txt".into()));
        println!("[variant 1b] read outcome  : {read_outcome:?}");
        let leaked = matches!(&read_outcome, Ok(MontyObject::String(s)) if s.contains("SECRET-HOST-CONTENT"));
        assert!(!leaked, "HOST FILE DISCLOSURE via renamed escaping symlink");
    }
}
