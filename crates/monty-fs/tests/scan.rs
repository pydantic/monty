//! Tests for `Path.scan` against mounts: what each mode returns, what a glob
//! pattern prunes, and that symlinks and `..` never reach outside the mount.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{fs, iter::repeat_n};

use monty_fs::{Mount, MountCallOutcome, MountError, MountMode, MountTable, OverlayState};
use monty_types::{
    ExcType, MontyObject, OsFunctionCall, PathStringDataArgs, ScanArgs,
    scan::{EntryInfo, parse_scan_reply},
};
use tempfile::TempDir;

#[expect(dead_code, reason = "shared helper module; not every test crate uses all of it")]
mod common;
use common::{symlink_dir, symlink_file, symlinks_supported};

/// Creates the tree every test scans.
///
/// ```text
/// tmpdir/
///   a.txt
///   b.py
///   sub/
///     c.txt
///     deep/
///       d.py
/// ```
fn create_tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    let p = dir.path();
    fs::write(p.join("a.txt"), "a").unwrap();
    fs::write(p.join("b.py"), "b").unwrap();
    fs::create_dir_all(p.join("sub/deep")).unwrap();
    fs::write(p.join("sub/c.txt"), "c").unwrap();
    fs::write(p.join("sub/deep/d.py"), "d").unwrap();
    dir
}

/// Mounts `dir` at `/mnt` in `mode`.
fn mount(dir: &TempDir, mode: MountMode) -> MountTable {
    let mut table = MountTable::new();
    table.mount("/mnt", dir.path(), mode, None).unwrap();
    table
}

/// Every mode a scan must behave the same in.
fn all_modes() -> [MountMode; 3] {
    [
        MountMode::ReadWrite,
        MountMode::ReadOnly,
        MountMode::OverlayMemory(OverlayState::new()),
    ]
}

/// Runs a scan, returning the reply decoded as `(path, kind)` pairs sorted by
/// path, where kind is `d`(ir), `f`(ile), `D`/`F` for links to them, `l` for a
/// dangling link and `?` for anything else.
fn scan(table: &mut MountTable, args: ScanArgs) -> Result<Vec<(String, char)>, MountError> {
    let MountCallOutcome::Handled(result) = table.handle_os_call(OsFunctionCall::Scan(args)) else {
        panic!("a scan under /mnt must be handled by the mount");
    };
    let reply = result?;
    let mut entries: Vec<(String, char)> = parse_scan_reply(&reply)
        .unwrap()
        .into_iter()
        .map(|entry| (entry.path, kind(entry.info)))
        .collect();
    entries.sort();
    Ok(entries)
}

/// The one-letter kind [`scan`] reports.
fn kind(info: EntryInfo) -> char {
    match (info.is_dir, info.is_file, info.is_symlink) {
        (true, _, false) => 'd',
        (_, true, false) => 'f',
        (true, _, true) => 'D',
        (_, true, true) => 'F',
        (false, false, true) => 'l',
        (false, false, false) => '?',
    }
}

/// `(path, kind)` pairs from string literals.
fn entries(items: &[(&str, char)]) -> Vec<(String, char)> {
    items.iter().map(|(path, kind)| ((*path).to_owned(), *kind)).collect()
}

/// A glob scan of `parts` below `path`.
fn glob(path: &str, parts: &[&str]) -> ScanArgs {
    ScanArgs::glob(
        path.into(),
        parts.iter().map(|part| (*part).to_owned()).collect(),
        None,
        false,
    )
}

#[test]
fn listing_to_depth() {
    for mode in all_modes() {
        let dir = create_tree();
        let mut table = mount(&dir, mode);
        let root_only = scan(&mut table, ScanArgs::listing("/mnt".into(), Some(0), false)).unwrap();
        assert_eq!(root_only, entries(&[("", 'd')]));
        let children = scan(&mut table, ScanArgs::listing("/mnt".into(), Some(1), false)).unwrap();
        assert_eq!(
            children,
            entries(&[("", 'd'), ("a.txt", 'f'), ("b.py", 'f'), ("sub", 'd')])
        );
        let tree = scan(&mut table, ScanArgs::listing("/mnt/".into(), None, false)).unwrap();
        assert_eq!(
            tree,
            entries(&[
                ("", 'd'),
                ("a.txt", 'f'),
                ("b.py", 'f'),
                ("sub", 'd'),
                ("sub/c.txt", 'f'),
                ("sub/deep", 'd'),
                ("sub/deep/d.py", 'f'),
            ])
        );
    }
}

#[test]
fn missing_and_file_roots() {
    for mode in all_modes() {
        let dir = create_tree();
        let mut table = mount(&dir, mode);
        let err = scan(&mut table, ScanArgs::listing("/mnt/nope".into(), None, false)).unwrap_err();
        let exc = err.into_exception();
        assert_eq!(exc.exc_type(), ExcType::FileNotFoundError);
        assert_eq!(exc.message(), Some("[Errno 2] No such file or directory: '/mnt/nope'"));
        // a file root is described, not listed, so a literal glob can test it
        let file = scan(&mut table, ScanArgs::listing("/mnt/a.txt".into(), None, false)).unwrap();
        assert_eq!(file, entries(&[("", 'f')]));
    }
}

#[test]
fn glob_pattern_prunes_the_reply() {
    for mode in all_modes() {
        let dir = create_tree();
        let mut table = mount(&dir, mode);
        // only the root is listed: nothing below `sub` is read
        let top = scan(&mut table, glob("/mnt", &["*.txt"])).unwrap();
        assert_eq!(top, entries(&[("", 'd'), ("a.txt", 'f'), ("b.py", 'f'), ("sub", 'd')]));
        // `sub/c.txt` is never listed: `deep` is looked up, then listed
        let deep = scan(&mut table, glob("/mnt", &["s*", "deep", "*"])).unwrap();
        assert_eq!(
            deep,
            entries(&[
                ("", 'd'),
                ("a.txt", 'f'),
                ("b.py", 'f'),
                ("sub", 'd'),
                ("sub/deep", 'd'),
                ("sub/deep/d.py", 'f'),
            ])
        );
        // a pattern with no match below the root reads nothing more
        let none = scan(&mut table, glob("/mnt", &["nope", "*"])).unwrap();
        assert_eq!(none, entries(&[("", 'd')]));
    }
}

#[test]
fn dot_dot_in_a_pattern_stays_below_the_scan_root() {
    let dir = create_tree();
    let mut table = mount(&dir, MountMode::ReadWrite);
    let reply = scan(&mut table, glob("/mnt/sub", &["..", "*"])).unwrap();
    assert_eq!(reply, entries(&[("", 'd')]));
    let reply = scan(&mut table, glob("/mnt/sub", &["*", "..", "..", "..", "*"])).unwrap();
    assert_eq!(reply, entries(&[("", 'd'), ("c.txt", 'f'), ("deep", 'd')]));
}

#[test]
fn scans_outside_every_mount_are_not_handled() {
    let dir = create_tree();
    let mut table = mount(&dir, MountMode::ReadWrite);
    let call = OsFunctionCall::Scan(ScanArgs::listing("/mnt/..".into(), None, false));
    assert!(matches!(table.handle_os_call(call), MountCallOutcome::NotHandled(_)));
}

#[test]
fn overlay_writes_are_scanned() {
    let dir = create_tree();
    let mut table = mount(&dir, MountMode::OverlayMemory(OverlayState::new()));
    let write = OsFunctionCall::WriteText(PathStringDataArgs {
        path: "/mnt/sub/new.txt".into(),
        data: "new".to_owned(),
    });
    assert!(matches!(table.handle_os_call(write), MountCallOutcome::Handled(Ok(_))));
    let unlink = OsFunctionCall::Unlink("/mnt/a.txt".into());
    assert!(matches!(table.handle_os_call(unlink), MountCallOutcome::Handled(Ok(_))));
    let reply = scan(&mut table, glob("/mnt", &["**", "*.txt"])).unwrap();
    assert_eq!(
        reply,
        entries(&[
            ("", 'd'),
            ("b.py", 'f'),
            ("sub", 'd'),
            ("sub/c.txt", 'f'),
            ("sub/deep", 'd'),
            ("sub/deep/d.py", 'f'),
            ("sub/new.txt", 'f'),
        ])
    );
    // nothing reached the real directory
    assert!(!dir.path().join("sub/new.txt").exists());
}

#[test]
fn symlinks_are_described_and_followed_only_on_request() {
    if !symlinks_supported() {
        return;
    }
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    let dir = create_tree();
    symlink_dir("sub", dir.path().join("linkdir"));
    symlink_file("a.txt", dir.path().join("linkfile"));
    symlink_file("missing", dir.path().join("dangling"));
    symlink_dir(outside.path(), dir.path().join("escape"));
    let mut table = mount(&dir, MountMode::ReadWrite);

    // links are described from inside the mount; one leaving it, or dangling, is hidden
    let top = scan(&mut table, ScanArgs::listing("/mnt".into(), Some(1), false)).unwrap();
    assert_eq!(
        top,
        entries(&[
            ("", 'd'),
            ("a.txt", 'f'),
            ("b.py", 'f'),
            ("linkdir", 'D'),
            ("linkfile", 'F'),
            ("sub", 'd'),
        ])
    );

    let walked = scan(&mut table, ScanArgs::listing("/mnt".into(), None, false)).unwrap();
    assert!(!walked.iter().any(|(path, _)| path.starts_with("linkdir/")));
    let followed = scan(&mut table, ScanArgs::listing("/mnt".into(), None, true)).unwrap();
    assert!(followed.contains(&("linkdir/deep/d.py".to_owned(), 'f')));
    assert!(!followed.iter().any(|(path, _)| path.contains("secret")));

    // `**` does not descend a symlinked directory, but `*` may match one and descend
    let recursive = scan(&mut table, glob("/mnt", &["**", "*.py"])).unwrap();
    assert!(!recursive.iter().any(|(path, _)| path.starts_with("linkdir/")));
    let wildcard = scan(&mut table, glob("/mnt", &["*", "c.txt"])).unwrap();
    assert!(wildcard.contains(&("linkdir/c.txt".to_owned(), 'f')));
    let followed_glob = scan(
        &mut table,
        ScanArgs::glob("/mnt".into(), vec!["**".to_owned(), "*.py".to_owned()], None, true),
    )
    .unwrap();
    assert!(followed_glob.contains(&("linkdir/deep/d.py".to_owned(), 'f')));

    // a literal pattern looks the name up, and finds hidden links as hidden as a listing does
    for name in ["escape", "dangling"] {
        let reply = scan(&mut table, glob("/mnt", &["*", "..", name])).unwrap();
        assert!(!reply.iter().any(|(path, _)| path == name), "{name} leaked: {reply:?}");
    }
}

/// A root the mount cannot list fails the scan, as CPython's `scandir` would;
/// one below the root reads as empty, so a walk skips it.
#[test]
#[cfg(unix)]
fn unreadable_root_raises_and_unreadable_subdirectory_reads_as_empty() {
    let dir = create_tree();
    let locked = dir.path().join("sub/deep");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Running as root, permissions are not enforced and there is nothing to test.
    let enforced = fs::read_dir(&locked).is_err();
    let outcome = enforced.then(|| {
        let mut table = mount(&dir, MountMode::ReadOnly);
        let root = scan(&mut table, ScanArgs::listing("/mnt/sub/deep".into(), None, false));
        let below = scan(&mut table, ScanArgs::listing("/mnt/sub".into(), None, false));
        (root, below)
    });
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let Some((root, below)) = outcome else {
        return;
    };
    let exc = root.unwrap_err().into_exception();
    assert_eq!(exc.exc_type(), ExcType::PermissionError);
    assert_eq!(below.unwrap(), entries(&[("", 'd'), ("c.txt", 'f'), ("deep", 'd')]));
}

#[test]
fn reply_is_charged_to_the_mount_memory_limit() {
    let dir = create_tree();
    for i in 0..64 {
        fs::write(dir.path().join(format!("file{i:03}.txt")), "").unwrap();
    }
    let mount = Mount::new("/mnt", dir.path(), MountMode::ReadOnly, None)
        .unwrap()
        .with_memory_usage_limit(2_000);
    let mut table = MountTable::new();
    table.push_mount(mount).unwrap();
    let err = scan(&mut table, ScanArgs::listing("/mnt".into(), None, false)).unwrap_err();
    assert_eq!(err.into_exception().exc_type(), ExcType::MemoryError);
}

#[test]
fn overlong_patterns_are_refused() {
    let dir = create_tree();
    let mut table = mount(&dir, MountMode::ReadOnly);
    let parts: Vec<&str> = repeat_n("*", 100).collect();
    let err = scan(&mut table, glob("/mnt", &parts)).unwrap_err();
    let exc = err.into_exception();
    assert_eq!(exc.exc_type(), ExcType::OSError);
    assert_eq!(
        exc.message(),
        Some("[Errno 36] File name too long: '*/*/*/*/*/*/*/*/*/*/…/*/*/*/*/*/*/*/*/*/*'")
    );
}

#[test]
fn a_reply_round_trips_through_its_encoding() {
    let dir = create_tree();
    let mut table = mount(&dir, MountMode::ReadOnly);
    let MountCallOutcome::Handled(Ok(reply)) = table.handle_os_call(OsFunctionCall::Scan(ScanArgs::listing(
        "/mnt/sub".into(),
        Some(1),
        false,
    ))) else {
        panic!("expected a reply");
    };
    let first = MontyObject::tuple([
        MontyObject::string(String::new()),
        MontyObject::bool(true),
        MontyObject::bool(false),
        MontyObject::bool(false),
    ]);
    assert_eq!(reply.as_ref().type_name(), "list");
    assert_eq!(parse_scan_reply(&reply).unwrap().len(), 3);
    assert_eq!(parse_scan_reply(&MontyObject::list([first])).unwrap()[0].path, "");
}

#[test]
fn overlay_data_shares_the_scan_budget() {
    let dir = create_tree();
    let mount = Mount::new("/mnt", dir.path(), MountMode::OverlayMemory(OverlayState::new()), None)
        .unwrap()
        .with_memory_usage_limit(4_000);
    let mut table = MountTable::new();
    table.push_mount(mount).unwrap();
    // a listing that fits the whole limit no longer fits beside retained overlay data
    assert!(scan(&mut table, ScanArgs::listing("/mnt".into(), None, false)).is_ok());
    let write = OsFunctionCall::WriteText(PathStringDataArgs {
        path: "/mnt/big.txt".into(),
        data: "x".repeat(3_500),
    });
    assert!(matches!(table.handle_os_call(write), MountCallOutcome::Handled(Ok(_))));
    let err = scan(&mut table, ScanArgs::listing("/mnt".into(), None, false)).unwrap_err();
    assert_eq!(err.into_exception().exc_type(), ExcType::MemoryError);
}

#[test]
fn huge_pattern_parts_are_refused_without_copying_them() {
    let dir = create_tree();
    let mut table = mount(&dir, MountMode::ReadOnly);
    let part = "x".repeat(10_000_000);
    let err = scan(&mut table, glob("/mnt", &[&part, "*"])).unwrap_err();
    assert_eq!(err.into_exception().exc_type(), ExcType::OSError);
    // a part one character past the name limit is refused however its last character is encoded
    let part = "x".repeat(255) + "é";
    let err = scan(&mut table, glob("/mnt", &[&part, "*"])).unwrap_err();
    assert_eq!(err.into_exception().exc_type(), ExcType::OSError);
}

/// The visit cap is a `RuntimeError`: an `OSError` would be swallowed by glob as an empty match.
#[test]
fn scan_limit_is_a_runtime_error() {
    let exc = MountError::ScanLimitExceeded(10).into_exception();
    assert_eq!(exc.exc_type(), ExcType::RuntimeError);
    assert_eq!(exc.message(), Some("directory scan examined more than 10 entries"));
}
