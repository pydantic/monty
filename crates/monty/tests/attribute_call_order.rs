use monty::MontyRun;
#[cfg(not(feature = "memory-model-checks"))]
use monty::{Dump, MontyRepl, ReplProgress, Session, SessionRef, dump};
use monty_types::CompileOptions;
#[cfg(not(feature = "memory-model-checks"))]
use monty_types::{ExtFunctionResult, MontyObject, PrintWriter, ResourceTracker};

#[test]
fn attribute_lookup_precedes_arguments() {
    let mut run = MontyRun::new(
        include_str!("../test_cases/call__attribute_order.py").to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    run.run_no_limits(vec![]).unwrap();
}

#[test]
#[cfg(not(feature = "memory-model-checks"))]
fn prepared_method_survives_suspended_argument_snapshot() {
    for (setup, expression, expected, check) in [
        (
            "class C:\n def method(self, value): return value + 1\nx = C()",
            "x.method(host_call())",
            MontyObject::int(5),
            "assert x.method(1) == 2",
        ),
        (
            "xs = []",
            "xs.append(host_call())",
            MontyObject::none(),
            "assert xs == [4]",
        ),
    ] {
        let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
        repl.feed_run(setup, vec![], PrintWriter::Disabled).unwrap();
        let progress = repl.feed_start(expression, vec![], PrintWriter::Disabled).unwrap();
        let bytes = dump("test.py", None, SessionRef::Suspended(&progress)).unwrap();
        let Session::Suspended(progress) = Dump::load(&bytes).unwrap().state else {
            panic!("expected suspended argument call");
        };
        let ReplProgress::FunctionCall(call) = *progress else {
            panic!("expected function call");
        };
        let ReplProgress::Complete {
            repl: mut restored,
            value,
        } = call
            .resume(ExtFunctionResult::Return(MontyObject::int(4)), PrintWriter::Disabled)
            .unwrap()
        else {
            panic!("expected completion");
        };
        assert_eq!(value, expected);
        restored.feed_run(check, vec![], PrintWriter::Disabled).unwrap();
    }
}

#[test]
#[cfg(feature = "ref-count-return")]
fn builtin_call_preparation_does_not_allocate_per_call() {
    let allocations = |iterations| {
        let source = format!("xs = []\nfor _ in range({iterations}):\n xs.append(1)\n xs.pop()");
        let output = MontyRun::new(source, "test.py", vec![], CompileOptions::default())
            .unwrap()
            .run_ref_counts(vec![])
            .unwrap();
        assert!(output.unreachable.is_empty(), "{:?}", output.unreachable);
        output.allocations_since_gc
    };
    assert_eq!(allocations(1), allocations(50));
}
