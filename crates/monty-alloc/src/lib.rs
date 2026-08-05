#![doc = include_str!("../README.md")]
#![expect(unsafe_code, reason = "Custom allocator is unsafe, the logic we add is all safe")]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    fmt,
    io::{self, Write},
    process,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// Bytes currently charged, and the limit they may not exceed (`usize::MAX`
/// until [`set_limit`] lowers it). Counting starts with the process: a counter
/// armed later would see `dealloc`s it never charged and underflow.
static LIVE: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);

/// The leanest the process has ever been at an arming point: what the worker
/// costs to exist, before any session ran. Deriving each limit from the
/// *current* live total instead would let memory retained between sessions
/// ratchet it up checkout after checkout.
static BASELINE: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Room above the limit for machinery the session did not ask for and cannot
/// see: buffers the worker needs to serve it at all, and the type checker's
/// stubs and caches. Both are generous, since too tight a cap kills healthy
/// workers, which is far worse than a limit that binds a little late.
const BASE_HEADROOM: usize = 4 * 1024 * 1024;
const TYPE_CHECK_HEADROOM: usize = 32 * 1024 * 1024;

/// Applies a session's memory limit, or lifts it while no session has one.
/// Call it after every request, since a session can also arrive (or end)
/// through a restored dump or a reset; the result depends only on the limit and
/// the baseline, so re-applying mid-session is a no-op.
///
/// On a 32-bit target (wasm) a limit near 4 GiB saturates the arithmetic and
/// leaves the worker uncapped — there is no cap to express.
// TODO: update once there's one memory-limit implementation. The interpreter
// still meters allocations itself, so a session's limit is enforced twice and
// whichever binds first decides how exceeding it surfaces.
pub fn set_limit(max_memory: Option<u64>, type_check: bool) {
    let live = LIVE.load(Ordering::Relaxed);
    // `fetch_min` both reads and lowers the baseline: the first arming, on a
    // pristine worker, sets it, and a later leaner moment can only improve it.
    let baseline = BASELINE.fetch_min(live, Ordering::Relaxed).min(live);
    let limit = match max_memory {
        Some(bytes) => {
            let headroom = if type_check { TYPE_CHECK_HEADROOM } else { BASE_HEADROOM };
            baseline
                .saturating_add(usize::try_from(bytes).unwrap_or(usize::MAX))
                .saturating_add(headroom)
        }
        None => usize::MAX,
    };
    LIMIT.store(limit, Ordering::Relaxed);
}

/// The system allocator, plus the live-byte count that enforces the memory
/// limit and a null check that ends the process deliberately.
pub struct LimitedAllocator;

// SAFETY: every method forwards its arguments unchanged to `System` and returns
// what `System` returned (or diverges). No pointer is fabricated, aliased or
// freed here, so this upholds exactly the invariants `System` upholds.
unsafe impl GlobalAlloc for LimitedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        charge(layout.size());
        // SAFETY: `layout` comes from the caller and is forwarded unchanged.
        let ptr = unsafe { System.alloc(layout) };
        if ptr.is_null() {
            out_of_memory(format_args!(
                "monty worker: allocation of {} bytes failed",
                layout.size()
            ));
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        refund(layout.size());
        // SAFETY: `ptr` came from our `alloc`/`realloc` with this same `layout`.
        unsafe { System.dealloc(ptr, layout) };
    }

    // Overridden rather than left to the default (which routes through `alloc`)
    // so `System` keeps using calloc's pre-zeroed pages.
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        charge(layout.size());
        // SAFETY: `layout` comes from the caller and is forwarded unchanged.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if ptr.is_null() {
            out_of_memory(format_args!(
                "monty worker: allocation of {} bytes failed",
                layout.size()
            ));
        }
        ptr
    }

    // Overridden for the same reason: the default reallocates and copies, while
    // `System` can often grow a block in place.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= layout.size() {
            charge(new_size - layout.size());
        } else {
            refund(layout.size() - new_size);
        }
        // SAFETY: `ptr`/`layout` describe a live block from this allocator, and
        // `new_size` is the caller's — all forwarded unchanged.
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if new_ptr.is_null() {
            out_of_memory(format_args!("monty worker: allocation of {new_size} bytes failed"));
        }
        new_ptr
    }
}

/// Adds `size` to the live total, exiting if that exceeds the limit. Charges
/// *before* allocating, so the allocation is refused rather than committed; the
/// overcount left when an allocation then fails is moot, as both paths end here.
#[inline]
fn charge(size: usize) {
    if LIVE.fetch_add(size, Ordering::Relaxed).saturating_add(size) > LIMIT.load(Ordering::Relaxed) {
        out_of_memory(format_args!(
            "monty worker: allocation of {size} bytes exceeds the memory limit"
        ));
    }
}

/// Returns `size` to the live total. `Relaxed` throughout: the count only has to
/// be eventually right, and no other memory is published through it.
#[inline]
fn refund(size: usize) {
    LIVE.fetch_sub(size, Ordering::Relaxed);
}

/// Reports why memory ran out and ends the process — never by panicking, whose
/// machinery allocates. How it ends is the `exit-code` feature's choice; see the
/// crate docs. Skipping destructors can leave a partial frame on the transport,
/// which the host already treats as a dead worker.
#[cold]
#[inline(never)]
fn out_of_memory(reason: fmt::Arguments<'_>) -> ! {
    // A genuinely exhausted host can fail the write and re-enter: let the first
    // caller write and send any re-entrant one straight to the end.
    static REPORTING: AtomicBool = AtomicBool::new(false);
    // Lift the limit first — writing to stderr allocates (the handle's lock),
    // which under an exceeded limit would re-enter and be silenced below,
    // losing the message. Safe because this path never returns.
    LIMIT.store(usize::MAX, Ordering::Relaxed);
    if !REPORTING.swap(true, Ordering::Relaxed) {
        let _ = writeln!(io::stderr(), "{reason}");
    }
    #[cfg(feature = "exit-code")]
    process::exit(monty_proto::OOM_EXIT_CODE);
    #[cfg(not(feature = "exit-code"))]
    process::abort();
}
