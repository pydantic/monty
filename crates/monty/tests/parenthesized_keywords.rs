use monty::MontyRun;
use monty_types::{CompileOptions, ExcType, MontyObject};

const KEYWORD_ERROR: &str = "expression cannot contain assignment, perhaps you meant \"==\"?";

#[test]
fn parenthesized_keyword_names_are_rejected_before_execution() {
    for code in [
        "missing((x)=1)",
        "missing(((x))=1)",
        "missing((café)=1)",
        "obj.method((x)=1)",
        "[missing((x)=i) for i in []]",
        "if False:\n    missing((x)=1)",
        "def f():\n    missing((x)=1)",
        "def f(value=missing((x)=1)): pass",
        "@missing((x)=1)\ndef f(): pass",
        "class C((metaclass)=type): pass",
        "from __future__ import annotations\nx: missing((x)=1)",
        "from __future__ import annotations\ndef f(x: missing((x)=1)): pass",
        "from __future__ import annotations\ndef f() -> missing((x)=1): pass",
    ] {
        let error = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect_err(code);
        assert_eq!(error.exc_type(), ExcType::SyntaxError, "{code}");
        assert_eq!(error.message(), Some(KEYWORD_ERROR), "{code}");
    }
}

#[test]
fn runtime_compilation_rejects_parenthesized_keyword_names() {
    for function in ["eval", "exec"] {
        for source in ["missing((x)=1)", "  missing((x)=1)", "\nmissing((x)=1)"] {
            let mut runner = MontyRun::new(
                format!("{function}(source)"),
                "test.py",
                vec!["source".to_owned()],
                CompileOptions::default(),
            )
            .unwrap();
            let error = runner
                .run_no_limits(vec![MontyObject::string(source.to_owned())])
                .unwrap_err();
            assert_eq!(error.exc_type(), ExcType::SyntaxError, "{function}: {source:?}");

            let message = error.message().unwrap();
            if function == "exec" && source.starts_with(' ') {
                assert!(message.contains("indent"), "{message}");
            } else {
                let line = if source.starts_with('\n') { 2 } else { 1 };
                assert_eq!(message, format!("{KEYWORD_ERROR} (<string>, line {line})"));
            }
        }
    }
}

#[test]
fn parentheses_in_keyword_values_and_strings_are_valid() {
    let code = r#"
assert dict(x=(1)) == {'x': 1}
assert dict(x=((1))) == {'x': 1}
assert dict(x='(name)=value') == {'x': '(name)=value'}
assert eval("dict(x=(1))") == {'x': 1}
exec("assert dict(x=(1)) == {'x': 1}")
"#;
    let mut runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn newer_syntax_is_not_rejected_by_version_diagnostics() {
    for code in [
        "match 1:\n    case +1: pass",
        "from __future__ import annotations\nx: [*xs for xs in []]",
        "from __future__ import annotations\nx: {*xs for xs in []}",
        "from __future__ import annotations\nx: {**d for d in []}",
        "from __future__ import annotations\nx: (*xs for xs in [])",
    ] {
        MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect(code);
    }
}
