//! Static attribute definitions for builtin types.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext, HeapData},
    intern::StaticStrings,
    py_value::PyValue,
    types::{
        PyTrait, Type, complex, date, datetime, list, re_match, re_pattern, str, time, timedelta, timezone, tuple,
    },
    value::{EitherStr, Value},
};

pub(crate) type BuiltinCall = for<'h> fn(Type, Value, ArgValues, &mut VM<'h>) -> RunResult<CallResult>;
pub(crate) type MethodCall = for<'h> fn(&mut PyValue<'h>, ArgValues, &mut VM<'h>) -> RunResult<CallResult>;

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
        Type::LruCacheWrapper => <super::lru_cache::PyLruCacheImpl as PyAttrs<'static>>::ATTRS,
        Type::NamedTuple => <super::namedtuple::PyNamedTupleImpl as PyAttrs<'static>>::ATTRS,
        Type::Bytes => <super::bytes::PyBytes<'static> as PyAttrs<'static>>::ATTRS,
        Type::Path => <super::path::PyPathImpl as PyAttrs<'static>>::ATTRS,
        Type::Random => <super::random::PyRandomImpl as PyAttrs<'static>>::ATTRS,
        Type::HashlibHash | Type::Blake2b | Type::Blake2s => <super::hashlib::PyHashImpl as PyAttrs<'static>>::ATTRS,
        Type::HashlibHashXof => <super::hashlib::PyHashXofImpl as PyAttrs<'static>>::ATTRS,
        Type::CsvWriter => <super::csv::PyCsvWriterImpl as PyAttrs<'static>>::ATTRS,
        Type::CsvDictWriter => <super::csv::PyCsvDictWriterImpl as PyAttrs<'static>>::ATTRS,
        Type::DirEntry => <super::dir_scan::PyDirEntryImpl as PyAttrs<'static>>::ATTRS,
        Type::ScandirIterator => <super::dir_scan::PyScandirImpl as PyAttrs<'static>>::ATTRS,
        Type::Dict => <super::dict::PyDictImpl as PyAttrs<'static>>::ATTRS,
        Type::DefaultDict => <super::dict::PyDefaultDictImpl as PyAttrs<'static>>::ATTRS,
        Type::Counter => <super::dict::PyCounterImpl as PyAttrs<'static>>::ATTRS,
        Type::Set => <super::set::PySetImpl as PyAttrs<'static>>::ATTRS,
        Type::FrozenSet => <super::set::PyFrozenSetImpl as PyAttrs<'static>>::ATTRS,
        Type::DictKeys => <super::dict_view::PyDictKeysViewImpl as PyAttrs<'static>>::ATTRS,
        Type::DictItems => <super::dict_view::PyDictItemsViewImpl as PyAttrs<'static>>::ATTRS,
        Type::DateTime => <super::datetime::PyDateTimeImpl as PyAttrs<'static>>::ATTRS,
        Type::Deque => <super::deque::PyDequeImpl as PyAttrs<'static>>::ATTRS,
        Type::TextIOWrapper | Type::BufferedReader | Type::BufferedWriter | Type::BufferedRandom => {
            <super::file::PyFileImpl as PyAttrs<'static>>::ATTRS
        }
        Type::Decimal => <super::decimal::py::PyDecimalImpl as PyAttrs<'static>>::ATTRS,
        Type::DecimalContext => <super::decimal::py_context::PyContextImpl as PyAttrs<'static>>::ATTRS,
        Type::DecimalSignalDict => <super::decimal::py_context::PySignalDictImpl as PyAttrs<'static>>::ATTRS,
        Type::ForwardRef => <super::forward_ref::PyForwardRefImpl as PyAttrs<'static>>::ATTRS,
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
        call(&mut receiver.py_value(vm), args, vm)
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
    if owner == Type::Dict {
        // Dict variants share the native Dict representation.
        return call(&mut receiver.py_value(vm), args, vm);
    }
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
    let result = call(&mut date_receiver.py_value(vm), args, vm)?;
    if attr.as_str(vm.interns) == "replace" {
        let CallResult::Value(result) = result else {
            unreachable!("date.replace returns a value")
        };
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
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::mem::discriminant;

    use monty_macros::pyattrs;
    use monty_types::{PrintWriter, ResourceTracker};

    use super::*;
    use crate::{
        heap::{Heap as ObjectHeap, HeapData, HeapReader},
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
            assert!(matches!(
                sort(&mut owned.0.py_value(vm), ArgValues::Empty, vm).unwrap(),
                CallResult::Value(Value::None)
            ));
            let HeapData::List(list) = vm.heap.get(list_id) else {
                panic!("expected list")
            };
            assert!(matches!(list.as_slice(), [Value::Int(1), Value::Int(2)]));

            let Some(AttrDef::Method(count)) = tuple::PyTupleImpl::lookup_attr(StaticStrings::Count) else {
                panic!("expected count method")
            };
            assert!(matches!(
                count(&mut owned.1.py_value(vm), ArgValues::One(Value::Int(2)), vm).unwrap(),
                CallResult::Value(Value::Int(2))
            ));

            let Some(AttrDef::Method(conjugate)) = PyFloatImpl::lookup_attr(StaticStrings::Conjugate) else {
                panic!("expected immediate method")
            };
            assert!(
                matches!(conjugate(&mut PyValue::Float(2.0), ArgValues::Empty, vm).unwrap(), CallResult::Value(Value::Float(value)) if value.to_bits() == 2.0_f64.to_bits())
            );
            assert!(
                matches!(conjugate(&mut PyValue::Float(-2.5), ArgValues::Empty, vm).unwrap(), CallResult::Value(Value::Float(value)) if value.to_bits() == (-2.5_f64).to_bits())
            );
            assert!(conjugate(&mut PyValue::Float(2.0), ArgValues::One(Value::Int(1)), vm).is_err());
        });
    }

    #[test]
    fn tables_and_lookups_agree() {
        let interns = Interns::new("");
        for owner in [
            Type::Dict,
            Type::DefaultDict,
            Type::Counter,
            Type::DictKeys,
            Type::DictItems,
            Type::Bytes,
            Type::Set,
            Type::FrozenSet,
            Type::Deque,
            Type::DateTime,
            Type::TextIOWrapper,
            Type::BufferedReader,
            Type::BufferedWriter,
            Type::BufferedRandom,
            Type::Path,
            Type::Random,
            Type::HashlibHash,
            Type::HashlibHashXof,
            Type::Blake2b,
            Type::Blake2s,
            Type::CsvWriter,
            Type::CsvDictWriter,
            Type::DirEntry,
            Type::ScandirIterator,
            Type::NamedTuple,
            Type::LruCacheWrapper,
            Type::Decimal,
            Type::DecimalContext,
            Type::DecimalSignalDict,
            Type::ForwardRef,
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
            assert!(
                owner
                    .get_attr(&EitherStr::Heap("missing_builtin_method".into()), &interns)
                    .is_none()
            );
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
                assert!(
                    owned
                        .0
                        .py_value(vm)
                        .py_call_attr(vm, &name, ArgValues::One(argument))
                        .is_err()
                );

                let argument = owned.0.clone_with_heap(vm.heap);
                assert!(
                    owned
                        .1
                        .py_value(vm)
                        .py_call_attr(vm, &name, ArgValues::One(argument))
                        .is_err()
                );
            }
        });
    }
}
