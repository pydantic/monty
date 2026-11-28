use monty_types::{PrintWriter, ResourceTracker};
use num_bigint::BigInt;

use super::*;
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    heap::{Heap, HeapData, HeapReader},
    heap_data::HeapKind,
    intern::{CompileInterns, Interns},
    run::{Program, SessionTables},
    types::{Bytes, List, LongInt, NamedTuple, str::allocate_string},
};

fn with_test_vm<R>(f: impl for<'h> FnOnce(&mut VM<'h>) -> R) -> R {
    let mut heap = Heap::new(16, ResourceTracker::default());
    let mut tables = SessionTables::default();
    let program = Program::for_tests();
    HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
        let mut vm = VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
        f(&mut vm)
    })
}

#[test]
fn access_allocated_payload_by_type() {
    with_test_vm(|vm| {
        let bytes = Value::allocate(Bytes::new(vec![1, 2]), vm.heap);
        let point = Value::allocate(NamedTuple::new("Point".to_owned(), vec![], vec![]), vm.heap);

        assert_eq!(
            bytes.read_as::<Bytes>(vm.heap).unwrap().get(vm.heap).as_slice(),
            &[1, 2]
        );
        assert!(bytes.read_as::<List>(vm.heap).is_none());
        assert!(Value::None.read_as::<Bytes>(vm.heap).is_none());

        assert_eq!(bytes.get_as::<Bytes>(vm.heap).unwrap().as_slice(), &[1, 2]);
        assert_eq!(point.get_as::<NamedTuple>(vm.heap).unwrap().name(vm.interns), "Point");
        assert!(point.get_as::<Bytes>(vm.heap).is_none());
        assert!(Value::None.get_as::<Bytes>(vm.heap).is_none());

        point.drop_with(vm);
        bytes.drop_with(vm);
    });
}

#[test]
fn borrow_bytes_from_intern_table_or_heap() {
    let mut interns = Interns::default();
    let id = CompileInterns::direct(&mut interns).intern_bytes(b"literal");
    let mut heap = Heap::new(16, ResourceTracker::default());
    let literal = Value::InternBytes(id);
    let allocated = Value::allocate(Bytes::from(&b"allocated"[..]), &heap);
    assert_eq!(literal.as_bytes_heap(&heap, &interns), Some(&b"literal"[..]));
    assert_eq!(allocated.as_bytes_heap(&heap, &interns), Some(&b"allocated"[..]));
    assert_eq!(Value::None.as_bytes_heap(&heap, &interns), None);
    allocated.drop_with(&mut heap);
}

#[test]
fn extract_bad_arg_named() {
    with_test_vm(|vm| {
        let res = StrArg::extract(
            Value::Int(42),
            vm,
            ArgErrCtx::BadArgNamed {
                func_name: "test_fn",
                arg_name: "param",
            },
        );
        let exc = res.unwrap_err().into_python_exception(vm.interns, |_| None);
        assert_eq!(exc.message(), Some("test_fn() argument 'param' must be str, not int"));
    });
}

#[test]
fn extract_bad_arg_pos() {
    with_test_vm(|vm| {
        let res = StrArg::extract(
            Value::Int(42),
            vm,
            ArgErrCtx::BadArgPos {
                func_name: "test_fn",
                pos: 2,
            },
        );
        let exc = res.unwrap_err().into_python_exception(vm.interns, |_| None);
        assert_eq!(exc.message(), Some("test_fn() argument 2 must be str, not int"));
    });
}

#[test]
fn extract_plain() {
    with_test_vm(|vm| {
        let res = StrArg::extract(Value::Int(42), vm, ArgErrCtx::Plain);
        let exc = res.unwrap_err().into_python_exception(vm.interns, |_| None);
        assert_eq!(exc.message(), Some("expected str, not int"));
    });
}

#[test]
fn extract_raise_overflow() {
    with_test_vm(|vm| {
        let res_max = i32::extract(Value::Int(i64::from(i32::MAX) + 1), vm, ArgErrCtx::Plain);
        let exc_max = res_max.unwrap_err().into_python_exception(vm.interns, |_| None);
        assert_eq!(exc_max.message(), Some("signed integer is greater than maximum"));

        let res_min = i32::extract(Value::Int(i64::from(i32::MIN) - 1), vm, ArgErrCtx::Plain);
        let exc_min = res_min.unwrap_err().into_python_exception(vm.interns, |_| None);
        assert_eq!(exc_min.message(), Some("signed integer is less than minimum"));
    });
}

#[test]
fn i32_conversions() {
    with_test_vm(|vm| {
        assert_eq!(i32::from_value(Value::Bool(true), vm).unwrap(), 1);
        assert_eq!(i32::from_value(Value::Bool(false), vm).unwrap(), 0);
        assert_eq!(i32::from_value(Value::Int(42), vm).unwrap(), 42);

        let val = Value::None;
        let err = i32::from_value(val, vm).unwrap_err();
        if let FromValueFail::WrongType(v) = err {
            v.drop_with(vm);
        } else {
            panic!("expected WrongType");
        }

        let id = vm
            .heap
            .allocate(HeapData::LongInt(LongInt::new(BigInt::from(i64::MAX) + 1)));
        let val = Value::Ref(HeapKind::LongInt, id);
        assert!(matches!(i32::from_value(val, vm), Err(FromValueFail::Raise(_))));

        let type_err = i32::type_error("str").into_python_exception(vm.interns, |_| None);
        assert_eq!(
            type_err.message(),
            Some("'str' object cannot be interpreted as an integer")
        );
    });
}

#[test]
fn i64_conversions() {
    with_test_vm(|vm| {
        assert_eq!(i64::from_value(Value::Bool(true), vm).unwrap(), 1);
        assert_eq!(i64::from_value(Value::Int(42), vm).unwrap(), 42);

        let val = Value::None;
        let err = i64::from_value(val, vm).unwrap_err();
        if let FromValueFail::WrongType(v) = err {
            v.drop_with(vm);
        } else {
            panic!("expected WrongType");
        }

        let id = vm
            .heap
            .allocate(HeapData::LongInt(LongInt::new(BigInt::from(i64::MAX) + 1)));
        let val = Value::Ref(HeapKind::LongInt, id);
        assert!(matches!(i64::from_value(val, vm), Err(FromValueFail::Raise(_))));

        let type_err = i64::type_error("str").into_python_exception(vm.interns, |_| None);
        assert_eq!(
            type_err.message(),
            Some("'str' object cannot be interpreted as an integer")
        );
    });
}

#[test]
fn isize_conversions() {
    with_test_vm(|vm| {
        assert_eq!(isize::from_value(Value::Bool(true), vm).unwrap(), 1);
        assert_eq!(isize::from_value(Value::Int(42), vm).unwrap(), 42);

        let val = Value::None;
        let err = isize::from_value(val, vm).unwrap_err();
        if let FromValueFail::WrongType(v) = err {
            v.drop_with(vm);
        } else {
            panic!("expected WrongType");
        }

        let id = vm
            .heap
            .allocate(HeapData::LongInt(LongInt::new(BigInt::from(i64::MAX) + 1)));
        let val = Value::Ref(HeapKind::LongInt, id);
        assert!(matches!(isize::from_value(val, vm), Err(FromValueFail::Raise(_))));

        let type_err = isize::type_error("str").into_python_exception(vm.interns, |_| None);
        assert_eq!(
            type_err.message(),
            Some("'str' object cannot be interpreted as an integer")
        );
    });
}

#[test]
fn u64_conversions() {
    with_test_vm(|vm| {
        assert_eq!(u64::from_value(Value::Bool(true), vm).unwrap(), 1);
        assert_eq!(u64::from_value(Value::Int(0), vm).unwrap(), 0);
        assert_eq!(u64::from_value(Value::Int(42), vm).unwrap(), 42);

        assert!(matches!(
            u64::from_value(Value::Int(-1), vm),
            Err(FromValueFail::Raise(_))
        ));

        let val = Value::None;
        let err = u64::from_value(val, vm).unwrap_err();
        if let FromValueFail::WrongType(v) = err {
            v.drop_with(vm);
        } else {
            panic!("expected WrongType");
        }

        // Negative BigInt
        let id = vm.heap.allocate(HeapData::LongInt(LongInt::new(BigInt::from(-100))));
        let val = Value::Ref(HeapKind::LongInt, id);
        assert!(matches!(u64::from_value(val, vm), Err(FromValueFail::Raise(_))));

        // Positive BigInt within u64
        let id = vm
            .heap
            .allocate(HeapData::LongInt(LongInt::new(BigInt::from(u64::MAX))));
        let val = Value::Ref(HeapKind::LongInt, id);
        assert_eq!(u64::from_value(val, vm).unwrap(), u64::MAX);

        // BigInt overflow
        let id = vm
            .heap
            .allocate(HeapData::LongInt(LongInt::new(BigInt::from(u64::MAX) + 1)));
        let val = Value::Ref(HeapKind::LongInt, id);
        assert!(matches!(u64::from_value(val, vm), Err(FromValueFail::Raise(_))));

        let type_err = u64::type_error("str").into_python_exception(vm.interns, |_| None);
        assert_eq!(
            type_err.message(),
            Some("'str' object cannot be interpreted as an integer")
        );
    });
}

#[test]
fn bool_conversions() {
    with_test_vm(|vm| {
        assert!(bool::from_value(Value::Bool(true), vm).unwrap());
        assert!(!bool::from_value(Value::Bool(false), vm).unwrap());

        let err = bool::from_value(Value::Int(1), vm).unwrap_err();
        if let FromValueFail::WrongType(v) = err {
            v.drop_with(vm);
        } else {
            panic!("expected WrongType");
        }

        let type_err = bool::type_error("int").into_python_exception(vm.interns, |_| None);
        assert_eq!(type_err.message(), Some("a bool is required"));
    });
}

#[test]
fn str_arg_conversions() {
    with_test_vm(|vm| {
        let s = allocate_string("hello", vm.heap);
        let str_arg = StrArg::from_value(s, vm).unwrap();
        assert_eq!(str_arg.as_str(vm), "hello");
        str_arg.drop_extracted(vm.heap);

        let s2 = allocate_string("world", vm.heap);
        let str_arg2 = StrArg::from_value(s2, vm).unwrap();
        str_arg2.drop_with(vm);

        let err = StrArg::from_value(Value::Int(42), vm).unwrap_err();
        if let FromValueFail::WrongType(v) = err {
            v.drop_with(vm);
        } else {
            panic!("expected WrongType");
        }
    });
}

#[test]
fn option_and_lax_bool_and_value() {
    with_test_vm(|vm| {
        // Option<T>
        assert_eq!(Option::<i64>::from_value(Value::Int(42), vm).unwrap(), Some(42));
        Option::<StrArg>::drop_extracted(None, vm.heap);
        let s = allocate_string("opt", vm.heap);
        let opt = Option::<StrArg>::from_value(s, vm).unwrap();
        opt.drop_extracted(vm.heap);

        let opt_err = Option::<i64>::type_error("str").into_python_exception(vm.interns, |_| None);
        assert_eq!(
            opt_err.message(),
            Some("'str' object cannot be interpreted as an integer")
        );

        // LaxBool
        assert!(LaxBool::from_value(Value::Bool(true), vm).unwrap().bool());
        assert!(!LaxBool::from_value(Value::Int(0), vm).unwrap().bool());
        assert!(LaxBool::new(true).bool());

        // Value::from_value & drop_extracted
        assert!(matches!(Value::from_value(Value::Int(42), vm), Ok(Value::Int(42))));
        Value::Int(42).drop_extracted(vm.heap);

        // extract_into
        let mut slot = None;
        i64::extract_into(Value::Int(42), &mut slot, vm, ArgErrCtx::Plain).unwrap();
        assert_eq!(slot, Some(42));

        // resolve_index_dunder
        assert!(matches!(resolve_index_dunder(Value::Int(42), vm), Ok(Value::Int(42))));
        assert!(matches!(
            resolve_index_dunder(Value::Bool(true), vm),
            Ok(Value::Bool(true))
        ));
    });
}

#[derive(Debug)]
struct DummyUnconstrained;

impl FromValue for DummyUnconstrained {
    fn from_value(value: Value, _vm: &mut VM<'_>) -> Result<Self, FromValueFail> {
        Err(FromValueFail::WrongType(value))
    }
}

#[test]
fn default_trait_impls_and_unconstrained() {
    with_test_vm(|vm| {
        // Default drop_extracted no-op
        DummyUnconstrained.drop_extracted(vm.heap);

        // Default type_error with None EXPECTED_TYPE_NAME
        let err = DummyUnconstrained::type_error("int").into_python_exception(vm.interns, |_| None);
        assert_eq!(err.message(), Some("expected a different type, not int"));

        // report_extract_fail fallback with BadArgPos and BadArgNamed when EXPECTED_TYPE_NAME is None
        let res_pos = DummyUnconstrained::extract(
            Value::Int(1),
            vm,
            ArgErrCtx::BadArgPos {
                func_name: "test_fn",
                pos: 1,
            },
        );
        let exc_pos = res_pos.unwrap_err().into_python_exception(vm.interns, |_| None);
        assert_eq!(exc_pos.message(), Some("expected a different type, not int"));

        let res_named = DummyUnconstrained::extract(
            Value::Int(1),
            vm,
            ArgErrCtx::BadArgNamed {
                func_name: "test_fn",
                arg_name: "x",
            },
        );
        let exc_named = res_named.unwrap_err().into_python_exception(vm.interns, |_| None);
        assert_eq!(exc_named.message(), Some("expected a different type, not int"));
    });
}

#[test]
fn long_int_and_index_dunder_edge_cases() {
    with_test_vm(|vm| {
        // non-LongInt heap value
        let s = allocate_string("long enough to require a heap string", vm.heap);
        assert!(!s.is_long_int());
        assert!(s.as_long_int(vm).is_none());
        assert!(!Value::None.is_long_int());
        assert!(Value::None.as_long_int(vm).is_none());

        // resolve_index_dunder with None and Str
        assert!(matches!(resolve_index_dunder(Value::None, vm), Ok(Value::None)));
        let s_dunder = resolve_index_dunder(s, vm).unwrap();
        s_dunder.drop_with(vm);
    });
}

#[derive(FromArgs)]
#[from_args(name = "test_vm_field")]
struct VmFirstArgs {
    #[from_args(pos_only)]
    vm: i64,
    #[from_args(pos_only)]
    other: i64,
    #[from_args(pos_only, default = { return 7; })]
    extra: i64,
}

#[test]
fn from_args_vm_field_and_block_default() {
    with_test_vm(|vm| {
        let args = ArgValues::Two(Value::Int(10), Value::Int(20));
        let parsed = VmFirstArgs::from_args(args, vm).unwrap();
        assert_eq!(parsed.vm, 10);
        assert_eq!(parsed.other, 20);
        assert_eq!(parsed.extra, 7);
    });
}
