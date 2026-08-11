//! Security boundary tests for filesystem mounts.
//!
//! Exhaustively verifies that sandbox code cannot escape the mount boundary
//! via path traversal, null bytes, symlinks, or any other technique.
//! Tests cover all mount modes to ensure the security invariant holds everywhere.

#[cfg(unix)]
use std::{ffi::OsStr, os::unix::ffi::OsStrExt, os::unix::fs::symlink};
use std::{fmt, fs, io::ErrorKind};

use monty_fs::{MountCallOutcome, MountError, MountMode, MountTable, OverlayState};
use monty_types::{
    ExcType, FileMode, MkdirCallArgs, MontyObject, MontyPath, OpenCallArgs, OsFunctionCall, PathBytesDataArgs,
    PathStringDataArgs, RenameCallArgs,
};
use tempfile::TempDir;

#[expect(dead_code, reason = "shared helper module; not every test crate uses all of it")]
mod common;
use common::{symlink_dir, symlink_file, symlinks_supported};

// =============================================================================
// Helpers
// =============================================================================

/// Local discriminator used by the call helpers to pick the right
/// [`OsFunctionCall`] variant. Mirrors the old `OsFunction` enum shape so the
/// rest of the test body reads the same — the wrapper just builds the typed
/// args struct around each call.
#[derive(Clone, Copy)]
enum PathOp {
    Exists,
    IsFile,
    IsDir,
    IsSymlink,
    ReadText,
    ReadBytes,
    Stat,
    Iterdir,
    Unlink,
    Mkdir,
    WriteText,
    WriteBytes,
}

impl PathOp {
    /// Builds an [`OsFunctionCall`] for a path-only operation. Panics if used
    /// with an op that needs extra args (use the helpers below for those).
    fn build_path_only(self, path: &str) -> OsFunctionCall {
        let p = MontyPath::new(path.to_owned());
        match self {
            Self::Exists => OsFunctionCall::Exists(p),
            Self::IsFile => OsFunctionCall::IsFile(p),
            Self::IsDir => OsFunctionCall::IsDir(p),
            Self::IsSymlink => OsFunctionCall::IsSymlink(p),
            Self::ReadText => OsFunctionCall::ReadText(p),
            Self::ReadBytes => OsFunctionCall::ReadBytes(p),
            Self::Stat => OsFunctionCall::Stat(p),
            Self::Iterdir => OsFunctionCall::Iterdir(p),
            Self::Unlink => OsFunctionCall::Unlink(p),
            Self::Mkdir | Self::WriteText | Self::WriteBytes => {
                panic!("op {self:?} requires extra args — use the dedicated helper")
            }
        }
    }
}

impl fmt::Debug for PathOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Exists => "Exists",
            Self::IsFile => "IsFile",
            Self::IsDir => "IsDir",
            Self::IsSymlink => "IsSymlink",
            Self::ReadText => "ReadText",
            Self::ReadBytes => "ReadBytes",
            Self::Stat => "Stat",
            Self::Iterdir => "Iterdir",
            Self::Unlink => "Unlink",
            Self::Mkdir => "Mkdir",
            Self::WriteText => "WriteText",
            Self::WriteBytes => "WriteBytes",
        })
    }
}

/// Creates the standard test directory.
fn create_test_dir() -> TempDir {
    let dir = TempDir::new().expect("failed to create temp dir");
    let p = dir.path();

    fs::write(p.join("hello.txt"), "hello world\n").unwrap();
    fs::create_dir_all(p.join("subdir/deep")).unwrap();
    fs::write(p.join("subdir/nested.txt"), "nested content").unwrap();
    fs::write(p.join("subdir/deep/file.txt"), "deep file").unwrap();

    dir
}

/// Creates a mount table with mount at `/mnt` in the given mode.
fn mount_at_mnt(tmpdir: &TempDir, mode: MountMode) -> MountTable {
    let mut mt = MountTable::new();
    mt.mount("/mnt", tmpdir.path(), mode, None).unwrap();
    mt
}

/// Adapts the owning `handle_os_call` API back to the `Option` shape the
/// assertions below are written against.
fn dispatch(mt: &mut MountTable, c: OsFunctionCall) -> Option<Result<MontyObject, MountError>> {
    match mt.handle_os_call(c) {
        MountCallOutcome::Handled(result) => Some(result),
        MountCallOutcome::NotHandled(_) => None,
    }
}

/// Shorthand: call handle_os_call with a single path argument.
fn call(mt: &mut MountTable, op: PathOp, path: &str) -> Option<Result<MontyObject, MountError>> {
    dispatch(mt, op.build_path_only(path))
}

/// Shorthand: call `mkdir` handle_os_call with the supplied kwargs.
fn call_mkdir(
    mt: &mut MountTable,
    path: &str,
    parents: bool,
    exist_ok: bool,
) -> Option<Result<MontyObject, MountError>> {
    dispatch(
        mt,
        OsFunctionCall::Mkdir(MkdirCallArgs {
            path: MontyPath::new(path.to_owned()),
            parents,
            exist_ok,
        }),
    )
}

/// Asserts that the operation is blocked: either an error (PathEscape, NoMountPoint, Io)
/// or `None` (no matching mount for the normalized path).
fn assert_blocked(mt: &mut MountTable, op: PathOp, path: &str) {
    // `Mkdir` needs extra args, so route it through the dedicated helper
    // with the boring `parents=false, exist_ok=false` defaults — the
    // boundary check happens before any kwargs are inspected.
    let result = match op {
        PathOp::Mkdir => call_mkdir(mt, path, false, false),
        _ => call(mt, op, path),
    };
    match result {
        Some(Err(
            MountError::PathEscape { .. }
            | MountError::NoMountPoint(_)
            | MountError::Io(_, _)
            | MountError::EmbeddedNullByte(_),
        ))
        | None => {}
        Some(Ok(val)) => panic!("expected blocked, got Ok({val:?}) for path: {path}"),
        Some(Err(other)) => panic!("unexpected error variant for {path}: {other}"),
    }
}

/// Asserts a boolean query answers `False` rather than raising.
///
/// `pathlib` predicates never raise, so a path leaving the mount comes back as
/// `False` — indistinguishable from "does not exist". Raising would leak that
/// something is there to be blocked.
fn assert_invisible(mt: &mut MountTable, op: PathOp, path: &str) {
    match call(mt, op, path) {
        Some(Ok(MontyObject::Bool(false))) => {}
        other => panic!("expected Ok(Bool(false)) for path: {path}, got {other:?}"),
    }
}

/// Asserts blocked for a write operation with content.
fn assert_write_blocked(mt: &mut MountTable, op: PathOp, path: &str) {
    let p = MontyPath::new(path.to_owned());
    let call_variant = match op {
        PathOp::WriteText => OsFunctionCall::WriteText(PathStringDataArgs {
            path: p,
            data: "attack".to_owned(),
        }),
        PathOp::WriteBytes => OsFunctionCall::WriteBytes(PathBytesDataArgs {
            path: p,
            data: b"attack".to_vec(),
        }),
        other => panic!("assert_write_blocked: unexpected op {other:?}"),
    };
    let result = dispatch(mt, call_variant);
    match result {
        Some(Err(
            MountError::PathEscape { .. }
            | MountError::NoMountPoint(_)
            | MountError::Io(_, _)
            | MountError::EmbeddedNullByte(_),
        ))
        | None => {}
        Some(Ok(val)) => panic!("expected write blocked, got Ok({val:?}) for path: {path}"),
        Some(Err(other)) => panic!("unexpected error variant for write to {path}: {other}"),
    }
}

/// Asserts that `open(path, mode)` is blocked at open time.
fn assert_open_blocked(mt: &mut MountTable, path: &str, mode: &str) {
    let mode = mode.parse::<FileMode>().expect("test mode parses");
    let result = dispatch(
        mt,
        OsFunctionCall::Open(OpenCallArgs {
            path: MontyPath::new(path.to_owned()),
            mode,
        }),
    );
    match result {
        Some(Err(
            MountError::PathEscape { .. }
            | MountError::NoMountPoint(_)
            | MountError::Io(_, _)
            | MountError::EmbeddedNullByte(_),
        ))
        | None => {}
        Some(Ok(val)) => panic!("expected open blocked, got Ok({val:?}) for path: {path} mode: {mode:?}"),
        Some(Err(other)) => panic!("unexpected error variant for open of {path}: {other}"),
    }
}

/// All mount modes to test against.
fn all_modes() -> Vec<(&'static str, MountMode)> {
    vec![
        ("ReadWrite", MountMode::ReadWrite),
        ("ReadOnly", MountMode::ReadOnly),
        ("OverlayMemory", MountMode::OverlayMemory(OverlayState::new())),
    ]
}

// =============================================================================
// Path traversal attacks
// =============================================================================

#[test]
fn traversal_dotdot_from_root() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, PathOp::ReadText, "/mnt/../etc/passwd");
        assert_blocked(&mut mt, PathOp::Exists, "/mnt/../etc/passwd");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_from_subdir() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, PathOp::ReadText, "/mnt/subdir/../../etc/passwd");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_many_dotdots() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, PathOp::ReadText, "/mnt/a/../../../../../../../etc/passwd");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_write_text() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_write_blocked(&mut mt, PathOp::WriteText, "/mnt/../escape.txt");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_write_bytes() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_write_blocked(&mut mt, PathOp::WriteBytes, "/mnt/../escape.bin");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_open() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        // `w`/`a` open performs an out-of-mount write at open time; `r` would
        // read an out-of-mount file. All must be blocked.
        assert_open_blocked(&mut mt, "/mnt/../open_escape.txt", "w");
        assert_open_blocked(&mut mt, "/mnt/../open_escape.txt", "a");
        assert_open_blocked(&mut mt, "/mnt/../../etc/passwd", "r");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_mkdir() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, PathOp::Mkdir, "/mnt/../escape_dir");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_unlink() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, PathOp::Unlink, "/mnt/../some_file");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_stat() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, PathOp::Stat, "/mnt/../etc/passwd");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn traversal_dotdot_iterdir() {
    for (label, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_blocked(&mut mt, PathOp::Iterdir, "/mnt/..");
        eprintln!("  {label}: passed");
    }
}

#[test]
fn valid_dotdot_within_mount() {
    // /mnt/subdir/../hello.txt normalizes to /mnt/hello.txt which is valid.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = call(&mut mt, PathOp::ReadText, "/mnt/subdir/../hello.txt")
        .unwrap()
        .unwrap();
    assert_eq!(result, MontyObject::String("hello world\n".to_owned()));
}

// =============================================================================
// Null byte injection
// =============================================================================

/// Asserts a call raises `ValueError` with exactly CPython's wording.
#[track_caller]
fn assert_null_byte_error(mt: &mut MountTable, call: OsFunctionCall, expected: &str) {
    let exc = dispatch(mt, call)
        .expect("a null byte is refused with or without a mount")
        .expect_err("expected a ValueError")
        .into_exception();
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message().unwrap_or(""), expected);
}

/// A null byte anywhere in the path raises, wherever it sits.
#[test]
fn null_byte_position_does_not_matter() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
    for path in [
        "/mnt/hello\x00.txt",
        "/mnt/\x00hello.txt",
        "/mnt/hello.txt\x00",
        "/mnt/sub\x00dir/nested.txt",
    ] {
        assert_null_byte_error(&mut mt, PathOp::ReadText.build_path_only(path), "embedded null byte");
    }
}

/// CPython's wording is not uniform: the content operations report the byte
/// from `open()`, while the metadata ones name the syscall they were about to
/// make. Each of these strings was taken from CPython 3.14.
#[test]
fn null_byte_messages_match_cpython_per_operation() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
    let bad = "/mnt/evil\x00.txt";

    for (op, expected) in [
        (PathOp::ReadText, "embedded null byte"),
        (PathOp::ReadBytes, "embedded null byte"),
        (PathOp::Stat, "stat: embedded null character in path"),
        (PathOp::Iterdir, "scandir: embedded null character in path"),
        (PathOp::Unlink, "unlink: embedded null character in path"),
    ] {
        assert_null_byte_error(&mut mt, op.build_path_only(bad), expected);
    }

    assert_null_byte_error(
        &mut mt,
        OsFunctionCall::Rmdir(MontyPath::new(bad.to_owned())),
        "rmdir: embedded null character in path",
    );
    assert_null_byte_error(
        &mut mt,
        OsFunctionCall::Mkdir(MkdirCallArgs {
            path: MontyPath::new(bad.to_owned()),
            parents: false,
            exist_ok: false,
        }),
        "mkdir: embedded null character in path",
    );

    // A rename names the argument that carried the byte.
    assert_null_byte_error(
        &mut mt,
        OsFunctionCall::Rename(RenameCallArgs {
            src: MontyPath::new(bad.to_owned()),
            dst: MontyPath::new("/mnt/ok.txt".to_owned()),
        }),
        "rename: embedded null character in src",
    );
    assert_null_byte_error(
        &mut mt,
        OsFunctionCall::Rename(RenameCallArgs {
            src: MontyPath::new("/mnt/hello.txt".to_owned()),
            dst: MontyPath::new(bad.to_owned()),
        }),
        "rename: embedded null character in dst",
    );
}

/// `resolve()` names the `lstat` it was about to make, as CPython's does.
/// `absolute()` gets the generic wording instead: Monty raises where CPython
/// returns the path untouched, so there is no syscall to name.
#[test]
fn null_byte_messages_for_resolve_and_absolute() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
    let bad = "/mnt/evil\x00.txt";

    assert_null_byte_error(
        &mut mt,
        OsFunctionCall::Resolve(MontyPath::new(bad.to_owned())),
        "lstat: embedded null character in path",
    );
    assert_null_byte_error(
        &mut mt,
        OsFunctionCall::Absolute(MontyPath::new(bad.to_owned())),
        "embedded null byte",
    );
}

/// A path that is both over-long and null-containing reports its length, where
/// CPython reports the null byte: the length check is the one that stays O(1)
/// on a hostile path, so it must not run behind a full scan for `\0`.
#[test]
fn overlong_path_outranks_a_null_byte() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
    let bad = format!("/mnt/{}\x00.txt", "a".repeat(256));

    for call in [
        PathOp::Stat.build_path_only(&bad),
        OsFunctionCall::Rename(RenameCallArgs {
            src: MontyPath::new("/mnt/hello.txt".to_owned()),
            dst: MontyPath::new(bad.clone()),
        }),
    ] {
        let exc = dispatch(&mut mt, call)
            .expect("handled without routing")
            .expect_err("expected an OSError")
            .into_exception();
        assert_eq!(exc.exc_type(), ExcType::OSError);
        assert_eq!(
            exc.message().unwrap_or(""),
            r"[Errno 36] File name too long: '/mnt/aaaaaaaaaaaaaaa…aaaaaaaaaaaaaaa\x00.txt'"
        );
    }
}

/// A path may name at most 64 components, counted as sent.
///
/// The kernel's own `ENAMETOOLONG` never fires under descriptor-relative
/// resolution, which walks a path component by component and so never hands
/// the whole thing to a syscall. Without this limit `PATH_MAX` alone allowed
/// ~2000 components, and every walker costs at least a lookup per level.
#[test]
fn too_many_components_is_refused() {
    let dir = create_test_dir();
    // "/mnt/<n>" splits to n + 2 components, the leading empty included.
    let at_limit = format!("/mnt/{}", vec!["d"; 62].join("/"));
    let over_limit = format!("/mnt/{}", vec!["d"; 63].join("/"));

    for (_, mode) in all_modes() {
        let mut mt = mount_at_mnt(&dir, mode);

        // At the limit the path is merely missing, not refused.
        assert_eq!(
            call(&mut mt, PathOp::Exists, &at_limit).unwrap().unwrap(),
            MontyObject::Bool(false)
        );
        let err = call(&mut mt, PathOp::Stat, &at_limit).unwrap().unwrap_err();
        assert_eq!(err.into_exception().exc_type(), ExcType::FileNotFoundError);

        // One deeper is refused, with the same wording an over-long path gets.
        let exc = call(&mut mt, PathOp::Stat, &over_limit)
            .unwrap()
            .unwrap_err()
            .into_exception();
        assert_eq!(exc.exc_type(), ExcType::OSError);
        assert!(
            exc.message().unwrap_or("").starts_with("[Errno 36] File name too long"),
            "got {:?}",
            exc.message()
        );
        // Predicates swallow it, as they swallow every `OSError`.
        assert_eq!(
            call(&mut mt, PathOp::Exists, &over_limit).unwrap().unwrap(),
            MontyObject::Bool(false)
        );
    }

    // The rename destination is checked too, and neither side needs a mount.
    let mut empty = MountTable::new();
    let outcome = dispatch(
        &mut empty,
        OsFunctionCall::Rename(RenameCallArgs {
            src: MontyPath::new("/nowhere/a.txt".to_owned()),
            dst: MontyPath::new(over_limit),
        }),
    )
    .expect("a too-deep destination is refused without a mount");
    assert!(matches!(outcome, Err(MountError::Io(_, _))), "got {outcome:?}");
}

/// The predicates answer `False` instead of raising, as CPython's do —
/// `pathlib` swallows `ValueError` there exactly as it swallows `OSError`.
#[test]
fn null_byte_predicates_answer_false() {
    let dir = create_test_dir();
    for (_, mode) in all_modes() {
        let mut mt = mount_at_mnt(&dir, mode);
        for op in [PathOp::Exists, PathOp::IsFile, PathOp::IsDir, PathOp::IsSymlink] {
            assert_invisible(&mut mt, op, "/mnt/hello\x00.txt");
        }
    }
}

/// The refusal precedes routing, so it does not depend on a mount covering
/// the path — CPython raises before any syscall too, and a null byte must
/// never reach the host's `os` callback.
#[test]
fn null_byte_is_refused_without_a_mount() {
    let mut mt = MountTable::new();
    assert_null_byte_error(
        &mut mt,
        PathOp::ReadText.build_path_only("/nowhere/evil\x00.txt"),
        "embedded null byte",
    );
    assert_invisible(&mut mt, PathOp::Exists, "/nowhere/evil\x00.txt");
}

/// Writes are refused in every mode, and nothing is recorded under the name.
///
/// `OverlayMemory` is the mode that needs its own check: the key never reaches
/// a syscall, so without one it would accept a name no other mode does.
#[test]
fn null_byte_write_ops_in_every_mode() {
    let dir = create_test_dir();
    for (name, mode) in all_modes() {
        let mut mt = mount_at_mnt(&dir, mode);
        assert_write_blocked(&mut mt, PathOp::WriteText, "/mnt/evil\x00.txt");
        assert_write_blocked(&mut mt, PathOp::WriteBytes, "/mnt/evil\x00.bin");
        assert_blocked(&mut mt, PathOp::Mkdir, "/mnt/evil\x00dir");
        assert_open_blocked(&mut mt, "/mnt/evil\x00.txt", "w");
        assert_open_blocked(&mut mt, "/mnt/evil\x00.txt", "a");

        // Nothing was created or shadowed under any of those names.
        assert_invisible(&mut mt, PathOp::Exists, "/mnt/evil\x00.txt");
        assert_eq!(
            call(&mut mt, PathOp::ReadText, "/mnt/hello.txt").unwrap().unwrap(),
            MontyObject::String("hello world\n".to_owned()),
            "{name}: an unrelated read must still work"
        );
    }
}

// =============================================================================
// Symlink escape
// =============================================================================

mod symlink_tests {
    use super::*;

    /// Overlay writes refuse to land on a symlink, even one resolving inside
    /// the mount: a direct mount writes *through* the link to its target, and
    /// shadowing the link name in memory instead would silently alias the two.
    #[test]
    fn overlay_write_over_inbound_symlink_is_refused() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        symlink_file(dir.path().join("hello.txt"), dir.path().join("link.txt"));

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        let outcome = dispatch(
            &mut mt,
            OsFunctionCall::WriteText(PathStringDataArgs {
                path: MontyPath::new("/mnt/link.txt".to_owned()),
                data: "aliased".to_owned(),
            }),
        )
        .unwrap();
        assert!(
            matches!(&outcome, Err(MountError::PathEscape { .. })),
            "overlay write over an in-mount symlink must be refused, got {outcome:?}"
        );

        // The link's target must be untouched.
        assert_eq!(
            fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
            "hello world\n"
        );
    }

    /// A dangling in-mount symlink gets the same refusal — writing "through" it
    /// would create the target on a direct mount, which the overlay cannot do.
    #[test]
    fn overlay_write_over_dangling_symlink_is_refused() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        symlink_file(dir.path().join("missing.txt"), dir.path().join("dangle.txt"));

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        let outcome = dispatch(
            &mut mt,
            OsFunctionCall::WriteText(PathStringDataArgs {
                path: MontyPath::new("/mnt/dangle.txt".to_owned()),
                data: "ghost".to_owned(),
            }),
        )
        .unwrap();
        assert!(
            matches!(&outcome, Err(MountError::PathEscape { .. })),
            "overlay write over a dangling symlink must be refused, got {outcome:?}"
        );
        assert!(
            !dir.path().join("missing.txt").exists(),
            "link target must not be created"
        );
    }

    /// Renaming a directory that contains a symlink is refused outright.
    ///
    /// The link has no overlay representation, so the move could only skip it,
    /// stranding a live file: unreachable at the new name, still readable at
    /// the old one inside a directory the overlay now reports as deleted.
    #[test]
    fn overlay_rename_of_dir_containing_a_symlink_is_refused() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        symlink_file("nested.txt", dir.path().join("subdir/link.txt"));

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        let outcome = dispatch(
            &mut mt,
            OsFunctionCall::Rename(RenameCallArgs {
                src: MontyPath::new("/mnt/subdir".to_owned()),
                dst: MontyPath::new("/mnt/moved".to_owned()),
            }),
        )
        .unwrap();
        assert!(
            matches!(&outcome, Err(MountError::PathEscape { .. })),
            "renaming a directory containing a symlink must be refused, got {outcome:?}"
        );

        // The refusal must be total: the directory keeps its name and contents,
        // rather than half-moving and leaving the link behind.
        assert_eq!(
            call(&mut mt, PathOp::Exists, "/mnt/subdir").unwrap().unwrap(),
            MontyObject::Bool(true)
        );
        assert_eq!(
            call(&mut mt, PathOp::ReadText, "/mnt/subdir/nested.txt")
                .unwrap()
                .unwrap(),
            MontyObject::String("nested content".to_owned())
        );
        assert_eq!(
            call(&mut mt, PathOp::Exists, "/mnt/moved").unwrap().unwrap(),
            MontyObject::Bool(false)
        );
    }

    /// `unlink` and `rmdir` are writes, so the symlink refusal covers them too.
    ///
    /// Tombstoning a link's spelling would report it gone while its target
    /// stayed readable under the real name — the aliasing the write policy
    /// exists to prevent, arrived at by deleting instead of writing.
    #[test]
    fn overlay_delete_through_a_symlink_is_refused() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        symlink_dir("subdir", dir.path().join("link_dir"));
        symlink_file("hello.txt", dir.path().join("link.txt"));

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        // Below an intermediate link, on the link itself, and rmdir of a link
        // to a directory (POSIX answers ENOTDIR; either way it is not removed).
        for path in ["/mnt/link_dir/nested.txt", "/mnt/link.txt"] {
            let outcome = call(&mut mt, PathOp::Unlink, path).unwrap();
            assert!(
                matches!(&outcome, Err(MountError::PathEscape { .. })),
                "unlink of {path} must be refused, got {outcome:?}"
            );
        }
        let outcome = dispatch(
            &mut mt,
            OsFunctionCall::Rmdir(MontyPath::new("/mnt/link_dir".to_owned())),
        )
        .unwrap();
        assert!(
            matches!(&outcome, Err(MountError::PathEscape { .. })),
            "rmdir of a symlink must be refused, got {outcome:?}"
        );

        // Nothing was shadowed: the real name still reads. The link's own
        // spelling answers `False`, as any refused path does.
        assert_eq!(
            call(&mut mt, PathOp::ReadText, "/mnt/subdir/nested.txt")
                .unwrap()
                .unwrap(),
            MontyObject::String("nested content".to_owned())
        );
        assert_eq!(
            call(&mut mt, PathOp::Exists, "/mnt/link_dir/nested.txt")
                .unwrap()
                .unwrap(),
            MontyObject::Bool(false)
        );
    }

    /// A rename destination is classified like any other write target, so it
    /// cannot land on an existing symlink — which `write_text` already refuses
    /// for the same path.
    #[test]
    fn overlay_rename_onto_a_symlink_is_refused() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        symlink_file("hello.txt", dir.path().join("link.txt"));
        fs::write(dir.path().join("src.txt"), "moved").unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        let outcome = dispatch(
            &mut mt,
            OsFunctionCall::Rename(RenameCallArgs {
                src: MontyPath::new("/mnt/src.txt".to_owned()),
                dst: MontyPath::new("/mnt/link.txt".to_owned()),
            }),
        )
        .unwrap();
        assert!(
            matches!(&outcome, Err(MountError::PathEscape { .. })),
            "renaming onto a symlink must be refused, got {outcome:?}"
        );

        // The link's target is untouched — read by its real name, since the
        // link's own spelling is refused like every other.
        assert_eq!(
            call(&mut mt, PathOp::ReadText, "/mnt/hello.txt").unwrap().unwrap(),
            MontyObject::String("hello world\n".to_owned())
        );
        assert_eq!(
            call(&mut mt, PathOp::Exists, "/mnt/src.txt").unwrap().unwrap(),
            MontyObject::Bool(true)
        );
    }

    /// A rename *source* is a write path too — it gets tombstoned — so a
    /// symlink in its parent chain is refused exactly as `unlink` on the
    /// identical path is. Only the source's final component is exempt.
    #[test]
    fn overlay_rename_out_of_a_symlinked_directory_is_refused() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        symlink_dir("subdir", dir.path().join("link_dir"));

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        let outcome = dispatch(
            &mut mt,
            OsFunctionCall::Rename(RenameCallArgs {
                src: MontyPath::new("/mnt/link_dir/nested.txt".to_owned()),
                dst: MontyPath::new("/mnt/moved.txt".to_owned()),
            }),
        )
        .unwrap();
        assert!(
            matches!(&outcome, Err(MountError::PathEscape { .. })),
            "renaming out of a symlinked directory must be refused, got {outcome:?}"
        );

        // `unlink` on that same path is refused identically — the consistency
        // this test exists to pin.
        let unlinked = call(&mut mt, PathOp::Unlink, "/mnt/link_dir/nested.txt").unwrap();
        assert!(
            matches!(&unlinked, Err(MountError::PathEscape { .. })),
            "unlink must agree with rename, got {unlinked:?}"
        );

        // Neither spelling was shadowed, so both still agree with the host.
        assert_eq!(
            call(&mut mt, PathOp::ReadText, "/mnt/subdir/nested.txt")
                .unwrap()
                .unwrap(),
            MontyObject::String("nested content".to_owned())
        );
        assert_eq!(
            call(&mut mt, PathOp::Exists, "/mnt/moved.txt").unwrap().unwrap(),
            MontyObject::Bool(false)
        );
    }

    /// A symlink to a directory is refused like any other, so its target's
    /// tree cannot be walked into the overlay under a new name.
    #[test]
    fn overlay_rename_of_a_symlink_to_a_dir_is_refused() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        symlink_dir("subdir", dir.path().join("link_dir"));

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        let outcome = dispatch(
            &mut mt,
            OsFunctionCall::Rename(RenameCallArgs {
                src: MontyPath::new("/mnt/link_dir".to_owned()),
                dst: MontyPath::new("/mnt/moved".to_owned()),
            }),
        )
        .unwrap();
        assert!(
            matches!(&outcome, Err(MountError::PathEscape { .. })),
            "renaming a symlink to a directory must be refused, got {outcome:?}"
        );

        // Nothing moved, and the target keeps its own name and children.
        assert_eq!(
            call(&mut mt, PathOp::Exists, "/mnt/moved").unwrap().unwrap(),
            MontyObject::Bool(false)
        );
        assert_eq!(
            call(&mut mt, PathOp::IsDir, "/mnt/subdir").unwrap().unwrap(),
            MontyObject::Bool(true)
        );
        assert_eq!(
            call(&mut mt, PathOp::ReadText, "/mnt/subdir/nested.txt")
                .unwrap()
                .unwrap(),
            MontyObject::String("nested content".to_owned())
        );
    }

    /// `open(..., 'a')` on a file that already exists below an in-mount symlink
    /// is refused at open time, not on the first append. Only the append path
    /// takes the "target already exists" shortcut, which used to skip the
    /// parent walk that every other overlay write runs.
    #[test]
    fn overlay_append_open_below_inbound_symlink_dir_is_refused() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        symlink_dir("subdir", dir.path().join("link_dir"));

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        assert_open_blocked(&mut mt, "/mnt/link_dir/nested.txt", "a");
        assert_eq!(
            fs::read_to_string(dir.path().join("subdir/nested.txt")).unwrap(),
            "nested content"
        );
    }

    /// Even a symlink resolving to an in-mount directory blocks overlay writes
    /// through it: entries would be keyed under the link's spelling, invisible
    /// via the resolved name. Direct mode follows such links; the overlay
    /// refuses, whether the link is the immediate parent or deeper in the path.
    #[test]
    fn overlay_write_through_inbound_symlink_dir_is_refused() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        // A relative target — an absolute one is never followed, even in-mount
        // (see `limitations/filesystem.md`).
        symlink_dir("subdir", dir.path().join("link_dir"));

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        let outcome = dispatch(
            &mut mt,
            OsFunctionCall::Mkdir(MkdirCallArgs {
                path: MontyPath::new("/mnt/link_dir/new".to_owned()),
                parents: true,
                exist_ok: false,
            }),
        )
        .unwrap();
        assert!(
            matches!(&outcome, Err(MountError::PathEscape { .. })),
            "overlay mkdir through an in-mount symlink dir must be refused, got {outcome:?}"
        );

        // The link as a non-immediate parent is caught by the component walk,
        // not just the final-parent lookup ("subdir/deep" exists for real).
        let outcome = dispatch(
            &mut mt,
            OsFunctionCall::WriteText(PathStringDataArgs {
                path: MontyPath::new("/mnt/link_dir/deep/x.txt".to_owned()),
                data: "aliased".to_owned(),
            }),
        )
        .unwrap();
        assert!(
            matches!(&outcome, Err(MountError::PathEscape { .. })),
            "overlay write below an in-mount symlink dir must be refused, got {outcome:?}"
        );
        assert!(!dir.path().join("subdir/deep/x.txt").exists(), "host must be untouched");
    }

    #[test]
    fn symlink_to_outside_directory() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret data").unwrap();

        // Create symlink inside mount pointing outside.
        symlink_dir(outside.path(), dir.path().join("escape_link"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_blocked(&mut mt, PathOp::ReadText, "/mnt/escape_link/secret.txt");
        assert_invisible(&mut mt, PathOp::Exists, "/mnt/escape_link/secret.txt");
        assert_blocked(&mut mt, PathOp::Iterdir, "/mnt/escape_link");
    }

    #[test]
    fn symlink_to_outside_file() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();

        symlink_file(outside.path().join("secret.txt"), dir.path().join("link_to_file"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_blocked(&mut mt, PathOp::ReadText, "/mnt/link_to_file");
    }

    #[test]
    fn symlink_open_escape() {
        if !symlinks_supported() {
            return;
        }
        // `open()` on a path that escapes the mount via a symlink must be
        // rejected — for read (would read an outside file) and for write
        // (the open-time truncate would write outside the mount).
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        symlink_dir(outside.path(), dir.path().join("escape_link"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_open_blocked(&mut mt, "/mnt/escape_link/secret.txt", "r");
        assert_open_blocked(&mut mt, "/mnt/escape_link/new.txt", "w");
        assert_open_blocked(&mut mt, "/mnt/escape_link/new.txt", "a");
    }

    #[test]
    fn symlink_to_parent() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        let parent = dir.path().parent().unwrap();

        symlink_dir(parent, dir.path().join("parent_link"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_blocked(&mut mt, PathOp::Iterdir, "/mnt/parent_link");
    }

    #[test]
    #[cfg(unix)] // Relative symlink targets are not supported on Windows
    fn relative_symlink_escape() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();

        // Create symlink that uses relative path to escape.
        symlink_dir("../../", dir.path().join("rel_escape"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_blocked(&mut mt, PathOp::Iterdir, "/mnt/rel_escape");
    }

    /// `is_symlink()` does not follow the final component, so an outbound link
    /// that is itself inside the mount still answers `True` — as in CPython, and
    /// revealing nothing about where it points. The following predicates differ.
    #[test]
    fn is_symlink_reports_an_outbound_link_that_lives_in_the_mount() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        symlink_file(outside.path().join("secret.txt"), dir.path().join("escape_link"));

        for mode in [MountMode::ReadWrite, MountMode::OverlayMemory(OverlayState::new())] {
            let mut mt = mount_at_mnt(&dir, mode);
            assert_eq!(
                call(&mut mt, PathOp::IsSymlink, "/mnt/escape_link").unwrap().unwrap(),
                MontyObject::Bool(true)
            );
            assert_invisible(&mut mt, PathOp::Exists, "/mnt/escape_link");
            assert_invisible(&mut mt, PathOp::IsFile, "/mnt/escape_link");
        }
    }

    #[test]
    fn symlink_escape_no_info_leak() {
        if !symlinks_supported() {
            return;
        }
        // Error messages should only contain virtual path, not host path.
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        symlink_dir(outside.path(), dir.path().join("escape"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, PathOp::ReadText, "/mnt/escape/secret");
        match result {
            Some(Err(ref err)) => {
                let msg = format!("{err}");
                let host_str = dir.path().to_string_lossy();
                assert!(
                    !msg.contains(host_str.as_ref()),
                    "error message should not contain host path: {msg}"
                );
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn symlink_escape_overlay_memory() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        symlink_dir(outside.path(), dir.path().join("escape"));

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        assert_blocked(&mut mt, PathOp::ReadText, "/mnt/escape/secret.txt");
        assert_invisible(&mut mt, PathOp::Exists, "/mnt/escape/secret.txt");
    }

    #[test]
    fn symlink_within_mount_allowed() {
        if !symlinks_supported() {
            return;
        }
        // Symlinks that stay within the mount boundary should work.
        let dir = create_test_dir();
        symlink_file("hello.txt", dir.path().join("internal_link"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, PathOp::ReadText, "/mnt/internal_link").unwrap().unwrap();
        assert_eq!(result, MontyObject::String("hello world\n".to_owned()));
    }

    #[test]
    fn symlink_to_directory_within_mount_allowed() {
        if !symlinks_supported() {
            return;
        }
        // Symlink to a subdirectory within the mount should work for all operations.
        let dir = create_test_dir();
        symlink_dir("subdir", dir.path().join("dir_link"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

        // Reading a file through the symlinked directory should work.
        let result = call(&mut mt, PathOp::ReadText, "/mnt/dir_link/nested.txt")
            .unwrap()
            .unwrap();
        assert_eq!(result, MontyObject::String("nested content".to_owned()));

        // Listing the symlinked directory should work.
        let result = call(&mut mt, PathOp::Iterdir, "/mnt/dir_link");
        assert!(result.unwrap().is_ok());

        // Checking existence through the symlink should work.
        let result = call(&mut mt, PathOp::Exists, "/mnt/dir_link/deep/file.txt")
            .unwrap()
            .unwrap();
        assert_eq!(result, MontyObject::Bool(true));
    }

    #[test]
    fn chained_symlinks_within_mount_allowed() {
        if !symlinks_supported() {
            return;
        }
        // A symlink pointing to another symlink, both within the mount, should work.
        let dir = create_test_dir();
        symlink_file("hello.txt", dir.path().join("link1"));
        symlink_file("link1", dir.path().join("link2"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, PathOp::ReadText, "/mnt/link2").unwrap().unwrap();
        assert_eq!(result, MontyObject::String("hello world\n".to_owned()));
    }

    #[test]
    fn chained_symlinks_escape_blocked() {
        if !symlinks_supported() {
            return;
        }
        // A symlink within mount pointing to another symlink that escapes should be blocked.
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();

        // link1 -> outside (escapes), link2 -> link1 (chain escapes)
        symlink_dir(outside.path(), dir.path().join("link1"));
        symlink_dir(dir.path().join("link1"), dir.path().join("link2"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_blocked(&mut mt, PathOp::ReadText, "/mnt/link2/secret.txt");
    }

    #[test]
    fn mkdir_parents_through_symlink_escape_blocked_readwrite() {
        if !symlinks_supported() {
            return;
        }
        // Regression test: mkdir(parents=True) through a symlinked ancestor must
        // not create directories outside the mount boundary in ReadWrite mode.
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();

        // Create a symlink inside the mount that points outside.
        symlink_dir(outside.path(), dir.path().join("escape"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

        let result = call_mkdir(&mut mt, "/mnt/escape/pwned", true, true);
        match result {
            Some(Err(MountError::PathEscape { .. } | MountError::Io(_, _))) => {}
            Some(Ok(_)) => panic!("mkdir through symlink escape should be blocked"),
            other => panic!("unexpected result: {other:?}"),
        }

        // Verify nothing was created outside the mount.
        assert!(
            !outside.path().join("pwned").exists(),
            "directory was created outside the mount!"
        );
    }

    #[test]
    fn mkdir_parents_through_symlink_escape_blocked_readonly() {
        if !symlinks_supported() {
            return;
        }
        // ReadOnly mode should also block mkdir through symlink escape.
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        symlink_dir(outside.path(), dir.path().join("escape"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadOnly);

        let result = call_mkdir(&mut mt, "/mnt/escape/pwned", true, true);
        match result {
            Some(Err(MountError::PathEscape { .. } | MountError::Io(_, _) | MountError::ReadOnly(_))) => {}
            Some(Ok(_)) => panic!("mkdir through symlink escape should be blocked"),
            other => panic!("unexpected result: {other:?}"),
        }

        assert!(
            !outside.path().join("pwned").exists(),
            "directory was created outside the mount!"
        );
    }

    #[test]
    fn mkdir_parents_through_nested_symlink_escape_blocked() {
        if !symlinks_supported() {
            return;
        }
        // mkdir(parents=True) through a symlinked directory deeper in the tree.
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();

        // subdir/link -> outside
        symlink_dir(outside.path(), dir.path().join("subdir").join("link"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

        let result = call_mkdir(&mut mt, "/mnt/subdir/link/deep/dir", true, true);
        match result {
            Some(Err(MountError::PathEscape { .. } | MountError::Io(_, _))) => {}
            Some(Ok(_)) => panic!("mkdir through nested symlink escape should be blocked"),
            other => panic!("unexpected result: {other:?}"),
        }

        assert!(
            !outside.path().join("deep").exists(),
            "directory was created outside the mount!"
        );
    }

    #[test]
    fn mkdir_parents_within_mount_allowed() {
        // mkdir(parents=True) for paths entirely within the mount should succeed.
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

        let result = call_mkdir(&mut mt, "/mnt/new/nested/dir", true, true);
        assert!(result.unwrap().is_ok(), "mkdir within mount should succeed");
        assert!(dir.path().join("new/nested/dir").exists());
    }

    #[test]
    fn mkdir_parents_through_internal_symlink_allowed() {
        if !symlinks_supported() {
            return;
        }
        // mkdir(parents=True) through a symlink that stays within the mount is fine.
        let dir = create_test_dir();

        // Create a symlink within mount pointing to another dir within mount.
        symlink_dir("subdir", dir.path().join("internal_link"));

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

        let result = call_mkdir(&mut mt, "/mnt/internal_link/new_child", true, true);
        assert!(result.unwrap().is_ok(), "mkdir through internal symlink should succeed");
        assert!(dir.path().join("subdir/new_child").exists());
    }

    /// Depth of the chain the walk is exercised against. Deep enough that a
    /// walk re-spelling the whole prefix per component would be quadratic.
    const DEEP_CHAIN: usize = 40;

    /// The overlay's symlink refusal holds at every depth of a long chain.
    ///
    /// Guards the descriptor-descending walk in `reject_symlink_chain`: it looks
    /// up one component at a time rather than a growing prefix, so a link that
    /// used to be seen by a whole-path lookup must still be seen by a
    /// single-component one — wherever in the chain it sits.
    #[test]
    fn overlay_symlink_is_refused_at_every_depth_of_a_deep_chain() {
        if !symlinks_supported() {
            return;
        }
        let names: Vec<String> = (0..DEEP_CHAIN).map(|i| format!("d{i}")).collect();
        let chain = names.join("/");

        // A link-free chain of the same depth still resolves, so the walk is
        // being passed rather than short-circuited.
        let dir = create_test_dir();
        fs::create_dir_all(dir.path().join(&chain)).unwrap();
        fs::write(dir.path().join(&chain).join("leaf.txt"), "deep").unwrap();
        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        assert_eq!(
            call(&mut mt, PathOp::ReadText, &format!("/mnt/{chain}/leaf.txt"))
                .unwrap()
                .unwrap(),
            MontyObject::String("deep".to_owned())
        );

        // Now rebuild the chain with one component replaced by a symlink to a
        // sibling directory of the same shape, at each depth in turn.
        for swapped in 0..DEEP_CHAIN {
            let dir = create_test_dir();
            let prefix = names[..swapped].join("/");
            let parent = if prefix.is_empty() {
                dir.path().to_owned()
            } else {
                dir.path().join(&prefix)
            };
            // `real/<rest of chain>` holds the leaf; the swapped component is a
            // link to `real`, so the path only resolves by following it.
            fs::create_dir_all(parent.join("real").join(names[swapped + 1..].join("/"))).unwrap();
            fs::write(
                parent
                    .join("real")
                    .join(names[swapped + 1..].join("/"))
                    .join("leaf.txt"),
                "deep",
            )
            .unwrap();
            symlink_dir("real", parent.join(&names[swapped]));

            let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
            let vpath = format!("/mnt/{chain}/leaf.txt");
            let outcome = call(&mut mt, PathOp::ReadText, &vpath).unwrap();
            assert!(
                matches!(&outcome, Err(MountError::PathEscape { .. })),
                "a link at depth {swapped} must be refused, got {outcome:?}"
            );
            assert_eq!(
                call(&mut mt, PathOp::Exists, &vpath).unwrap().unwrap(),
                MontyObject::Bool(false),
                "a link at depth {swapped} must make the path invisible"
            );
        }
    }
}

// =============================================================================
// Hard link tests (`ln` without `-s`)
// =============================================================================
//
// Hard links are fundamentally different from symbolic links: a hard link is
// just another directory entry for the same inode, not a pointer to a path.
// `fs::canonicalize()` returns the path as-given (within the mount), so hard
// links always pass the boundary check regardless of where the original file
// lives.
//
// This is acceptable because sandboxed code cannot create hard links (no
// `os.link` is exposed), so hard links can only be placed in the mount by
// the host — an explicit choice to expose that content.

mod hard_link_tests {
    use super::*;

    #[test]
    fn hard_link_within_mount_allowed() {
        // A hard link to a file within the mount should work normally.
        let dir = create_test_dir();
        fs::hard_link(dir.path().join("hello.txt"), dir.path().join("hardlink.txt")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, PathOp::ReadText, "/mnt/hardlink.txt").unwrap().unwrap();
        assert_eq!(result, MontyObject::String("hello world\n".to_owned()));
    }

    #[test]
    fn hard_link_from_outside_accessible() {
        // A hard link to a file outside the mount is indistinguishable from a
        // regular file at the path level — canonicalize returns the in-mount
        // path, so the boundary check passes. This is by design: only the host
        // can create hard links in the mounted directory, so this represents an
        // explicit choice to expose the content.
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("external.txt");
        fs::write(&outside_file, "external content").unwrap();

        fs::hard_link(&outside_file, dir.path().join("hardlink_ext.txt")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, PathOp::ReadText, "/mnt/hardlink_ext.txt")
            .unwrap()
            .unwrap();
        assert_eq!(result, MontyObject::String("external content".to_owned()));
    }

    #[test]
    fn hard_link_is_not_detected_as_symlink() {
        // Hard links should report as regular files, not symlinks.
        let dir = create_test_dir();
        fs::hard_link(dir.path().join("hello.txt"), dir.path().join("hardlink.txt")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, PathOp::IsFile, "/mnt/hardlink.txt").unwrap().unwrap();
        assert_eq!(result, MontyObject::Bool(true));

        let result = call(&mut mt, PathOp::IsSymlink, "/mnt/hardlink.txt").unwrap().unwrap();
        assert_eq!(result, MontyObject::Bool(false));
    }

    /// A broken symlink (target doesn't exist) inside the mount that points
    /// outside must not allow `write_text` / `write_bytes` to follow it.
    #[test]
    #[cfg(unix)]
    fn broken_symlink_write_escape_blocked() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        let escape_target = outside.path().join("pwned.txt");

        // Create symlink inside mount -> outside file that doesn't exist yet.
        symlink(&escape_target, dir.path().join("broken_link.txt")).unwrap();

        // Sanity: it's a broken symlink.
        assert!(!dir.path().join("broken_link.txt").exists());
        assert!(dir.path().join("broken_link.txt").symlink_metadata().is_ok());

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_write_blocked(&mut mt, PathOp::WriteText, "/mnt/broken_link.txt");
        assert_write_blocked(&mut mt, PathOp::WriteBytes, "/mnt/broken_link.txt");

        // The target file must NOT have been created.
        assert!(
            !escape_target.exists(),
            "broken symlink write escape: file was created outside the mount!"
        );
    }

    /// Overlay mode refuses to write over a symlink the descriptor cannot
    /// follow — shadowing it in memory would alias the name, so the write
    /// raises `PermissionError` instead, and the real target is never created.
    #[test]
    #[cfg(unix)]
    fn broken_symlink_overlay_write_is_refused() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        let escape_target = outside.path().join("pwned.txt");

        symlink(&escape_target, dir.path().join("broken_link.txt")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));

        let outcome = dispatch(
            &mut mt,
            OsFunctionCall::WriteText(PathStringDataArgs {
                path: MontyPath::new("/mnt/broken_link.txt".to_owned()),
                data: "safe".to_owned(),
            }),
        )
        .unwrap();
        assert!(
            matches!(&outcome, Err(MountError::PathEscape { .. })),
            "overlay write over an outbound symlink must be refused, got {outcome:?}"
        );

        // Real FS target was NOT created.
        assert!(!escape_target.exists());
    }

    /// Iterdir must filter out symlinks pointing outside the mount (including
    /// broken ones) while keeping regular files and inbound symlinks.
    #[test]
    #[cfg(unix)]
    fn iterdir_filters_outbound_symlinks_but_keeps_regular_and_inbound() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("external.txt"), "external").unwrap();

        // Outbound symlink (points outside mount) — should be filtered.
        symlink(outside.path().join("external.txt"), dir.path().join("escape_link")).unwrap();
        // Inbound symlink (points inside mount) — should be kept.
        symlink("hello.txt", dir.path().join("internal_link")).unwrap();

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, PathOp::Iterdir, "/mnt").unwrap().unwrap();

        if let MontyObject::List(entries) = &result {
            let names: Vec<String> = entries
                .iter()
                .filter_map(|e| {
                    if let MontyObject::Path(p) = e {
                        p.rsplit('/').next().map(ToOwned::to_owned)
                    } else {
                        None
                    }
                })
                .collect();
            assert!(
                !names.contains(&"escape_link".to_owned()),
                "outbound symlink should be filtered from iterdir"
            );
            assert!(
                names.contains(&"internal_link".to_owned()),
                "inbound symlink should be kept in iterdir"
            );
            assert!(
                names.contains(&"hello.txt".to_owned()),
                "regular files should be present"
            );
        } else {
            panic!("expected List from iterdir, got {result:?}");
        }
    }

    /// An in-mount symlink whose *name* is not valid UTF-8 must still be listed.
    ///
    /// The in-mount check has to run against the raw directory entry: rebuilding
    /// the path from a lossy name looks up something that does not exist, so the
    /// link is silently dropped. Its listed name is still lossy — that predates
    /// this and is unchanged here.
    #[test]
    #[cfg(unix)]
    fn iterdir_keeps_inbound_symlink_with_non_utf8_name() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        let raw_name = OsStr::from_bytes(b"nonutf8\xff_link");
        if symlink("hello.txt", dir.path().join(raw_name)).is_err() {
            // APFS and friends reject non-UTF-8 filenames outright, so the bug
            // this guards is unobservable there. ext4 exercises it.
            eprintln!("skipped: filesystem rejects non-UTF-8 filenames");
            return;
        }

        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let result = call(&mut mt, PathOp::Iterdir, "/mnt").unwrap().unwrap();
        let names = sorted_names_from_list(&result);

        let lossy = raw_name.to_string_lossy().to_string();
        assert!(
            names.contains(&lossy),
            "in-mount symlink with a non-UTF-8 name was dropped: {names:?}"
        );
    }

    /// Renaming a directory whose real contents include a non-UTF-8 name must
    /// fail loudly and leave everything in place.
    ///
    /// Such a name cannot be an overlay key, so the rename plan cannot
    /// represent the move. It used to be built from the lossy name instead,
    /// which stats a path that does not exist — the file silently vanished
    /// from the destination while the source read as deleted.
    #[test]
    #[cfg(unix)]
    fn overlay_rename_of_dir_with_non_utf8_named_entry_is_refused() {
        let dir = create_test_dir();
        let raw_name = OsStr::from_bytes(b"nonutf8\xff.txt");
        if fs::write(dir.path().join("subdir").join(raw_name), b"data").is_err() {
            // APFS and friends reject non-UTF-8 filenames outright, so the bug
            // this guards is unobservable there. ext4 exercises it.
            eprintln!("skipped: filesystem rejects non-UTF-8 filenames");
            return;
        }

        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        let outcome = dispatch(
            &mut mt,
            OsFunctionCall::Rename(monty_types::RenameCallArgs {
                src: MontyPath::new("/mnt/subdir".to_owned()),
                dst: MontyPath::new("/mnt/moved".to_owned()),
            }),
        )
        .unwrap();
        assert!(
            matches!(&outcome, Err(MountError::Io(err, _)) if err.kind() == ErrorKind::InvalidData),
            "rename over a non-UTF-8 name must fail with InvalidData, got {outcome:?}"
        );

        // The refusal must leave the source tree fully visible and create nothing.
        let exists = call(&mut mt, PathOp::Exists, "/mnt/subdir/nested.txt")
            .unwrap()
            .unwrap();
        assert_eq!(exists, MontyObject::Bool(true));
        let moved = call(&mut mt, PathOp::Exists, "/mnt/moved").unwrap().unwrap();
        assert_eq!(moved, MontyObject::Bool(false));
    }

    /// Overlay mode should expose the same visible real entries as direct mode:
    /// inbound symlinks stay visible, outbound and broken symlinks are filtered.
    #[test]
    #[cfg(unix)]
    fn overlay_iterdir_filters_symlinks_like_direct_mode() {
        if !symlinks_supported() {
            return;
        }
        let dir = create_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("external.txt"), "external").unwrap();

        symlink(outside.path().join("external.txt"), dir.path().join("escape_link")).unwrap();
        symlink(outside.path().join("missing.txt"), dir.path().join("broken_link")).unwrap();
        symlink("hello.txt", dir.path().join("internal_link")).unwrap();

        let mut direct = mount_at_mnt(&dir, MountMode::ReadWrite);
        let direct_result = call(&mut direct, PathOp::Iterdir, "/mnt").unwrap().unwrap();

        let mut overlay = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        let overlay_result = call(&mut overlay, PathOp::Iterdir, "/mnt").unwrap().unwrap();

        let direct_names = sorted_names_from_list(&direct_result);
        let overlay_names = sorted_names_from_list(&overlay_result);

        assert_eq!(overlay_names, direct_names);
        assert!(overlay_names.contains(&"internal_link".to_owned()));
        assert!(!overlay_names.contains(&"escape_link".to_owned()));
        assert!(!overlay_names.contains(&"broken_link".to_owned()));
    }
}

/// Extracts sorted entry basenames from an `iterdir()` result list.
#[cfg(unix)]
fn sorted_names_from_list(obj: &MontyObject) -> Vec<String> {
    match obj {
        MontyObject::List(entries) => {
            let mut names: Vec<String> = entries
                .iter()
                .filter_map(|entry| match entry {
                    MontyObject::Path(path) => path.rsplit('/').next().map(ToOwned::to_owned),
                    _ => None,
                })
                .collect();
            names.sort();
            names
        }
        other => panic!("expected List from iterdir result, got {other:?}"),
    }
}

// =============================================================================
// Virtual path normalization edge cases
// =============================================================================

#[test]
fn double_slashes() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    // Double slashes should be normalized.
    assert_eq!(
        call(&mut mt, PathOp::ReadText, "/mnt//hello.txt").unwrap().unwrap(),
        MontyObject::String("hello world\n".to_owned())
    );
}

#[test]
fn dot_components() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    assert_eq!(
        call(&mut mt, PathOp::ReadText, "/mnt/./hello.txt").unwrap().unwrap(),
        MontyObject::String("hello world\n".to_owned())
    );
    assert_eq!(
        call(&mut mt, PathOp::ReadText, "/mnt/./subdir/./nested.txt")
            .unwrap()
            .unwrap(),
        MontyObject::String("nested content".to_owned())
    );
}

#[test]
fn triple_dots_literal_name() {
    // "..." is a valid filename, not a path traversal.
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    // Trying to read a file named "..." that doesn't exist should give NotFound, not PathEscape.
    let result = call(&mut mt, PathOp::Exists, "/mnt/...");
    match result {
        Some(Ok(MontyObject::Bool(false))) => {} // Good — just doesn't exist.
        Some(Err(MountError::Io(_, _))) => {}    // Also acceptable.
        other => panic!("expected false or Io error for /mnt/..., got {other:?}"),
    }
}

// =============================================================================
// Mount configuration validation
// =============================================================================

#[test]
fn mount_relative_virtual_path_rejected() {
    let dir = TempDir::new().unwrap();
    let mut mt = MountTable::new();
    let err = mt
        .mount("relative/path", dir.path(), MountMode::ReadWrite, None)
        .unwrap_err();
    assert!(matches!(err, MountError::InvalidMount(_)));
}

#[test]
fn mount_nonexistent_host_path() {
    let mut mt = MountTable::new();
    let err = mt
        .mount(
            "/mnt",
            "/nonexistent/path/that/does/not/exist",
            MountMode::ReadWrite,
            None,
        )
        .unwrap_err();
    assert!(matches!(err, MountError::InvalidMount(_)));
}

#[test]
fn mount_file_as_host_path() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("not_a_dir.txt");
    fs::write(&file_path, "content").unwrap();

    let mut mt = MountTable::new();
    let err = mt.mount("/mnt", &file_path, MountMode::ReadWrite, None).unwrap_err();
    assert!(matches!(err, MountError::InvalidMount(_)));
}

// =============================================================================
// Information leakage
// =============================================================================

#[test]
fn path_escape_error_only_contains_virtual_path() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    // A drive-letter segment is refused as an escape on every host.
    let result = call(&mut mt, PathOp::ReadText, "/mnt/C:/evil");
    match result {
        Some(Err(MountError::PathEscape { virtual_path })) => {
            assert_eq!(virtual_path, "/mnt/C:/evil");
            // Verify host path is not in the error.
            let host_str = dir.path().to_string_lossy();
            assert!(
                !virtual_path.contains(host_str.as_ref()),
                "PathEscape should not contain host path"
            );
        }
        other => panic!("expected PathEscape, got {other:?}"),
    }
}

/// The null-byte `ValueError` quotes no path at all, as CPython's does — it is
/// raised from argument parsing, before anything echoes the path back.
#[test]
fn null_byte_error_quotes_no_path() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let exc = dispatch(&mut mt, PathOp::ReadText.build_path_only("/mnt/evil\x00.txt"))
        .expect("handled")
        .expect_err("expected a ValueError")
        .into_exception();
    let msg = exc.message().expect("exception should have message");
    assert_eq!(msg, "embedded null byte");
    assert!(!msg.contains('\0'), "the message must not echo the path back");
}

#[test]
fn no_mount_point_returns_none() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = call(&mut mt, PathOp::ReadText, "/outside/secret.txt");
    assert!(
        result.is_none(),
        "expected None for path outside all mounts, got {result:?}"
    );
}

#[test]
fn error_into_exception_preserves_virtual_path() {
    // Verify that into_exception() doesn't leak host paths.
    let err = MountError::PathEscape {
        virtual_path: "/mnt/evil".to_owned(),
    };
    let exc = err.into_exception();
    let msg = exc.message().expect("exception should have message");
    assert!(msg.contains("/mnt/evil"));
    assert!(!msg.contains("/tmp/"), "should not contain tmp host paths");
    assert!(!msg.contains("/var/"), "should not contain var host paths");
}

// =============================================================================
// Operations on empty/unconfigured mount table
// =============================================================================

#[test]
fn empty_table_all_ops_unhandled() {
    let mut mt = MountTable::new();

    for func in [
        PathOp::Exists,
        PathOp::IsFile,
        PathOp::IsDir,
        PathOp::ReadText,
        PathOp::Stat,
        PathOp::Iterdir,
    ] {
        let result = call(&mut mt, func, "/any/path");
        assert!(
            result.is_none(),
            "empty table should return None for {func:?}, got {result:?}"
        );
    }
}

// =============================================================================
// Traversal via rename
// =============================================================================

#[test]
fn rename_traversal_src() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = dispatch(
        &mut mt,
        OsFunctionCall::Rename(monty_types::RenameCallArgs {
            src: MontyPath::new("/mnt/../etc/passwd".to_owned()),
            dst: MontyPath::new("/mnt/stolen.txt".to_owned()),
        }),
    );
    // The src normalizes to `/etc/passwd`, under no mount, so the rename is
    // refused outright rather than handed to the fallback with the mounted dst.
    match result {
        Some(Err(MountError::CrossMountRename { .. })) => {}
        other => panic!("expected rename src traversal blocked, got {other:?}"),
    }
}

#[test]
fn rename_traversal_dst() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = dispatch(
        &mut mt,
        OsFunctionCall::Rename(monty_types::RenameCallArgs {
            src: MontyPath::new("/mnt/hello.txt".to_owned()),
            dst: MontyPath::new("/mnt/../escape.txt".to_owned()),
        }),
    );
    // Likewise for a dst that normalizes out of the mount.
    match result {
        Some(Err(MountError::CrossMountRename { .. })) => {}
        other => panic!("expected rename dst traversal blocked, got {other:?}"),
    }
}

// =============================================================================
// Sandbox escape via rename of symlink pointing outside mount
// =============================================================================

/// Regression test for a critical vulnerability: renaming a symlink that points
/// outside the mount boundary, then reading the renamed path, must NOT leak
/// the contents of the symlink target.
///
/// Attack flow:
/// 1. Host dir contains a symlink `escape_link -> <outside_file>`
/// 2. Sandbox renames `/mnt/escape_link` to `/mnt/renamed`
/// 3. Sandbox reads `/mnt/renamed` — overlay serves the `RealFileRef` whose
///    `host_path` is the original symlink; `fs::read` follows it and returns
///    the outside file's contents, completely bypassing boundary checks.
#[test]
fn rename_symlink_escape_overlay_read_text() {
    if !symlinks_supported() {
        return;
    }
    // Create the mount directory and a file *outside* it.
    let mount_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();
    let secret = "TOP SECRET CONTENT";
    fs::write(outside_dir.path().join("secret.txt"), secret).unwrap();

    // Place a symlink inside the mount that points outside the boundary.
    symlink_file(
        outside_dir.path().join("secret.txt"),
        mount_dir.path().join("escape_link"),
    );

    let mut mt = mount_at_mnt(&mount_dir, MountMode::OverlayMemory(OverlayState::new()));

    // Step 1: Rename the symlink within the mount.
    let rename_result = dispatch(
        &mut mt,
        OsFunctionCall::Rename(monty_types::RenameCallArgs {
            src: MontyPath::new("/mnt/escape_link".to_owned()),
            dst: MontyPath::new("/mnt/renamed".to_owned()),
        }),
    );
    // The rename itself may succeed or may be blocked — either is acceptable.
    // The critical invariant is that reading the renamed path must NEVER
    // return the outside file's contents.

    if matches!(rename_result, Some(Ok(_))) {
        // Rename succeeded — now try to read the renamed path.
        let read_result = call(&mut mt, PathOp::ReadText, "/mnt/renamed");
        match read_result {
            Some(Ok(MontyObject::String(content))) => {
                assert_ne!(
                    content, secret,
                    "SECURITY: overlay read_text leaked file contents from outside the mount boundary \
                     via a renamed symlink"
                );
            }
            Some(Err(_)) => {
                // An error (e.g. PathEscape, NotFound) is a valid safe outcome.
            }
            None => {
                // No mount matched — also safe.
            }
            other => panic!("unexpected read result: {other:?}"),
        }
    }
}

/// Same as above but for `read_bytes`.
#[test]
fn rename_symlink_escape_overlay_read_bytes() {
    if !symlinks_supported() {
        return;
    }
    let mount_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();
    let secret = b"TOP SECRET BYTES";
    fs::write(outside_dir.path().join("secret.bin"), secret.as_slice()).unwrap();

    symlink_file(
        outside_dir.path().join("secret.bin"),
        mount_dir.path().join("escape_link"),
    );

    let mut mt = mount_at_mnt(&mount_dir, MountMode::OverlayMemory(OverlayState::new()));

    let rename_result = dispatch(
        &mut mt,
        OsFunctionCall::Rename(monty_types::RenameCallArgs {
            src: MontyPath::new("/mnt/escape_link".to_owned()),
            dst: MontyPath::new("/mnt/renamed".to_owned()),
        }),
    );

    if matches!(rename_result, Some(Ok(_))) {
        let read_result = call(&mut mt, PathOp::ReadBytes, "/mnt/renamed");
        match read_result {
            Some(Ok(MontyObject::Bytes(content))) => {
                assert_ne!(
                    content,
                    secret.as_slice(),
                    "SECURITY: overlay read_bytes leaked file contents from outside the mount boundary \
                     via a renamed symlink"
                );
            }
            Some(Err(_)) => {}
            None => {}
            other => panic!("unexpected read result: {other:?}"),
        }
    }
}

// =============================================================================
// Host-absolute path segments (drive letters, UNC, backslashes)
// =============================================================================
//
// On Windows `PathBuf::join` discards the base when the joined segment carries
// a drive/UNC/root prefix. The rejection is deliberately not `cfg(windows)` —
// virtual paths are POSIX everywhere — so these tests run on every host.

/// Payloads whose leading segment a host parser may treat as absolute. The UNC
/// entry uses an RFC-2606 `.invalid` host, so even a vulnerable build reaches
/// no real host during the SMB/NTLM handshake it must not start.
const HOST_ABSOLUTE_PAYLOADS: &[&str] = &[
    r"/mnt/C:\Windows\System32\drivers\etc\hosts",
    r"/mnt/C:",
    r"/mnt/C:/Windows",
    r"/mnt/\\monty-test.invalid\share\probe",
    r"/mnt/back\slash",
    r"/mnt/sub/../C:\escape",
    r"/mnt/sub/nested\..\..\escape",
];

/// Asserts the request is refused before any host access. `ReadOnly` counts
/// too — on a read-only mount that check precedes path parsing. `Io` does not:
/// it would mean the escaping I/O already happened.
#[track_caller]
fn assert_refused_before_io(mt: &mut MountTable, op: PathOp, path: &str, mode_name: &str) {
    let result = match op {
        PathOp::Mkdir => call_mkdir(mt, path, true, false),
        PathOp::WriteText | PathOp::WriteBytes => {
            let p = MontyPath::new(path.to_owned());
            dispatch(
                mt,
                match op {
                    PathOp::WriteText => OsFunctionCall::WriteText(PathStringDataArgs {
                        path: p,
                        data: "attack".to_owned(),
                    }),
                    _ => OsFunctionCall::WriteBytes(PathBytesDataArgs {
                        path: p,
                        data: b"attack".to_vec(),
                    }),
                },
            )
        }
        _ => call(mt, op, path),
    };
    match result {
        Some(Err(MountError::PathEscape { .. } | MountError::ReadOnly(_))) => {}
        other => panic!("[{mode_name}] expected refusal for {op:?} on {path}, got {other:?}"),
    }
}

/// Classifies an outcome, dropping the caller's own path that errors echo back
/// (raw `Debug` would report two identical refusals as different). Keeps the
/// `Io` kind, since NotFound-vs-PermissionDenied would itself be an oracle.
fn outcome_class(result: Option<&Result<MontyObject, MountError>>) -> String {
    match result {
        None => "NotHandled".to_owned(),
        Some(Ok(value)) => format!("Ok({value:?})"),
        Some(Err(MountError::PathEscape { .. })) => "PathEscape".to_owned(),
        Some(Err(MountError::NoMountPoint(_))) => "NoMountPoint".to_owned(),
        Some(Err(MountError::Io(err, _))) => format!("Io({:?})", err.kind()),
        Some(Err(other)) => format!("Other({other})"),
    }
}

#[test]
fn host_absolute_segment_rejected_in_all_modes() {
    for payload in HOST_ABSOLUTE_PAYLOADS {
        for (mode_name, mode) in all_modes() {
            let dir = create_test_dir();
            let mut mt = mount_at_mnt(&dir, mode);
            assert_refused_before_io(&mut mt, PathOp::Exists, payload, mode_name);
        }
    }
}

/// Every operation must refuse, not just the one the reporter happened to try.
#[test]
fn host_absolute_segment_rejected_for_every_operation() {
    let ops = [
        PathOp::Exists,
        PathOp::IsFile,
        PathOp::IsDir,
        PathOp::IsSymlink,
        PathOp::ReadText,
        PathOp::ReadBytes,
        PathOp::Stat,
        PathOp::Iterdir,
        PathOp::Unlink,
        PathOp::Mkdir,
        PathOp::WriteText,
        PathOp::WriteBytes,
    ];
    for op in ops {
        for (mode_name, mode) in all_modes() {
            let dir = create_test_dir();
            let mut mt = mount_at_mnt(&dir, mode);
            assert_refused_before_io(&mut mt, op, r"/mnt/C:\Windows\x", mode_name);
        }
    }
}

/// The write vector from the original report: `mkdir(parents=True)` with a
/// drive-prefixed segment must be refused outright in every mode.
#[test]
fn host_absolute_mkdir_parents_is_rejected() {
    for (mode_name, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_refused_before_io(&mut mt, PathOp::Mkdir, r"/mnt/C:\Temp\pwned", mode_name);
    }
}

/// A leading-slash segment has no host meaning on any platform, so it is
/// normalized and confined as an ordinary nested path rather than rejected.
///
/// Uses a literal POSIX-absolute payload, not a temp-dir path: the latter is
/// drive-prefixed on Windows and would be refused there, leaving the
/// confinement unexercised on the platform this check exists for.
#[test]
fn posix_absolute_segment_is_confined_inside_the_mount() {
    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);

    let result = call_mkdir(&mut mt, "/mnt//etc/monty_probe", true, false);
    assert!(matches!(result, Some(Ok(_))), "expected confinement, got {result:?}");

    let nested = dir.path().join("etc").join("monty_probe");
    assert!(
        nested.exists(),
        "expected creation inside the mount, at {}",
        nested.display()
    );
}

/// Whatever a payload names, `mkdir(parents=True)` must create nothing at that
/// host location. Uses a real out-of-mount path so the assertion has a genuine
/// target to check on either platform.
#[test]
fn mkdir_parents_creates_nothing_at_the_host_location() {
    let outside = TempDir::new().unwrap();
    let target = outside.path().join("pwned");
    assert!(!target.exists(), "precondition: target must not pre-exist");
    let payload = format!("/mnt/{}", target.to_str().expect("temp path is UTF-8"));

    let dir = create_test_dir();
    let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
    let result = call_mkdir(&mut mt, &payload, true, false);

    assert!(
        !target.exists(),
        "SANDBOX ESCAPE: mkdir created {} outside the mount",
        target.display()
    );
    // Confined rather than refused (Unix), or refused outright (Windows,
    // where the temp path carries a drive prefix) — never anything else.
    if matches!(result, Some(Ok(_))) {
        let nested = dir.path().join(target.strip_prefix("/").unwrap_or(&target));
        assert!(
            nested.exists(),
            "expected the path to be created inside the mount, at {}",
            nested.display()
        );
    }
}

/// A missing and an existing out-of-mount path must be indistinguishable, or
/// `exists` leaks host filesystem layout.
///
/// Both payloads are refused on every host; the pair only has bite on Windows,
/// where a lost check would let the drive prefix clobber the mount base and
/// make the first resolve to a real host file. On Unix a backslash is an
/// ordinary filename character, so a lost check leaves both as in-mount misses
/// — indistinguishable anyway, and caught instead by the refusal tests above.
#[test]
fn host_absolute_exists_is_not_an_oracle() {
    // A literal drive-prefixed pair rather than a temp path, whose shape would
    // differ per platform. One names a file that exists on a Windows host, the
    // other one that cannot.
    let payloads = [
        r"/mnt/C:\Windows\System32\ntdll.dll",
        r"/mnt/C:\Windows\no_such_file_xyz",
    ];

    for (mode_name, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        let outcomes: Vec<String> = payloads
            .iter()
            .map(|payload| outcome_class(call(&mut mt, PathOp::Exists, payload).as_ref()))
            .collect();
        assert_eq!(
            outcomes[0], outcomes[1],
            "[{mode_name}] existence oracle: present and absent out-of-mount paths differ"
        );
    }
}

/// The oracle test above only bites if `exists` still answers truthfully for
/// paths inside the mount — a build that refused everything would satisfy it
/// while being useless.
#[test]
fn exists_still_discriminates_inside_the_mount() {
    for (mode_name, mode) in all_modes() {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, mode);
        assert_eq!(
            outcome_class(call(&mut mt, PathOp::Exists, "/mnt/hello.txt").as_ref()),
            "Ok(Bool(true))",
            "[{mode_name}] an in-mount file should be visible"
        );
        assert_eq!(
            outcome_class(call(&mut mt, PathOp::Exists, "/mnt/no_such_file.txt").as_ref()),
            "Ok(Bool(false))",
            "[{mode_name}] a missing in-mount file should report false"
        );
    }
}

/// `OverlayMemory` never builds a host path, so it bypassed the check until it
/// was applied to the overlay key too.
#[test]
fn host_absolute_segment_is_not_an_overlay_key() {
    for payload in HOST_ABSOLUTE_PAYLOADS {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, MountMode::OverlayMemory(OverlayState::new()));
        assert_refused_before_io(&mut mt, PathOp::WriteText, payload, "OverlayMemory");
        assert_refused_before_io(&mut mt, PathOp::Exists, payload, "OverlayMemory");
        assert_refused_before_io(&mut mt, PathOp::ReadText, payload, "OverlayMemory");
    }
}

/// A colon that is not a drive prefix must not trip the boundary check. Whether
/// the host then accepts the name is its own business — Windows refuses
/// `::double.txt` and stores `note:2026.txt` as an alternate data stream — so
/// only `PathEscape` is a failure, and the round-trip is checked only if it did.
#[test]
fn colon_names_that_are_not_drive_prefixes_are_not_rejected_by_the_check() {
    for name in ["note:2026.txt", "::double.txt", "ab:cd.txt", "log:12:30.txt"] {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        let path = format!("/mnt/{name}");
        let wrote = dispatch(
            &mut mt,
            OsFunctionCall::WriteText(PathStringDataArgs {
                path: MontyPath::new(path.clone()),
                data: "ok".to_owned(),
            }),
        );
        assert!(
            !matches!(wrote, Some(Err(MountError::PathEscape { .. }))),
            "the boundary check should not reject {name}, got {wrote:?}"
        );
        if matches!(wrote, Some(Ok(_))) {
            let read = call(&mut mt, PathOp::ReadText, &path);
            assert!(
                matches!(read, Some(Ok(MontyObject::String(ref s))) if s == "ok"),
                "reading {name} should round-trip, got {read:?}"
            );
        }
    }
}

/// Refused on every host, including Unix where CPython accepts them: Windows
/// parses `a:b` as drive-relative, so the rule has to be uniform. Pins the
/// divergence so it cannot change silently.
#[test]
fn single_letter_colon_names_are_refused_on_all_hosts() {
    for name in ["a:b.txt", "C:.txt", "z:"] {
        let dir = create_test_dir();
        let mut mt = mount_at_mnt(&dir, MountMode::ReadWrite);
        assert_refused_before_io(&mut mt, PathOp::WriteText, &format!("/mnt/{name}"), "ReadWrite");
    }
}
