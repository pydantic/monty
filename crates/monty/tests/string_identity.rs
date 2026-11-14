//! Identity behavior for inline strings.

use monty::MontyRun;
use monty_types::{CompileOptions, MontyObject};

/// Evaluates a Python snippet under Monty and returns its final value.
fn eval(code: &str) -> MontyObject {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![])
        .unwrap()
}

#[test]
fn separately_created_inline_strings_share_content_identity() {
    assert_eq!(
        eval("a = 'abcdefghijklmno!'[:-1]\nb = 'abcdefghijklmno!'[:-1]\n(a is b, id(a) == id(b))"),
        MontyObject::tuple([MontyObject::bool(true), MontyObject::bool(true)]),
    );
}

#[test]
fn separately_created_heap_strings_keep_distinct_identity() {
    assert_eq!(
        eval("a = bin(4_000_000_000)\nb = bin(4_000_000_000)\n(a is b, id(a) != id(b))"),
        MontyObject::tuple([MontyObject::bool(false), MontyObject::bool(true)]),
    );
}

#[test]
fn inline_identity_preserves_the_last_byte_and_length() {
    assert_eq!(
        eval("a = ('a' * 14) + '\\0'\nb = ('a' * 14) + '\\x01'\nc = 'a' * 14\n(id(a) != id(b), id(a) != id(c))"),
        MontyObject::tuple([MontyObject::bool(true), MontyObject::bool(true)]),
    );
}
