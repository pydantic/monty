//! Verifies that the buffered file I/O paths correctly account memory
//! against `ResourceLimits::max_memory`.
//!
//! The full-file buffer is a heap entry like any other, so its allocation
//! flows through `Heap::allocate` and bumps `current_memory()`. These tests
//! pin that contract: a buffer larger than `max_memory` is rejected, a
//! successful read raises `current_memory()` by roughly the file size, and
//! `close()` drops it back down.

use monty::{
    CompileOptions, ExcType, FileMode, LimitedTracker, MontyFileHandle, MontyObject, MontyRun, PrintWriter,
    ResourceLimits,
};

fn file_handle(path: &str, mode: &str) -> MontyFileHandle {
    MontyFileHandle {
        path: path.to_owned(),
        mode: mode.parse::<FileMode>().unwrap(),
        position: 0,
    }
}

/// Drives `open()` → `read()` against a fabricated file body, returning the
/// `(memory_after_load, final_complete)` pair so tests can assert both that
/// the buffer counted against `current_memory()` *and* the final program
/// result.
///
/// `tracker_factory` lets each test configure its own limits.
fn open_then_read(
    code: &str,
    expected_io_fn_name: &str,
    handle: MontyFileHandle,
    io_result: MontyObject,
    limits: ResourceLimits,
) -> Result<(usize, MontyObject), monty::MontyException> {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], LimitedTracker::new(limits), PrintWriter::Stdout)
        .unwrap();
    let open_call = progress.into_os_call().expect("expected Open OsCall");
    assert_eq!(open_call.function_call.name(), "Open");
    let progress = open_call
        .resume(MontyObject::FileHandle(handle), PrintWriter::Stdout)
        .unwrap();
    let io_call = progress.into_os_call().expect("expected read OsCall");
    assert_eq!(io_call.function_call.name(), expected_io_fn_name);
    let progress = io_call.resume(io_result, PrintWriter::Stdout)?;
    // The next pause is the snapshot point at which we want the tracker
    // reading — typically a follow-up OS call inserted by the test to
    // observe state. Tests that complete immediately use `into_complete`.
    match progress {
        monty::RunProgress::OsCall(next_call) => {
            let mem = next_call.tracker().current_memory();
            // Resume the follow-up call so the runner can finish — tests
            // typically use a no-op `Getenv` for this purpose.
            let progress = next_call
                .resume(MontyObject::String(String::new()), PrintWriter::Stdout)
                .unwrap();
            let complete = progress.into_complete().expect("expected Complete");
            Ok((mem, complete))
        }
        monty::RunProgress::Complete(value) => Ok((0, value)),
        _ => panic!("unexpected progress variant"),
    }
}

// ---------------------------------------------------------------------------
// A buffer that would exceed `max_memory` must raise `MemoryError` — the
// buffer goes through `heap.allocate`, which is the same tracking path used
// by every other heap entry.
// ---------------------------------------------------------------------------

#[test]
fn buffered_read_respects_max_memory() {
    // A 10KB file body with `max_memory` set well below it. The `heap.allocate`
    // inside `to_value` (for the host's `MontyObject::String` return) is
    // what must trip the limit.
    let code = r"
f = open('/big.txt')
f.read(5)
";
    let body = "x".repeat(10_000);
    let limits = ResourceLimits::new().max_memory(1024);
    let result = open_then_read(
        code,
        "Path.read_text",
        file_handle("/big.txt", "r"),
        MontyObject::String(body),
        limits,
    );
    let exc = result.expect_err("read should fail due to max_memory");
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

// ---------------------------------------------------------------------------
// A successful buffered read must visibly raise `current_memory()` — the
// buffer is counted against the same pool every other heap entry uses.
// We sample the tracker at a second OS call (Getenv) inserted after the
// read so the buffer is still held.
// ---------------------------------------------------------------------------

#[test]
fn buffered_read_bumps_current_memory() {
    let code = r"
import os
f = open('/data.txt')
f.read(5)
# Trigger a Getenv OS call so the test can sample `current_memory()`
# while the file (and its buffer) are still alive.
os.getenv('PROBE')
";
    let body = "abcdefghijklmnopqrstuvwxyz".repeat(100); // 2600 bytes
    let limits = ResourceLimits::new().max_memory(1_000_000);
    let (mem_after_read, _) = open_then_read(
        code,
        "Path.read_text",
        file_handle("/data.txt", "r"),
        MontyObject::String(body),
        limits,
    )
    .expect("should succeed");
    // The buffer alone is 2600 bytes; total tracked memory will be a bit
    // higher (open-file struct, path, sliced result). Assert a generous
    // lower bound to keep the test stable across `py_estimate_size` tweaks.
    assert!(
        mem_after_read >= 2600,
        "expected at least 2600 bytes tracked, got {mem_after_read}",
    );
}

// ---------------------------------------------------------------------------
// `close()` releases the buffer, so `current_memory()` drops back near the
// pre-load baseline. Without the explicit release this test would still
// see ~buffer_size bytes pinned.
// ---------------------------------------------------------------------------

#[test]
fn close_releases_buffer_memory() {
    let code = r"
import os
f = open('/data.txt')
f.read(5)
f.close()
os.getenv('PROBE')
";
    let body = "abcdefghijklmnopqrstuvwxyz".repeat(100); // 2600 bytes
    let limits = ResourceLimits::new().max_memory(1_000_000);
    let (mem_after_close, _) = open_then_read(
        code,
        "Path.read_text",
        file_handle("/data.txt", "r"),
        MontyObject::String(body),
        limits,
    )
    .expect("should succeed");
    // After close the buffer should be freed. The closed-file wrapper and
    // the `f.read(5)` slice still live; both are << 1000 bytes. Setting
    // the upper bound at 1500 leaves slack for incidental tracking
    // (interpreter state) without re-admitting the 2600-byte buffer.
    assert!(
        mem_after_close < 1500,
        "expected buffer to be released, but {mem_after_close} bytes still tracked",
    );
}

// ---------------------------------------------------------------------------
// A file read but never `close()`d must still release its buffer once the
// `OpenFile` becomes unreachable (`f = 0`). Regression for two coupled refcount
// bugs in the buffered-read path: the OS-call pin over-counted the file so it
// was never freed on `f = 0`, and the free walker (`py_dec_ref_ids_for_data`)
// had no `OpenFile` arm, so even once freed the buffer was not released. Both
// must be fixed for `current_memory()` to drop back to baseline here.
// ---------------------------------------------------------------------------

#[test]
fn dropping_unclosed_file_releases_buffer() {
    let code = r"
import os
f = open('/data.txt')
f.read(5)
f = 0            # OpenFile becomes unreachable WITHOUT close(); refcount -> 0
os.getenv('PROBE')
";
    let body = "abcdefghijklmnopqrstuvwxyz".repeat(100); // 2600 bytes
    let limits = ResourceLimits::new().max_memory(1_000_000);
    let (mem, _) = open_then_read(
        code,
        "Path.read_text",
        file_handle("/data.txt", "r"),
        MontyObject::String(body),
        limits,
    )
    .expect("should succeed");
    assert!(
        mem < 1500,
        "OpenFile buffer leaked: {mem} bytes still tracked after the unclosed file was freed",
    );
}
