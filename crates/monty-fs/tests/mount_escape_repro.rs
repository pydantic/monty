//! Reproductions of known, unfixed mount-confinement defects.
//!
//! Each test asserts the *current, vulnerable* behaviour so the suite stays
//! green; invert the assertion marked `FIX:` once the defect is fixed.

use std::{fs, io::ErrorKind};

use monty_fs::{Mount, MountCallOutcome, MountError, MountMode, MountTable, OverlayState};
use monty_types::{MontyObject, MontyPath, OsFunctionCall, RenameCallArgs};
use tempfile::TempDir;

// Only the directory helper is used here; the rest is shared with other tests.
#[expect(dead_code)]
mod common;
use common::symlink_dir;

/// Dispatches `call`, panicking on the `NotHandled` these tests never expect.
fn handled(mt: &mut MountTable, call: OsFunctionCall) -> Result<MontyObject, MountError> {
    match mt.handle_os_call(call) {
        MountCallOutcome::Handled(result) => result,
        MountCallOutcome::NotHandled(call) => panic!("expected the mount table to handle {call:?}"),
    }
}

/// Builds a `Path.rename(src, dst)` call.
fn rename(src: &str, dst: &str) -> OsFunctionCall {
    OsFunctionCall::Rename(RenameCallArgs {
        src: MontyPath::new(src.to_owned()),
        dst: MontyPath::new(dst.to_owned()),
    })
}

/// Reads `path` through the table.
fn read_text(mt: &mut MountTable, path: &str) -> Result<MontyObject, MountError> {
    handled(mt, OsFunctionCall::ReadText(MontyPath::new(path.to_owned())))
}

/// A mount root is only a path string, so a rebuilt table (one per feed)
/// re-resolves it — and renaming a symlink over that name, which sandbox code
/// can do because a rename never dereferences, redirects the next rebuild.
#[test]
fn mount_root_redirected_by_rename_between_rebuilds() {
    let base = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();

    let shared = base.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::write(outside.path().join("secret.txt"), "HOST SECRET").unwrap();
    // Host-planted link out of the tree; the sandbox only moves it.
    symlink_dir(outside.path(), base.path().join("prepared-link"));

    // The reusable configuration, as `MountSpec` (monty-pool) records it.
    let child_host_path = Mount::new("/child", &shared, MountMode::ReadOnly, None)
        .unwrap()
        .host_path()
        .to_path_buf();

    // Feed 1: swap the real directory out and the link in, under one name.
    let mut writable = MountTable::new();
    writable
        .mount("/parent", base.path(), MountMode::ReadWrite, None)
        .unwrap();
    handled(&mut writable, rename("/parent/shared", "/parent/old-shared")).unwrap();
    handled(&mut writable, rename("/parent/prepared-link", "/parent/shared")).unwrap();

    // Feed 2: the same configuration, rebuilt into a fresh table.
    let mut rebuilt = MountTable::new();
    rebuilt
        .mount("/child", &child_host_path, MountMode::ReadOnly, None)
        .unwrap();

    // FIX: the rebuild must stay pinned to the directory validated above.
    let leaked = read_text(&mut rebuilt, "/child/secret.txt").expect("reproduction: reads outside the mount");
    assert_eq!(leaked, MontyObject::String("HOST SECRET".to_owned()));
}

/// Once either side of a rename is covered, the table owns the call: handing it
/// to the fallback would give a handler answering on raw virtual paths the
/// mounted side, never reaching the mode check in `Mount::execute`.
#[test]
fn rename_with_one_side_out_of_mount_is_refused() {
    let host = TempDir::new().unwrap();
    fs::write(host.path().join("source.txt"), "public").unwrap();
    let other = TempDir::new().unwrap();

    let mut mt = MountTable::new();
    mt.mount("/data", host.path(), MountMode::ReadOnly, None).unwrap();
    mt.mount("/other", other.path(), MountMode::ReadOnly, None).unwrap();

    // A destination in a *different* mount is refused.
    assert_cross_mount(&mut mt, rename("/data/source.txt", "/other/result.txt"));

    // So is one under no mount at all, in either direction.
    assert_cross_mount(&mut mt, rename("/data/source.txt", "/outside/result.txt"));
    assert_cross_mount(&mut mt, rename("/outside/source.txt", "/data/result.txt"));
    assert!(host.path().join("source.txt").exists());

    // A rename with neither side mounted is still the fallback's to answer.
    match mt.handle_os_call(rename("/outside/source.txt", "/elsewhere/result.txt")) {
        MountCallOutcome::NotHandled(call) => assert_eq!(call.fs_primary_path(), Some("/outside/source.txt")),
        outcome @ MountCallOutcome::Handled(_) => panic!("expected NotHandled, got {outcome:?}"),
    }
}

/// Asserts `call` was refused as a cross-mount rename (`EXDEV`).
fn assert_cross_mount(mt: &mut MountTable, call: OsFunctionCall) {
    match mt.handle_os_call(call) {
        MountCallOutcome::Handled(Err(MountError::CrossMountRename { .. })) => {}
        outcome => panic!("expected CrossMountRename, got {outcome:?}"),
    }
}

/// `PATH_MAX` bounds the bytes the sandbox sent, not the path they collapse to,
/// so padding a request with `..` no longer buys unlimited length.
#[test]
fn overlong_path_rejected_even_when_it_normalizes_short() {
    let host = TempDir::new().unwrap();
    fs::write(host.path().join("hello.txt"), "hello").unwrap();

    let long_and_deep = format!("/mnt/{}hello.txt", "a/".repeat(5_000));
    let long_but_collapsing = format!("/mnt/{}{}hello.txt", "a/".repeat(5_000), "../".repeat(5_000));
    assert!(long_and_deep.len() > 4096 && long_but_collapsing.len() > 4096);

    // Both backends resolve paths through their own entry point, so both are checked.
    for mode in [MountMode::ReadOnly, MountMode::OverlayMemory(OverlayState::new())] {
        let mut mt = MountTable::new();
        mt.mount("/mnt", host.path(), mode, None).unwrap();

        // A path without `..` is rejected before any host I/O.
        assert_too_long(read_text(&mut mt, &long_and_deep));

        // The same length collapsing to `/mnt/hello.txt` is rejected on the same
        // grounds, rather than being measured after the `..` components cancel out.
        assert_too_long(read_text(&mut mt, &long_but_collapsing));

        // The path it collapses to is served normally, so only length is at issue.
        let read = read_text(&mut mt, "/mnt/a/../hello.txt").unwrap();
        assert_eq!(read, MontyObject::String("hello".to_owned()));
    }
}

/// Asserts an operation failed with the `ENAMETOOLONG` form of [`MountError`].
fn assert_too_long(result: Result<MontyObject, MountError>) {
    match result {
        Err(MountError::Io(err, _)) => {
            assert_eq!(err.kind(), ErrorKind::InvalidFilename);
            assert_eq!(err.to_string(), "File name too long");
        }
        other => panic!("expected the overlong path to be rejected, got {other:?}"),
    }
}
