//! Tests for [`JsonMontyObject`] — the natural-form JSON serializer used by
//! the Python bindings' `MontyComplete.json_output()` method.
//!
//! This format differs from the derived serde format tested in
//! `json_serde.rs`: JSON-native Python values serialize bare, and
//! non-JSON-native types are wrapped in a `{"$<tag>": ...}` object.

use monty::{DictPairs, ExcType, JsonMontyObject, MontyObject, MontyRun};

fn to_json(obj: &MontyObject) -> String {
    serde_json::to_string(&JsonMontyObject(obj)).unwrap()
}

// === JSON-native primitives serialize bare ===

#[test]
fn json_output_primitives_bare() {
    assert_eq!(to_json(&MontyObject::None), "null");
    assert_eq!(to_json(&MontyObject::Bool(true)), "true");
    assert_eq!(to_json(&MontyObject::Bool(false)), "false");
    assert_eq!(to_json(&MontyObject::Int(42)), "42");
    assert_eq!(to_json(&MontyObject::Int(-7)), "-7");
    assert_eq!(to_json(&MontyObject::Float(1.5)), "1.5");
    assert_eq!(to_json(&MontyObject::String("hi".into())), r#""hi""#);
}

#[test]
fn json_output_list_bare_array() {
    let obj = MontyObject::List(vec![
        MontyObject::Int(1),
        MontyObject::String("two".into()),
        MontyObject::Float(3.0),
        MontyObject::None,
    ]);
    assert_eq!(to_json(&obj), r#"[1,"two",3.0,null]"#);
}

#[test]
fn json_output_dict_string_keys_bare_object() {
    let ex = MontyRun::new("{'a': 1, 'b': 'two'}".to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(to_json(&result), r#"{"a":1,"b":"two"}"#);
}

// === Non-JSON-native types get `$`-tagged ===

#[test]
fn json_output_tuple_tagged() {
    let obj = MontyObject::Tuple(vec![MontyObject::Int(1), MontyObject::String("two".into())]);
    assert_eq!(to_json(&obj), r#"{"$tuple":[1,"two"]}"#);
}

#[test]
fn json_output_bytes_tagged() {
    let obj = MontyObject::Bytes(vec![104, 105]);
    assert_eq!(to_json(&obj), r#"{"$bytes":[104,105]}"#);
}

#[test]
fn json_output_ellipsis_tagged() {
    // Tagged rather than bare so it never collides with a plain `"..."`
    // string result.
    assert_eq!(to_json(&MontyObject::Ellipsis), r#"{"$ellipsis":"..."}"#);
}

#[test]
fn json_output_non_finite_floats_tagged() {
    // `serde_json` would silently emit `null` for non-finite floats,
    // colliding with `None`. The `$float` tag preserves them.
    assert_eq!(to_json(&MontyObject::Float(f64::NAN)), r#"{"$float":"nan"}"#);
    assert_eq!(to_json(&MontyObject::Float(f64::INFINITY)), r#"{"$float":"inf"}"#);
    assert_eq!(to_json(&MontyObject::Float(f64::NEG_INFINITY)), r#"{"$float":"-inf"}"#);
}

#[test]
fn json_output_set_and_frozenset_tagged() {
    let s = MontyObject::Set(vec![MontyObject::Int(1), MontyObject::Int(2)]);
    assert_eq!(to_json(&s), r#"{"$set":[1,2]}"#);

    let fs = MontyObject::FrozenSet(vec![MontyObject::Int(3)]);
    assert_eq!(to_json(&fs), r#"{"$frozenset":[3]}"#);
}

#[test]
fn json_output_exception_tagged() {
    let obj = MontyObject::Exception {
        exc_type: ExcType::ValueError,
        arg: Some("oops".into()),
    };
    assert_eq!(to_json(&obj), r#"{"$exception":{"type":"ValueError","arg":"oops"}}"#);

    // `arg` omitted when None.
    let obj = MontyObject::Exception {
        exc_type: ExcType::TypeError,
        arg: None,
    };
    assert_eq!(to_json(&obj), r#"{"$exception":{"type":"TypeError"}}"#);
}

#[test]
fn json_output_repr_tagged() {
    let obj = MontyObject::Repr("<function foo>".into());
    assert_eq!(to_json(&obj), r#"{"$repr":"<function foo>"}"#);
}

// === Non-string dict keys trigger `$dict` fallback preserving key types ===

#[test]
fn json_output_dict_int_keys_tagged() {
    // Any non-string key switches the whole dict to `{"$dict": [[k, v], ...]}`
    // so the original key type is preserved round-trip.
    let ex = MontyRun::new("{1: 'a', 2: 'b'}".to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(to_json(&result), r#"{"$dict":[[1,"a"],[2,"b"]]}"#);
}

#[test]
fn json_output_dict_tuple_keys_tagged() {
    let ex = MontyRun::new("{(1, 2): 'a', (3, 4): 'b'}".to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(
        to_json(&result),
        r#"{"$dict":[[{"$tuple":[1,2]},"a"],[{"$tuple":[3,4]},"b"]]}"#
    );
}

#[test]
fn json_output_dict_mixed_keys_tagged() {
    // With one int and one string key the dict still goes through the
    // `$dict` path — a bare JSON object would have to coerce `1` to `"1"`,
    // colliding with any real `"1"` string key.
    let ex = MontyRun::new("{1: 'a', '1': 'b'}".to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(to_json(&result), r#"{"$dict":[[1,"a"],["1","b"]]}"#);
}

#[test]
fn json_output_dict_none_and_bool_keys_tagged() {
    let ex = MontyRun::new("{None: 1, True: 2, False: 3}".to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(to_json(&result), r#"{"$dict":[[null,1],[true,2],[false,3]]}"#);
}

// === Dataclass and namedtuple share a `{"$<tag>": {...}, "name": "..."}` shape ===

#[test]
fn json_output_namedtuple_fields_and_name() {
    // namedtuple body is a JSON object of field->value, and the class name
    // is emitted as a sibling "name" key. Constructed directly because
    // Monty's parser does not currently support class definitions.
    let obj = MontyObject::NamedTuple {
        type_name: "mymodule.Point".into(),
        field_names: vec!["x".into(), "y".into()],
        values: vec![MontyObject::Int(1), MontyObject::Int(2)],
    };
    assert_eq!(
        to_json(&obj),
        r#"{"$namedtuple":{"x":1,"y":2},"name":"mymodule.Point"}"#
    );
}

#[test]
fn json_output_dataclass_attrs_and_name() {
    let attrs: DictPairs = vec![
        (MontyObject::String("x".into()), MontyObject::Int(1)),
        (MontyObject::String("y".into()), MontyObject::Int(2)),
    ]
    .into();
    let obj = MontyObject::Dataclass {
        name: "Point".into(),
        type_id: 0,
        field_names: vec!["x".into(), "y".into()],
        attrs,
        frozen: false,
    };
    assert_eq!(to_json(&obj), r#"{"$dataclass":{"x":1,"y":2},"name":"Point"}"#);
}

#[test]
fn json_output_dataclass_nested() {
    // A dataclass attribute whose value is itself a dataclass should be
    // emitted recursively in the natural form, producing nested two-key
    // `{"$dataclass": ..., "name": ...}` objects.
    let inner_attrs: DictPairs = vec![(MontyObject::String("v".into()), MontyObject::Int(7))].into();
    let inner = MontyObject::Dataclass {
        name: "Inner".into(),
        type_id: 0,
        field_names: vec!["v".into()],
        attrs: inner_attrs,
        frozen: false,
    };
    let outer_attrs: DictPairs = vec![(MontyObject::String("inner".into()), inner)].into();
    let outer = MontyObject::Dataclass {
        name: "Outer".into(),
        type_id: 1,
        field_names: vec!["inner".into()],
        attrs: outer_attrs,
        frozen: false,
    };
    assert_eq!(
        to_json(&outer),
        r#"{"$dataclass":{"inner":{"$dataclass":{"v":7},"name":"Inner"}},"name":"Outer"}"#
    );
}

// === Dates serialize as structured objects ===

#[test]
fn json_output_date_structured() {
    let ex = MontyRun::new(
        "import datetime; datetime.date(2024, 3, 15)".to_owned(),
        "test.py",
        vec![],
    )
    .unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(to_json(&result), r#"{"year":2024,"month":3,"day":15}"#);
}

#[test]
fn json_output_bigint_raw_number() {
    // Values within i64 range go through the fast path and serialize as normal numbers.
    let ex = MontyRun::new("2 ** 62".to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(to_json(&result), "4611686018427387904");

    // Beyond i64 range (but within u64) still uses a native integer serializer.
    let ex = MontyRun::new("2 ** 63".to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(to_json(&result), "9223372036854775808");

    // Genuine big integers — beyond u64 — serialize as raw JSON numbers via
    // serde_json's arbitrary-precision support.
    let ex = MontyRun::new("2 ** 200".to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(
        to_json(&result),
        "1606938044258990275541962092341162602522202993782792835301376"
    );

    // Negative bigints also serialize as raw JSON numbers.
    let ex = MontyRun::new("-(2 ** 200)".to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(
        to_json(&result),
        "-1606938044258990275541962092341162602522202993782792835301376"
    );
}

#[test]
fn json_output_type_tagged() {
    // `type(int)` evaluates to the `type` metaclass itself, which maps to
    // `MontyObject::Type(Type::Type)` and serializes as `{"$type": "type"}`.
    let ex = MontyRun::new("type(int)".to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(to_json(&result), r#"{"$type":"type"}"#);

    // A plain `int` reference likewise maps to `MontyObject::Type`.
    let ex = MontyRun::new("int".to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(to_json(&result), r#"{"$type":"int"}"#);
}

#[test]
fn json_output_path_tagged() {
    let ex = MontyRun::new("from pathlib import Path\nPath('hello')".to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(to_json(&result), r#"{"$path":"hello"}"#);
}

// === Nested structures propagate the natural form recursively ===

#[test]
fn json_output_nested() {
    let ex = MontyRun::new(
        "{'items': [1, 'two', None, (10, 20)], 'flag': True}".to_owned(),
        "test.py",
        vec![],
    )
    .unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    assert_eq!(
        to_json(&result),
        r#"{"items":[1,"two",null,{"$tuple":[10,20]}],"flag":true}"#
    );
}
