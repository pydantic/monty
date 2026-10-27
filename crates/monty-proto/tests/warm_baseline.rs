//! Memory accounting for a warm type checker, measured by the worker's own allocator.
//!
//! A test binary of its own: it installs [`LimitedAllocator`] so `LIVE_MEMORY` and
//! `BASELINE_MEMORY` move as they do in a worker, and keeps to one test so no other
//! thread allocates while it reads them.

use std::sync::atomic::Ordering;

use monty_alloc::LimitedAllocator;
use monty_proto::{
    FrameReader, PROTOCOL_VERSION, pb,
    worker::{Child, HandleOutcome, dispatch_frame},
    write_frame,
};
use monty_types::{BASELINE_MEMORY, MONTY_VERSION, memory_limit_with_headroom};

#[global_allocator]
static ALLOCATOR: LimitedAllocator = LimitedAllocator;

/// Left allocated by each session, below the warm checker's ~6 MiB so `fetch_min` alone would keep it.
const RESIDUE: usize = 3 * 1024 * 1024;

/// Security-relevant: memory a type-checked session leaves behind must count against the
/// next session's headroom, so a leaking worker still outgrows it and is replaced.
/// Dropping the consumed warm checker must not let that residue into the baseline.
#[test]
fn consumed_warm_checker_keeps_residue_out_of_the_baseline() {
    let mut child = Child::with_prewarmed_type_checker();
    // the order `monty subprocess` uses: warm up, then arm after every request
    child.warm_up();
    arm(&child);
    let mut residue = Vec::new();
    for _ in 0..3 {
        let before = BASELINE_MEMORY.load(Ordering::Relaxed);
        request(
            &mut child,
            pb::parent_request::Kind::Configure(pb::Configure {
                script_name: "main.py".to_owned(),
                type_check: true,
                monty_version: MONTY_VERSION.to_owned(),
                protocol_version: PROTOCOL_VERSION,
                ..Default::default()
            }),
        );
        let fed = request(
            &mut child,
            pb::parent_request::Kind::Feed(pb::Feed {
                code: "x: int = 1".to_owned(),
                cwd: "/".to_owned(),
                ..Default::default()
            }),
        );
        assert!(matches!(fed, pb::child_event::Kind::Complete(_)), "{fed:?}");
        residue.push(vec![1u8; RESIDUE]);
        request(&mut child, pb::parent_request::Kind::Reset(pb::Reset {}));
        child.warm_up();

        let after = BASELINE_MEMORY.load(Ordering::Relaxed);
        assert!(
            after.abs_diff(before) < RESIDUE / 3,
            "baseline moved from {before} to {after} bytes across one checkout"
        );
    }
}

/// Sends one request and re-arms the hard limit, as the subprocess loop does,
/// returning the turn-ending event.
fn request(child: &mut Child, kind: pb::parent_request::Kind) -> pb::child_event::Kind {
    let mut frame = Vec::new();
    write_frame(
        &mut frame,
        &pb::ParentRequest {
            kind: Some(kind),
            trace_parent: None,
        },
    )
    .expect("framing a request never fails");
    let (events, outcome) = dispatch_frame(child, &frame);
    assert_eq!(outcome, HandleOutcome::Continue);
    arm(child);
    let mut reader = FrameReader::new(events.as_slice());
    let mut last = None;
    while let Some(event) = reader.read::<pb::ChildEvent>().expect("reply frames decode") {
        last = event.kind;
    }
    last.expect("a turn always ends with one event")
}

/// Applies the hard limit of the session the child holds, which also settles the baseline.
fn arm(child: &Child) {
    let budget = child.session_budget();
    monty_alloc::set_hard_limit(memory_limit_with_headroom(budget.max_memory, budget.type_check))
        .expect("LimitedAllocator is installed");
}
