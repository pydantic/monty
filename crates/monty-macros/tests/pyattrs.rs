//! Compile generated adapters against a small runtime and exercise their calls.
use std::marker::PhantomData;

use monty_macros::pyattrs;

use crate::{
    args::ArgValues,
    bytecode::VM,
    exception_private::RunResult,
    py_attrs::{PyAttrs, PyAttrs as NativeAttrs},
    py_value::PyValue,
    symbols::StaticStrings,
    types::TypeRef,
    value::Value,
};

#[path = "../src/pyattrs.rs"]
mod expansion;

mod types {
    #[derive(Clone, Copy)]
    pub struct TypeRef(pub &'static str);
}
mod args {
    pub struct ArgValues;
}
mod value {
    #[derive(Debug, PartialEq)]
    pub struct Value(pub i64);
}
mod exception_private {
    pub type RunResult<T> = Result<T, &'static str>;
}
mod symbols {
    #[derive(Clone, Copy, PartialEq)]
    pub enum StaticStrings {
        Count,
        FromNumber,
        Min,
    }
}
mod bytecode {
    use super::*;
    pub struct VM<'h>(pub PhantomData<&'h ()>);
    #[derive(Debug, PartialEq)]
    pub struct CallResult(pub Value);
    impl From<Value> for CallResult {
        fn from(value: Value) -> Self {
            Self(value)
        }
    }
}
mod py_value {
    use super::*;
    pub enum PyValue<'h> {
        Int(PyInt<'h>),
        Float(f64),
    }
}
mod py_attrs {
    use super::*;
    pub trait PyAttrs<'h> {
        type Receiver;
        const TABLE: AttrTable;
    }
    pub struct AttrTable {
        pub entries: &'static [(StaticStrings, AttrDef)],
        pub lookup: fn(StaticStrings) -> Option<AttrDef>,
    }
    #[derive(Clone, Copy)]
    pub enum AttrDef {
        Method(for<'h> fn(&mut PyValue<'h>, ArgValues, &mut VM<'h>) -> RunResult<bytecode::CallResult>),
        ClassMethod(for<'h> fn(TypeRef, ArgValues, &mut VM<'h>) -> RunResult<bytecode::CallResult>),
        Value(fn(&mut VM<'_>) -> Value),
    }
    pub fn invalid_receiver(_: StaticStrings, _: ArgValues, _: &mut VM<'_>) -> RunResult<bytecode::CallResult> {
        Err("invalid receiver")
    }
}

type Arguments = ArgValues;
type AttributeResult = RunResult<Value>;

enum PyInt<'h> {
    Small(i64),
    Big(&'h str),
}
struct IntegerMethods;
#[pyattrs]
impl<'h> NativeAttrs<'h> for IntegerMethods {
    type Receiver = PyInt<'h>;

    #[pymethod(Count)]
    #[expect(
        clippy::unnecessary_wraps,
        reason = "callable adapters require a fallible handler signature"
    )]
    fn representation(integer: &PyInt<'_>, _: Arguments, _: &mut VM<'_>) -> AttributeResult {
        Ok(Value(match integer {
            PyInt::Small(value) => *value,
            PyInt::Big(digits) => i64::try_from(digits.len()).unwrap(),
        }))
    }
}
struct FloatMethods;
#[pyattrs]
impl PyAttrs<'_> for FloatMethods {
    type Receiver = f64;

    #[pymethod(Count)]
    #[expect(
        clippy::unnecessary_wraps,
        reason = "callable adapters require a fallible handler signature"
    )]
    fn count(_: f64, _: ArgValues, _: &mut VM<'_>) -> RunResult<bytecode::CallResult> {
        Ok(bytecode::CallResult(Value(7)))
    }
    #[pyclassmethod(FromNumber)]
    #[expect(
        clippy::unnecessary_wraps,
        reason = "callable adapters require a fallible handler signature"
    )]
    fn make(_: StaticStrings, class: TypeRef, _: ArgValues, _: &mut VM<'_>) -> RunResult<bytecode::CallResult> {
        Ok(bytecode::CallResult(Value(i64::try_from(class.0.len()).unwrap())))
    }
    #[pyvalue(Min)]
    fn minimum(_: &mut VM<'_>) -> Value {
        Value(-1)
    }
}

#[test]
fn explicit_names_and_both_integer_representations() {
    let mut vm = VM(PhantomData);
    let Some(py_attrs::AttrDef::Method(call)) = (IntegerMethods::TABLE.lookup)(StaticStrings::Count) else {
        panic!("method missing")
    };
    assert_eq!(
        call(&mut PyValue::Int(PyInt::Small(42)), ArgValues, &mut vm).unwrap().0,
        Value(42)
    );
    assert_eq!(
        call(
            &mut PyValue::Int(PyInt::Big("18446744073709551616")),
            ArgValues,
            &mut vm
        )
        .unwrap()
        .0,
        Value(20)
    );
    assert_eq!(
        call(&mut PyValue::Float(0.0), ArgValues, &mut vm),
        Err("invalid receiver")
    );
}

#[test]
fn call_results_class_receivers_and_values() {
    let mut vm = VM(PhantomData);
    let Some(py_attrs::AttrDef::Method(call)) = (FloatMethods::TABLE.lookup)(StaticStrings::Count) else {
        panic!("method missing")
    };
    assert_eq!(call(&mut PyValue::Float(1.0), ArgValues, &mut vm).unwrap().0, Value(7));
    let Some(py_attrs::AttrDef::ClassMethod(call)) = (FloatMethods::TABLE.lookup)(StaticStrings::FromNumber) else {
        panic!("classmethod missing")
    };
    assert_eq!(call(TypeRef("float"), ArgValues, &mut vm).unwrap().0, Value(5));
    let Some(py_attrs::AttrDef::Value(get)) = (FloatMethods::TABLE.lookup)(StaticStrings::Min) else {
        panic!("value missing")
    };
    assert_eq!(get(&mut vm), Value(-1));
}

#[test]
fn rejects_invalid_attribute_declarations() {
    for (members, expected) in [
        ("", "declare type Receiver"),
        ("type Receiver = &bool;", "expected a named receiver type"),
        ("type Receiver = i32;", "unsupported native receiver type"),
        ("type Receiver = HeapRead;", "expected a heap payload type"),
        ("type Receiver = HeapRead<'h>;", "expected a heap payload type"),
        (
            "type Receiver = bool; const KEY: u8 = 0;",
            "#[pyattrs] supports Receiver and associated functions only",
        ),
        (
            "type Receiver = bool; #[pymethod()] fn first() {}",
            "expected a StaticStrings key",
        ),
        (
            "type Receiver = bool; #[pymethod(A, B)] fn first() {}",
            "expected one StaticStrings key",
        ),
        (
            "type Receiver = bool; #[pymethod(A)] #[pyvalue(B)] fn first() {}",
            "use one Python attribute annotation per function",
        ),
        (
            "type Receiver = bool; #[pymethod(A)] #[cfg(test)] fn first() {}",
            "conditional Python attributes are not supported",
        ),
        (
            "type Receiver = bool; #[pymethod(A)] fn first() {} #[pyvalue(A)] fn second() {}",
            "duplicate Python attribute name",
        ),
    ] {
        let input = syn::parse_str(&format!("impl PyAttrs for Methods {{ {members} }}")).unwrap();
        assert_eq!(expansion::expand(input).unwrap_err().to_string(), expected, "{members}");
    }

    let inherent = syn::parse_quote!(impl Methods {});
    assert_eq!(
        expansion::expand(inherent).unwrap_err().to_string(),
        "#[pyattrs] requires a trait implementation"
    );
}
