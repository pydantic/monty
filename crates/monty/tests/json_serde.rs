//! Tests for JSON serialization and deserialization of `MontyObject`.
//!
//! A value serializes as its arena: the node list plus the root index, with
//! each node externally tagged (`{"Int":42}`). Serialization tests use
//! [`insta::assert_snapshot!`] with inline snapshots so the expected JSON stays
//! next to the assertion and can be refreshed with `cargo insta review`.
//! Round-trip tests stay on `assert_eq!` because they compare values
//! structurally, not as strings.

use insta::assert_snapshot;
use monty::MontyRun;
use monty_types::{CompileOptions, ExcType, MontyObject};

/// Evaluate a Python snippet under Monty and return its final value.
fn eval(code: &str) -> MontyObject {
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    ex.run_no_limits(vec![]).unwrap()
}

fn to_json(value: &MontyObject) -> String {
    serde_json::to_string(value).unwrap()
}

// === JSON Serialization Tests ===

#[test]
fn json_output_primitives() {
    // a leaf is a one-node arena rooted at 0
    assert_snapshot!(to_json(&MontyObject::int(42)), @r#"{"graph":{"nodes":[{"Int":42}]},"root":0}"#);
    assert_snapshot!(to_json(&MontyObject::float(1.5)), @r#"{"graph":{"nodes":[{"Float":1.5}]},"root":0}"#);
    assert_snapshot!(to_json(&MontyObject::string("hi")), @r#"{"graph":{"nodes":[{"String":"hi"}]},"root":0}"#);
    assert_snapshot!(to_json(&MontyObject::bool(true)), @r#"{"graph":{"nodes":[{"Bool":true}]},"root":0}"#);
    assert_snapshot!(to_json(&MontyObject::none()), @r#"{"graph":{"nodes":["None"]},"root":0}"#);
}

#[test]
fn json_output_list() {
    // children come first, the list holds their indexes
    assert_snapshot!(
        to_json(&eval("[1, 'two', 3.0]")),
        @r#"{"graph":{"nodes":[{"Int":1},{"String":"two"},{"Float":3.0},{"List":[0,1,2]}]},"root":3}"#
    );
}

#[test]
fn json_output_dict() {
    assert_snapshot!(
        to_json(&eval("{'a': 1, 'b': 2}")),
        @r#"{"graph":{"nodes":[{"String":"a"},{"Int":1},{"String":"b"},{"Int":2},{"Dict":[[0,1],[2,3]]}]},"root":4}"#
    );
}

#[test]
fn json_output_shared_child() {
    // a sub-object referenced twice is one node referenced twice
    assert_snapshot!(
        to_json(&eval("x = [1]\n(x, x)")),
        @r#"{"graph":{"nodes":[{"Int":1},{"List":[0]},{"Tuple":[1,1]}]},"root":2}"#
    );
}

#[test]
fn json_output_exception() {
    assert_snapshot!(
        to_json(&eval("ValueError('bad')")),
        @r#"{"graph":{"nodes":[{"Exception":{"exc_type":"ValueError","arg":"bad"}}]},"root":0}"#
    );
}

#[test]
fn json_output_builtin_function() {
    assert_snapshot!(
        to_json(&eval("print")),
        @r#"{"graph":{"nodes":[{"BuiltinFunction":"print"}]},"root":0}"#
    );
}

#[test]
fn json_output_cycle_list() {
    // a reference back to an enclosing container is a `Cycle` leaf
    assert_snapshot!(
        to_json(&eval("a = []; a.append(a); a")),
        @r#"{"graph":{"nodes":[{"Cycle":"[...]"},{"List":[0]}]},"root":1}"#
    );
}

// === JSON Deserialization Tests ===

#[test]
fn json_deserialize_primitives() {
    let int: MontyObject = serde_json::from_str(r#"{"graph":{"nodes":[{"Int":42}]},"root":0}"#).unwrap();
    let null: MontyObject = serde_json::from_str(r#"{"graph":{"nodes":["None"]},"root":0}"#).unwrap();
    assert_eq!(int, MontyObject::int(42));
    assert_eq!(null, MontyObject::none());
}

#[test]
fn json_deserialize_builtin_function() {
    let value: MontyObject =
        serde_json::from_str(r#"{"graph":{"nodes":[{"BuiltinFunction":"print"}]},"root":0}"#).unwrap();
    assert_eq!(value, eval("print"));
}

#[test]
fn json_roundtrip() {
    let values = [
        eval("[1, {'k': (2, 3.5)}, None, b'x', ...]"),
        eval("a = []; a.append(a); b = {}; b['b'] = b; [a, b]"),
        MontyObject::exception(ExcType::TypeError, Some("bad".to_owned())),
    ];
    for value in values {
        let json = to_json(&value);
        let back: MontyObject = serde_json::from_str(&json).unwrap();
        assert_eq!(back, value, "{json}");
    }
}

// === Cycle placeholders ===

#[test]
fn cycle_placeholders_follow_the_container() {
    // A cycle carries only its placeholder, chosen by the container's type.
    let result = eval("a = []; a.append(a); b = {}; b['b'] = b; [a, b]");
    assert_eq!(
        result,
        MontyObject::list([
            MontyObject::list([MontyObject::cycle("[...]")]),
            MontyObject::dict([(MontyObject::string("b"), MontyObject::cycle("{...}"))]),
        ])
    );
    assert_eq!(result.to_string(), "[[[...]], {'b': {...}}]");
}
