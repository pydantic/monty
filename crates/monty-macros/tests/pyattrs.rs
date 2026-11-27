//! Compile generated adapters against a small runtime and exercise their calls.
use std::marker::PhantomData;

use monty_macros::pyattrs;

use crate::{
    args::ArgValues,
    bytecode::VM,
    exception_private::RunResult,
    intern::StaticStrings,
    py_attrs::{PyAttrs, PyAttrs as NativeAttrs},
    py_value::PyValue,
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
mod heap {
    use super::*;

    pub trait DropWithContext {
        fn drop_with(self, vm: &mut VM<'_>);
    }

    impl DropWithContext for Value {
        fn drop_with(self, _: &mut VM<'_>) {}
    }
}
mod intern {
    #[derive(Clone, Copy, PartialEq)]
    pub enum StaticStrings {
        Count,
        FromNumber,
        Min,
        Size,
        InstanceValue,
        Other,
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
    /// Adapts both infallible and fallible native accessors to the table signature.
    pub trait IntoAttrResult<T> {
        fn into_attr_result(self) -> RunResult<T>;
    }

    impl<T> IntoAttrResult<T> for T {
        fn into_attr_result(self) -> RunResult<T> {
            Ok(self)
        }
    }

    impl<T> IntoAttrResult<T> for RunResult<T> {
        fn into_attr_result(self) -> Self {
            self
        }
    }

    pub trait PyAttrs<'h> {
        type Receiver;
        const TABLE: AttrTable;
    }
    pub struct AttrTable {
        pub entries: &'static [(StaticStrings, AttrDef)],
        pub lookup: fn(StaticStrings) -> Option<AttrDef>,
    }
    type Getter = for<'h> fn(&mut PyValue<'h>, &mut VM<'h>) -> RunResult<Value>;
    type Setter = for<'h> fn(&mut PyValue<'h>, Value, &mut VM<'h>) -> RunResult<()>;

    #[derive(Clone, Copy)]
    pub enum AccessorKind {
        GetSet,
        InstanceOnly,
    }

    #[derive(Clone, Copy)]
    pub enum AttrDef {
        Method(for<'h> fn(&mut PyValue<'h>, ArgValues, &mut VM<'h>) -> RunResult<bytecode::CallResult>),
        ClassMethod(for<'h> fn(TypeRef, ArgValues, &mut VM<'h>) -> RunResult<bytecode::CallResult>),
        Value(fn(&mut VM<'_>) -> Value),
        GetSet {
            get: Getter,
            set: Option<Setter>,
            kind: AccessorKind,
        },
    }
    pub fn invalid_receiver_error(_: StaticStrings) -> &'static str {
        "invalid receiver"
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
    #[pyset(Size)]
    #[expect(clippy::unnecessary_wraps, reason = "setter adapter requires a fallible handler")]
    fn set_size(_: f64, _: Value, _: &mut VM<'_>) -> RunResult<()> {
        Ok(())
    }

    #[pyget(Size)]
    fn size(_: f64, _: &mut VM<'_>) -> Value {
        Value(3)
    }

    #[pyget(InstanceValue, instance_only)]
    fn instance_value(_: f64, _: &mut VM<'_>) -> Value {
        Value(1)
    }

    #[pyget(Other)]
    fn other(_: f64, _: &mut VM<'_>) -> Value {
        Value(0)
    }

    #[pyvalue(Min)]
    fn minimum(_: &mut VM<'_>) -> Value {
        Value(-1)
    }
}

#[test]
fn pairs_getter_and_setter_in_one_entry() {
    let mut vm = VM(PhantomData);
    let Some(py_attrs::AttrDef::GetSet {
        get,
        set: Some(set),
        kind: py_attrs::AccessorKind::GetSet,
    }) = (FloatMethods::TABLE.lookup)(StaticStrings::Size)
    else {
        panic!("getter/setter missing");
    };
    assert_eq!(
        FloatMethods::TABLE
            .entries
            .iter()
            .filter(|entry| entry.0 == StaticStrings::Size)
            .count(),
        1
    );
    assert!(matches!(
        (FloatMethods::TABLE.lookup)(StaticStrings::InstanceValue),
        Some(py_attrs::AttrDef::GetSet {
            kind: py_attrs::AccessorKind::InstanceOnly,
            set: None,
            ..
        })
    ));
    assert!(matches!(
        (FloatMethods::TABLE.lookup)(StaticStrings::Other),
        Some(py_attrs::AttrDef::GetSet {
            kind: py_attrs::AccessorKind::GetSet,
            set: None,
            ..
        })
    ));
    assert_eq!(get(&mut PyValue::Float(1.0), &mut vm), Ok(Value(3)));
    assert_eq!(
        get(&mut PyValue::Int(PyInt::Small(1)), &mut vm),
        Err("invalid receiver")
    );
    assert_eq!(set(&mut PyValue::Float(1.0), Value(4), &mut vm), Ok(()));
    assert_eq!(
        set(&mut PyValue::Int(PyInt::Small(1)), Value(4), &mut vm),
        Err("invalid receiver")
    );
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
            "type Receiver = bool; #[pyget(A, member)] fn get() {}",
            "unexpected attribute option",
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
        (
            "type Receiver = bool; #[pyset(A)] fn set() {}",
            "#[pyset] requires a matching #[pyget]",
        ),
        (
            "type Receiver = bool; #[pyget(A)] fn first() {} #[pyget(A)] fn second() {}",
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
