use monty_types::{ExcType, PrintWriter, ResourceTracker};

use super::{ArgValues, Bound, KwargsValues, Param, ParamSpec, Value};
use crate::{
    bytecode::VM,
    heap::{DropWithContext, Heap, HeapData, HeapReader},
    intern::StaticStrings,
    run::{Program, SessionTables},
    types::{Bytes, str::allocate_string_on_heap},
};

pub(super) fn with_test_vm<R>(f: impl for<'h> FnOnce(&mut VM<'h>) -> R) -> R {
    let mut heap = Heap::new(16, ResourceTracker::default());
    let mut tables = SessionTables::default();
    let program = Program::for_tests();
    HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
        let mut vm = VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
        f(&mut vm)
    })
}

#[test]
fn positional_only_accepts_required_and_optional_counts() {
    let mut heap = Heap::new(16, ResourceTracker::default());
    for required in 0..=3 {
        let spec = ParamSpec::<3>::positional_only(required);
        for count in 0..=4 {
            let values = (0..count).map(|_| Value::None).collect();
            let args = ArgValues::from_parts(values, KwargsValues::Empty);
            let result = spec.unpack_positional("f", args, &mut heap);
            assert_eq!(result.is_ok(), (required..=3).contains(&count));
            if let Ok(slots) = result {
                assert_eq!(slots.map(|slot| slot.is_some()), [count >= 1, count >= 2, count >= 3]);
            }
        }
    }
}

#[test]
fn positional_extractors_accept_an_empty_keyword_collection() {
    let mut heap = Heap::new(16, ResourceTracker::default());
    let args = |values: &[i64]| ArgValues::ArgsKargs {
        args: values.iter().copied().map(Value::Int).collect(),
        kwargs: KwargsValues::Inline(Vec::new()),
    };

    args(&[]).check_zero_args("f", &mut heap).unwrap();
    assert!(matches!(args(&[1]).get_one_arg("f", &mut heap), Ok(Value::Int(1))));
    assert!(matches!(
        args(&[1, 2]).get_two_args("f", &mut heap),
        Ok((Value::Int(1), Value::Int(2)))
    ));

    assert!(matches!(args(&[]).get_zero_one_arg("f", &mut heap), Ok(None)));
    assert!(matches!(
        args(&[1]).get_zero_one_arg("f", &mut heap),
        Ok(Some(Value::Int(1)))
    ));
    assert!(matches!(
        args(&[1]).get_one_two_args("f", &mut heap),
        Ok((Value::Int(1), None))
    ));
    assert!(matches!(
        args(&[1, 2]).get_one_two_args("f", &mut heap),
        Ok((Value::Int(1), Some(Value::Int(2))))
    ));
}

#[test]
fn bind_named_slots_and_collect_extra_arguments() {
    with_test_vm(|vm| {
        let spec = ParamSpec::new([
            Param::pos_only("x"),
            Param::keyword(StaticStrings::Start).optional(),
            Param::kw_only(StaticStrings::Key),
        ])
        .varargs("args")
        .varkwargs("kwargs");

        let key = vm.interns.intern_static(StaticStrings::Key);
        let extra = vm.interns.intern_static(StaticStrings::Stop);
        let args = ArgValues::ArgsKargs {
            args: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
            kwargs: KwargsValues::Inline(vec![(key, Value::Int(4)), (extra, Value::Int(5))]),
        };
        let mut bound = Bound::new();
        spec.bind("f", &mut bound, args, vm).unwrap();

        assert!(matches!(bound.take(0), Some(Value::Int(1))));
        assert!(matches!(bound.take(1), Some(Value::Int(2))));
        assert!(matches!(bound.take(2), Some(Value::Int(4))));
        assert!(matches!(bound.take_varargs().as_slice(), [Value::Int(3)]));
        let kwargs = bound.take_varkwargs();
        assert_eq!(kwargs.len(), 1);
        assert!(matches!(kwargs.get("stop", vm.heap, vm.interns), Some(Value::Int(5))));
        kwargs.drop_with(vm);
        bound.drop_with(vm);
    });
}

#[test]
fn bind_non_string_keyword_releases_arguments() {
    with_test_vm(|vm| {
        let spec = ParamSpec::new([Param::keyword(StaticStrings::Start)]);
        let unsupported = ParamSpec::new([Param::pos_only("start")]).kwargs_not_supported_yet();
        for (spec, kind, message) in [
            (spec, ExcType::TypeError, "keywords must be strings"),
            (
                unsupported,
                ExcType::NotImplementedError,
                "f() does not yet support keyword arguments",
            ),
        ] {
            #[cfg(feature = "ref-count-return")]
            let before = vm.heap.entry_count();

            // Include an already-bound argument, the offending pair, and an unvisited pair.
            let key = vm.heap.allocate(HeapData::Bytes(Bytes::new(b"key".to_vec())));
            let args = ArgValues::ArgsKargs {
                args: vec![allocate_string_on_heap("positional", vm.heap)],
                kwargs: KwargsValues::Pairs(vec![
                    (Value::Ref(key), allocate_string_on_heap("offending", vm.heap)),
                    (
                        allocate_string_on_heap("later", vm.heap),
                        allocate_string_on_heap("unvisited", vm.heap),
                    ),
                ]),
            };
            let mut bound = Bound::new();
            let error = spec.bind("f", &mut bound, args, vm).unwrap_err();
            bound.drop_with(vm);

            let exception = error.into_python_exception(vm.interns, |_| None);
            assert_eq!(exception.exc_type(), kind);
            assert_eq!(exception.message(), Some(message));
            #[cfg(feature = "ref-count-return")]
            assert_eq!(vm.heap.entry_count(), before);
        }
    });
}
