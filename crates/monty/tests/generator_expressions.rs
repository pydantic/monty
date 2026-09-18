//! Monty-specific generator-expression behavior which cannot share CPython fixtures.

use insta::assert_snapshot;
use monty::{MontyRun, RunProgress};
use monty_types::{CompileOptions, ExcType, ExtFunctionResult, MontyObject, MontyType, PrintWriter, ResourceTracker};

/// Runs `code` and returns its rendered exception.
fn run_error(code: &str) -> monty_types::MontyException {
    MontyRun::new(code.to_owned(), "generator.py", vec![], CompileOptions::default())
        .expect("generator source should compile")
        .run_no_limits(vec![])
        .expect_err("generator source should fail")
}

#[test]
fn generator_crosses_the_host_boundary_as_repr() {
    let result = MontyRun::new(
        "(x for x in [1])".to_owned(),
        "generator.py",
        vec![],
        CompileOptions::default(),
    )
    .expect("generator source should compile")
    .run_no_limits(vec![])
    .expect("generator creation should succeed");
    assert_eq!(result, MontyObject::repr("<generator object <genexpr>>".to_owned()));
}

#[test]
fn generator_type_crosses_the_host_boundary_exactly() {
    let result = MontyRun::new(
        "type(x for x in [1])".to_owned(),
        "generator.py",
        vec![],
        CompileOptions::default(),
    )
    .expect("generator source should compile")
    .run_no_limits(vec![])
    .expect("generator type lookup should succeed");
    assert_eq!(result, MontyObject::type_object(MontyType::Generator));
}

#[test]
fn pep_479_uses_a_merged_traceback_without_cause_chaining() {
    let error =
        run_error("def stop():\n    raise StopIteration('bad')\n\ngenerator = (stop() for _ in [0])\nnext(generator)");
    assert_eq!(error.exc_type(), ExcType::RuntimeError);
    assert_snapshot!(error.to_string(), @r#"
    Traceback (most recent call last):
      File "generator.py", line 5, in <module>
        next(generator)
        ~~~~~~~~~~~~~~~
      File "generator.py", line 4, in <genexpr>
        generator = (stop() for _ in [0])
                     ~~~~~~
      File "generator.py", line 2, in stop
        raise StopIteration('bad')
    RuntimeError: generator raised StopIteration
    "#);
}

#[test]
fn task_switching_retains_the_generator_frame() {
    let error = run_error(
        "import asyncio\nasync def child():\n    return 1\ngenerator = (asyncio.run(asyncio.gather(child())) for _ in [0])\nnext(generator)",
    );
    assert_eq!(error.exc_type(), ExcType::NotImplementedError);
    assert_snapshot!(error.to_string(), @r#"
    Traceback (most recent call last):
      File "generator.py", line 5, in <module>
        next(generator)
        ~~~~~~~~~~~~~~~
      File "generator.py", line 4, in <genexpr>
        generator = (asyncio.run(asyncio.gather(child())) for _ in [0])
                     ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: generator expression: resolving async futures is not yet supported in this context
    "#);
}

#[test]
fn rejected_task_switching_leaves_coroutines_unscheduled() {
    for body in [
        "asyncio.run(asyncio.gather(coro))",
        "asyncio.run(asyncio.gather(asyncio.gather(coro)))",
        "consume()",
        "asyncio.run(via_await())",
    ] {
        let code = format!(
            "\
import asyncio
calls = []
async def child():
    calls.append(1)
    return 42
coro = child()
def consume():
    return asyncio.run(asyncio.gather(coro))
async def via_await():
    return await asyncio.gather(coro)
generator = ({body} for _ in [0])
try:
    next(generator)
    assert False, 'expected NotImplementedError'
except NotImplementedError as exc:
    assert str(exc) == 'generator expression: resolving async futures is not yet supported in this context'
assert calls == []
assert next(generator, None) is None
assert asyncio.run(asyncio.gather(coro)) == [42]
assert calls == [1]
assert list(asyncio.run(child()) for _ in [0]) == [42]
assert list(asyncio.run(asyncio.gather()) for _ in [0]) == [[]]
"
        );
        let result = MontyRun::new(code, "generator.py", vec![], CompileOptions::default())
            .expect("generator source should compile")
            .run_no_limits(vec![])
            .expect("rejected task switch should leave the scheduler usable");
        assert_eq!(result, MontyObject::none());
    }
}

#[test]
fn rejected_future_wait_leaves_the_future_awaitable() {
    for body in ["asyncio.run(future)", "asyncio.run(asyncio.gather(future))"] {
        let code = format!(
            "\
import asyncio
future = external()
generator = ({body} for _ in [0])
try:
    next(generator)
    assert False, 'expected NotImplementedError'
except NotImplementedError as exc:
    assert str(exc) == 'generator expression: resolving async futures is not yet supported in this context'
assert next(generator, None) is None
assert await future == 42
assert list(asyncio.run(future) for _ in [0]) == [42]
assert list(asyncio.run(asyncio.gather(future)) for _ in [0]) == [[42]]
"
        );
        let progress = MontyRun::new(
            code,
            "generator.py",
            vec!["external".to_owned()],
            CompileOptions::default(),
        )
        .expect("generator source should compile")
        .start(
            vec![MontyObject::function("external".to_owned(), None)],
            ResourceTracker::default(),
            PrintWriter::Disabled,
        )
        .unwrap();
        let RunProgress::FunctionCall(call) = progress else {
            panic!("expected external call before creating the generator");
        };
        let call_id = call.call_id;
        let RunProgress::ResolveFutures(waiting) = call.resume_pending(PrintWriter::Disabled).unwrap() else {
            panic!("expected top-level await after rejecting the generator's wait");
        };
        let progress = waiting
            .resume(
                vec![(call_id, ExtFunctionResult::Return(MontyObject::int(42)))],
                PrintWriter::Disabled,
            )
            .unwrap();
        assert_eq!(progress.into_complete().unwrap(), MontyObject::none());
    }
}

#[test]
fn unsupported_suspension_retains_the_generator_frame() {
    let error = run_error("generator = (external() for _ in [0])\nnext(generator)");
    assert_eq!(error.exc_type(), ExcType::NotImplementedError);
    assert_snapshot!(error.to_string(), @r#"
    Traceback (most recent call last):
      File "generator.py", line 2, in <module>
        next(generator)
        ~~~~~~~~~~~~~~~
      File "generator.py", line 1, in <genexpr>
        generator = (external() for _ in [0])
                     ~~~~~~~~~~
    NotImplementedError: generator expression: external function 'external' is not yet supported in this context
    "#);
}
