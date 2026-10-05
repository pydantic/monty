use std::{fmt, str::FromStr};

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{Dict, Instance, PyTrait, Type, allocate_tuple, class::native_base},
    value::{EitherStr, Value},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, strum::EnumString)]
pub(crate) enum NativeOp {
    #[strum(serialize = "__new__")]
    New,
    #[strum(serialize = "__init__")]
    Init,
    #[strum(serialize = "__len__")]
    Len,
    #[strum(serialize = "__getitem__")]
    Getitem,
    #[strum(serialize = "__setitem__")]
    Setitem,
    #[strum(serialize = "__iter__")]
    Iter,
    #[strum(serialize = "__contains__")]
    Contains,
    #[strum(serialize = "__eq__")]
    Eq,
    #[strum(serialize = "__repr__")]
    Repr,
    #[strum(serialize = "__str__")]
    Str,
    #[strum(serialize = "append")]
    Append,
    #[strum(serialize = "insert")]
    Insert,
    #[strum(serialize = "pop")]
    Pop,
    #[strum(serialize = "remove")]
    Remove,
    #[strum(serialize = "clear")]
    Clear,
    #[strum(serialize = "copy")]
    Copy,
    #[strum(serialize = "extend")]
    Extend,
    #[strum(serialize = "index")]
    Index,
    #[strum(serialize = "count")]
    Count,
    #[strum(serialize = "reverse")]
    Reverse,
    #[strum(serialize = "sort")]
    Sort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct NativeMethod {
    pub owner: Type,
    pub op: NativeOp,
}

impl fmt::Display for NativeMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}.{:?}", self.owner, self.op)
    }
}

pub(crate) fn member(owner: Type, name: &str) -> Option<Value> {
    if owner == Type::List && name == "__hash__" {
        return Some(Value::None);
    }
    let op = NativeOp::from_str(name).ok()?;
    let allowed = match owner {
        Type::Object => op == NativeOp::New,
        Type::List => op != NativeOp::Str,
        Type::Exception(_) => matches!(op, NativeOp::New | NativeOp::Init),
        _ => false,
    };
    allowed.then_some(Value::ModuleFunction(ModuleFunctions::NativeClass(NativeMethod {
        owner,
        op,
    })))
}

pub(crate) fn clone_args(args: &ArgValues, vm: &VM<'_>) -> ArgValues {
    fn keywords(kwargs: &KwargsValues, vm: &VM<'_>) -> KwargsValues {
        match kwargs {
            KwargsValues::Empty => KwargsValues::Empty,
            KwargsValues::Inline(items) => {
                KwargsValues::Inline(items.iter().map(|(k, v)| (*k, v.clone_with_heap(vm.heap))).collect())
            }
            KwargsValues::Pairs(items) => KwargsValues::Pairs(
                items
                    .iter()
                    .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                    .collect(),
            ),
            KwargsValues::Dict(dict) => KwargsValues::Pairs(
                dict.iter()
                    .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                    .collect(),
            ),
        }
    }
    match args {
        ArgValues::Empty => ArgValues::Empty,
        ArgValues::One(v) => ArgValues::One(v.clone_with_heap(vm.heap)),
        ArgValues::Two(a, b) => ArgValues::Two(a.clone_with_heap(vm.heap), b.clone_with_heap(vm.heap)),
        ArgValues::Kwargs(k) => ArgValues::Kwargs(keywords(k, vm)),
        ArgValues::ArgsKargs { args, kwargs } => ArgValues::from_parts(
            args.iter().map(|v| v.clone_with_heap(vm.heap)).collect(),
            keywords(kwargs, vm),
        ),
    }
}

pub(crate) fn allocate_instance(class: HeapId, arguments: &ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let native = match native_base(class, vm) {
        Some(Type::List) => Some(Type::List.call(vm, ArgValues::Empty)?),
        _ => None,
    };
    let mut attrs = Dict::new();
    if matches!(native_base(class, vm), Some(Type::Exception(_))) {
        let copied = clone_args(arguments, vm);
        let (pos, kwargs) = copied.into_parts();
        kwargs.drop_with(vm);
        let args = allocate_tuple(pos.collect(), vm.heap);
        attrs = Dict::from_pairs(
            vec![(Value::InternString(vm.interns.intern_static(StaticStrings::Args)), args)],
            vm,
        )?;
    }
    vm.heap.inc_ref(class);
    let id = vm.heap.allocate(HeapData::Instance(Box::new(Instance::with_native(
        class, attrs, native,
    ))));
    Ok(Value::Ref(id))
}

pub(crate) fn list_storage(value: &Value, vm: &VM<'_>) -> Option<HeapId> {
    let Value::Ref(id) = value else {
        return None;
    };
    match vm.heap.get(*id) {
        HeapData::List(_) => Some(*id),
        HeapData::Instance(instance) => match instance.native() {
            Some(Value::Ref(id)) if matches!(vm.heap.get(*id), HeapData::List(_)) => Some(*id),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn exception_type(id: HeapId, vm: &VM<'_>) -> Option<ExcType> {
    let HeapData::Instance(instance) = vm.heap.get(id) else {
        return None;
    };
    match native_base(instance.class(), vm) {
        Some(Type::Exception(exc)) => Some(exc),
        _ => None,
    }
}

impl NativeMethod {
    pub fn binds_instance(self) -> bool {
        self.op != NativeOp::New
    }

    pub fn call(self, vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
        let (mut positional, kwargs) = args.into_parts();
        let Some(receiver) = positional.next() else {
            kwargs.drop_with(vm);
            return Err(ExcType::type_error("native method requires a receiver"));
        };
        let args = ArgValues::from_parts(positional.collect(), kwargs);
        defer_drop!(receiver, vm);
        if self.op == NativeOp::New {
            defer_drop!(args, vm);
            let Value::Ref(class) = receiver else {
                return Err(ExcType::type_error("__new__ requires a sandbox class"));
            };
            if !matches!(vm.heap.get(*class), HeapData::Class(_)) {
                return Err(ExcType::type_error("__new__ requires a sandbox class"));
            }
            let layout = native_base(*class, vm).unwrap_or(Type::Object);
            let compatible = match (self.owner, layout) {
                (Type::Exception(a), Type::Exception(b)) => b.is_subclass_of(a),
                (a, b) => a == b,
            };
            if !compatible {
                return Err(ExcType::type_error("__new__ called with an incompatible native base"));
            }
            return allocate_instance(*class, args, vm).map(CallResult::Value);
        }
        if let Type::Exception(owner) = self.owner {
            let args = args.reject_kwargs("BaseException.__init__", vm.heap)?;
            let Value::Ref(id) = receiver else {
                args.drop_with(vm);
                return Err(ExcType::type_error("exception method requires an exception instance"));
            };
            if !exception_type(*id, vm).is_some_and(|exc| exc.is_subclass_of(owner)) {
                args.drop_with(vm);
                return Err(ExcType::type_error(
                    "exception method requires a compatible exception instance",
                ));
            }
            let (pos, kwargs) = args.into_parts();
            kwargs.drop_with(vm);
            let tuple = allocate_tuple(pos.collect(), vm.heap);
            let HeapReadOutput::Instance(mut instance) = vm.heap.read(*id) else {
                unreachable!()
            };
            let name = Value::InternString(vm.interns.intern_static(StaticStrings::Args));
            instance.set_attr_unchecked(name, tuple, vm)?.drop_with(vm);
            return Ok(CallResult::Value(Value::None));
        }
        let Some(id) = list_storage(receiver, vm) else {
            args.drop_with(vm);
            return Err(ExcType::type_error("list method requires a list instance"));
        };
        if self.op == NativeOp::Init {
            let values = Type::List.call(vm, args)?;
            defer_drop!(values, vm);
            let Some(HeapReadOutput::List(source)) = values.read_heap(vm) else {
                unreachable!()
            };
            let items = source.clone_all_items(vm)?;
            let HeapReadOutput::List(mut target) = vm.heap.read(id) else {
                unreachable!()
            };
            let old = target.get_mut(vm.heap).replace_items(items);
            old.drop_with(vm);
            return Ok(CallResult::Value(Value::None));
        }
        vm.heap.inc_ref(id);
        let storage = Value::Ref(id);
        defer_drop!(storage, vm);
        let result = match self.op {
            NativeOp::Len => {
                args.check_zero_args("list.__len__", vm.heap)?;
                Value::Int(i64::try_from(storage.py_len(vm).unwrap()).map_err(|_| ExcType::overflow_c_ssize_t())?)
            }
            NativeOp::Iter => {
                args.check_zero_args("list.__iter__", vm.heap)?;
                storage.py_iter(vm)?
            }
            NativeOp::Repr => {
                args.check_zero_args("list.__repr__", vm.heap)?;
                storage.py_repr(vm)?
            }
            NativeOp::Getitem => {
                let key = args.get_one_arg("list.__getitem__", vm.heap)?;
                defer_drop!(key, vm);
                storage.py_getitem(key, vm)?
            }
            NativeOp::Setitem => {
                let (key, value) = args.get_two_args("list.__setitem__", vm.heap)?;
                let HeapReadOutput::List(mut list) = vm.heap.read(id) else {
                    unreachable!()
                };
                list.py_setitem(key, value, vm)?;
                Value::None
            }
            NativeOp::Contains => {
                let value = args.get_one_arg("list.__contains__", vm.heap)?;
                defer_drop!(value, vm);
                Value::Bool(storage.py_contains(value, vm)?)
            }
            NativeOp::Eq => {
                let other = args.get_one_arg("list.__eq__", vm.heap)?;
                defer_drop!(other, vm);
                if let Some(other_id) = list_storage(other, vm) {
                    vm.heap.inc_ref(other_id);
                    let rhs = Value::Ref(other_id);
                    defer_drop!(rhs, vm);
                    Value::Bool(storage.py_eq(rhs, vm)?)
                } else {
                    Value::NotImplemented
                }
            }
            op => {
                let name = match op {
                    NativeOp::Append => StaticStrings::Append,
                    NativeOp::Insert => StaticStrings::Insert,
                    NativeOp::Pop => StaticStrings::Pop,
                    NativeOp::Remove => StaticStrings::Remove,
                    NativeOp::Clear => StaticStrings::Clear,
                    NativeOp::Copy => StaticStrings::Copy,
                    NativeOp::Extend => StaticStrings::Extend,
                    NativeOp::Index => StaticStrings::Index,
                    NativeOp::Count => StaticStrings::Count,
                    NativeOp::Reverse => StaticStrings::Reverse,
                    NativeOp::Sort => StaticStrings::Sort,
                    _ => {
                        args.drop_with(vm);
                        return Err(ExcType::type_error("unsupported native operation"));
                    }
                };
                let attr = EitherStr::Interned(vm.interns.intern_static(name));
                let HeapReadOutput::List(mut list) = vm.heap.read(id) else {
                    unreachable!()
                };
                return list.py_call_attr(vm, &attr, args);
            }
        };
        Ok(CallResult::Value(result))
    }
}

/// Direct native parents; every chain terminates at object.
pub(crate) fn exception_bases(exc: ExcType) -> Vec<Type> {
    let parent = match exc {
        ExcType::BaseException => return vec![Type::Object],
        ExcType::Exception | ExcType::SystemExit | ExcType::KeyboardInterrupt => ExcType::BaseException,
        ExcType::OverflowError | ExcType::ZeroDivisionError => ExcType::ArithmeticError,
        ExcType::IndexError | ExcType::KeyError => ExcType::LookupError,
        ExcType::NotImplementedError | ExcType::RecursionError => ExcType::RuntimeError,
        ExcType::FrozenInstanceError => ExcType::AttributeError,
        ExcType::UnboundLocalError => ExcType::NameError,
        ExcType::UnicodeDecodeError
        | ExcType::UnicodeEncodeError
        | ExcType::JsonDecodeError
        | ExcType::BinasciiError => ExcType::ValueError,
        ExcType::ModuleNotFoundError => ExcType::ImportError,
        ExcType::FileNotFoundError
        | ExcType::FileExistsError
        | ExcType::IsADirectoryError
        | ExcType::NotADirectoryError
        | ExcType::PermissionError
        | ExcType::TimeoutError => ExcType::OSError,
        ExcType::UnsupportedOperation => {
            return vec![Type::Exception(ExcType::OSError), Type::Exception(ExcType::ValueError)];
        }
        _ => ExcType::Exception,
    };
    vec![Type::Exception(parent)]
}
