//! Allocator-requested memory costs; DHAT overhead, allocator metadata and RSS are excluded.

use dhat::{Alloc, HeapStats, Profiler};
use monty::{Dump, MontyRepl, RunProgress, SessionRef, dump};
use monty_bench::{heap_checkpoint, heap_graph_session, heap_suspend_resume, heap_suspension_program};
use monty_types::{PrintWriter, ResourceTracker};

#[global_allocator]
static ALLOCATOR: Alloc = Alloc;

/// Warms process caches before measuring each phase under a fresh allocation profile.
fn main() {
    let warm = heap_graph_session();
    let bytes = dump("bench.py", None, &[], SessionRef::Idle(&warm)).unwrap();
    drop(warm);
    let run = heap_suspension_program();
    drop(
        run.clone()
            .start(vec![], ResourceTracker::default(), PrintWriter::Disabled)
            .unwrap(),
    );
    println!("repl_stack_bytes\t{}", size_of::<MontyRepl>());
    println!("suspended_stack_bytes\t{}", size_of::<RunProgress>());
    println!("dump_bytes\t{}", bytes.len());
    println!("phase\tretained_bytes\tpeak_bytes\ttotal_bytes\tallocations\tremaining_bytes");
    measure("suspend", || {
        run.clone()
            .start(vec![], ResourceTracker::default(), PrintWriter::Disabled)
            .unwrap()
    });
    measure("suspend_resume", || heap_suspend_resume(&run));
    measure("build_graph", heap_graph_session);
    let graph = heap_graph_session();
    measure("dump_graph", || {
        dump("bench.py", None, &[], SessionRef::Idle(&graph)).unwrap()
    });
    drop(graph);
    measure("load_graph_including_input", || {
        let input = bytes.clone();
        let loaded = Dump::load(&input).unwrap();
        (input, loaded)
    });
    for passes in [0, 1, 10, 100] {
        let code = format!(
            "total = 0\nfor _ in range({passes}):\n    for i in range(5_000):\n        total += items[i][0]\ntotal"
        );
        measure(&format!("checkpoint_{passes}_including_input"), || {
            let input = bytes.clone();
            let output = heap_checkpoint(&input, &code);
            (input, output)
        });
    }
}

/// Measures through the returned value's destruction, reporting retained and transient allocations separately.
fn measure<R>(name: &str, f: impl FnOnce() -> R) {
    let profiler = Profiler::builder().testing().build();
    let value = f();
    let retained = HeapStats::get().curr_bytes;
    drop(value);
    let stats = HeapStats::get();
    drop(profiler);
    println!(
        "{name}\t{retained}\t{}\t{}\t{}\t{}",
        stats.max_bytes, stats.total_bytes, stats.total_blocks, stats.curr_bytes
    );
}
