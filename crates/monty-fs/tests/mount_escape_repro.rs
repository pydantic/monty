//! Reproductions of known, unfixed mount-confinement defects.
//!
//! Each test asserts the *current, vulnerable* behaviour so the suite stays
//! green; invert the assertion marked `FIX:` once the defect is fixed.

use std::{fs, io::ErrorKind};

use monty_fs::{Mount, MountCallOutcome, MountError, MountMode, MountTable};
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

/// `route_call` looks the rename destination up with `?`, so an unmounted
/// destination abandons routing and hands the mounted source to the host
/// fallback — never reaching the mode check in `Mount::execute`.
#[test]
fn rename_out_of_mount_bypasses_the_mount_table() {
    let host = TempDir::new().unwrap();
    fs::write(host.path().join("source.txt"), "public").unwrap();
    let other = TempDir::new().unwrap();

    let mut mt = MountTable::new();
    mt.mount("/data", host.path(), MountMode::ReadOnly, None).unwrap();
    mt.mount("/other", other.path(), MountMode::ReadOnly, None).unwrap();

    // Control: a destination in a *different* mount is refused.
    match mt.handle_os_call(rename("/data/source.txt", "/other/result.txt")) {
        MountCallOutcome::Handled(Err(MountError::CrossMountRename { .. })) => {}
        outcome => panic!("expected CrossMountRename, got {outcome:?}"),
    }

    // FIX: an unmounted destination must be refused too, not handed on.
    match mt.handle_os_call(rename("/data/source.txt", "/outside/result.txt")) {
        MountCallOutcome::NotHandled(call) => assert_eq!(call.fs_primary_path(), Some("/data/source.txt")),
        outcome @ MountCallOutcome::Handled(_) => panic!("reproduction expects NotHandled, got {outcome:?}"),
    }
    assert!(host.path().join("source.txt").exists());
}

/// `PATH_MAX` is checked after normalization, so it bounds the collapsed path
/// rather than the bytes the sandbox sent.
#[test]
fn overlong_path_accepted_when_it_normalizes_short() {
    let host = TempDir::new().unwrap();
    fs::write(host.path().join("hello.txt"), "hello").unwrap();

    let mut mt = MountTable::new();
    mt.mount("/mnt", host.path(), MountMode::ReadOnly, None).unwrap();

    // Control: the same length without `..` is rejected before any host I/O.
    let long_and_deep = format!("/mnt/{}hello.txt", "a/".repeat(5_000));
    assert!(long_and_deep.len() > 4096);
    match read_text(&mut mt, &long_and_deep) {
        Err(MountError::Io(err, _)) => {
            assert_eq!(err.kind(), ErrorKind::InvalidFilename);
            assert_eq!(err.to_string(), "File name too long");
        }
        other => panic!("expected the overlong path to be rejected, got {other:?}"),
    }

    let long_but_collapsing = format!("/mnt/{}{}hello.txt", "a/".repeat(5_000), "../".repeat(5_000));
    assert!(long_but_collapsing.len() > 4096);

    // FIX: reject on raw length, before normalization allocates per segment.
    let read = read_text(&mut mt, &long_but_collapsing).expect("reproduction: the overlong request is accepted");
    assert_eq!(read, MontyObject::String("hello".to_owned()));
}
