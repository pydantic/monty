//! Compile generated adapters against a small runtime and exercise their calls.
use std::marker::PhantomData;

use monty_macros::pyattrs;

use crate::{
    args::ArgValues,
    bytecode::VM,
    exception_private::RunResult,
    intern::StaticStrings,
    py_attrs::{PyAttrs, PyAttrs as NativeAttrs},
    py_class::PyClass,
    py_value::PyValue,
    value::Value,
};

mod types {
    #[derive(Clone, Copy)]
    pub enum Type {
        Dict,
    }
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
mod intern {
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
mod py_class {
    #[derive(Clone, Copy)]
    pub struct PyClass<'h>(pub &'h str);
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
        Method {
            call: for<'h> fn(&mut PyValue<'h>, ArgValues, &mut VM<'h>) -> RunResult<bytecode::CallResult>,
            owner: Option<types::Type>,
        },
        ClassMethod(for<'h> fn(PyClass<'h>, ArgValues, &mut VM<'h>) -> RunResult<bytecode::CallResult>),
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

    #[pymethod(Count, Dict)]
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
    fn make(_: StaticStrings, class: PyClass<'_>, _: ArgValues, _: &mut VM<'_>) -> RunResult<bytecode::CallResult> {
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
    let Some(py_attrs::AttrDef::Method { call, owner }) = (IntegerMethods::TABLE.lookup)(StaticStrings::Count) else {
        panic!("method missing")
    };
    assert!(owner.is_none());
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
    let Some(py_attrs::AttrDef::Method { call, owner }) = (FloatMethods::TABLE.lookup)(StaticStrings::Count) else {
        panic!("method missing")
    };
    assert!(matches!(owner, Some(types::Type::Dict)));
    assert_eq!(call(&mut PyValue::Float(1.0), ArgValues, &mut vm).unwrap().0, Value(7));
    let Some(py_attrs::AttrDef::ClassMethod(call)) = (FloatMethods::TABLE.lookup)(StaticStrings::FromNumber) else {
        panic!("classmethod missing")
    };
    assert_eq!(call(PyClass("float"), ArgValues, &mut vm).unwrap().0, Value(5));
    let Some(py_attrs::AttrDef::Value(get)) = (FloatMethods::TABLE.lookup)(StaticStrings::Min) else {
        panic!("value missing")
    };
    assert_eq!(get(&mut vm), Value(-1));
}
