//! Monty-specific generator-expression behavior which cannot share CPython fixtures.

use insta::assert_snapshot;
use monty::MontyRun;
use monty_types::{CompileOptions, ExcType, MontyObject};

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
    assert_eq!(result, MontyObject::Repr("<generator object <genexpr>>".to_owned()));
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
