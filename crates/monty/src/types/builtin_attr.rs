//! Static attribute definitions for builtin types.

use std::mem::ManuallyDrop;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::RunResult,
    heap::HeapObjectRead,
    intern::StaticStrings,
    types::{Type, complex, date, list, re_match, re_pattern, str, time, timedelta, timezone, tuple},
    value::Value,
};

pub(crate) type BuiltinCall = for<'h> fn(StaticStrings, Type, Value, ArgValues, &mut VM<'h>) -> RunResult<CallResult>;
pub(crate) type MethodCall = for<'h> fn(&Value, ArgValues, &mut VM<'h>) -> RunResult<Value>;

/// Python attribute definitions and the native view passed to their methods.
pub(crate) trait PyAttrs<'h> {
    type Receiver;
    const ATTRS: &'static [(StaticStrings, AttrDef)];
}

pub(crate) fn call_method<'h, T>(
    receiver: &HeapObjectRead<'h, T>,
    call: MethodCall,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<CallResult> {
    // The read handle keeps the receiver alive. This Value only lends its identity
    // to the adapter, so it must neither acquire nor release an owned reference.
    let receiver = ManuallyDrop::new(Value::Ref(receiver.id()));
    call(&receiver, args, vm).map(CallResult::Value)
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

pub(crate) const fn lookup_attr(owner: Type, name: StaticStrings) -> Option<AttrDef> {
    match owner {
        Type::Str => str::PyStr::lookup_attr(name),
        Type::List => list::PyListImpl::lookup_attr(name),
        Type::Tuple => tuple::PyTupleImpl::lookup_attr(name),
        Type::Complex => complex::PyComplexImpl::lookup_attr(name),
        Type::Date => date::PyDateImpl::lookup_attr(name),
        Type::Time => time::PyTimeImpl::lookup_attr(name),
        Type::TimeDelta => timedelta::PyTimeDeltaImpl::lookup_attr(name),
        Type::TimeZone => timezone::PyTimeZoneImpl::lookup_attr(name),
        Type::RePattern => re_pattern::PyRePatternImpl::lookup_attr(name),
        Type::ReMatch => re_match::PyReMatchImpl::lookup_attr(name),
        _ => None,
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
    fn tables_preserve_registered_names_order_and_kinds() {
        let expected: &[(Type, &[(StaticStrings, u8)])] = &[
            (
                Type::Str,
                &[
                    (StaticStrings::Lower, 0),
                    (StaticStrings::Upper, 0),
                    (StaticStrings::Capitalize, 0),
                    (StaticStrings::Title, 0),
                    (StaticStrings::Swapcase, 0),
                    (StaticStrings::Casefold, 0),
                    (StaticStrings::Isalpha, 0),
                    (StaticStrings::Isdigit, 0),
                    (StaticStrings::Isalnum, 0),
                    (StaticStrings::Isnumeric, 0),
                    (StaticStrings::Isspace, 0),
                    (StaticStrings::Islower, 0),
                    (StaticStrings::Isupper, 0),
                    (StaticStrings::Isascii, 0),
                    (StaticStrings::Isdecimal, 0),
                    (StaticStrings::Find, 0),
                    (StaticStrings::Rfind, 0),
                    (StaticStrings::Index, 0),
                    (StaticStrings::Rindex, 0),
                    (StaticStrings::Count, 0),
                    (StaticStrings::Startswith, 0),
                    (StaticStrings::Endswith, 0),
                    (StaticStrings::Strip, 0),
                    (StaticStrings::Lstrip, 0),
                    (StaticStrings::Rstrip, 0),
                    (StaticStrings::Removeprefix, 0),
                    (StaticStrings::Removesuffix, 0),
                    (StaticStrings::Split, 0),
                    (StaticStrings::Rsplit, 0),
                    (StaticStrings::Splitlines, 0),
                    (StaticStrings::Partition, 0),
                    (StaticStrings::Rpartition, 0),
                    (StaticStrings::Replace, 0),
                    (StaticStrings::Center, 0),
                    (StaticStrings::Ljust, 0),
                    (StaticStrings::Rjust, 0),
                    (StaticStrings::Zfill, 0),
                    (StaticStrings::Expandtabs, 0),
                    (StaticStrings::Format, 0),
                    (StaticStrings::Encode, 0),
                    (StaticStrings::Isidentifier, 0),
                    (StaticStrings::Istitle, 0),
                    (StaticStrings::Isprintable, 0),
                    (StaticStrings::Join, 0),
                ],
            ),
            (
                Type::List,
                &[
                    (StaticStrings::Append, 0),
                    (StaticStrings::Insert, 0),
                    (StaticStrings::Pop, 0),
                    (StaticStrings::Remove, 0),
                    (StaticStrings::Clear, 0),
                    (StaticStrings::Copy, 0),
                    (StaticStrings::Extend, 0),
                    (StaticStrings::Index, 0),
                    (StaticStrings::Count, 0),
                    (StaticStrings::Reverse, 0),
                    (StaticStrings::Sort, 0),
                ],
            ),
            (Type::Tuple, &[(StaticStrings::Index, 0), (StaticStrings::Count, 0)]),
            (
                Type::Complex,
                &[(StaticStrings::Conjugate, 0), (StaticStrings::FromNumber, 1)],
            ),
            (
                Type::Date,
                &[
                    (StaticStrings::Isoformat, 0),
                    (StaticStrings::Strftime, 0),
                    (StaticStrings::Replace, 0),
                    (StaticStrings::Weekday, 0),
                    (StaticStrings::Isoweekday, 0),
                    (StaticStrings::Today, 1),
                    (StaticStrings::Fromisoformat, 1),
                    (StaticStrings::Min, 2),
                    (StaticStrings::Max, 2),
                    (StaticStrings::Resolution, 2),
                ],
            ),
            (
                Type::Time,
                &[
                    (StaticStrings::Isoformat, 0),
                    (StaticStrings::Strftime, 0),
                    (StaticStrings::Replace, 0),
                    (StaticStrings::Utcoffset, 0),
                    (StaticStrings::Tzname, 0),
                    (StaticStrings::Dst, 0),
                    (StaticStrings::Fromisoformat, 1),
                    (StaticStrings::Min, 2),
                    (StaticStrings::Max, 2),
                    (StaticStrings::Resolution, 2),
                ],
            ),
            (
                Type::TimeDelta,
                &[
                    (StaticStrings::TotalSeconds, 0),
                    (StaticStrings::Min, 2),
                    (StaticStrings::Max, 2),
                    (StaticStrings::Resolution, 2),
                ],
            ),
            (
                Type::TimeZone,
                &[
                    (StaticStrings::Utcoffset, 0),
                    (StaticStrings::Tzname, 0),
                    (StaticStrings::Dst, 0),
                    (StaticStrings::Utc, 2),
                    (StaticStrings::Min, 2),
                    (StaticStrings::Max, 2),
                ],
            ),
            (
                Type::RePattern,
                &[
                    (StaticStrings::Search, 0),
                    (StaticStrings::Match, 0),
                    (StaticStrings::Fullmatch, 0),
                    (StaticStrings::Findall, 0),
                    (StaticStrings::Sub, 0),
                    (StaticStrings::Split, 0),
                    (StaticStrings::Finditer, 0),
                ],
            ),
            (
                Type::ReMatch,
                &[
                    (StaticStrings::Group, 0),
                    (StaticStrings::Groups, 0),
                    (StaticStrings::Groupdict, 0),
                    (StaticStrings::Start, 0),
                    (StaticStrings::End, 0),
                    (StaticStrings::Span, 0),
                ],
            ),
        ];
        for &(owner, entries) in expected {
            let actual = attrs(owner)
                .iter()
                .map(|&(name, definition)| {
                    let kind = match definition {
                        AttrDef::Method(_) => 0,
                        AttrDef::ClassMethod(_) => 1,
                        AttrDef::Value(_) => 2,
                    };
                    (name, kind)
                })
                .collect::<Vec<_>>();
            assert_eq!(actual, entries, "{owner:?}");
        }
    }

    #[test]
    fn tables_and_lookups_agree() {
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
            assert!(lookup_attr(owner, StaticStrings::Fromkeys).is_none());
            for &(name, definition) in attrs(owner) {
                let found = lookup_attr(owner, name).expect("table entry has a lookup arm");
                assert_eq!(discriminant(&definition), discriminant(&found));
            }
        }
        assert!(attrs(Type::Int).is_empty());
        assert!(lookup_attr(Type::Int, StaticStrings::Upper).is_none());
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
