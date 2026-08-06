//! An `OverlayMemory` rename records a `RealFileRef` instead of copying the
//! bytes, and reads followed that cached host path unchecked. Every path that
//! dereferences one is exercised here: `read_text`, `read_bytes`, and the
//! `existing_file_len` / `existing_file_bytes` pair an append goes through.

#[cfg(windows)]
use std::process::Command;
use std::{
    fs,
    path::{Path, PathBuf},
};
#[cfg(unix)]
use std::{io::ErrorKind, os::unix::fs::PermissionsExt};

use monty_fs::{MountCallOutcome, MountError, MountMode, MountTable, OverlayState};
use monty_types::{MontyObject, OsFunctionCall, PathStringDataArgs, RenameCallArgs};
use tempfile::TempDir;

mod common;
use common::{symlink_dir, symlink_file, try_rename_mount_root};

/// Marker written to the out-of-mount file; its appearance in any result is
/// the disclosure these tests guard against.
const SECRET: &str = "SECRET-HOST-CONTENT";

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

/// A mount holding one cached `RealFileRef` at `/mnt/moved.txt` that a
/// host-side actor has since re-pointed at an out-of-mount secret. Sandboxed
/// code cannot create that symlink itself; this models a host process sharing
/// the mount. Owns its temp dirs so they outlive the dispatch.
struct StaleRef {
    mounts: MountTable,
    _mount_dir: TempDir,
    _outside_dir: TempDir,
    /// The out-of-mount file whose contents must never surface.
    secret: PathBuf,
}

impl StaleRef {
    /// Caches the ref via a rename, then stales it by swapping the backing file
    /// for a symlink out of the mount.
    fn new() -> Self {
        let mount_dir = TempDir::new().unwrap();
        let outside_dir = TempDir::new().unwrap();

        let secret = outside_dir.path().join("secret.txt");
        fs::write(&secret, SECRET).unwrap();

        fs::create_dir_all(mount_dir.path().join("inner")).unwrap();
        let real_file = mount_dir.path().join("inner/file.txt");
        fs::write(&real_file, "public").unwrap();

        let mut mounts = mount_overlay(mount_dir.path());

        // Caches a RealFileRef whose host_path is `real_file` — validated, in bounds.
        dispatch(&mut mounts, rename_call("/mnt/inner/file.txt", "/mnt/moved.txt")).expect("rename should succeed");

        // The host-side actor re-points it out of the mount, staling the cache.
        fs::remove_file(&real_file).unwrap();
        symlink_file(&secret, &real_file);

        Self {
            mounts,
            _mount_dir: mount_dir,
            _outside_dir: outside_dir,
            secret,
        }
    }

    /// Asserts `call` neither succeeded nor leaked the secret.
    fn assert_rejected(&mut self, call: OsFunctionCall) {
        let outcome = dispatch(&mut self.mounts, call);
        let leaked = match &outcome {
            Ok(MontyObject::String(s)) => s.contains(SECRET),
            Ok(MontyObject::Bytes(b)) => b.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()),
            _ => false,
        };
        assert!(
            !leaked,
            "HOST FILE DISCLOSURE: dereferencing a stale overlay ref returned the contents of {}",
            self.secret.display()
        );
        assert!(
            matches!(outcome, Err(MountError::PathEscape { .. })),
            "expected PathEscape, got {outcome:?}"
        );
    }
}

/// `read_text` must re-validate the cached ref rather than trust it.
#[test]
fn stale_ref_is_revalidated_on_read_text() {
    StaleRef::new().assert_rejected(OsFunctionCall::ReadText("/mnt/moved.txt".into()));
}

/// `read_bytes` shares the `RealFileRef` shortcut and must re-validate too.
#[test]
fn stale_ref_is_revalidated_on_read_bytes() {
    StaleRef::new().assert_rejected(OsFunctionCall::ReadBytes("/mnt/moved.txt".into()));
}

/// Appending materializes the backing file into the overlay via
/// `existing_file_len` then `existing_file_bytes`, so both must re-validate —
/// otherwise the secret lands in overlay state and reads back from memory.
#[test]
fn stale_ref_is_revalidated_on_append() {
    let mut scenario = StaleRef::new();
    scenario.assert_rejected(OsFunctionCall::AppendText(PathStringDataArgs {
        path: "/mnt/moved.txt".into(),
        data: "appended".to_owned(),
    }));

    // An append that slipped through would leave the secret in overlay state,
    // where a later read serves it from memory without touching the host.
    scenario.assert_rejected(OsFunctionCall::ReadText("/mnt/moved.txt".into()));
}

/// Swapping the mount root for an escaping symlink after the ref was cached
/// must not redirect the read.
///
/// The ref is mount-relative and the mount holds a descriptor, so the swap is
/// invisible and the read still serves the original file. With a host path it
/// had to be *rejected* instead.
#[test]
fn mount_root_swap_does_not_redirect_a_cached_ref() {
    let base = TempDir::new().unwrap();
    let mount_path = base.path().join("mount");
    let elsewhere = base.path().join("elsewhere");
    fs::create_dir_all(mount_path.join("inner")).unwrap();
    fs::create_dir_all(elsewhere.join("inner")).unwrap();
    fs::write(mount_path.join("inner/file.txt"), "public").unwrap();
    fs::write(elsewhere.join("inner/file.txt"), SECRET).unwrap();

    let mut mt = mount_overlay(&mount_path);
    dispatch(&mut mt, rename_call("/mnt/inner/file.txt", "/mnt/moved.txt")).expect("rename should succeed");

    // Swap the whole mount root for a link to `elsewhere`, so the ref's path
    // *as a string* now names a directory that was never mounted. Windows
    // refuses the rename while the mount holds the directory open, which denies
    // the swap outright rather than merely making it ineffective.
    if try_rename_mount_root(&mount_path, base.path().join("mount_old")) {
        symlink_dir(&elsewhere, &mount_path);
    }

    let outcome = dispatch(&mut mt, OsFunctionCall::ReadText("/mnt/moved.txt".into()));
    let leaked = matches!(&outcome, Ok(MontyObject::String(s)) if s.contains(SECRET));
    assert!(
        !leaked,
        "HOST FILE DISCLOSURE: swapping the mount root redirected the read"
    );
    assert_eq!(outcome.unwrap(), MontyObject::String("public".to_owned()));
}

/// Renaming an in-mount symlink whose target is unreadable must report the I/O
/// error, not a path escape.
///
/// `PathEscape` means the boundary caught something; a permission failure on a
/// target that is plainly inside the mount is not that.
#[test]
#[cfg(unix)]
fn renaming_symlink_to_denied_target_reports_the_io_error() {
    let mount_dir = TempDir::new().unwrap();
    let sub = mount_dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("target.txt"), "content").unwrap();
    symlink_file("sub/target.txt", mount_dir.path().join("link.txt"));
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(sub.join("target.txt")).is_ok() {
        eprintln!("skipped: running as root, mode 0o000 is not enforced");
        return;
    }

    let mut mt = mount_overlay(mount_dir.path());
    let outcome = dispatch(&mut mt, rename_call("/mnt/link.txt", "/mnt/moved.txt"));

    fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(&outcome, Err(MountError::Io(err, _)) if err.kind() == ErrorKind::PermissionDenied),
        "expected a permission error, got {outcome:?}"
    );
}

/// Control — the same escaping symlink read through the normal resolution path
/// is rejected, so the leak above is specific to the `RealFileRef` shortcut.
#[test]
fn direct_read_through_escaping_symlink_is_rejected() {
    let mount_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();

    let secret = outside_dir.path().join("secret.txt");
    fs::write(&secret, SECRET).unwrap();
    symlink_file(&secret, mount_dir.path().join("link.txt"));

    let mut mt = mount_overlay(mount_dir.path());
    let outcome = dispatch(&mut mt, OsFunctionCall::ReadText("/mnt/link.txt".into()));

    let leaked = matches!(&outcome, Ok(MontyObject::String(s)) if s.contains(SECRET));
    assert!(!leaked, "control failed: the ordinary resolution path also leaks");
}

/// `collect_real_descendants`' capture-time skip relies on std classifying a
/// Windows junction as a symlink. If this flips, revisit that skip.
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

/// Renaming an escaping symlink in must be refused outright, so
/// `reject_escaping_symlink` is exercised rather than the read-time check behind it.
#[test]
fn renaming_escaping_symlink_into_overlay_is_rejected() {
    let mount_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();

    let secret = outside_dir.path().join("secret.txt");
    fs::write(&secret, SECRET).unwrap();
    symlink_file(&secret, mount_dir.path().join("link.txt"));

    let mut mt = mount_overlay(mount_dir.path());
    let rename_outcome = dispatch(&mut mt, rename_call("/mnt/link.txt", "/mnt/captured.txt"));

    assert!(
        matches!(rename_outcome, Err(MountError::PathEscape { .. })),
        "expected the rename to be refused with PathEscape, got {rename_outcome:?}"
    );
}
