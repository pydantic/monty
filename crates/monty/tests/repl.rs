//! Tests for stateful REPL execution with no replay.
//!
//! The REPL session keeps heap/global namespace state between snippets and executes
//! only the newly fed snippet each time.

use insta::assert_snapshot;
use monty::{
    ExtFunctionResult, MontyException, MontyObject, MontyRepl, NoLimitTracker, PrintWriter, ReplContinuationMode,
    ReplProgress, ReplStartError, ResourceTracker, detect_repl_continuation_mode,
};

#[test]
fn repl_executes_only_new_code() {
    let mut repl = MontyRepl::new("repl.py", NoLimitTracker);
    let init_output = feed_run_print(&mut repl, "counter = 0").unwrap();
    assert_eq!(init_output, MontyObject::None);

    // Execute a snippet that mutates state.
    let output = feed_run_print(&mut repl, "counter = counter + 1").unwrap();
    assert_eq!(output, MontyObject::None);

    // Feed only the read expression. If replay happened, we'd get 2 instead of 1.
    let output = feed_run_print(&mut repl, "counter").unwrap();
    assert_eq!(output, MontyObject::Int(1));
}

fn feed_run_print(repl: &mut MontyRepl<impl ResourceTracker>, code: &str) -> Result<MontyObject, MontyException> {
    repl.feed_run(code, vec![], PrintWriter::Stdout)
}

fn init_repl(code: &str) -> (MontyRepl<NoLimitTracker>, MontyObject) {
    let mut repl = MontyRepl::new("repl.py", NoLimitTracker);
    let output = feed_run_print(&mut repl, code).unwrap();
    (repl, output)
}

#[test]
fn repl_persists_state_and_definitions() {
    let (mut repl, _) = init_repl("x = 10");

    feed_run_print(&mut repl, "def add(v):\n    return x + v").unwrap();
    feed_run_print(&mut repl, "x = 20").unwrap();
    let output = feed_run_print(&mut repl, "add(22)").unwrap();
    assert_eq!(output, MontyObject::Int(42));
}

#[test]
fn repl_function_redefinition_uses_latest_definition() {
    let (mut repl, init_output) = init_repl("");
    assert_eq!(init_output, MontyObject::None);

    feed_run_print(&mut repl, "def f():\n    return 1").unwrap();
    assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(1));

    feed_run_print(&mut repl, "def f():\n    return 2").unwrap();
    assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(2));
}

#[test]
fn repl_nested_function_redefinition_updates_callers() {
    let (mut repl, init_output) = init_repl("");
    assert_eq!(init_output, MontyObject::None);

    feed_run_print(&mut repl, "def g():\n    return 10").unwrap();
    feed_run_print(&mut repl, "def f():\n    return g() + 1").unwrap();
    assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(11));

    feed_run_print(&mut repl, "def g():\n    return 41").unwrap();
    assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(42));
}

#[test]
fn repl_runtime_error_keeps_partial_state_consistent() {
    let (mut repl, init_output) = init_repl("");
    assert_eq!(init_output, MontyObject::None);

    let result = feed_run_print(&mut repl, "def f():\n    return 41\nx = 1\nraise RuntimeError('boom')");
    assert!(result.is_err(), "snippet should raise RuntimeError");

    // Definitions and assignments that happened before the exception should remain valid.
    assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(41));
    assert_eq!(feed_run_print(&mut repl, "x").unwrap(), MontyObject::Int(1));
}

#[test]
fn repl_heap_mutations_are_not_replayed() {
    let (mut repl, _) = init_repl("items = []");

    feed_run_print(&mut repl, "items.append(1)").unwrap();
    assert_eq!(
        feed_run_print(&mut repl, "items").unwrap(),
        MontyObject::List(vec![MontyObject::Int(1)])
    );

    feed_run_print(&mut repl, "items.append(2)").unwrap();
    assert_eq!(
        feed_run_print(&mut repl, "items").unwrap(),
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])
    );
}

#[test]
fn repl_detects_continuation_mode_for_common_cases() {
    assert_eq!(
        detect_repl_continuation_mode("value = 1\n"),
        ReplContinuationMode::Complete
    );
    assert_eq!(
        detect_repl_continuation_mode("if True:\n"),
        ReplContinuationMode::IncompleteBlock
    );
    assert_eq!(
        detect_repl_continuation_mode("[1,\n"),
        ReplContinuationMode::IncompleteImplicit
    );
}

#[test]
fn repl_tracebacks_use_incrementing_python_input_filenames() {
    let (mut repl, init_output) = init_repl("");
    assert_eq!(init_output, MontyObject::None);

    let first = feed_run_print(&mut repl, "missing_name").unwrap_err();
    let second = feed_run_print(&mut repl, "missing_name").unwrap_err();

    assert_eq!(first.traceback().len(), 1);
    assert_eq!(second.traceback().len(), 1);
    assert_eq!(first.traceback()[0].filename, "<python-input-0>");
    assert_eq!(second.traceback()[0].filename, "<python-input-1>");
}

#[test]
fn repl_cross_snippet_traceback_resolves_against_defining_source() {
    // Tracebacks for a function defined in snippet 0 and called in snippet 1
    // must resolve frame positions against the source of the snippet that
    // actually produced the `CodeRange`, not the source of the snippet that
    // triggered the exception. `CodeRange` stores raw byte offsets, so
    // indexing snippet 0's offsets into snippet 1's source would give wrong
    // line/column/preview-line data (or worse).
    let (mut repl, _) = init_repl("");

    feed_run_print(&mut repl, "def f():\n    raise ValueError('boom')").unwrap();
    let err = feed_run_print(&mut repl, "f()").unwrap_err();

    let tb = err.traceback();
    assert_eq!(tb.len(), 2, "expected call-site + raise-site frames");

    // Frame 0: the call site, snippet 1.
    assert_eq!(tb[0].filename, "<python-input-1>");
    assert_eq!(tb[0].start.line, 1);
    assert_eq!(tb[0].preview_line.as_deref(), Some("f()"));

    // Frame 1: the raise inside f(), defined in snippet 0.
    assert_eq!(tb[1].filename, "<python-input-0>");
    assert_eq!(tb[1].start.line, 2);
    assert_eq!(
        tb[1].preview_line.as_deref(),
        Some("    raise ValueError('boom')"),
        "preview line must come from the snippet that defined f, not the current snippet"
    );
}

#[test]
fn repl_dump_load_survives_between_snippets() {
    let (mut repl, _) = init_repl("total = 1");
    feed_run_print(&mut repl, "total = total + 1").unwrap();

    let bytes = repl.dump().unwrap();
    let mut loaded: MontyRepl<NoLimitTracker> = MontyRepl::load(&bytes).unwrap();

    feed_run_print(&mut loaded, "total = total * 21").unwrap();
    let output = feed_run_print(&mut loaded, "total").unwrap();
    assert_eq!(output, MontyObject::Int(42));
}

#[test]
fn repl_dump_load_preserves_heap_aliasing() {
    let (mut repl, _) = init_repl("a = []\nb = a");

    feed_run_print(&mut repl, "a.append(1)").unwrap();

    let bytes = repl.dump().unwrap();
    let mut loaded: MontyRepl<NoLimitTracker> = MontyRepl::load(&bytes).unwrap();

    feed_run_print(&mut loaded, "b.append(2)").unwrap();
    assert_eq!(
        feed_run_print(&mut loaded, "a").unwrap(),
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])
    );
    assert_eq!(
        feed_run_print(&mut loaded, "b").unwrap(),
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])
    );
}

#[test]
fn repl_start_external_call_resumes_to_updated_repl() {
    let (repl, init_output) = init_repl("");
    assert_eq!(init_output, MontyObject::None);

    // With LoadGlobalCallable, function calls go directly to FunctionCall
    let progress = repl.feed_start("ext_fn(41) + 1", vec![], PrintWriter::Stdout).unwrap();
    let call = progress.into_function_call().expect("expected function call");
    assert_eq!(call.function_name, "ext_fn");
    assert_eq!(call.args, vec![MontyObject::Int(41)]);

    let progress = call.resume(MontyObject::Int(41), PrintWriter::Stdout).unwrap();
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::Int(42));
    assert_eq!(feed_run_print(&mut repl, "x = 5").unwrap(), MontyObject::None);
    assert_eq!(feed_run_print(&mut repl, "x").unwrap(), MontyObject::Int(5));
}

#[test]
fn repl_progress_dump_load_roundtrip() {
    let (repl, _) = init_repl("");

    // With LoadGlobalCallable, ext_fn goes directly to FunctionCall
    let progress = repl.feed_start("ext_fn(20) + 22", vec![], PrintWriter::Stdout).unwrap();

    let bytes = progress.dump().unwrap();
    let loaded: ReplProgress<NoLimitTracker> = ReplProgress::load(&bytes).unwrap();

    let call = loaded.into_function_call().expect("expected function call");
    assert_eq!(call.args, vec![MontyObject::Int(20)]);

    let progress = call.resume(MontyObject::Int(20), PrintWriter::Stdout).unwrap();
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::Int(42));
    assert_eq!(feed_run_print(&mut repl, "z = 1").unwrap(), MontyObject::None);
    assert_eq!(feed_run_print(&mut repl, "z").unwrap(), MontyObject::Int(1));
}

#[test]
fn repl_start_run_pending_resolve_futures_roundtrip() {
    let (mut repl, _) = init_repl("");
    feed_run_print(
        &mut repl,
        r"
async def main():
    value = await foo()
    return value + 1
",
    )
    .unwrap();

    let progress = repl.feed_start("await main()", vec![], PrintWriter::Stdout).unwrap();
    // With LoadGlobalCallable, foo() goes directly to FunctionCall
    let call = progress.into_function_call().expect("expected function call");
    let call_id = call.call_id;

    let progress = call.resume_pending(PrintWriter::Stdout).unwrap();
    let bytes = progress.dump().unwrap();
    let loaded: ReplProgress<NoLimitTracker> = ReplProgress::load(&bytes).unwrap();
    let state = loaded.into_resolve_futures().expect("expected resolve futures");
    assert_eq!(state.pending_call_ids(), &[call_id]);

    let progress = state
        .resume(
            vec![(call_id, ExtFunctionResult::Return(MontyObject::Int(41)))],
            PrintWriter::Stdout,
        )
        .unwrap();
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::Int(42));
    assert_eq!(
        feed_run_print(&mut repl, "final_value = 42").unwrap(),
        MontyObject::None
    );
    assert_eq!(feed_run_print(&mut repl, "final_value").unwrap(), MontyObject::Int(42));
}

#[test]
fn repl_start_runtime_error_preserves_repl_state() {
    // Simulate an agent loop: create variables, then a later snippet raises.
    // The REPL must survive so subsequent snippets can access prior variables.
    let (repl, _) = init_repl("x = 10");

    // Snippet that sets a new variable then raises — returned via ReplStartError.
    let err = repl
        .feed_start("y = 20\nraise ValueError('boom')", vec![], PrintWriter::Stdout)
        .expect_err("expected ReplStartError");
    let ReplStartError { mut repl, error } = *err;
    assert_eq!(error.exc_type(), monty::ExcType::ValueError);
    assert_eq!(error.message(), Some("boom"));

    // Variables from BEFORE the error snippet survive.
    assert_eq!(feed_run_print(&mut repl, "x").unwrap(), MontyObject::Int(10));
    // Variable assigned BEFORE the raise within the erroring snippet also survives.
    assert_eq!(feed_run_print(&mut repl, "y").unwrap(), MontyObject::Int(20));
    // New snippets continue to work normally.
    assert_eq!(feed_run_print(&mut repl, "x + y + 12").unwrap(), MontyObject::Int(42));
}

#[test]
fn repl_start_runtime_error_during_external_call_preserves_repl_state() {
    // An external function returns an error, which should come back as ReplStartError
    // with the REPL session preserved.
    let (repl, _) = init_repl("z = 99");

    let progress = repl.feed_start("ext_fn(1)", vec![], PrintWriter::Stdout).unwrap();
    let call = progress.into_function_call().expect("expected function call");

    // Resume with an exception from the external function.
    let exc = monty::MontyException::new(monty::ExcType::RuntimeError, Some("ext failed".to_string()));
    let err = call
        .resume(ExtFunctionResult::Error(exc), PrintWriter::Stdout)
        .expect_err("expected ReplStartError");
    let ReplStartError { mut repl, error } = *err;
    assert_eq!(error.exc_type(), monty::ExcType::RuntimeError);

    // Variable from before the error is still accessible.
    assert_eq!(feed_run_print(&mut repl, "z").unwrap(), MontyObject::Int(99));
}

#[test]
fn repl_dataclass_method_call_yields_function_call_with_method_flag() {
    // Create a REPL with a dataclass input and call a method on it.
    // This exercises the MethodCall path in repl.rs handle_repl_vm_result.
    let point = MontyObject::Dataclass {
        name: "Point".to_string(),
        type_id: 0,
        field_names: vec!["x".to_string(), "y".to_string()],
        attrs: vec![
            (MontyObject::String("x".to_string()), MontyObject::Int(1)),
            (MontyObject::String("y".to_string()), MontyObject::Int(2)),
        ]
        .into(),
        frozen: true,
    };

    let repl = MontyRepl::new("repl.py", NoLimitTracker);

    // Calling point.sum() should yield a FunctionCall with method_call=true.
    // Pass the dataclass as an input to feed_start() so it gets a namespace slot.
    let progress = repl
        .feed_start("point.sum()", vec![("point".to_string(), point)], PrintWriter::Stdout)
        .unwrap();
    let call = progress.into_function_call().expect("expected method call");

    assert_eq!(call.function_name, "sum");
    assert!(call.method_call, "should be a method call");
    // First arg should be the dataclass instance (self)
    assert!(matches!(&call.args[0], MontyObject::Dataclass { name, .. } if name == "Point"));

    // Resume with a return value (sum of x + y = 3)
    let progress = call.resume(MontyObject::Int(3), PrintWriter::Stdout).unwrap();
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::Int(3));

    // Verify REPL state is preserved after method call
    assert_eq!(feed_run_print(&mut repl, "1 + 1").unwrap(), MontyObject::Int(2));
}

#[test]
fn repl_start_new_external_function_in_later_block() {
    // Verify that an external function never referenced in prior blocks can be
    // called for the first time in a later REPL snippet.
    let (mut repl, _) = init_repl("x = 10");

    feed_run_print(&mut repl, "y = x + 5").unwrap();

    // Now call a brand-new external function that was never mentioned before.
    let progress = repl.feed_start("new_ext(y)", vec![], PrintWriter::Stdout).unwrap();
    let call = progress.into_function_call().expect("expected function call");
    assert_eq!(call.function_name, "new_ext");
    assert_eq!(call.args, vec![MontyObject::Int(15)]);

    let progress = call.resume(MontyObject::Int(100), PrintWriter::Stdout).unwrap();
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::Int(100));

    // REPL state from before the external call is still intact.
    assert_eq!(feed_run_print(&mut repl, "x").unwrap(), MontyObject::Int(10));
    assert_eq!(feed_run_print(&mut repl, "y").unwrap(), MontyObject::Int(15));
}

// ===========================================================================
// Function-call mode — calling Python functions from Rust
// ===========================================================================

/// Helper to create a REPL session pre-seeded with code for function calling.
fn repl_with_code(code: &str) -> MontyRepl<NoLimitTracker> {
    let mut repl = MontyRepl::new("session_test.py", NoLimitTracker);
    repl.feed_run(code, vec![], PrintWriter::Stdout).unwrap();
    repl
}

#[test]
fn call_simple_function() {
    let mut s = repl_with_code("def add(a, b): return a + b");
    let result = s
        .call_function(
            "add",
            vec![MontyObject::Int(2), MontyObject::Int(3)],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::Int(5));
}

#[test]
fn call_function_no_args() {
    let mut s = repl_with_code("def greet(): return 'hello'");
    let result = s.call_function("greet", vec![], PrintWriter::Stdout).unwrap();
    assert_eq!(result, MontyObject::String("hello".to_owned()));
}

#[test]
fn call_function_returns_none() {
    let mut s = repl_with_code("def noop(): pass");
    let result = s.call_function("noop", vec![], PrintWriter::Stdout).unwrap();
    assert_eq!(result, MontyObject::None);
}

#[test]
fn call_function_one_arg() {
    let mut s = repl_with_code("def double(x): return x * 2");
    let result = s
        .call_function("double", vec![MontyObject::Int(21)], PrintWriter::Stdout)
        .unwrap();
    assert_eq!(result, MontyObject::Int(42));
}

#[test]
fn call_function_string_args() {
    let mut s = repl_with_code("def concat(a, b): return a + b");
    let result = s
        .call_function(
            "concat",
            vec![
                MontyObject::String("hello ".to_owned()),
                MontyObject::String("world".to_owned()),
            ],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::String("hello world".to_owned()));
}

#[test]
fn call_function_multiple_times() {
    let mut s = repl_with_code("def inc(x): return x + 1");
    for i in 0..5 {
        let result = s
            .call_function("inc", vec![MontyObject::Int(i)], PrintWriter::Stdout)
            .unwrap();
        assert_eq!(result, MontyObject::Int(i + 1));
    }
}

#[test]
fn call_function_with_list() {
    let mut s = repl_with_code("def length(lst): return len(lst)");
    let result = s
        .call_function(
            "length",
            vec![MontyObject::List(vec![
                MontyObject::Int(1),
                MontyObject::Int(2),
                MontyObject::Int(3),
            ])],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::Int(3));
}

#[test]
fn call_function_retains_global_state() {
    let mut s = repl_with_code(
        "\
counter = 0
def increment():
    global counter
    counter = counter + 1
    return counter
",
    );
    assert_eq!(
        s.call_function("increment", vec![], PrintWriter::Stdout).unwrap(),
        MontyObject::Int(1)
    );
    assert_eq!(
        s.call_function("increment", vec![], PrintWriter::Stdout).unwrap(),
        MontyObject::Int(2)
    );
    assert_eq!(
        s.call_function("increment", vec![], PrintWriter::Stdout).unwrap(),
        MontyObject::Int(3)
    );
}

#[test]
fn call_function_multiple_functions() {
    let mut s = repl_with_code(
        "\
def add(a, b): return a + b
def mul(a, b): return a * b
",
    );
    assert_eq!(
        s.call_function(
            "add",
            vec![MontyObject::Int(3), MontyObject::Int(4)],
            PrintWriter::Stdout
        )
        .unwrap(),
        MontyObject::Int(7)
    );
    assert_eq!(
        s.call_function(
            "mul",
            vec![MontyObject::Int(3), MontyObject::Int(4)],
            PrintWriter::Stdout
        )
        .unwrap(),
        MontyObject::Int(12)
    );
}

#[test]
fn call_function_calls_other_function() {
    let mut s = repl_with_code(
        "\
def double(x): return x * 2
def quadruple(x): return double(double(x))
",
    );
    let result = s
        .call_function("quadruple", vec![MontyObject::Int(5)], PrintWriter::Stdout)
        .unwrap();
    assert_eq!(result, MontyObject::Int(20));
}

#[test]
fn call_function_with_defaults() {
    let mut s = repl_with_code("def greet(name, greeting='Hello'): return greeting + ' ' + name");
    let result = s
        .call_function(
            "greet",
            vec![MontyObject::String("world".to_owned())],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::String("Hello world".to_owned()));
}

#[test]
fn call_closure() {
    let mut s = repl_with_code(
        "\
def make_adder(n):
    def adder(x):
        return x + n
    return adder

add5 = make_adder(5)
",
    );
    let result = s
        .call_function("add5", vec![MontyObject::Int(10)], PrintWriter::Stdout)
        .unwrap();
    assert_eq!(result, MontyObject::Int(15));
}

#[test]
fn call_nonexistent_function() {
    let mut s = repl_with_code("def foo(): return 1");
    let err = s.call_function("bar", vec![], PrintWriter::Stdout).unwrap_err();
    assert_snapshot!(err, @"NameError: name 'bar' is not defined");
}

#[test]
fn call_non_callable() {
    let mut s = repl_with_code("x = 42");
    let err = s.call_function("x", vec![], PrintWriter::Stdout).unwrap_err();
    assert_snapshot!(err, @"TypeError: 'int' object is not callable");
}

#[test]
fn call_function_raises_exception() {
    let mut s = repl_with_code("def boom(): raise ValueError('kaboom')");
    let err = s.call_function("boom", vec![], PrintWriter::Stdout).unwrap_err();
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "<python-input-0>", line 1, in boom
        def boom(): raise ValueError('kaboom')
    ValueError: kaboom
    "#);
}

#[test]
fn call_function_wrong_arg_count() {
    let mut s = repl_with_code("def add(a, b): return a + b");
    let err = s
        .call_function("add", vec![MontyObject::Int(1)], PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "<python-input-0>", line 1, in <module>
        def add(a, b): return a + b
            ~~~
    TypeError: add() missing 1 required positional argument: 'b'
    "#);
}

#[test]
fn function_names() {
    let s = repl_with_code(
        "\
x = 42
def foo(): pass
def bar(): pass
",
    );
    let mut names = s.function_names();
    names.sort_unstable();
    assert_eq!(names, vec!["bar", "foo"]);
}

#[test]
fn has_function() {
    let s = repl_with_code("def my_func(): pass\nx = 10");
    assert!(s.has_function("my_func"));
    assert!(!s.has_function("x")); // not callable
    assert!(!s.has_function("nonexistent"));
}

#[test]
fn call_function_captures_print() {
    let mut s = repl_with_code("def say_hello(name): print('Hello ' + name)");
    let mut output = String::new();
    let result = s
        .call_function(
            "say_hello",
            vec![MontyObject::String("world".to_owned())],
            PrintWriter::CollectString(&mut output),
        )
        .unwrap();
    assert_eq!(result, MontyObject::None);
    assert_eq!(output, "Hello world\n");
}

#[test]
fn call_function_returns_list() {
    let mut s = repl_with_code("def make_list(n): return list(range(n))");
    let result = s
        .call_function("make_list", vec![MontyObject::Int(3)], PrintWriter::Stdout)
        .unwrap();
    assert_eq!(
        result,
        MontyObject::List(vec![MontyObject::Int(0), MontyObject::Int(1), MontyObject::Int(2)])
    );
}

#[test]
fn call_function_returns_dict() {
    let mut s = repl_with_code(
        "\
def make_point(x, y):
    return {'x': x, 'y': y}
",
    );
    let result = s
        .call_function(
            "make_point",
            vec![MontyObject::Int(1), MontyObject::Int(2)],
            PrintWriter::Stdout,
        )
        .unwrap();
    if let MontyObject::Dict(pairs) = result {
        assert_eq!(pairs.into_iter().count(), 2);
    } else {
        panic!("expected dict, got: {result:?}");
    }
}

#[test]
fn call_function_many_args() {
    let mut s = repl_with_code("def sum_all(a, b, c, d, e): return a + b + c + d + e");
    let result = s
        .call_function(
            "sum_all",
            vec![
                MontyObject::Int(1),
                MontyObject::Int(2),
                MontyObject::Int(3),
                MontyObject::Int(4),
                MontyObject::Int(5),
            ],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::Int(15));
}

#[test]
fn call_function_that_calls_undefined_name_fails() {
    let mut s = repl_with_code("def call_missing(): return unknown_func()");
    let err = s
        .call_function("call_missing", vec![], PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err, @"NotImplementedError: MontyRepl::call_function: external functions are not yet supported in this context");
}

#[test]
fn call_function_with_heap_defaults() {
    let mut s = repl_with_code("def greet(name, greeting='Hi'): return greeting + ' ' + name");
    let result = s
        .call_function(
            "greet",
            vec![MontyObject::String("Alice".to_owned())],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::String("Hi Alice".to_owned()));
}

#[test]
fn convert_args_single_repr_fails() {
    let mut s = repl_with_code("def identity(x): return x");
    let err = s
        .call_function(
            "identity",
            vec![MontyObject::Repr("bad".to_owned())],
            PrintWriter::Stdout,
        )
        .unwrap_err();
    assert_snapshot!(err, @"RuntimeError: invalid argument type: 'Repr' is not a valid input value");
}

#[test]
fn convert_args_two_second_repr_fails() {
    let mut s = repl_with_code("def add(a, b): return a + b");
    let err = s
        .call_function(
            "add",
            vec![MontyObject::Int(1), MontyObject::Repr("bad".to_owned())],
            PrintWriter::Stdout,
        )
        .unwrap_err();
    assert_snapshot!(err, @"RuntimeError: invalid argument type: 'Repr' is not a valid input value");
}

#[test]
fn convert_args_two_first_repr_fails() {
    let mut s = repl_with_code("def add(a, b): return a + b");
    let err = s
        .call_function(
            "add",
            vec![MontyObject::Repr("bad".to_owned()), MontyObject::Int(1)],
            PrintWriter::Stdout,
        )
        .unwrap_err();
    assert_snapshot!(err, @"RuntimeError: invalid argument type: 'Repr' is not a valid input value");
}

#[test]
fn convert_args_many_middle_repr_fails() {
    let mut s = repl_with_code("def f(a, b, c, d): return a");
    let err = s
        .call_function(
            "f",
            vec![
                MontyObject::Int(1),
                MontyObject::Int(2),
                MontyObject::Repr("bad".to_owned()),
                MontyObject::Int(4),
            ],
            PrintWriter::Stdout,
        )
        .unwrap_err();
    assert_snapshot!(err, @"RuntimeError: invalid argument type: 'Repr' is not a valid input value");
}

#[test]
fn call_builtin_via_session() {
    let mut s = repl_with_code("my_len = len");
    let result = s
        .call_function(
            "my_len",
            vec![MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::Int(2));
}
