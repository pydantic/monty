//! Static attribute definitions for builtin types.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext, HeapData},
    intern::StaticStrings,
    types::{
        PyTrait, Type, complex, date, datetime, list, re_match, re_pattern, str, time, timedelta, timezone, tuple,
    },
    value::{EitherStr, Value},
};

pub(crate) type BuiltinCall = for<'h> fn(Type, Value, ArgValues, &mut VM<'h>) -> RunResult<CallResult>;
/// Heap receivers stay protected by their read handle; temporary Values borrow their references.
pub(crate) type MethodCall = for<'h> fn(&Value, ArgValues, &mut VM<'h>) -> RunResult<Value>;

/// Python attribute definitions and the native view passed to their methods.
pub(crate) trait PyAttrs<'h> {
    type Receiver;
    const ATTRS: &'static [(StaticStrings, AttrDef)];
}

#[derive(Clone, Copy)]
pub(crate) enum AttrDef {
    Method(MethodCall),
    ClassMethod(BuiltinCall),
    Value(fn(&mut VM<'_>) -> Value),
}

#[cfg_attr(not(test), expect(dead_code, reason = "retained for builtin attribute enumeration"))]
pub(crate) const fn attrs(owner: Type) -> &'static [(StaticStrings, AttrDef)] {
    match owner {
        Type::Str => <str::PyStr<'static> as PyAttrs<'static>>::ATTRS,
        Type::List => <list::PyListImpl as PyAttrs<'static>>::ATTRS,
        Type::Tuple => <tuple::PyTupleImpl as PyAttrs<'static>>::ATTRS,
        Type::Complex => <complex::PyComplexImpl as PyAttrs<'static>>::ATTRS,
        Type::Date => <date::PyDateImpl as PyAttrs<'static>>::ATTRS,
        Type::Time => <time::PyTimeImpl as PyAttrs<'static>>::ATTRS,
        Type::TimeDelta => <timedelta::PyTimeDeltaImpl as PyAttrs<'static>>::ATTRS,
        Type::TimeZone => <timezone::PyTimeZoneImpl as PyAttrs<'static>>::ATTRS,
        Type::RePattern => <re_pattern::PyRePatternImpl as PyAttrs<'static>>::ATTRS,
        Type::ReMatch => <re_match::PyReMatchImpl as PyAttrs<'static>>::ATTRS,
        _ => &[],
    }
}

pub(crate) fn invoke_unbound(
    owner: Type,
    attr: &EitherStr,
    call: MethodCall,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    let (receiver, args) = match args {
        ArgValues::One(receiver) => (receiver, ArgValues::Empty),
        ArgValues::Two(receiver, arg) => (receiver, ArgValues::One(arg)),
        ArgValues::ArgsKargs { mut args, kwargs } if !args.is_empty() => {
            let receiver = args.remove(0);
            (receiver, ArgValues::from_parts(args, kwargs))
        }
        args => {
            args.drop_with(vm);
            return Err(ExcType::type_error(format!(
                "unbound method {}.{}() needs an argument",
                owner.dunder_name(vm.heap, vm.interns),
                attr.as_str(vm.interns)
            )));
        }
    };
    let mut pending = DropGuard::new((receiver, args), vm);
    let ((receiver, _), vm) = pending.as_parts_mut();
    let receiver_type = receiver.py_type(vm);
    if !receiver_type.is_instance_of(owner) {
        return Err(ExcType::type_error(format!(
            "descriptor '{}' for '{}' objects doesn't apply to a '{}' object",
            attr.as_str(vm.interns),
            owner.dunder_name(vm.heap, vm.interns),
            receiver.py_type_name(vm),
        )));
    }
    let ((receiver, args), vm) = pending.into_parts();
    defer_drop!(receiver, vm);
    if receiver_type == owner {
        call(receiver, args, vm).map(CallResult::Value)
    } else {
        call_subtype(owner, attr, call, receiver, args, vm)
    }
}

fn call_subtype(
    owner: Type,
    attr: &EitherStr,
    call: MethodCall,
    receiver: &Value,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    let Value::Ref(id) = receiver else {
        unreachable!("native subclasses are heap objects")
    };
    if owner == Type::Tuple || attr.as_str(vm.interns) == "strftime" {
        // Named tuples use their tuple operations; date.strftime(datetime) reads
        // the datetime's time and timezone, unlike the other date descriptors.
        return receiver.py_value(vm).py_call_attr(vm, attr, args);
    }
    let HeapData::DateTime(datetime) = vm.heap.get(*id) else {
        unreachable!("date's native subclass is datetime")
    };
    let (year, month, day, ..) = datetime::to_components(datetime).expect("datetime has valid civil components");
    // Date descriptors read only the date portion, even when datetime overrides
    // the same method name. Project it rather than passing a mismatched heap handle.
    let date = date::from_ymd(year, i32::from(month), i32::from(day)).expect("datetime date is valid");
    let date_receiver = Value::Ref(vm.heap.allocate(HeapData::Date(date)));
    defer_drop!(date_receiver, vm);
    let result = call(date_receiver, args, vm)?;
    if attr.as_str(vm.interns) == "replace" {
        defer_drop!(result, vm);
        let Value::Ref(id) = result else {
            unreachable!("date.replace returns a date")
        };
        let HeapData::Date(date) = vm.heap.get(*id) else {
            unreachable!("date.replace returns a date")
        };
        let (year, month, day) = date::to_ymd(*date);
        // CPython's date.replace constructs the receiver's type using only the
        // date components, so a datetime result is naive and starts at midnight.
        Ok(CallResult::Value(datetime::allocate_naive(
            year,
            i32::try_from(month).expect("month is in 1..=12"),
            i32::try_from(day).expect("day is in 1..=31"),
            0,
            0,
            0,
            0,
            vm.heap,
        )))
    } else {
        Ok(CallResult::Value(result))
    }
}

#[cfg(test)]
mod tests {
    use std::mem::discriminant;

    use monty_macros::pyattrs;
    use monty_types::{PrintWriter, ResourceTracker};

    use super::*;
    use crate::{
        heap::{Heap as ObjectHeap, HeapData, HeapReadOutput, HeapReader},
        intern::Interns,
        run::{Program, SessionTables},
        types::{List, PyTrait},
        value::EitherStr,
    };

    struct PyFloatImpl;

    #[pyattrs]
    impl<'heap> PyAttrs<'heap> for PyFloatImpl {
        type Receiver = f64;

        #[pymethod]
        fn conjugate(value: f64, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
            args.check_zero_args("float.conjugate", vm.heap)?;
            Ok(Value::Float(value))
        }
    }

    #[test]
    fn generated_adapters_use_native_receivers() {
        const SORT: Option<AttrDef> = list::PyListImpl::lookup_attr(StaticStrings::Sort);
        const LIST_TABLE: &[(StaticStrings, AttrDef)] = attrs(Type::List);
        assert!(matches!(SORT, Some(AttrDef::Method(_))));
        assert_eq!(LIST_TABLE.len(), <list::PyListImpl as PyAttrs<'static>>::ATTRS.len());

        let mut heap = ObjectHeap::new(16, ResourceTracker::default());
        let list_id = heap.allocate(HeapData::List(List::new(vec![Value::Int(2), Value::Int(1)])));
        let tuple = tuple::allocate_tuple([Value::Int(2), Value::Int(2)].into_iter().collect(), &heap);
        let mut tables = SessionTables::default();
        let program = Program::for_tests();
        HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
            let mut vm = VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
            let vm = &mut vm;
            let list = Value::Ref(list_id);
            let owned = (list, tuple);
            crate::defer_drop!(owned, vm);

            let Some(AttrDef::Method(sort)) = SORT else {
                panic!("expected sort method")
            };
            assert!(matches!(sort(&owned.0, ArgValues::Empty, vm).unwrap(), Value::None));
            let HeapData::List(list) = vm.heap.get(list_id) else {
                panic!("expected list")
            };
            assert!(matches!(list.as_slice(), [Value::Int(1), Value::Int(2)]));

            let Some(AttrDef::Method(count)) = tuple::PyTupleImpl::lookup_attr(StaticStrings::Count) else {
                panic!("expected count method")
            };
            assert!(matches!(
                count(&owned.1, ArgValues::One(Value::Int(2)), vm).unwrap(),
                Value::Int(2)
            ));

            let Some(AttrDef::Method(conjugate)) = PyFloatImpl::lookup_attr(StaticStrings::Conjugate) else {
                panic!("expected immediate method")
            };
            assert!(
                matches!(conjugate(&Value::Float(2.0), ArgValues::Empty, vm).unwrap(), Value::Float(value) if value.to_bits() == 2.0_f64.to_bits())
            );
            assert!(
                matches!(conjugate(&Value::Float(-2.5), ArgValues::Empty, vm).unwrap(), Value::Float(value) if value.to_bits() == (-2.5_f64).to_bits())
            );
            assert!(conjugate(&Value::Float(2.0), ArgValues::One(Value::Int(1)), vm).is_err());
        });
    }

    #[test]
    fn tables_and_lookups_agree() {
        let interns = Interns::new("");
        for owner in [
            Type::Str,
            Type::List,
            Type::Tuple,
            Type::Complex,
            Type::Date,
            Type::Time,
            Type::TimeDelta,
            Type::TimeZone,
            Type::RePattern,
            Type::ReMatch,
        ] {
            assert!(owner.get_attr(&EitherStr::Heap("fromkeys".into()), &interns).is_none());
            for &(name, definition) in attrs(owner) {
                let spelling: &str = name.into();
                let attr = EitherStr::Heap(spelling.to_owned());
                let found = owner.get_attr(&attr, &interns).expect("table entry has a lookup arm");
                assert_eq!(discriminant(&definition), discriminant(&found));
            }
        }
        assert!(attrs(Type::Int).is_empty());
        assert!(Type::Int.get_attr(&EitherStr::Heap("upper".into()), &interns).is_none());
        assert!(PyFloatImpl::lookup_attr(StaticStrings::Upper).is_none());
    }

    #[test]
    fn missing_sequence_methods_release_heap_arguments() {
        let mut heap = ObjectHeap::new(16, ResourceTracker::default());
        let list_id = heap.allocate(HeapData::List(List::new(vec![])));
        let tuple = tuple::allocate_tuple([Value::Int(1)].into_iter().collect(), &heap);
        let mut tables = SessionTables::default();
        let program = Program::for_tests();
        HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
            let mut vm = VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
            let vm = &mut vm;
            let owned = (Value::Ref(list_id), tuple);
            crate::defer_drop!(owned, vm);
            for name in [
                EitherStr::Interned(vm.interns.intern_static(StaticStrings::Upper)),
                EitherStr::Heap("missing_method".into()),
            ] {
                let argument = owned.1.clone_with_heap(vm.heap);
                let HeapReadOutput::List(mut list) = vm.heap.read(list_id) else {
                    panic!("expected list")
                };
                assert!(list.py_call_attr(vm, &name, ArgValues::One(argument)).is_err());

                let argument = owned.0.clone_with_heap(vm.heap);
                let Value::Ref(tuple_id) = &owned.1 else {
                    panic!("expected tuple reference")
                };
                let HeapReadOutput::Tuple(mut tuple) = vm.heap.read(*tuple_id) else {
                    panic!("expected tuple")
                };
                assert!(tuple.py_call_attr(vm, &name, ArgValues::One(argument)).is_err());
            }
        });
    }
}
