//! Static attribute definitions for builtin types.

use std::{marker::PhantomData, mem::ManuallyDrop};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::RunResult,
    heap::HeapObjectRead,
    intern::StaticStrings,
    types::{Type, complex, date, list, re_match, re_pattern, time, timedelta, timezone, tuple},
    value::Value,
};

pub(crate) type BuiltinCall = for<'h> fn(StaticStrings, Type, Value, ArgValues, &mut VM<'h>) -> RunResult<CallResult>;
pub(crate) type MethodCall = for<'h> fn(&Value, ArgValues, &mut VM<'h>) -> RunResult<Value>;

/// A namespace's native receiver strategy and Python attribute definitions.
pub(crate) trait PyAttrs {
    type Receiver;
    const ATTRS: &'static [(StaticStrings, AttrDef)];
}

pub(crate) struct MutHeap<T>(PhantomData<T>);
pub(crate) struct Heap<T>(PhantomData<T>);
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "supported for immediate builtin implementations")
)]
pub(crate) struct Immediate<T>(PhantomData<T>);

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
        Type::List => <list::ListImpl as PyAttrs>::ATTRS,
        Type::Tuple => <tuple::TupleImpl as PyAttrs>::ATTRS,
        Type::Complex => <complex::ComplexImpl as PyAttrs>::ATTRS,
        Type::Date => <date::DateImpl as PyAttrs>::ATTRS,
        Type::Time => <time::TimeImpl as PyAttrs>::ATTRS,
        Type::TimeDelta => <timedelta::TimeDeltaImpl as PyAttrs>::ATTRS,
        Type::TimeZone => <timezone::TimeZoneImpl as PyAttrs>::ATTRS,
        Type::RePattern => <re_pattern::RePatternImpl as PyAttrs>::ATTRS,
        Type::ReMatch => <re_match::ReMatchImpl as PyAttrs>::ATTRS,
        _ => &[],
    }
}

pub(crate) const fn lookup_attr(owner: Type, name: StaticStrings) -> Option<AttrDef> {
    match owner {
        Type::List => list::ListImpl::lookup_attr(name),
        Type::Tuple => tuple::TupleImpl::lookup_attr(name),
        Type::Complex => complex::ComplexImpl::lookup_attr(name),
        Type::Date => date::DateImpl::lookup_attr(name),
        Type::Time => time::TimeImpl::lookup_attr(name),
        Type::TimeDelta => timedelta::TimeDeltaImpl::lookup_attr(name),
        Type::TimeZone => timezone::TimeZoneImpl::lookup_attr(name),
        Type::RePattern => re_pattern::RePatternImpl::lookup_attr(name),
        Type::ReMatch => re_match::ReMatchImpl::lookup_attr(name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::mem::discriminant;

    use monty_types::{PrintWriter, ResourceTracker};

    use super::*;
    use crate::{
        heap::{Heap as ObjectHeap, HeapData, HeapReader},
        run::{Program, SessionTables},
        types::List,
    };

    struct FloatImpl;

    #[monty_macros::pyattrs]
    impl PyAttrs for FloatImpl {
        type Receiver = Immediate<f64>;

        #[pymethod]
        fn conjugate(value: f64, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
            args.check_zero_args("float.conjugate", vm.heap)?;
            Ok(Value::Float(value))
        }
    }

    #[test]
    fn generated_adapters_use_native_receivers() {
        const SORT: Option<AttrDef> = list::ListImpl::lookup_attr(StaticStrings::Sort);
        const LIST_TABLE: &[(StaticStrings, AttrDef)] = attrs(Type::List);
        assert!(matches!(SORT, Some(AttrDef::Method(_))));
        assert_eq!(LIST_TABLE.len(), <list::ListImpl as PyAttrs>::ATTRS.len());

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

            let Some(AttrDef::Method(count)) = tuple::TupleImpl::lookup_attr(StaticStrings::Count) else {
                panic!("expected count method")
            };
            assert!(matches!(
                count(&owned.1, ArgValues::One(Value::Int(2)), vm).unwrap(),
                Value::Int(2)
            ));

            let Some(AttrDef::Method(conjugate)) = FloatImpl::lookup_attr(StaticStrings::Conjugate) else {
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
            for &(name, definition) in attrs(owner) {
                let found = lookup_attr(owner, name).expect("table entry has a lookup arm");
                assert_eq!(discriminant(&definition), discriminant(&found));
            }
        }
    }
}
