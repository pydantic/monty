//! End-to-end pool tests against the real `monty` binary — including the
//! headline scenarios: a worker dying mid-execution (kill, crash, timeout)
//! must surface as a clean error and never poison the pool.

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;
use std::{
    env,
    fmt::Write as _,
    fs,
    future::ready,
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::Once,
    thread,
    time::Duration,
};
#[cfg(target_os = "linux")]
use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

// only the unix-gated exit-code test snapshots a message
#[cfg(unix)]
use insta::assert_snapshot;
use monty_pool::{
    MountSpec, MountSpecMode, Pool, PoolConfig, PoolError, PrintFuture, ReplConfig, ResumeValue, TurnEvent,
    on_print_sync,
};
// only the unix-gated raw-path test forges worker frames
#[cfg(unix)]
use monty_proto::{encode_framed_into, pb};
use monty_types::{
    AutoOsCalls, CallArgs, DateTimeSource, ExcType, MontyException, MontyObject, NameLookupResult, PrintStream,
    RandomSeed, RandomStart, ResourceLimits, SleepMode, TypeCheckingConfig, TypeCheckingFormat,
    unstable::{self, MontyNode},
};
use tokio::time::sleep;
#[cfg(unix)]
use tokio::time::timeout;

/// Locates (building once if needed) the `monty` CLI binary for tests.
fn monty_binary() -> PathBuf {
    static BUILD: Once = Once::new();
    if let Ok(path) = env::var("MONTY_TEST_BIN") {
        return PathBuf::from(path);
    }
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_owned();
    let target = env::var_os("CARGO_TARGET_DIR").map_or_else(|| workspace.join("target"), PathBuf::from);
    let path = target.join("debug").join(format!("monty{}", env::consts::EXE_SUFFIX));
    BUILD.call_once(|| {
        if !path.exists() {
            let status = Command::new(env!("CARGO"))
                .args(["build", "-p", "monty-runtime"])
                .status()
                .expect("failed to run cargo build -p monty-runtime");
            assert!(status.success(), "building the monty binary failed");
        }
    });
    assert!(path.exists(), "monty binary missing at {}", path.display());
    path
}

fn config() -> PoolConfig {
    PoolConfig::subprocess(monty_binary())
}

/// A discard-everything print callback, coercible to `OnPrint` at each callsite.
fn no_print(_: PrintStream, _: &str) -> PrintFuture {
    Box::pin(ready(()))
}

#[track_caller]
fn expect_complete(event: TurnEvent) -> MontyObject {
    match event {
        TurnEvent::Complete(value) => value,
        other => panic!("expected Complete, got {other:?}"),
    }
}

/// Drives a feed the way an auto-answering host does: every `OsCall` is offered
/// to the feed's mounts, returning the first event no mount covers.
///
/// Mount-covered calls surface like any other suspension now, so a test that
/// only cares about the *result* of mounted I/O runs its feed through this.
async fn feed_with_mounts(
    checkout: &mut monty_pool::Checkout,
    result: Result<TurnEvent, PoolError>,
) -> Result<TurnEvent, PoolError> {
    let event = result?;
    let mut event = event;
    loop {
        if !matches!(event, TurnEvent::OsCall { .. }) {
            return Ok(event);
        }
        match checkout.resume_from_mounts(&mut no_print).await? {
            Some(next) => event = next,
            None => return Ok(event),
        }
    }
}

/// Creates a directory symlink, reporting the failure a host without the
/// privilege gives so the caller can skip rather than fail.
fn try_symlink_dir(original: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return symlink(original, link);
    #[cfg(windows)]
    return symlink_dir(original, link);
}

/// Kills a process by pid with SIGKILL — simulates a hard crash.
fn kill_pid(pid: u32) {
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

/// Writes the executable stand-in for `monty` that a test drives, from a child shell rather than
/// this process: a descriptor open for writing here would be inherited by any concurrently
/// forking test, and executing the file while that child still holds it fails with `ETXTBSY`.
#[cfg(unix)]
fn write_fake_monty(dir: &Path, script: &str) -> PathBuf {
    let fake = dir.join("monty");
    let status = Command::new("sh")
        .args(["-c", r#"printf '%s' "$2" > "$1" && chmod 755 "$1""#, "sh"])
        .arg(&fake)
        .arg(script)
        .status()
        .unwrap();
    assert!(status.success(), "writing the stand-in `monty` failed");
    fake
}

// =============================================================================
// Happy path
// =============================================================================

/// Rejected eager answers leave the checkout and worker at the same call.
#[tokio::test]
async fn allow_eager_await_validates_replies_before_sending() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let event = session
        .feed("await f()", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    let TurnEvent::FunctionCall {
        call_id,
        allow_eager_await,
        ..
    } = event
    else {
        panic!("expected function call, got {event:?}")
    };
    assert!(allow_eager_await);
    for results in [
        vec![],
        vec![(call_id + 1, ResumeValue::Return(MontyObject::int(42)))],
        vec![(call_id, ResumeValue::Future)],
        vec![(call_id, ResumeValue::NotFound)],
    ] {
        assert!(matches!(
            session.resume_futures(results, &mut no_print).await,
            Err(PoolError::Protocol(_))
        ));
    }
    let done = session
        .resume_futures(
            vec![(call_id, ResumeValue::Return(MontyObject::int(42)))],
            &mut no_print,
        )
        .await
        .unwrap();
    assert_eq!(expect_complete(done), MontyObject::int(42));
    session.finish().await.unwrap();
}

/// An over-threshold frame round-trips through `decode_event`'s
/// `block_in_place` branch (multi-thread runtime) without corruption.
#[tokio::test(flavor = "multi_thread")]
async fn large_value_roundtrips_through_offloaded_decode() {
    large_value_roundtrip().await;
}

/// The same roundtrip on a current-thread runtime, exercising the inline
/// fallback (`block_in_place` would panic there).
#[tokio::test]
async fn large_value_roundtrips_on_current_thread_runtime() {
    large_value_roundtrip().await;
}

/// Feeds a 2 MB string (over the offload threshold), then a small turn to
/// show the session stayed healthy.
async fn large_value_roundtrip() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let event = session
        .feed("'x' * 2_000_000", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("x".repeat(2_000_000)));
    let event = session
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(2));
    session.finish().await.unwrap();
}

#[tokio::test]
async fn feed_and_finish_reuses_the_worker() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let first_pid = session.pid().unwrap();

    let event = session
        .feed("x = 40\nx + 2", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(42));
    // session state persists across feeds on the same checkout
    let event = session
        .feed("x * 2", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(80));
    session.finish().await.unwrap();
    assert_eq!(pool.idle_workers(), 1);

    // the same process serves the next checkout, with a FRESH session:
    // `x` from the previous session must not leak, so reading it suspends
    // at NameLookup and resolving it as undefined raises NameError
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    assert_eq!(session.pid().unwrap(), first_pid);
    let event = session.feed("x", vec![], vec![], false, &mut no_print).await.unwrap();
    assert!(matches!(event, TurnEvent::NameLookup { name, .. } if name == "x"));
    let err = session
        .resume_name_lookup(NameLookupResult::Undefined, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.message(), Some("name 'x' is not defined"));
    session.finish().await.unwrap();
}

/// An `Error` answer to a name lookup is raised inside the sandbox — so the
/// snippet can catch it — and the session stays usable.
#[tokio::test]
async fn name_lookup_error_is_raised_in_the_sandbox() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let event = session
        .feed(
            "try:\n    secret\nexcept PermissionError as e:\n    caught = str(e)\ncaught",
            vec![],
            vec![],
            false,
            &mut no_print,
        )
        .await
        .unwrap();
    assert!(matches!(event, TurnEvent::NameLookup { ref name, .. } if name == "secret"));
    let error = MontyException::new(ExcType::PermissionError, Some("secret is off limits".to_owned()));
    let event = session.resume_name_lookup(error, &mut no_print).await.unwrap();
    assert_eq!(
        expect_complete(event),
        MontyObject::string("secret is off limits".to_owned())
    );
    // uncaught, the error ends the turn as a runtime error with a traceback
    let event = session
        .feed("secret", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert!(matches!(event, TurnEvent::NameLookup { ref name, .. } if name == "secret"));
    let error = MontyException::new(ExcType::PermissionError, Some("still off limits".to_owned()));
    let err = session.resume_name_lookup(error, &mut no_print).await.unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.exc_type(), ExcType::PermissionError);
    assert_eq!(exc.message(), Some("still off limits"));
    assert!(
        !exc.traceback().is_empty(),
        "the sandbox frame must be on the traceback"
    );
    let event = session
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(2));
    session.finish().await.unwrap();
}

#[tokio::test]
async fn cyclic_return_value_decodes_and_keeps_the_worker_alive() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    // a cyclic dict completes with a `Cycle` placeholder in the payload; the
    // parent must decode it rather than discarding the worker as misbehaving
    let event = session
        .feed("d = {}\nd['self'] = d\nd", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    let value = expect_complete(event);
    let Some(pairs) = value.as_ref().pairs() else {
        panic!("expected Dict");
    };
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0, MontyObject::string("self".to_owned()));
    assert!(matches!(unstable::node(pairs[0].1), MontyNode::Cycle(placeholder) if placeholder == "{...}"));
    // the session must still be usable on the same worker
    let event = session
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(2));
    session.finish().await.unwrap();
    assert_eq!(pool.idle_workers(), 1);
}

/// The arena has no nesting bound: a lookup answer nested a hundred lists
/// deep crosses the wire and comes back the same shape.
#[tokio::test]
async fn deeply_nested_name_lookup_value_crosses_the_wire() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let event = session
        .feed("missing", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert!(matches!(event, TurnEvent::NameLookup { ref name, .. } if name == "missing"));
    let deep = (0..100).fold(MontyObject::int(1), |inner, _| MontyObject::list([inner]));
    let event = session
        .resume_name_lookup(Some(deep.clone()), &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), deep);
    session.finish().await.unwrap();
}

/// A mount whose host path is not valid UTF-8 works: host paths never cross
/// the wire (the parent services mount I/O itself), so the OS-native path is
/// used as-is. Linux-only: macOS (APFS) rejects non-UTF-8 filenames outright
/// and they cannot be constructed portably elsewhere.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn non_utf8_mount_path_works() {
    let dir = tempfile::tempdir().unwrap();
    let weird_dir = dir.path().join(OsStr::from_bytes(b"weird-\xff"));
    fs::create_dir(&weird_dir).unwrap();
    fs::write(weird_dir.join("data.txt"), "non-utf8 host dir").unwrap();

    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let result = session
        .feed(
            "from pathlib import Path\nPath('/mnt/data/data.txt').read_text()",
            vec![],
            vec![MountSpec::new("/mnt/data", weird_dir, MountSpecMode::ReadOnly).unwrap()],
            false,
            &mut no_print,
        )
        .await;
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    assert_eq!(
        expect_complete(event),
        MontyObject::string("non-utf8 host dir".to_owned())
    );
    session.finish().await.unwrap();
}

/// A mount whose host path does not exist is rejected where the directory is
/// opened — building the [`MountSpec`] — so no feed inherits the failure.
#[tokio::test]
async fn invalid_mount_host_path_is_rejected_cleanly() {
    let err = MountSpec::new(
        "/mnt/data",
        PathBuf::from("/nonexistent/monty/mount/dir"),
        MountSpecMode::ReadOnly,
    )
    .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert!(
        exc.message().is_some_and(|m| m.contains("cannot open host path")),
        "unexpected message: {:?}",
        exc.message()
    );

    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let event = session
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(2));
    session.finish().await.unwrap();
}

/// The sandbox working directory is session state: the first feed sets it
/// (its first mount, else `/`), and later feeds keep it — `os.chdir`
/// included — unless `feed_with_cwd` switches it.
#[tokio::test]
async fn working_directory_persists_across_feeds() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("data.txt"), "relative!").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    let mount = || vec![MountSpec::new("/mnt", dir.path(), MountSpecMode::ReadOnly).unwrap()];
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();

    // The first feed's mount, so relative paths reach into it; `__file__` is placed under it.
    let code = "\
import os
from pathlib import Path
before = (os.getcwd(), __file__, open('data.txt').read())
os.chdir('sub')
(before, Path.cwd(), Path('..').resolve())";
    let result = session.feed(code, vec![], mount(), false, &mut no_print).await;
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    assert_eq!(
        expect_complete(event),
        MontyObject::tuple([
            MontyObject::tuple([
                MontyObject::string("/mnt".to_owned()),
                MontyObject::string("/mnt/main.py".to_owned()),
                MontyObject::string("relative!".to_owned()),
            ]),
            MontyObject::path("/mnt/sub".to_owned()),
            MontyObject::path("/mnt".to_owned()),
        ])
    );

    // The chdir persists, even into a feed without the mount.
    let event = session
        .feed("os.getcwd()", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("/mnt/sub".to_owned()));

    // An explicit cwd switches it, and the switch persists too.
    let result = session
        .feed_with_cwd("os.getcwd()", vec![], mount(), Some("/mnt/"), false, &mut no_print)
        .await;
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("/mnt".to_owned()));

    // A relative cwd is refused before anything is sent; the session survives unchanged.
    let err = session
        .feed_with_cwd("1", vec![], vec![], Some("data"), false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected a runtime error, got {err:?}");
    };
    assert_eq!(exc.exc_type(), ExcType::ValueError);
    assert_eq!(exc.message().unwrap(), "cwd must be an absolute POSIX path: \"data\"");
    let event = session
        .feed("os.getcwd()", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("/mnt".to_owned()));
    session.finish().await.unwrap();

    // A session whose first feed has no mount starts at the root and stays
    // there when a later feed mounts something.
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let event = session
        .feed(
            "import os\n(os.getcwd(), __file__)",
            vec![],
            vec![],
            false,
            &mut no_print,
        )
        .await
        .unwrap();
    assert_eq!(
        expect_complete(event),
        MontyObject::tuple([
            MontyObject::string("/".to_owned()),
            MontyObject::string("/main.py".to_owned())
        ])
    );
    let result = session.feed("os.getcwd()", vec![], mount(), false, &mut no_print).await;
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("/".to_owned()));
    session.finish().await.unwrap();
}

/// A first feed that type checking rejects never reaches the worker's
/// directory switch, so the next feed still establishes the mount default.
#[tokio::test]
async fn working_directory_survives_a_rejected_first_feed() {
    let dir = tempfile::tempdir().unwrap();
    let mount = || vec![MountSpec::new("/mnt", dir.path(), MountSpecMode::ReadOnly).unwrap()];
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            type_check: true,
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    let err = session
        .feed("x: int = 'nope'", vec![], mount(), false, &mut no_print)
        .await
        .unwrap_err();
    assert!(matches!(err, PoolError::Typing(_)), "expected Typing, got {err:?}");
    let result = session
        .feed("import os\nos.getcwd()", vec![], mount(), false, &mut no_print)
        .await;
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("/mnt".to_owned()));
    session.finish().await.unwrap();
}

/// A first feed rejected as oversize never reaches the worker, so its
/// directory is not established: the next feed still sends the mount default.
#[tokio::test]
async fn working_directory_survives_an_oversize_first_feed() {
    let dir = tempfile::tempdir().unwrap();
    let mount = || vec![MountSpec::new("/mnt", dir.path(), MountSpecMode::ReadOnly).unwrap()];
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    // just over monty_proto's 256 MiB MAX_FRAME_LEN
    let huge = MontyObject::string("x".repeat(257 * 1024 * 1024));
    let err = session
        .feed("data", vec![("data".to_owned(), huge)], mount(), false, &mut no_print)
        .await
        .unwrap_err();
    assert!(matches!(err, PoolError::Runtime(_)), "expected Runtime, got {err:?}");
    let result = session
        .feed("import os\nos.getcwd()", vec![], mount(), false, &mut no_print)
        .await;
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("/mnt".to_owned()));
    session.finish().await.unwrap();
}

/// Mount-covered filesystem OS calls are serviced by the parent and never
/// surface to the caller — the feed just completes. Covers read, write,
/// mkdir kwargs, rename, and `open()` + file-handle ops through a mount.
#[tokio::test]
async fn mounted_filesystem_ops_are_serviced_by_the_parent() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("data.txt"), "mounted!").unwrap();

    let mount = || vec![MountSpec::new("/mnt", dir.path(), MountSpecMode::ReadWrite).unwrap()];
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();

    let result = session
        .feed(
            "from pathlib import Path\nPath('/mnt/data.txt').read_text()",
            vec![],
            mount(),
            false,
            &mut no_print,
        )
        .await;
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("mounted!".to_owned()));

    let code = "\
Path('/mnt/sub').mkdir(parents=True, exist_ok=True)
Path('/mnt/sub/out.txt').write_text('written')
Path('/mnt/sub/out.txt').rename('/mnt/sub/renamed.txt')
with open('/mnt/sub/renamed.txt') as f:
    body = f.read()
body";
    let result = session.feed(code, vec![], mount(), false, &mut no_print).await;
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("written".to_owned()));
    assert_eq!(
        fs::read_to_string(dir.path().join("sub/renamed.txt")).unwrap(),
        "written"
    );
    session.finish().await.unwrap();
}

/// A [`MountSpec`] reused across feeds stays bound to the directory it opened:
/// the sandbox renaming a symlink over that directory's name, from inside a
/// read-write mount of the parent, redirects the *path* and nothing else.
///
/// Skipped where the host cannot create symlinks (Windows without Developer
/// Mode or elevation), since planting the link needs one.
#[tokio::test]
async fn a_reused_mount_spec_survives_a_rename_of_its_host_directory() {
    let base = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let shared = base.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::write(shared.join("inside.txt"), "in-mount").unwrap();
    fs::write(outside.path().join("secret.txt"), "HOST SECRET").unwrap();
    // Host-planted link out of the tree; the sandbox only moves it.
    if try_symlink_dir(outside.path(), &base.path().join("prepared-link")).is_err() {
        eprintln!("skipping: this host cannot create symlinks");
        return;
    }

    // Both built once, as a host holding long-lived mount objects would.
    let child = MountSpec::new("/child", shared, MountSpecMode::ReadOnly).unwrap();
    let parent = MountSpec::new("/parent", base.path(), MountSpecMode::ReadWrite).unwrap();

    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();

    // Feed 1: swap the real directory out and the link in, under one name.
    let code = "\
from pathlib import Path
Path('/parent/shared').rename('/parent/old-shared')
Path('/parent/prepared-link').rename('/parent/shared')
'swapped'";
    let result = session.feed(code, vec![], vec![parent], false, &mut no_print).await;
    match feed_with_mounts(&mut session, result).await {
        Ok(event) => assert_eq!(expect_complete(event), MontyObject::string("swapped".to_owned())),
        // Windows refuses to rename a directory while a handle to it is open,
        // and the child mount holds one — so the swap cannot even be staged
        // there while the mount is alive.
        #[cfg(windows)]
        Err(_) => return,
        #[cfg(not(windows))]
        Err(err) => panic!("staging the swap failed: {err:?}"),
    }

    // Feed 2: the child mount, rebuilt from the same spec, still serves the
    // original directory and cannot see the one the name now points at.
    let code = "\
from pathlib import Path
f\"{Path('/child/inside.txt').read_text()}:{Path('/child/secret.txt').exists()}\"";
    let result = session.feed(code, vec![], vec![child], false, &mut no_print).await;
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("in-mount:False".to_owned()));
    session.finish().await.unwrap();
}

/// A write through a read-only mount raises `PermissionError` inside the
/// sandbox (catchable, session-preserving); overlay writes are visible within
/// the feed, never reach the host, and are discarded when the feed ends.
#[tokio::test]
async fn read_only_and_overlay_mount_semantics() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("data.txt"), "original").unwrap();

    let mount = |mode| vec![MountSpec::new("/mnt", dir.path(), mode).unwrap()];
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();

    let result = session
        .feed(
            "from pathlib import Path\nPath('/mnt/new.txt').write_text('x')",
            vec![],
            mount(MountSpecMode::ReadOnly),
            false,
            &mut no_print,
        )
        .await;
    let err = feed_with_mounts(&mut session, result).await.unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.exc_type().to_string(), "PermissionError");

    let code = "\
from pathlib import Path
Path('/mnt/data.txt').write_text('changed')
Path('/mnt/data.txt').read_text()";
    let result = session
        .feed(code, vec![], mount(MountSpecMode::Overlay), false, &mut no_print)
        .await;
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("changed".to_owned()));
    // the host file is untouched and the overlay does not persist to the next feed
    assert_eq!(fs::read_to_string(dir.path().join("data.txt")).unwrap(), "original");
    let result = session
        .feed(
            "Path('/mnt/data.txt').read_text()",
            vec![],
            mount(MountSpecMode::Overlay),
            false,
            &mut no_print,
        )
        .await;
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("original".to_owned()));
    session.finish().await.unwrap();
}

/// The per-mount `write_bytes_limit` is enforced by the parent-side table and
/// surfaces as a catchable `OSError` inside the sandbox.
#[tokio::test]
async fn mount_write_bytes_limit_is_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let code = "\
from pathlib import Path
try:
    Path('/mnt/big.txt').write_text('x' * 100)
except OSError as exc:
    msg = str(exc)
msg";
    let result = session
        .feed(
            code,
            vec![],
            vec![{
                let mut spec = MountSpec::new("/mnt", dir.path(), MountSpecMode::ReadWrite).unwrap();
                spec.write_bytes_limit = Some(10);
                spec
            }],
            false,
            &mut no_print,
        )
        .await;
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    assert_eq!(
        expect_complete(event),
        MontyObject::string("disk write limit of 10 bytes exceeded".to_owned())
    );
    session.finish().await.unwrap();
}

/// A custom `MountSpec::memory_usage_limit` reaches the parent-side table and
/// surfaces as a catchable `MemoryError` inside the sandbox.
#[tokio::test]
async fn mount_memory_usage_limit_is_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    // the retained overlay write and the transient read-back share the budget
    let code = "\
from pathlib import Path
p = Path('/mnt/f.bin')
p.write_bytes(b'a' * 600)
msg = ''
try:
    p.read_bytes()
except MemoryError as exc:
    msg = str(exc)
msg";
    let mut spec = MountSpec::new("/mnt", dir.path(), MountSpecMode::Overlay).unwrap();
    spec.memory_usage_limit = 1_000;
    let result = session.feed(code, vec![], vec![spec], false, &mut no_print).await;
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    assert_eq!(
        expect_complete(event),
        MontyObject::string("mount memory usage limit of 1 KB exceeded".to_owned())
    );
    session.finish().await.unwrap();
}

/// Every OS call surfaces; offering each to the mounts answers the covered
/// one and leaves the uncovered one for the caller.
#[tokio::test]
async fn uncovered_os_calls_surface_alongside_mounted_ones() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("data.txt"), "covered").unwrap();

    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let code = "\
from pathlib import Path
covered = Path('/mnt/data.txt').read_text()
covered + ':' + Path('/elsewhere/file.txt').read_text()";
    let result = session
        .feed(
            code,
            vec![],
            vec![MountSpec::new("/mnt", dir.path(), MountSpecMode::ReadOnly).unwrap()],
            false,
            &mut no_print,
        )
        .await;
    // the covered `/mnt` read is answered by the mount; the `/elsewhere` one
    // is handed back for the caller to answer
    let event = feed_with_mounts(&mut session, result).await.unwrap();
    let TurnEvent::OsCall {
        function_name, args, ..
    } = event
    else {
        panic!("expected OsCall, got {event:?}");
    };
    assert_eq!(function_name, "Path.read_text");
    assert_eq!(
        args,
        CallArgs::from(vec![MontyObject::path("/elsewhere/file.txt".to_owned())])
    );
    let event = session
        .resume(
            ResumeValue::Return(MontyObject::string("uncovered".to_owned())),
            &mut no_print,
        )
        .await
        .unwrap();
    assert_eq!(
        expect_complete(event),
        MontyObject::string("covered:uncovered".to_owned())
    );
    session.finish().await.unwrap();
}

/// A feed suspended mid-OsCall can be dumped and restored elsewhere; the
/// re-supplied mounts service the resumed feed's filesystem calls.
#[tokio::test]
async fn suspended_feed_restores_with_mounts_re_supplied() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("data.txt"), "after resume").unwrap();

    let mount = || vec![MountSpec::new("/mnt", dir.path(), MountSpecMode::ReadOnly).unwrap()];
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    // suspend on an *uncovered* call, dump, and restore into a fresh worker
    let code = "\
from pathlib import Path
external = Path('/external/answer.txt').read_text()
external + ' ' + Path('/mnt/data.txt').read_text()";
    let event = session.feed(code, vec![], mount(), false, &mut no_print).await.unwrap();
    assert!(matches!(event, TurnEvent::OsCall { .. }), "got {event:?}");
    let state = session.dump().await.unwrap();
    drop(session);

    let mut restored = pool.checkout(&ReplConfig::default()).await.unwrap();
    let (event, _) = restored.restore(state, mount(), &mut no_print).await.unwrap();
    // the re-announcement carries the full call payload, so the uncovered
    // call surfaces exactly like a fresh suspension — name and args intact;
    // answering it lets the feed continue, and its remaining mounted read is
    // serviced by the parent
    let Some(TurnEvent::OsCall {
        function_name, args, ..
    }) = &event
    else {
        panic!("expected OsCall, got {event:?}");
    };
    assert_eq!(function_name, "Path.read_text");
    assert_eq!(
        args,
        &CallArgs::from(vec![MontyObject::path("/external/answer.txt".to_owned())])
    );
    let result = restored
        .resume(
            ResumeValue::Return(MontyObject::string("hello".to_owned())),
            &mut no_print,
        )
        .await;
    // the feed's remaining `/mnt` read is answered by the re-supplied mount
    let event = feed_with_mounts(&mut restored, result).await.unwrap();
    assert_eq!(
        expect_complete(event),
        MontyObject::string("hello after resume".to_owned())
    );
    restored.finish().await.unwrap();
}

/// A dump taken while suspended on a mount-coverable OS call re-announces the
/// call in full, so mounts supplied to `restore` can answer it — the payload
/// survives the round trip even though the original feed had no mounts.
#[tokio::test]
async fn restored_os_call_is_serviced_by_restore_mounts() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("data.txt"), "from mount").unwrap();

    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    // feed with NO mounts: the read surfaces as an uncovered OsCall
    let event = session
        .feed(
            "from pathlib import Path\nPath('/mnt/data.txt').read_text()",
            vec![],
            vec![],
            false,
            &mut no_print,
        )
        .await
        .unwrap();
    assert!(matches!(event, TurnEvent::OsCall { .. }), "got {event:?}");
    let state = session.dump().await.unwrap();
    drop(session);

    // restore WITH the mount: the call is re-announced, and offering it to the
    // now-present mount answers it and runs the feed to completion
    let mut restored = pool.checkout(&ReplConfig::default()).await.unwrap();
    let (event, _) = restored
        .restore(
            state,
            vec![MountSpec::new("/mnt", dir.path(), MountSpecMode::ReadOnly).unwrap()],
            &mut no_print,
        )
        .await
        .unwrap();
    let event = event.expect("suspended dump must re-announce a turn event");
    assert!(matches!(event, TurnEvent::OsCall { .. }), "got {event:?}");
    let event = feed_with_mounts(&mut restored, Ok(event)).await.unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("from mount".to_owned()));
    restored.finish().await.unwrap();
}

/// An over-limit frame must fail as a clean, session-preserving error rather
/// than crashing the worker: `Worker::send` rejects it before writing any
/// bytes, so the stream stays synced. Covers both directions — a request the
/// parent cannot send (a huge input) and a result the worker cannot send
/// back.
///
/// Allocates ~257 MiB (just over the 256 MiB frame limit) in the test process
/// and again in the worker, so it is memory-heavy; disable it if it proves
/// flaky in CI.
#[tokio::test]
async fn oversize_frames_are_rejected_without_killing_the_worker() {
    // just over monty_proto's 256 MiB MAX_FRAME_LEN
    const OVERSIZE: usize = 257 * 1024 * 1024;

    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();

    // (1) parent -> child: an input larger than the frame limit cannot be
    // sent. The worker never receives the request, so the session survives.
    let huge = MontyObject::string("x".repeat(OVERSIZE));
    let err = session
        .feed("data", vec![("data".to_owned(), huge)], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime for oversize input, got {err:?}");
    };
    assert!(
        exc.message()
            .is_some_and(|m| m.contains("request frame") && m.contains("exceeds the maximum")),
        "unexpected message: {:?}",
        exc.message()
    );
    let event = session
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(2));

    // (2) child -> parent: a result larger than the frame limit cannot be sent
    // back. The worker answers with a clean error and keeps the session.
    let err = session
        .feed(&format!("'x' * {OVERSIZE}"), vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime for oversize result, got {err:?}");
    };
    assert!(
        exc.message()
            .is_some_and(|m| m.contains("result frame") && m.contains("exceeds the maximum")),
        "unexpected message: {:?}",
        exc.message()
    );
    let event = session
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(2));

    // (3) child -> parent suspension: external-call arguments larger than the
    // frame limit cannot be announced to the parent. The child aborts that
    // feed before entering the suspension, so the session remains usable.
    let err = session
        .feed(
            &format!(
                "result = 'not caught'\ntry:\n    fetch('x' * {OVERSIZE})\nexcept RuntimeError:\n    result = 'caught'\nresult"
            ),
            vec![],
            vec![],
            false,
            &mut no_print,
        )
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime for oversize external-call args, got {err:?}");
    };
    assert!(
        exc.message()
            .is_some_and(|m| m.contains("argument frame") && m.contains("exceeds the maximum")),
        "unexpected message: {:?}",
        exc.message()
    );
    let event = session
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(2));

    session.finish().await.unwrap();

    // (4) parent -> child resume: a return value too large to send leaves the
    // suspension *answerable*. The child never saw the oversize frame, so it is
    // still waiting — clearing the pending call here would strand it for ever.
    // A fresh checkout, so the earlier steps' namespace cannot interfere.
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let event = session
        .feed("len(grab())", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert!(matches!(event, TurnEvent::FunctionCall { .. }), "got {event:?}");
    let huge = MontyObject::string("x".repeat(OVERSIZE));
    let err = session
        .resume(ResumeValue::Return(huge), &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime for oversize resume value, got {err:?}");
    };
    assert!(
        exc.message()
            .is_some_and(|m| m.contains("request frame") && m.contains("exceeds the maximum")),
        "unexpected message: {:?}",
        exc.message()
    );
    // the same suspension still answers to a value that fits
    let event = session
        .resume(
            ResumeValue::Return(MontyObject::string("small".to_owned())),
            &mut no_print,
        )
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(5));

    // (5) parent -> child name-lookup resume: same invariant as (4) — the
    // rejected answer leaves the lookup answerable.
    let event = session
        .feed("missing", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert!(matches!(event, TurnEvent::NameLookup { ref name, .. } if name == "missing"));
    let huge = MontyObject::string("x".repeat(OVERSIZE));
    let err = session.resume_name_lookup(Some(huge), &mut no_print).await.unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime for oversize name-lookup value, got {err:?}");
    };
    assert!(
        exc.message()
            .is_some_and(|m| m.contains("request frame") && m.contains("exceeds the maximum")),
        "unexpected message: {:?}",
        exc.message()
    );
    let event = session
        .resume_name_lookup(Some(MontyObject::string("small".to_owned())), &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("small".to_owned()));

    // (6) parent -> child future resolution: the pending futures stay
    // resolvable after an oversize result is rejected.
    let code = "import asyncio\nasync def main():\n    return await go()\nasyncio.run(main())";
    let event = session.feed(code, vec![], vec![], false, &mut no_print).await.unwrap();
    let TurnEvent::FunctionCall { call_id, .. } = event else {
        panic!("expected FunctionCall, got {event:?}");
    };
    let event = session.resume(ResumeValue::Future, &mut no_print).await.unwrap();
    assert!(matches!(event, TurnEvent::ResolveFutures { .. }), "got {event:?}");
    let huge = MontyObject::string("x".repeat(OVERSIZE));
    let err = session
        .resume_futures(vec![(call_id, ResumeValue::Return(huge))], &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime for oversize future result, got {err:?}");
    };
    assert!(
        exc.message()
            .is_some_and(|m| m.contains("request frame") && m.contains("exceeds the maximum")),
        "unexpected message: {:?}",
        exc.message()
    );
    let event = session
        .resume_futures(
            vec![(call_id, ResumeValue::Return(MontyObject::int(99)))],
            &mut no_print,
        )
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(99));
    session.finish().await.unwrap();
}

/// A `dump()` too large for the frame limit while the session is *suspended*
/// must fail as a clean, session-preserving error — the suspension already
/// announced its resume point, so the child stays suspended and resumable.
///
/// Allocates ~300 MiB in the worker (plus the dump copy), so it is
/// memory-heavy; disable it if it proves flaky in CI.
#[tokio::test]
async fn oversize_dump_while_suspended_fails_cleanly_and_resumes() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    // a tiny suspension announcement on top of a heap too big to dump
    let code = "data = 'x' * (300 * 1024 * 1024)\nf()\nlen(data)";
    let event = session.feed(code, vec![], vec![], false, &mut no_print).await.unwrap();
    assert!(matches!(event, TurnEvent::FunctionCall { .. }), "got {event:?}");

    let err = session.dump().await.unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime for oversize dump, got {err:?}");
    };
    assert!(
        exc.message()
            .is_some_and(|m| m.contains("result frame") && m.contains("exceeds the maximum")),
        "unexpected message: {:?}",
        exc.message()
    );

    // the suspension is untouched: resuming completes the feed
    let event = session
        .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(300 * 1024 * 1024));
    session.finish().await.unwrap();
}

#[tokio::test]
async fn inputs_and_prints() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let mut output = String::new();
    let event = session
        .feed(
            "print('hello', name)\nlen(name)",
            vec![("name".to_owned(), MontyObject::string("monty".to_owned()))],
            vec![],
            false,
            &mut on_print_sync(|_, text: &str| output.push_str(text)),
        )
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(5));
    assert_eq!(output, "hello monty\n");
    session.finish().await.unwrap();
}

/// A loop of small prints must not cost one `Print` event each: the worker
/// batches them by size and elapsed time, which is what keeps the parent's
/// per-event work (telemetry included) proportional to output rather than to
/// how often the sandbox called `print()`.
#[tokio::test]
async fn prints_are_batched_into_few_events() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let mut output = String::new();
    let mut events = 0;
    let event = session
        .feed(
            "for i in range(2000):\n    print(i)",
            vec![],
            vec![],
            false,
            &mut on_print_sync(|_, text: &str| {
                events += 1;
                output.push_str(text);
            }),
        )
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::none());
    let mut expected = String::new();
    for i in 0..2000 {
        writeln!(expected, "{i}").unwrap();
    }
    assert_eq!(output, expected);
    assert!(
        events < 200,
        "expected far fewer than one event per print, got {events}"
    );
    session.finish().await.unwrap();
}

/// The size threshold still bounds a chunk when the interval never expires:
/// one huge write is split into ~8 KiB events rather than buffered whole. This
/// is also what keeps the buffer below the threshold on entry to each write, so
/// its remaining-room arithmetic cannot underflow.
#[tokio::test]
async fn one_huge_write_is_split_at_the_size_threshold() {
    const FLUSH_BYTES: usize = 8 * 1024;
    let pool = Pool::new(config()).await.unwrap();
    // an interval nothing will reach, so only the size threshold can flush
    let repl = ReplConfig {
        print_flush_interval: Some(Duration::from_secs(600)),
        ..ReplConfig::default()
    };
    let mut session = pool.checkout(&repl).await.unwrap();
    let mut chunks: Vec<usize> = Vec::new();
    let mut total = 0;
    session
        .feed(
            "print('x' * 100_000, end='')",
            vec![],
            vec![],
            false,
            &mut on_print_sync(|_, text: &str| {
                chunks.push(text.len());
                total += text.len();
            }),
        )
        .await
        .unwrap();
    assert_eq!(total, 100_000);
    assert!(chunks.len() > 1, "expected the write to be split, got {chunks:?}");
    assert!(
        chunks.iter().all(|len| *len <= FLUSH_BYTES),
        "every chunk must stay within the threshold, got {chunks:?}"
    );
    session.finish().await.unwrap();
}

/// A zero interval turns the timer off and restores line buffering, for a host
/// that wants each completed line delivered on its own.
#[tokio::test]
async fn zero_flush_interval_delivers_one_event_per_line() {
    let pool = Pool::new(config()).await.unwrap();
    let repl = ReplConfig {
        print_flush_interval: Some(Duration::ZERO),
        ..ReplConfig::default()
    };
    let mut session = pool.checkout(&repl).await.unwrap();
    let mut lines = Vec::new();
    session
        .feed(
            "for i in range(20):\n    print(i)",
            vec![],
            vec![],
            false,
            &mut on_print_sync(|_, text: &str| lines.push(text.to_owned())),
        )
        .await
        .unwrap();
    assert_eq!(lines.len(), 20);
    assert_eq!(lines[0], "0\n");
    assert_eq!(lines[19], "19\n");
    session.finish().await.unwrap();
}

/// With the timer off the contract is one event per *completed line*, not one
/// per write: a single `print()` carrying embedded newlines must still arrive
/// as one event per line.
#[tokio::test]
async fn zero_flush_interval_splits_embedded_newlines() {
    let pool = Pool::new(config()).await.unwrap();
    let repl = ReplConfig {
        print_flush_interval: Some(Duration::ZERO),
        ..ReplConfig::default()
    };
    let mut session = pool.checkout(&repl).await.unwrap();
    let mut events = Vec::new();
    session
        .feed(
            r#"print("a\nb")"#,
            vec![],
            vec![],
            false,
            &mut on_print_sync(|_, text: &str| events.push(text.to_owned())),
        )
        .await
        .unwrap();
    assert_eq!(events, vec!["a\n".to_owned(), "b\n".to_owned()]);
    session.finish().await.unwrap();
}

/// Output must not sit in the worker's buffer while the program computes in
/// silence: the interpreter polls the writer at its dispatch checkpoints, so a
/// line printed before a long stretch of quiet work is released on its own
/// rather than waiting for the next print.
#[tokio::test]
async fn silence_releases_buffered_output() {
    let pool = Pool::new(config()).await.unwrap();
    let repl = ReplConfig {
        print_flush_interval: Some(Duration::from_millis(1)),
        ..ReplConfig::default()
    };
    let mut session = pool.checkout(&repl).await.unwrap();
    let mut events: Vec<String> = Vec::new();
    session
        .feed(
            // the loop is far longer than the 1ms interval in any build
            "print('first')\nt = 0\nfor i in range(200_000):\n    t += i\nprint('second')",
            vec![],
            vec![],
            false,
            &mut on_print_sync(|_, text: &str| events.push(text.to_owned())),
        )
        .await
        .unwrap();
    assert_eq!(events.first().map(String::as_str), Some("first\n"));
    assert_eq!(events.concat(), "first\nsecond\n");
    session.finish().await.unwrap();
}

/// Batching must never reorder output against the suspension that follows it:
/// the worker drains its buffer before every turn-ending event, so a print
/// made just before a host call still arrives before the call is announced.
#[tokio::test]
async fn buffered_output_is_flushed_before_a_suspension() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let mut output = String::new();
    let event = session
        .feed(
            "print('before')\nask()",
            vec![],
            vec![],
            false,
            &mut on_print_sync(|_, text: &str| output.push_str(text)),
        )
        .await
        .unwrap();
    assert!(
        matches!(event, TurnEvent::FunctionCall { ref function_name, .. } if function_name == "ask"),
        "expected a call suspension, got {event:?}"
    );
    assert_eq!(output, "before\n");
    session.finish().await.unwrap();
}

#[tokio::test]
async fn external_function_round_trip() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let event = session
        .feed("fetch('https://x') + '!'", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    let TurnEvent::FunctionCall {
        function_name, args, ..
    } = event
    else {
        panic!("expected FunctionCall, got {event:?}");
    };
    assert_eq!(function_name, "fetch");
    assert_eq!(args, CallArgs::from(vec![MontyObject::string("https://x".to_owned())]));
    let event = session
        .resume(
            ResumeValue::Return(MontyObject::string("body".to_owned())),
            &mut no_print,
        )
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("body!".to_owned()));
    session.finish().await.unwrap();
}

#[tokio::test]
async fn runtime_error_keeps_session_and_worker() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    assert_eq!(
        expect_complete(
            session
                .feed("kept = 1", vec![], vec![], false, &mut no_print)
                .await
                .unwrap()
        ),
        MontyObject::none()
    );
    let err = session
        .feed("1 / 0", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.message(), Some("division by zero"));
    // session and worker survive
    assert_eq!(
        expect_complete(
            session
                .feed("kept + 41", vec![], vec![], false, &mut no_print)
                .await
                .unwrap()
        ),
        MontyObject::int(42)
    );
    session.finish().await.unwrap();
    assert_eq!(pool.idle_workers(), 1);
}

// =============================================================================
// Crash isolation — the headline feature
// =============================================================================

#[tokio::test]
async fn sigkill_mid_request_is_a_clean_crash_and_the_pool_recovers() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let pid = session.pid().unwrap();

    // kill the worker while it spins forever
    let killer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        kill_pid(pid);
    });
    let err = session
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    killer.join().unwrap();
    assert!(matches!(err, PoolError::Crashed { .. }), "got {err:?}");

    // the pool spawns a replacement transparently
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    assert_ne!(session.pid().unwrap(), pid);
    assert_eq!(
        expect_complete(
            session
                .feed("1 + 1", vec![], vec![], false, &mut no_print)
                .await
                .unwrap()
        ),
        MontyObject::int(2)
    );
    session.finish().await.unwrap();
}

#[tokio::test]
async fn worker_killed_while_idle_is_replaced_transparently() {
    let pool = Pool::new(config()).await.unwrap();
    let pids = pool.idle_worker_pids();
    assert_eq!(pids.len(), 1);
    kill_pid(pids[0]);
    // give the OS a moment to deliver the kill
    sleep(Duration::from_millis(100)).await;

    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    assert_eq!(
        expect_complete(
            session
                .feed("2 + 2", vec![], vec![], false, &mut no_print)
                .await
                .unwrap()
        ),
        MontyObject::int(4)
    );
    session.finish().await.unwrap();
}

/// A deep postfix spine (`a.x.x.x…`) currently overflows the child's stack
/// (recursive AST handling) — a REAL memory crash, exactly what subprocess
/// isolation exists for. If monty core gains AST-depth protection this
/// becomes a `Runtime`/`Crashed` either way; the invariant under test is
/// that the parent survives and the pool keeps serving.
#[tokio::test]
async fn hard_child_crash_does_not_harm_the_pool() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let mut code = String::with_capacity(300_002);
    code.push('a');
    for _ in 0..150_000 {
        code.push_str(".x");
    }
    let err = session
        .feed(&code, vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    assert!(
        matches!(err, PoolError::Crashed { .. } | PoolError::Runtime(_)),
        "got {err:?}"
    );
    // a stack overflow aborts with SIGABRT, which must never be mistaken for the
    // allocator's dedicated out-of-memory exit — the reason that exit exists
    if let PoolError::Runtime(exc) = &err {
        assert_ne!(
            exc.message(),
            Some("the worker exceeded its memory limit and was terminated")
        );
    }

    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    assert_eq!(
        expect_complete(
            session
                .feed("1 + 1", vec![], vec![], false, &mut no_print)
                .await
                .unwrap()
        ),
        MontyObject::int(2)
    );
    session.finish().await.unwrap();
}

#[tokio::test]
async fn deadline_kills_hung_worker_after_request_timeout() {
    let mut config = config();
    config.request_timeout = Some(Duration::from_secs(1));
    let pool = Pool::new(config).await.unwrap();
    // The checkout's Configure turn shares the pool-wide deadline, and a cold
    // child on a fully loaded test machine has (rarely) missed it — retry so
    // the setup is independent of the deadline actually under test below.
    let mut session = loop {
        match pool.checkout(&ReplConfig::default()).await {
            Ok(session) => break session,
            Err(PoolError::Timeout { .. }) => {}
            Err(err) => panic!("checkout failed: {err:?}"),
        }
    };
    let err = session
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    assert!(matches!(err, PoolError::Timeout { .. }), "got {err:?}");

    // pool still healthy
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    assert_eq!(
        expect_complete(
            session
                .feed("3 + 3", vec![], vec![], false, &mut no_print)
                .await
                .unwrap()
        ),
        MontyObject::int(6)
    );
    session.finish().await.unwrap();
}

#[tokio::test]
async fn child_resource_limits_do_not_kill_the_worker() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    let err = session
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    // the SANDBOX limit fired (TimeoutError exception), not the parent-side
    // deadline — the worker process is alive and finishable
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.exc_type().to_string(), "TimeoutError");
    session.finish().await.unwrap();
    assert_eq!(pool.idle_workers(), 1);
}

/// A session's `max_memory` must not disturb work that stays inside it.
#[tokio::test]
async fn max_memory_leaves_normal_work_alone() {
    let pool = Pool::new(config()).await.unwrap();
    let repl_config = ReplConfig {
        limits: Some(ResourceLimits::default().max_memory(1024 * 1024)),
        ..ReplConfig::default()
    };
    let mut session = pool.checkout(&repl_config).await.unwrap();
    assert_eq!(
        expect_complete(
            session
                .feed("1 + 1", vec![], vec![], false, &mut no_print)
                .await
                .unwrap()
        ),
        MontyObject::int(2)
    );
    session.finish().await.unwrap();
}

/// Only `OOM_EXIT_CODE` carries a meaning: any other code a worker might return
/// stays an opaque death rather than being read as a memory outcome.
#[cfg(unix)]
#[tokio::test]
async fn unrecognised_exit_code_stays_an_opaque_death() {
    let dir = tempfile::tempdir().unwrap();
    // outlives the parent's first write, so the death is always observed while
    // waiting for the reply rather than racing with `sending a request`
    let fake = write_fake_monty(dir.path(), "#!/bin/sh\nsleep 0.2\nexit 64\n");

    let pool = Pool::new(PoolConfig::subprocess(&fake)).await.unwrap();
    let err = match pool.checkout(&ReplConfig::default()).await {
        Ok(mut session) => session
            .feed("1 + 1", vec![], vec![], false, &mut no_print)
            .await
            .expect_err("expected the stand-in binary to fail the turn"),
        Err(err) => err,
    };
    let PoolError::Crashed { cause, .. } = &err else {
        panic!("expected Crashed, got {err:?}");
    };
    assert!(
        matches!(cause, monty_pool::CrashCause::Vanished { .. }),
        "an unrecognised exit code must not be classified, got {cause:?}"
    );
    // the message says only what the pool was doing and what it reaped — no
    // memory wording, which is what `OOM_EXIT_CODE` alone earns
    assert_snapshot!(err.to_string(), @"monty worker crashed while waiting for a reply (exit status: 64)");
}

/// A local child cannot pass a `ShutdownDump` off as a turn-ender on the raw
/// path: only a serving relay sends one, and accepting it would let a
/// compromised child hand a dump it minted itself out through a relay that
/// signs whatever it forwards.
#[cfg(unix)]
#[tokio::test]
async fn a_subprocess_shutdown_dump_is_refused_on_the_raw_path() {
    let dir = tempfile::tempdir().unwrap();
    // the two frames the stand-in replays, in order: `Ok` answers the
    // `Configure` that establishes the session, then the forged dump
    let mut replies = framed(&child_event(pb::child_event::Kind::Ok(pb::Ok {})));
    replies.extend(framed(&child_event(pb::child_event::Kind::Shutdown(
        pb::ShutdownDump {
            dump: Some(b"a dump the child minted itself".to_vec().into()),
        },
    ))));
    let replies_path = dir.path().join("replies.bin");
    fs::write(&replies_path, &replies).unwrap();

    // both frames are written up front and the process stays alive; the parent
    // reads them in turn as it sends `Configure` and then the raw request
    let fake = write_fake_monty(
        dir.path(),
        &format!("#!/bin/sh\ncat '{}'\nsleep 5\n", replies_path.display()),
    );

    let pool = Pool::new(PoolConfig::subprocess(&fake)).await.unwrap();
    let mut checkout = pool
        .checkout(&ReplConfig::default())
        .await
        .expect("the stand-in answers Configure with Ok");
    let request = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "1 + 1".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
        })),
        ..pb::ParentRequest::default()
    };
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    let err = checkout
        .turn_raw(&request, &mut on_event)
        .await
        .expect_err("a subprocess ShutdownDump must not be accepted");
    assert_snapshot!(
        err.to_string(),
        @"monty worker protocol error: subprocess worker sent a ShutdownDump"
    );
}

/// An event with no `kind` ends no turn on the raw path. Every wire field is
/// optional, so a hostile worker can send one whose oneof is unset — it must
/// not reach the raw driver's client as a successful turn-ender.
#[cfg(unix)]
#[tokio::test]
async fn an_event_with_no_kind_is_refused_on_the_raw_path() {
    let dir = tempfile::tempdir().unwrap();
    // `Ok` answers the establishing `Configure`, then the kindless frame
    let mut replies = framed(&child_event(pb::child_event::Kind::Ok(pb::Ok {})));
    replies.extend(framed(&pb::ChildEvent::default()));
    let replies_path = dir.path().join("replies.bin");
    fs::write(&replies_path, &replies).unwrap();

    let fake = write_fake_monty(
        dir.path(),
        &format!("#!/bin/sh\ncat '{}'\nsleep 5\n", replies_path.display()),
    );

    let pool = Pool::new(PoolConfig::subprocess(&fake)).await.unwrap();
    let mut checkout = pool
        .checkout(&ReplConfig::default())
        .await
        .expect("the stand-in answers Configure with Ok");
    let request = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "1 + 1".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
        })),
        ..pb::ParentRequest::default()
    };
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    let err = checkout
        .turn_raw(&request, &mut on_event)
        .await
        .expect_err("a kindless event must not be accepted as a turn-ender");
    assert_snapshot!(
        err.to_string(),
        @"monty worker protocol error: worker sent an event with no kind"
    );
}

/// A `FatalError` ends a raw turn *and* the worker behind it: the frame is
/// handed back for the driver to forward, but the dead child is reaped and
/// discarded rather than holding its pool slot until the next failure.
#[cfg(unix)]
#[tokio::test]
async fn a_fatal_error_on_the_raw_path_discards_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    // `Ok` answers the establishing `Configure`, then the fatal frame
    let mut replies = framed(&child_event(pb::child_event::Kind::Ok(pb::Ok {})));
    replies.extend(framed(&child_event(pb::child_event::Kind::FatalError(
        pb::FatalError {
            message: "the child is done".to_owned(),
        },
    ))));
    let replies_path = dir.path().join("replies.bin");
    fs::write(&replies_path, &replies).unwrap();

    let fake = write_fake_monty(
        dir.path(),
        &format!("#!/bin/sh\ncat '{}'\nsleep 5\n", replies_path.display()),
    );

    let pool = Pool::new(PoolConfig::subprocess(&fake)).await.unwrap();
    let mut checkout = pool
        .checkout(&ReplConfig::default())
        .await
        .expect("the stand-in answers Configure with Ok");
    let request = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "1 + 1".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
        })),
        ..pb::ParentRequest::default()
    };
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    let event = checkout
        .turn_raw(&request, &mut on_event)
        .await
        .expect("the fatal frame is the turn-ender");
    let Some(pb::child_event::Kind::FatalError(fatal)) = event.kind else {
        panic!("expected the FatalError frame back, got {:?}", event.kind);
    };
    assert_eq!(fatal.message, "the child is done");
    // the worker was discarded with the frame, not kept until the next failure
    let err = checkout.turn_raw(&request, &mut on_event).await.unwrap_err();
    assert!(matches!(err, PoolError::Finished), "got {err:?}");
}

/// Length-prefixes one event exactly as a worker writes it.
#[cfg(unix)]
fn framed(event: &pb::ChildEvent) -> Vec<u8> {
    let mut buf = Vec::new();
    encode_framed_into(event, &mut buf).expect("encode");
    buf
}

/// A `ChildEvent` carrying `kind` and nothing else.
#[cfg(unix)]
fn child_event(kind: pb::child_event::Kind) -> pb::ChildEvent {
    pb::ChildEvent {
        kind: Some(kind),
        ..pb::ChildEvent::default()
    }
}

/// A refused allocation is the one `Runtime` error whose worker is already
/// dead: it reports `MemoryError` (the worker's dedicated exit code, not an
/// unclassifiable `SIGABRT`), the checkout is finished, and the pool recovers.
/// Needs no limit: 1 EiB dwarfs the usable address space on any 64-bit host,
/// so the allocator refuses it regardless of overcommit policy.
#[tokio::test]
async fn refused_allocation_is_a_memory_error_and_the_pool_recovers() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    // no `max_memory`, so the sandbox tracker allows this outright
    let err = session
        .feed("x = ' ' * (1 << 60)", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.exc_type().to_string(), "MemoryError");
    assert_eq!(
        exc.message(),
        Some("the worker exceeded its memory limit and was terminated")
    );
    // unlike an in-sandbox exception, the worker is gone with it
    assert!(matches!(session.finish().await, Err(PoolError::Finished)));

    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    assert_eq!(
        expect_complete(
            session
                .feed("3 + 3", vec![], vec![], false, &mut no_print)
                .await
                .unwrap()
        ),
        MontyObject::int(6)
    );
    session.finish().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn special_files_in_mounts_are_rejected_without_blocking() {
    // Mount I/O runs on the parent task servicing the sandbox, so reading a
    // FIFO must fail fast (a real read would block until a writer appears —
    // with no deadline able to rescue the parent). The sandbox sees a
    // catchable PermissionError and the session stays usable.
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new("mkfifo").arg(dir.path().join("pipe")).status().unwrap();
    assert!(status.success(), "mkfifo failed");

    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let result = session
        .feed(
            "from pathlib import Path\nPath('/mnt/pipe').read_text()",
            vec![],
            vec![MountSpec::new("/mnt", dir.path(), MountSpecMode::ReadOnly).unwrap()],
            false,
            &mut no_print,
        )
        .await;
    let err = feed_with_mounts(&mut session, result).await.unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.exc_type().to_string(), "PermissionError");
    session.finish().await.unwrap();
}

#[tokio::test]
async fn suspension_time_does_not_consume_the_duration_budget() {
    // `max_feed_duration` measures sandbox execution time; the worker reports
    // it on every turn and its clock is paused while suspended. The host
    // staying away for twice the entire budget must therefore not time the
    // feed out.
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_millis(300))),
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    let event = session
        .feed("fetch('https://x') + '!'", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert!(matches!(event, TurnEvent::FunctionCall { .. }));

    sleep(Duration::from_millis(600)).await;

    let event = session
        .resume(
            ResumeValue::Return(MontyObject::string("body".to_owned())),
            &mut no_print,
        )
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::string("body!".to_owned()));
    session.finish().await.unwrap();
}

/// The session's `AutoOsCalls` travel in `Configure`: by default the worker
/// answers the clock and `random`'s seed itself, so neither costs a turn,
/// while each sleep is an `OsCall` whose `system_sleep` the caller waits out,
/// cut to the mode's maximum; a seed and a fixed clock are honoured exactly.
#[tokio::test]
async fn auto_os_calls_are_answered_in_the_worker() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            auto_os_calls: AutoOsCalls {
                sleep: SleepMode::System(Duration::from_millis(10)),
                ..AutoOsCalls::default()
            },
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    let code = "import asyncio, random, time\nfrom datetime import date\nt = time.time()\ntime.sleep(3600)\n\
                (date.today().year >= 2026, time.time() >= t + 0.01, \
                asyncio.run(asyncio.sleep(3600, 'woken')), 0 <= random.random() < 1)";
    let mut event = session.feed(code, vec![], vec![], false, &mut no_print).await.unwrap();
    let mut slept = vec![];
    while let TurnEvent::OsCall {
        function_name,
        system_sleep,
        ..
    } = &event
    {
        let delay = system_sleep.expect("only the sleeps reach the caller");
        slept.push((function_name.clone(), delay));
        sleep(delay).await;
        event = session
            .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
            .await
            .unwrap();
    }
    assert_eq!(
        slept,
        vec![
            ("system.sleep".to_owned(), Duration::from_millis(10)),
            ("system.async_sleep".to_owned(), Duration::from_millis(10)),
        ]
    );
    assert_eq!(
        expect_complete(event),
        MontyObject::tuple([
            MontyObject::bool(true),
            MontyObject::bool(true),
            MontyObject::string("woken"),
            MontyObject::bool(true),
        ])
    );
    session.finish().await.unwrap();

    let mut session = pool
        .checkout(&ReplConfig {
            auto_os_calls: AutoOsCalls {
                datetime: DateTimeSource::Fixed {
                    unix_seconds: 1_700_000_000,
                    microsecond: 0,
                },
                sleep: SleepMode::Zero,
                random_start: RandomStart::Seed(RandomSeed::Int(42.into())),
                ..AutoOsCalls::default()
            },
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    // CPython: random.seed(42); random.random()
    let code = "import random, time\ntime.sleep(3600)\n(time.time(), random.random())";
    let event = session.feed(code, vec![], vec![], false, &mut no_print).await.unwrap();
    assert_eq!(
        expect_complete(event),
        MontyObject::tuple([
            MontyObject::float(1_700_000_000.0),
            MontyObject::float(0.639_426_798_457_883_7),
        ])
    );
    session.finish().await.unwrap();
}

/// `CallHost` delivers the clock and sleep calls as `OsCall` turns instead.
#[tokio::test]
async fn call_host_delivers_clock_and_sleeps_as_os_calls() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            auto_os_calls: AutoOsCalls {
                datetime: DateTimeSource::CallHost,
                sleep: SleepMode::CallHost,
                ..AutoOsCalls::default()
            },
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    let code = "import time
time.sleep(1.5)
time.time()";
    let event = session.feed(code, vec![], vec![], false, &mut no_print).await.unwrap();
    let TurnEvent::OsCall {
        function_name, args, ..
    } = event
    else {
        panic!("expected OsCall, got {event:?}");
    };
    assert_eq!(function_name, "time.sleep");
    assert_eq!(args, CallArgs::from(vec![MontyObject::float(1.5)]));
    let event = session
        .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
        .await
        .unwrap();
    let TurnEvent::OsCall { function_name, .. } = event else {
        panic!("expected OsCall, got {event:?}");
    };
    assert_eq!(function_name, "time.time");
    let event = session
        .resume(ResumeValue::Return(MontyObject::float(7.5)), &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::float(7.5));
    session.finish().await.unwrap();
}

/// The pool aborts the first suspension past the limit uncatchably. The
/// session remains usable, but its suspension budget stays spent.
#[tokio::test]
async fn suspension_limit_aborts_the_feed() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_suspensions(3)),
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    let code = "n = 0\nwhile True:\n    try:\n        open('/etc/passwd')\n    except Exception:\n        n += 1";
    let mut event = session.feed(code, vec![], vec![], false, &mut no_print).await.unwrap();
    for _ in 0..2 {
        assert!(matches!(event, TurnEvent::OsCall { .. }), "got {event:?}");
        event = session.resume(ResumeValue::NotHandled, &mut no_print).await.unwrap();
    }
    assert!(matches!(event, TurnEvent::OsCall { .. }), "got {event:?}");
    let err = session
        .resume(ResumeValue::NotHandled, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.exc_type().to_string(), "RuntimeError");
    assert_eq!(exc.message(), Some("suspension limit 3 exceeded"));
    // three refusals were caught before the fourth suspension was aborted
    let event = session.feed("n", vec![], vec![], false, &mut no_print).await.unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(3));
    // the budget stays spent: a fresh feed's first suspension is aborted too
    let err = session
        .feed("fetch('x')", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.message(), Some("suspension limit 3 exceeded"));
    session.finish().await.unwrap();
}

/// The `max_suspensions` budget travels in the dump and is re-adopted on
/// restore, but the count is parent state and restarts at zero.
#[tokio::test]
async fn restored_session_readopts_its_suspension_limit() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_suspensions(1)),
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    let event = session
        .feed("fetch('x')", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert!(matches!(event, TurnEvent::FunctionCall { .. }));
    let state = session.dump().await.unwrap();
    drop(session);

    let mut restored = pool.checkout(&ReplConfig::default()).await.unwrap();
    let (event, _script_name) = restored.restore(state, vec![], &mut no_print).await.unwrap();
    // the re-announced suspension is the restored checkout's first
    assert!(matches!(event, Some(TurnEvent::FunctionCall { .. })), "got {event:?}");
    let event = restored
        .resume(ResumeValue::Return(MontyObject::int(1)), &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(1));
    // the dump's limit of one applies to the next suspension
    let err = restored
        .feed("fetch('y')", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.message(), Some("suspension limit 1 exceeded"));
    restored.finish().await.unwrap();
}

#[tokio::test]
async fn loaded_session_keeps_its_duration_budget() {
    // The `max_feed_duration` budget travels inside the dump — a session
    // restored via `restore` keeps the original limits even though the parent
    // never saw the original `ReplConfig`.
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    let state = session.dump().await.unwrap();
    drop(session);

    let mut restored = pool.checkout(&ReplConfig::default()).await.unwrap();
    let (event, _script_name) = restored.restore(state, vec![], &mut no_print).await.unwrap();
    assert!(event.is_none(), "idle dump should restore without a suspension");
    let err = restored
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.exc_type().to_string(), "TimeoutError");
    restored.finish().await.unwrap();
}

// =============================================================================
// Lifecycle
// =============================================================================

#[tokio::test]
async fn elastic_growth_up_to_max_then_exhausted() {
    let mut config = config();
    config.min_processes = 1;
    config.max_processes = 2;
    config.checkout_timeout = Some(Duration::from_millis(200));
    let pool = Pool::new(config).await.unwrap();

    let one = pool.checkout(&ReplConfig::default()).await.unwrap();
    let two = pool.checkout(&ReplConfig::default()).await.unwrap(); // grows beyond min
    let Err(err) = pool.checkout(&ReplConfig::default()).await else {
        panic!("expected Exhausted");
    };
    assert!(matches!(err, PoolError::Exhausted), "got {err:?}");

    one.finish().await.unwrap();
    let three = pool.checkout(&ReplConfig::default()).await.unwrap(); // reuses one's worker
    three.finish().await.unwrap();
    two.finish().await.unwrap();
    assert_eq!(pool.idle_workers(), 2);
}

#[tokio::test]
async fn dropping_a_checkout_kills_the_worker_but_frees_capacity() {
    let mut config = config();
    config.max_processes = 1;
    let pool = Pool::new(config).await.unwrap();

    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let pid = session.pid().unwrap();
    let _ = session
        .feed("x = 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    drop(session); // abandoned mid-session: worker killed, capacity released

    // with max_processes=1 this checkout only succeeds if capacity was freed
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    assert_ne!(session.pid().unwrap(), pid);
    assert_eq!(
        expect_complete(
            session
                .feed("5 + 5", vec![], vec![], false, &mut no_print)
                .await
                .unwrap()
        ),
        MontyObject::int(10)
    );
    session.finish().await.unwrap();
}

#[tokio::test]
async fn workers_are_recycled_after_max_checkouts() {
    let mut config = config();
    config.max_checkouts_per_worker = Some(1);
    let pool = Pool::new(config).await.unwrap();

    let session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let first_pid = session.pid().unwrap();
    session.finish().await.unwrap();
    assert_eq!(pool.idle_workers(), 0, "worker must be retired, not pooled");

    let session = pool.checkout(&ReplConfig::default()).await.unwrap();
    assert_ne!(session.pid().unwrap(), first_pid);
    session.finish().await.unwrap();
}

#[tokio::test]
async fn concurrent_checkouts_run_in_parallel() {
    let mut config = config();
    config.min_processes = 2;
    config.max_processes = 2;
    let pool = Pool::new(config).await.unwrap();

    let run_one = async {
        let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
        let event = session
            .feed("sum(range(1000))", vec![], vec![], false, &mut no_print)
            .await
            .unwrap();
        assert_eq!(expect_complete(event), MontyObject::int(499_500));
        session.finish().await.unwrap();
    };
    tokio::join!(run_one, async {
        let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
        let event = session
            .feed("sum(range(1000))", vec![], vec![], false, &mut no_print)
            .await
            .unwrap();
        assert_eq!(expect_complete(event), MontyObject::int(499_500));
        session.finish().await.unwrap();
    });
    assert_eq!(pool.idle_workers(), 2);
}

#[tokio::test]
async fn typing_error_via_pool() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            type_check: true,
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    let err = session
        .feed("x: int = 'nope'", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Typing(diagnostics) = err else {
        panic!("expected Typing, got {err:?}");
    };
    assert!(diagnostics.contains("invalid-assignment"), "{diagnostics}");
    // the session survives a typing rejection
    assert_eq!(
        expect_complete(
            session
                .feed("1 + 1", vec![], vec![], false, &mut no_print)
                .await
                .unwrap()
        ),
        MontyObject::int(2)
    );
    session.finish().await.unwrap();
}

/// Security-critical: a reused worker's type checker must not let one
/// session's files (script or stubs) influence the next session's checks.
/// The same pid is asserted so this cannot pass vacuously on a fresh worker —
/// the exact regression a broken `Reset`-time scrub would cause.
#[tokio::test]
async fn type_check_state_is_scrubbed_between_checkouts_on_the_same_worker() {
    let concise = TypeCheckingConfig {
        format: TypeCheckingFormat::Concise,
        color: false,
    };
    let pool = Pool::new(config()).await.unwrap();

    // Session A: commits a module-level name into `a.py` and carries stubs.
    let mut session = pool
        .checkout(&ReplConfig {
            script_name: "a.py".to_owned(),
            type_check: true,
            type_check_stubs: Some("STUB_SECRET: str = 'from-stubs'".to_owned()),
            type_check_config: concise,
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    let first_pid = session.pid().unwrap();
    let event = session
        .feed("SECRET = 'hunter2'", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::none());
    session.finish().await.unwrap();
    assert_eq!(pool.idle_workers(), 1);

    // Session B runs in the SAME process; A's files must be gone.
    let mut session = pool
        .checkout(&ReplConfig {
            script_name: "b.py".to_owned(),
            type_check: true,
            type_check_config: concise,
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    assert_eq!(
        session.pid().unwrap(),
        first_pid,
        "worker was not reused — test is vacuous"
    );

    let mut probe = async |code: &str| match session.feed(code, vec![], vec![], false, &mut no_print).await {
        Err(PoolError::Typing(diagnostics)) => diagnostics,
        other => panic!("expected a typing rejection for {code:?}, got {other:?}"),
    };
    assert_eq!(
        probe("from a import SECRET").await,
        "b.py:1:6: error[unresolved-import] Cannot resolve imported module `a`\n"
    );
    assert_eq!(
        probe("from repl_type_stubs import STUB_SECRET").await,
        "b.py:1:6: error[unresolved-import] Cannot resolve imported module `repl_type_stubs`\n"
    );
    assert_eq!(
        probe("x = STUB_SECRET").await,
        "b.py:1:5: error[unresolved-reference] Name `STUB_SECRET` used when not defined\n"
    );
    // and the scrub did not damage the checker itself
    let event = session
        .feed("x: int = 1\nx", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(1));
    session.finish().await.unwrap();
}

// =============================================================================
// Cancellation
// =============================================================================

/// A turn future dropped mid-flight leaves the protocol state unknowable: the
/// next call on the checkout must discard the worker and fail cleanly, and
/// the pool must stay healthy.
#[tokio::test]
async fn cancelled_turn_discards_the_worker_on_next_use() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();

    {
        // race the feed against a short sleep: the snippet never terminates,
        // so the sleep deterministically wins and the feed is cancelled while
        // awaiting the worker's reply
        let mut printer = no_print;
        let feed = session.feed("while True:\n    pass", vec![], vec![], false, &mut printer);
        tokio::select! {
            _result = feed => panic!("the feed cannot complete"),
            () = sleep(Duration::from_millis(10)) => {}
        }
    }

    let err = session
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Protocol(message) = err else {
        panic!("expected Protocol, got {err:?}");
    };
    assert_eq!(
        message,
        "a previous protocol turn was cancelled mid-flight; the worker was discarded"
    );

    // the worker was discarded and the pool serves a fresh one
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let event = session
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(2));
    session.finish().await.unwrap();
}

/// A cancellation condemns the worker on *any* next call: `resume_from_mounts`
/// must hit the cancellation check before its own pending-state validation
/// and report the discard, not a state error.
#[tokio::test]
async fn cancelled_turn_discards_the_worker_on_resume_from_mounts() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    {
        let mut printer = no_print;
        let feed = session.feed("while True:\n    pass", vec![], vec![], false, &mut printer);
        tokio::select! {
            _result = feed => panic!("the feed cannot complete"),
            () = sleep(Duration::from_millis(10)) => {}
        }
    }

    let err = session.resume_from_mounts(&mut no_print).await.unwrap_err();
    let PoolError::Protocol(message) = err else {
        panic!("expected Protocol, got {err:?}");
    };
    assert_eq!(
        message,
        "a previous protocol turn was cancelled mid-flight; the worker was discarded"
    );

    // the discard already happened: every later call reports the dead checkout
    let err = session
        .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
        .await
        .unwrap_err();
    assert!(matches!(err, PoolError::Finished), "got {err:?}");
}

// =============================================================================
// Dump / load across workers
// =============================================================================

#[tokio::test]
async fn dump_survives_worker_death_and_loads_elsewhere() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    assert_eq!(
        expect_complete(
            session
                .feed("base = 40", vec![], vec![], false, &mut no_print)
                .await
                .unwrap()
        ),
        MontyObject::none()
    );
    let event = session
        .feed("base + ext()", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    let TurnEvent::FunctionCall {
        call_id: _,
        ref function_name,
        ..
    } = event
    else {
        panic!("expected FunctionCall, got {event:?}");
    };
    assert_eq!(function_name, "ext");

    let state = session.dump().await.unwrap();
    drop(session); // kill the original worker outright

    // restore into a fresh worker by loading over its empty session
    let mut restored = pool.checkout(&ReplConfig::default()).await.unwrap();
    let (event, script_name) = restored.restore(state, vec![], &mut no_print).await.unwrap();
    // the worker echoes the dump's adopted script name back to the parent
    assert_eq!(script_name.as_deref(), Some("main.py"));
    let Some(TurnEvent::FunctionCall { ref function_name, .. }) = event else {
        panic!("expected re-announced FunctionCall, got {event:?}");
    };
    assert_eq!(function_name, "ext");
    let event = restored
        .resume(ResumeValue::Return(MontyObject::int(2)), &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(42));
    restored.finish().await.unwrap();
}

/// Zero is the explicit line-buffering sentinel, so a positive interval must
/// never round down into it. 100 prints would arrive as 100 events if the
/// sub-millisecond interval had truncated to zero; any batching at all proves
/// the timer is still on.
#[tokio::test]
async fn a_sub_millisecond_interval_does_not_become_line_buffering() {
    let pool = Pool::new(config()).await.unwrap();
    let repl = ReplConfig {
        print_flush_interval: Some(Duration::from_micros(400)),
        ..ReplConfig::default()
    };
    let mut session = pool.checkout(&repl).await.unwrap();
    let mut events = 0usize;
    session
        .feed(
            "for i in range(100):\n    print(i)",
            vec![],
            vec![],
            false,
            &mut on_print_sync(|_, _: &str| events += 1),
        )
        .await
        .unwrap();
    assert!(events < 100, "expected batching, got one event per line ({events})");
    session.finish().await.unwrap();
}

/// A `Load` restores the repl without materializing the checkout's stored
/// `Configure`, so the print flush interval has to be adopted when that config
/// arrives — otherwise a restored session silently falls back to the default
/// and ignores the line buffering its host asked for.
#[tokio::test]
async fn a_restored_session_honors_its_checkout_flush_interval() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    session
        .feed("base = 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    let state = session.dump().await.unwrap();
    drop(session);

    let repl = ReplConfig {
        print_flush_interval: Some(Duration::ZERO),
        ..ReplConfig::default()
    };
    let mut restored = pool.checkout(&repl).await.unwrap();
    restored.restore(state, vec![], &mut no_print).await.unwrap();
    let mut lines = Vec::new();
    restored
        .feed(
            "for i in range(20):\n    print(base + i)",
            vec![],
            vec![],
            false,
            &mut on_print_sync(|_, text: &str| lines.push(text.to_owned())),
        )
        .await
        .unwrap();
    assert_eq!(lines.len(), 20);
    assert_eq!(lines[0], "1\n");
    assert_eq!(lines[19], "20\n");
    restored.finish().await.unwrap();
}

// =============================================================================
// Environment isolation
// =============================================================================

/// Workers must be spawned with an empty environment: host secrets must never
/// be in a worker's memory, where a sandbox escape or memory disclosure could
/// reach them. Linux-only because it observes the child via /proc.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn worker_environment_is_empty() {
    // The test process itself always carries variables (PATH, CARGO, ...),
    // so an empty child environ proves nothing was inherited.
    assert!(env::var("PATH").is_ok(), "test process should have PATH set");

    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).await.unwrap();
    let environ = fs::read(format!("/proc/{}/environ", session.pid().unwrap())).unwrap();
    assert!(
        environ.is_empty(),
        "worker environment should be empty, got: {}",
        String::from_utf8_lossy(&environ).replace('\0', " ")
    );

    // The worker is fully functional without an environment.
    let event = session
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(2));
    session.finish().await.unwrap();
}

/// The per-feed budget restarts at each feed, so a session survives any number
/// of short feeds and the worker is never killed — the sandbox raises
/// `TimeoutError` well inside `feed_duration_limit_grace`.
#[tokio::test]
async fn max_feed_duration_bounds_each_feed_without_killing_the_worker() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    for _ in 0..3 {
        let event = session
            .feed("1 + 1", vec![], vec![], false, &mut no_print)
            .await
            .unwrap();
        assert_eq!(expect_complete(event), MontyObject::int(2));
    }
    let err = session
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.exc_type().to_string(), "TimeoutError");
    // The session survived, so the budget really did restart.
    let event = session
        .feed("2 + 2", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(4));
    session.finish().await.unwrap();
    assert_eq!(pool.idle_workers(), 1);
}

/// The per-turn budget is enforced in the sandbox too, and likewise leaves the
/// session usable.
#[tokio::test]
async fn max_turn_duration_bounds_a_runaway_turn() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_turn_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    let err = session
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.exc_type().to_string(), "TimeoutError");
    session.finish().await.unwrap();
    assert_eq!(pool.idle_workers(), 1);
}

/// Budgets near `Duration::MAX` must not overflow the parent's backstop
/// arithmetic (remaining budget plus grace), in either scope.
#[tokio::test]
async fn huge_feed_and_turn_budgets_do_not_overflow_the_backstop() {
    let pool = Pool::new(config()).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(
                ResourceLimits::default()
                    .max_feed_duration(Duration::MAX)
                    .max_turn_duration(Duration::MAX),
            ),
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    let event = session
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(2));
}

/// A worker that reports less feed time than it already reported cannot rewind
/// the parent's feed backstop.
///
/// The stand-in spends the whole 60s budget on its first suspension, then
/// claims zero on the second and goes quiet. The ratchet keeps the spent
/// figure, so the next turn is armed with the grace alone — without it the
/// parent would hand a hostile worker the budget back, every turn, for free.
#[cfg(unix)]
#[tokio::test]
async fn a_rewound_feed_clock_cannot_loosen_the_feed_backstop() {
    let dir = tempfile::tempdir().unwrap();
    let suspension = |feed_execution_micros| pb::ChildEvent {
        feed_execution_micros,
        ..child_event(pb::child_event::Kind::NameLookup(pb::NameLookup {
            name: "x".to_owned(),
            object_id: None,
        }))
    };
    // `Ok` answers Configure, then the honest suspension and the rewound one.
    // Nothing answers the third request: that is the turn the backstop must end.
    let mut replies = framed(&child_event(pb::child_event::Kind::Ok(pb::Ok {})));
    replies.extend(framed(&suspension(60_000_000)));
    replies.extend(framed(&suspension(0)));
    let replies_path = dir.path().join("replies.bin");
    fs::write(&replies_path, &replies).unwrap();
    let fake = write_fake_monty(
        dir.path(),
        &format!("#!/bin/sh\ncat '{}'\nsleep 30\n", replies_path.display()),
    );

    let grace = Duration::from_millis(100);
    let mut config = PoolConfig::subprocess(&fake);
    config.feed_duration_limit_grace = Some(grace);
    // The feed backstop must be what fires, not a blanket per-turn deadline.
    config.request_timeout = None;
    let pool = Pool::new(config).await.unwrap();
    let mut checkout = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_secs(60))),
            ..ReplConfig::default()
        })
        .await
        .expect("the stand-in answers Configure with Ok");

    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "x".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
        })),
        ..pb::ParentRequest::default()
    };
    let resume = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::ResumeNameLookup(pb::ResumeNameLookup {
            kind: Some(pb::resume_name_lookup::Kind::Undefined(pb::Unit {})),
            values: None,
        })),
        ..pb::ParentRequest::default()
    };
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    checkout.turn_raw(&feed, &mut on_event).await.unwrap();
    checkout.turn_raw(&resume, &mut on_event).await.unwrap();

    // Bounded so a regression fails here rather than waiting out the 60s
    // budget the rewind would have restored.
    let outcome = timeout(Duration::from_secs(5), checkout.turn_raw(&resume, &mut on_event)).await;
    let Ok(Err(PoolError::Timeout { timeout })) = outcome else {
        panic!("the spent feed budget must still backstop the next turn, got {outcome:?}");
    };
    assert_eq!(timeout, grace);
}

/// A second raw `Feed` restarts the parent's feed clock, as `Checkout::feed`
/// does — the previous feed's total must not shorten the new feed's backstop.
///
/// The deadline is read off the `Timeout` error rather than timed, so the
/// assertion is exact: `begin_feed` gives the whole budget back, and without
/// it the second feed would be armed with the grace alone.
#[cfg(unix)]
#[tokio::test]
async fn a_raw_feed_restarts_the_parent_feed_clock() {
    let dir = tempfile::tempdir().unwrap();
    let budget = Duration::from_millis(300);
    let grace = Duration::from_millis(100);
    // `Ok` answers Configure; the suspension ends the first feed having spent
    // the whole budget. The second feed goes unanswered.
    let mut replies = framed(&child_event(pb::child_event::Kind::Ok(pb::Ok {})));
    replies.extend(framed(&pb::ChildEvent {
        feed_execution_micros: u64::try_from(budget.as_micros()).unwrap(),
        ..child_event(pb::child_event::Kind::NameLookup(pb::NameLookup {
            name: "x".to_owned(),
            object_id: None,
        }))
    }));
    let replies_path = dir.path().join("replies.bin");
    fs::write(&replies_path, &replies).unwrap();
    let fake = write_fake_monty(
        dir.path(),
        &format!("#!/bin/sh\ncat '{}'\nsleep 30\n", replies_path.display()),
    );

    let mut config = PoolConfig::subprocess(&fake);
    config.feed_duration_limit_grace = Some(grace);
    config.request_timeout = None;
    let pool = Pool::new(config).await.unwrap();
    let mut checkout = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_feed_duration(budget)),
            ..ReplConfig::default()
        })
        .await
        .expect("the stand-in answers Configure with Ok");

    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "x".to_owned(),
            inputs: vec![].into(),
            values: None,
            skip_type_check: false,
            cwd: "/".to_owned(),
        })),
        ..pb::ParentRequest::default()
    };
    let mut on_event = |_: &pb::ChildEvent| Box::pin(ready(())) as PrintFuture;
    checkout.turn_raw(&feed, &mut on_event).await.unwrap();
    let err = checkout
        .turn_raw(&feed, &mut on_event)
        .await
        .expect_err("the stand-in never answers the second feed");
    let PoolError::Timeout { timeout } = err else {
        panic!("expected the feed backstop to fire, got {err:?}");
    };
    assert_eq!(timeout, budget + grace);
}

/// With the grace turned off the parent does not backstop that budget at all,
/// so the sandbox's own `TimeoutError` is what ends the feed — the worker must
/// still come back alive.
#[tokio::test]
async fn a_disabled_grace_leaves_the_sandbox_limit_in_charge() {
    let mut pool_config = config();
    pool_config.feed_duration_limit_grace = None;
    pool_config.turn_duration_limit_grace = None;
    let pool = Pool::new(pool_config).await.unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::default().max_feed_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .await
        .unwrap();
    let err = session
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.exc_type().to_string(), "TimeoutError");
    session.finish().await.unwrap();
    assert_eq!(pool.idle_workers(), 1);
}
