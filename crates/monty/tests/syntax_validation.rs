use monty::{MontyRepl, MontyRun};
use monty_types::{CompileOptions, ExcType, MontyObject, PrintWriter, ResourceTracker};

fn syntax_error(code: &str, message: &str) {
    let error = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect_err(code);
    assert_eq!(error.exc_type(), ExcType::SyntaxError, "{code}");
    assert_eq!(error.message(), Some(message), "{code}");
}

fn run(code: &str) -> MontyObject {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![])
        .unwrap()
}

#[test]
fn return_requires_a_function_scope() {
    for code in [
        "return",
        "return 1",
        "if False:\n    return 1",
        "for x in []:\n    return 1",
        "try:\n    return 1\nfinally:\n    pass",
        "class C:\n    return 1",
        "def f():\n    class C:\n        return 1",
    ] {
        syntax_error(code, "'return' outside function");
    }
    assert_eq!(run("def f():\n    return 42\nf()"), MontyObject::int(42));
    assert_eq!(
        run("class C:\n    def f(self):\n        return 42\nC().f()"),
        MontyObject::int(42)
    );
}

#[test]
fn exec_rejects_invalid_source_before_executing_any_of_it() {
    assert_eq!(
        run("x = 0\ntry:\n    exec('x = 1\\nreturn 42')\nexcept SyntaxError:\n    pass\nx"),
        MontyObject::int(0),
    );
    assert_eq!(run("exec('def f():\\n    return 42')\nf()"), MontyObject::int(42),);
}

#[test]
fn debug_cannot_be_bound_or_used_as_a_keyword() {
    for body in [
        "__debug__ = False",
        "__debug__: int",
        "__debug__ += 1",
        "(__debug__ := 1)",
        "x, __debug__ = 1, 2",
        "obj.__debug__ = 1",
        "obj.__debug__: int",
        "obj.__debug__ += 1",
        "for __debug__ in []: pass",
        "with value as __debug__: pass",
        "try:\n    pass\nexcept Exception as __debug__:\n    pass",
        "import math as __debug__",
        "import __debug__",
        "from math import pi as __debug__",
        "def __debug__(): pass",
        "class __debug__: pass",
        "def f(__debug__): pass",
        "def f(*__debug__): pass",
        "def f(**__debug__): pass",
        "lambda __debug__: 1",
        "dict(__debug__=1)",
        "[__debug__ for __debug__ in []]",
        "match value:\n    case [*__debug__]: pass",
        "match value:\n    case {**__debug__}: pass",
        "match value:\n    case C(__debug__=value): pass",
    ] {
        syntax_error(body, "cannot assign to __debug__");
        let nested = format!("def outer():\n    {}", body.replace('\n', "\n    "));
        syntax_error(&nested, "cannot assign to __debug__");
    }
    for code in ["del __debug__", "del obj.__debug__"] {
        syntax_error(code, "cannot delete __debug__");
    }
    assert_eq!(run("__debug__"), MontyObject::bool(true));
    assert_eq!(run("dict(**{'__debug__': 42})['__debug__']"), MontyObject::int(42));
}

#[test]
fn comprehension_walrus_cannot_rebind_any_iteration_variable() {
    for code in [
        "[(x := 1) for x in []]",
        "{(x := 1) for x in []}",
        "{x: (x := 1) for x in []}",
        "((x := 1) for x in [])",
        "[x for x in [] if (x := 1)]",
        "[(x := 1) for *x, y in []]",
        "[(x := 1) for [x, y] in []]",
        "[(x := y) for y in [] for x in []]",
        "[[(x := y) for y in []] for x in []]",
        "[(lambda value=(x := 1): value) for x in []]",
    ] {
        syntax_error(
            code,
            "assignment expression cannot rebind comprehension iteration variable 'x'",
        );
    }
    assert_eq!(run("[(lambda: (x := 42))() for x in [0]][0]"), MontyObject::int(42),);
    assert_eq!(run("[(y := x) for x in [42]]\ny"), MontyObject::int(42));
}

#[test]
fn comprehension_iterables_cannot_contain_walrus_expressions() {
    for code in [
        "[x for x in (items := [])]",
        "[x for x in [] for y in (items := [])]",
        "[x for x in [(value := y) for y in []]]",
        "[x for x in (lambda: (items := []))()]",
        "[x for x in (lambda value=(items := []): value)()]",
    ] {
        syntax_error(
            code,
            "assignment expression cannot be used in a comprehension iterable expression",
        );
    }
    syntax_error(
        "class C:\n    x: [(value := x) for x in []]",
        "assignment expression within a comprehension cannot be used in a class body",
    );
}

#[test]
fn annotation_checks_follow_compilation_and_symbol_table_rules() {
    for future in ["", "from __future__ import annotations\n"] {
        syntax_error(
            &format!("{future}x: [(x := 1) for x in []]"),
            "assignment expression cannot rebind comprehension iteration variable 'x'",
        );
        syntax_error(
            &format!("{future}x: lambda a, a: 1"),
            "duplicate argument 'a' in function definition",
        );
        syntax_error(&format!("{future}x: dict(__debug__=1)"), "cannot assign to __debug__");
    }
    syntax_error("x: dict(a=1, a=2)", "keyword argument repeated: a");
    for code in [
        "from __future__ import annotations\nx: dict(a=1, a=2)",
        "def f():\n    x: dict(a=1, a=2)",
        "obj.attr: dict(a=1, a=2)",
        "obj[0]: dict(a=1, a=2)",
        "(x): dict(a=1, a=2)",
    ] {
        MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect(code);
    }
}

#[test]
fn starred_target_checks_follow_annotation_codegen_rules() {
    for (expression, message) in [
        (
            "[a for *a in []]",
            "starred assignment target must be in a list or tuple",
        ),
        ("[a for a, *b, *c in []]", "multiple starred expressions in assignment"),
    ] {
        for source in [
            format!("x: {expression}"),
            format!("obj[{expression}]: int"),
            format!("def f[T: {expression}](): pass"),
        ] {
            syntax_error(&source, message);
        }
        for source in [
            format!("from __future__ import annotations\nx: {expression}"),
            format!("def f():\n    x: {expression}"),
            format!("obj[0]: {expression}"),
        ] {
            MontyRun::new(source.clone(), "test.py", vec![], CompileOptions::default()).expect(&source);
        }
    }
}

#[test]
fn exception_handlers_require_the_bare_except_to_come_last() {
    for suffix in ["except: pass", "except ValueError: pass"] {
        syntax_error(
            &format!("try: pass\nexcept: pass\n{suffix}"),
            "default 'except:' must be last",
        );
    }
    assert_eq!(
        run("try: 1 / 0\nexcept ValueError: value = 0\nexcept: value = 42\nvalue"),
        MontyObject::int(42)
    );
}

#[test]
fn parameters_and_annotations_cannot_conflict_with_scope_declarations() {
    syntax_error("def f(x):\n    global x", "name 'x' is parameter and global");
    syntax_error("def f(x):\n    nonlocal x", "name 'x' is parameter and nonlocal");
    syntax_error(
        "def f():\n    __debug__\n    global __debug__",
        "name '__debug__' is used prior to global declaration",
    );
    for body in ["x = 1\nprint(x)\nglobal x", "print(x)\nx: int\nglobal x"] {
        syntax_error(
            &format!("def f():\n    {}", body.replace('\n', "\n    ")),
            "name 'x' is used prior to global declaration",
        );
    }
    for body in ["x: int\nglobal x", "global x\nx: int"] {
        let code = format!("def f():\n    {}", body.replace('\n', "\n    "));
        syntax_error(&code, "annotated name 'x' can't be global");
    }
    syntax_error("x: int\nglobal x", "annotated name 'x' can't be global");
    for body in ["x: int\nnonlocal x", "nonlocal x\nx: int"] {
        syntax_error(
            &format!(
                "def outer():\n    x = 1\n    def inner():\n        {}",
                body.replace('\n', "\n        ")
            ),
            "annotated name 'x' can't be nonlocal",
        );
    }
    for code in ["global x\nx: int", "def f():\n    global x\n    (x): int"] {
        MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect(code);
    }
}

#[test]
fn private_name_aliases_share_a_symbol_in_compiler_checks() {
    syntax_error(
        "class C:\n    def f(__x, _C__x): pass",
        "duplicate argument '_C__x' in function definition",
    );
    syntax_error(
        "class C:\n    def f[__T, _C__T](): pass",
        "duplicate type parameter '_C__T'",
    );
    syntax_error(
        "class C:\n    def f(self):\n        return [(__x := 1) for _C__x in []]",
        "assignment expression cannot rebind comprehension iteration variable '__x'",
    );
    syntax_error(
        "class C:\n    def f(self):\n        __x: int\n        global _C__x",
        "annotated name '_C__x' can't be global",
    );
    syntax_error(
        "class C:\n    def f(self):\n        __x = 1\n        global _C__x",
        "name '_C__x' is assigned to before global declaration",
    );
    for expression in ["__x", "[y for y in __x]"] {
        syntax_error(
            &format!("class C:\n    def f(self):\n        {expression}\n        global _C__x"),
            "name '_C__x' is used prior to global declaration",
        );
    }
    for source in [
        "class C[T: (lambda __x, _C__x: 1)]: pass",
        "class C[__T: (lambda __U, _C__U: 1), __U]: pass",
        "class ___:\n    def f(__x, ____x): pass",
        "class C:\n    def f(__x__, _C__x__): pass",
        "class C:\n    def f(self):\n        dict(__x=1, _C__x=2)",
        "class C:\n    def f(self):\n        [__x for y in []]\n        global _C__x",
        "class C:\n    def f(self):\n        import math as __x\n        global _C__x",
    ] {
        MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default()).expect(source);
    }
    syntax_error(
        "class C[__U, __T: (lambda __U, _C__U: 1)]: pass",
        "duplicate argument '_C__U' in function definition",
    );
}

#[test]
fn await_requires_async_functions_but_keeps_montys_top_level_await() {
    for code in [
        "def f():\n    await value",
        "lambda: await value",
        "async def f():\n    return lambda: await value",
    ] {
        syntax_error(code, "'await' outside async function");
    }
    for code in [
        "await value",
        "async def f():\n    await value",
        "async def f():\n    return lambda value=await other: value",
    ] {
        MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect(code);
    }
}

#[test]
fn eager_annotation_comprehensions_cannot_be_async_but_generator_bodies_can() {
    for expression in ["[await value for x in []]", "[x async for x in values]"] {
        for template in [
            "x: EXPR",
            "def f(x: EXPR): pass",
            "async def f():\n    x: EXPR",
            "def f[T: EXPR](): pass",
            "def f[T = EXPR](): pass",
        ] {
            for prefix in ["", "from __future__ import annotations\n"] {
                syntax_error(
                    &format!("{prefix}{}", template.replace("EXPR", expression)),
                    "asynchronous comprehension outside of an asynchronous function",
                );
            }
        }
    }
    for source in [
        "def f():\n    x: (await value for x in [])",
        "class C:\n    x: (await value for x in [])",
        "def f[T](x: (await value for x in [])): pass",
        "def f[T: (await value for x in [])](): pass",
        "def f[T = (await value for x in [])](): pass",
        "def f():\n    x: (x async for x in values)",
        "def f():\n    x: ([await value for x in []] for y in [])",
        "def f():\n    x: (lambda: (await value for x in []))",
        "def f():\n    (await value for x in [])",
        "lambda: (await value for x in [])",
        "def f():\n    ([await value for x in []] for y in [])",
    ] {
        for prefix in ["", "from __future__ import annotations\n"] {
            MontyRun::new(
                format!("{prefix}{source}"),
                "test.py",
                vec![],
                CompileOptions::default(),
            )
            .expect(source);
        }
    }
    syntax_error(
        "def f[T](x: (x for x in await values)): pass",
        "await expression cannot be used within an annotation",
    );
}

#[test]
fn formatted_strings_cannot_be_match_patterns() {
    for pattern in ["f''", "f'{value}'", "'a' f'b'", "[f'{value}']", "t''", "t'{value}'"] {
        syntax_error(
            &format!("match value:\n    case {pattern}: pass"),
            "patterns may only match literals and attribute lookups",
        );
    }
}

#[test]
fn type_parameter_lists_and_expressions_are_validated() {
    for code in ["class C[__classdict__]: pass", "def f[*__classdict__](): pass"] {
        syntax_error(code, "reserved name '__classdict__' cannot be used for type parameter");
    }
    syntax_error("def f[T, T](): pass", "duplicate type parameter 'T'");
    syntax_error("class C[T, T]: pass", "duplicate type parameter 'T'");
    syntax_error(
        "def f[T = int, U](): pass",
        "non-default type parameter 'U' follows default type parameter",
    );
    for (expression, context) in [
        ("T: (x := 1)", "TypeVar bound"),
        ("T: (int, (x := 1))", "TypeVar constraint"),
        ("T = (x := 1)", "TypeVar default"),
        ("*T = (x := ())", "TypeVarTuple default"),
        ("**P = (x := [])", "ParamSpec default"),
    ] {
        syntax_error(
            &format!("def f[{expression}](): pass"),
            &format!("named expression cannot be used within a {context}"),
        );
    }
    let code = format!("def f[T: value{}](): pass", ".x".repeat(10_000));
    syntax_error(&code, "Source is too deeply nested");
}

#[test]
fn generic_annotations_have_a_distinct_scope_from_defaults_and_lambda_bodies() {
    for source in [
        "def f[T](x: [(y := i) for i in []]): pass",
        "def f[T]() -> [(y := i) for i in []]: pass",
        "def f[T](x: [(lambda a=(y := i): a) for i in []]): pass",
    ] {
        for prefix in ["", "from __future__ import annotations\n"] {
            syntax_error(
                &format!("{prefix}{source}"),
                "assignment expression within a comprehension cannot be used within the definition of a generic",
            );
        }
    }
    syntax_error(
        "class C[T](base := object): pass",
        "named expression cannot be used within the definition of a generic",
    );
    syntax_error(
        "class C[T](await value): pass",
        "await expression cannot be used within the definition of a generic",
    );
    for expression in ["yield 1", "yield from values"] {
        syntax_error(
            &format!("class C[T](({expression})): pass"),
            "yield expression cannot be used within the definition of a generic",
        );
    }
    for parameter in ["T: [(y := i) for i in []]", "T = [(y := i) for i in []]"] {
        syntax_error(
            &format!("def f[{parameter}](): pass"),
            "assignment expression within a comprehension cannot be used in a TypeVar bound",
        );
    }
    for source in [
        "def f[T](x=[(y := i) for i in []]): pass",
        "def f[T](x: [(lambda: (y := i))() for i in []]): pass",
    ] {
        MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default()).expect(source);
    }
}

#[test]
fn nonlocal_cannot_bind_a_type_parameter_but_can_bind_a_shadowing_local() {
    syntax_error(
        "class C:\n    def f[__T]():\n        def g():\n            nonlocal __T",
        "nonlocal binding not allowed for type parameter '_C__T'",
    );
    syntax_error(
        "class A:\n    def f[__B__x]():\n        class A__B:\n            def g():\n                nonlocal __x",
        "nonlocal binding not allowed for type parameter '_A__B__x'",
    );
    for code in [
        "def outer():\n    X = 1\n    def inner[X]():\n        nonlocal X",
        "def outer[X]():\n    def inner():\n        nonlocal X",
        "class C[X]:\n    nonlocal X",
    ] {
        syntax_error(code, "nonlocal binding not allowed for type parameter 'X'");
    }
    for code in [
        "def outer[X]():\n    X = 1\n    def inner():\n        nonlocal X",
        "def outer[X]():\n    def inner():\n        nonlocal X\n    X = 1",
        "def outer[X](X):\n    def inner():\n        nonlocal X",
    ] {
        MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect(code);
    }
    syntax_error(
        "def far():\n    X = 1\n    def middle():\n        global X\n        def inner():\n            nonlocal X",
        "no binding for nonlocal 'X' found",
    );
    // These valid generic closures still lack runtime support; class shadowing must not report a type-parameter error.
    for code in [
        "class C[X]:\n    X = 1\n    def method():\n        nonlocal X",
        "def outer[X]():\n    class C:\n        def method():\n            nonlocal X\n        X: int",
    ] {
        syntax_error(code, "no binding for nonlocal 'X' found");
    }
}

#[test]
fn format_specification_nesting_is_bounded_before_lowering() {
    for depth in [4, 10, 40, 400] {
        let mut field = "{spec}".to_owned();
        for _ in 1..depth {
            field = format!("{{value:{field}}}");
        }
        syntax_error(&format!("f\"{field}\""), "f-string: expressions nested too deeply");
    }
    assert_eq!(run("f'{42:{2}}'"), MontyObject::string("42".to_owned()));
    assert_eq!(run("f'{42:{len(f\"{2}\")}}'"), MontyObject::string("42".to_owned()));
    assert_eq!(
        run("a = 1\nb = 2\nc = '04d'\nf'{a:{b:{c}}}'"),
        MontyObject::string("01".to_owned())
    );
}

#[test]
fn repl_rejections_preserve_state_and_inherited_annotation_mode() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run("counter = 0", vec![], PrintWriter::Disabled).unwrap();
    for source in [
        "counter = 1\nreturn 42",
        "def injected():\n    return 42\nreturn 1",
        "counter = 1\n[(x := 1) for x in []]",
        "counter = 1\ndef f(__debug__): pass",
    ] {
        let error = repl.feed_run(source, vec![], PrintWriter::Disabled).unwrap_err();
        assert_eq!(error.exc_type(), ExcType::SyntaxError);
        assert_eq!(
            repl.feed_run("counter", vec![], PrintWriter::Disabled).unwrap(),
            MontyObject::int(0)
        );
    }
    let error = repl.feed_run("injected", vec![], PrintWriter::Disabled).unwrap_err();
    assert_eq!(error.exc_type(), ExcType::NameError);

    repl.feed_run("from __future__ import annotations", vec![], PrintWriter::Disabled)
        .unwrap();
    repl.feed_run("x: dict(a=1, a=2)", vec![], PrintWriter::Disabled)
        .unwrap();
    let error = repl
        .feed_run("x: dict((a)=1)", vec![], PrintWriter::Disabled)
        .unwrap_err();
    assert_eq!(error.exc_type(), ExcType::SyntaxError);
    assert_eq!(
        repl.feed_run("1 + 1", vec![], PrintWriter::Disabled).unwrap(),
        MontyObject::int(2)
    );
}
