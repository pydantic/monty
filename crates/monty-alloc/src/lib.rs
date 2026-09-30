#![doc = include_str!("../README.md")]
#![expect(unsafe_code, reason = "Custom allocator is unsafe, the logic we add is all safe")]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    fmt,
    io::{self, Write},
    process,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[cfg(feature = "exit-code")]
use monty_types::OOM_EXIT_CODE;
use monty_types::{BASELINE_MEMORY, LIVE_MEMORY};

/// The absolute ceiling for allocator-backed live bytes.
/// Counting starts with the process: a counter armed later would see `dealloc`s
/// it never charged and underflow.
static HARD_LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Applies a worker's hard memory budget relative to its baseline.
///
/// Callers choose any headroom above the interpreter's soft limit before
/// calling this function. Re-apply it after every request, since a session can
/// also arrive through a restored dump or end through a reset.
///
/// On a 32-bit target (wasm) a budget near 4 GiB saturates the arithmetic and
/// leaves the worker uncapped — there is no cap to express.
pub fn set_hard_limit(memory_budget: Option<usize>) -> Result<(), &'static str> {
    let live = LIVE_MEMORY.load(Ordering::Relaxed);
    if live == 0 {
        return Err("monty-alloc is not installed as the global allocator");
    }
    // `fetch_min` both reads and lowers the baseline: the first arming, on a
    // pristine worker, sets it, and a later leaner moment can only improve it.
    let baseline = BASELINE_MEMORY.fetch_min(live, Ordering::Relaxed).min(live);
    let hard_limit = memory_budget.map_or(usize::MAX, |bytes| baseline.saturating_add(bytes));
    HARD_LIMIT.store(hard_limit, Ordering::Relaxed);
    Ok(())
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
        if let Some(size_change) = new_size.checked_sub(layout.size()) {
            charge(size_change);
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

/// Adds `size` to the live total, exiting past the hard limit.
#[inline]
fn charge(size: usize) {
    let live = LIVE_MEMORY.fetch_add(size, Ordering::Relaxed).saturating_add(size);
    if live > HARD_LIMIT.load(Ordering::Relaxed) {
        out_of_memory(format_args!(
            "monty worker: allocation of {size} bytes exceeds the memory limit"
        ));
    }
}

/// Returns `size` to the live total. `Relaxed` throughout: the count only has to
/// be eventually right, and no other memory is published through it.
#[inline]
fn refund(size: usize) {
    LIVE_MEMORY.fetch_sub(size, Ordering::Relaxed);
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
    HARD_LIMIT.store(usize::MAX, Ordering::Relaxed);
    if !REPORTING.swap(true, Ordering::Relaxed) {
        let _ = writeln!(io::stderr(), "{reason}");
    }
    #[cfg(feature = "exit-code")]
    process::exit(OOM_EXIT_CODE);
    #[cfg(not(feature = "exit-code"))]
    process::abort();
}
