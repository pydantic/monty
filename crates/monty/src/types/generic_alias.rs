//! `types.GenericAlias` — the runtime value of `list[int]`, `dict[str, int]`,
//! `tuple[int, ...]` and the other parameterized builtin types.
//!
//! An alias records which type was subscripted and with what, and otherwise
//! behaves like the type it parameterizes: calling `list[int](x)` calls
//! `list(x)`, and any attribute other than `__origin__`, `__args__` and
//! `__parameters__` is looked up on the origin. Nothing inspects the
//! arguments, so `list[int]` and `list[3.5]` are equally valid, as in CPython.

use std::{
    fmt::Write,
    hash::{Hash, Hasher},
};

use ahash::AHasher;
use serde::{Deserialize, Serialize};
use smallvec::smallvec;

use crate::{
    args::ArgValues,
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::HashValue,
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::StaticStrings,
    types::{LazyHeapSet, PyTrait, Type, Union, instance::class_name, list::repr_check_time, tuple::allocate_tuple},
    value::{EitherStr, Value},
};

/// A parameterized builtin type such as `list[int]`.
///
/// `args` is an OWNED ref to the `__args__` tuple — the subscript itself when
/// it was a tuple (`dict[str, int]`), otherwise a one-item tuple wrapping it —
/// so `py_dec_ref_ids` and `for_each_child_id` must both release it.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GenericAlias {
    /// The subscripted type, `list` in `list[int]`. Never a class object:
    /// only the builtin types in [`Type::has_class_getitem`] are subscriptable.
    origin: Type,
    /// The `__args__` tuple, always a `Value::Ref` to a `HeapData::Tuple`,
    /// shared with every read of that attribute.
    args: Value,
}

impl GenericAlias {
    /// Builds `origin[key]`, taking ownership of `key`.
    ///
    /// `origin` must satisfy [`Type::has_class_getitem`]; callers raise the
    /// `type 'int' is not subscriptable` error for the rest.
    pub(crate) fn subscript(origin: Type, key: Value, vm: &mut VM<'_>) -> Value {
        debug_assert!(origin.has_class_getitem(), "{origin} is not a generic type");
        // A tuple key is `__args__` itself, as in CPython, so `tuple[()]`
        // has empty args and `list[int, str]` two.
        let is_tuple = matches!(&key, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Tuple(_)));
        let args = if is_tuple {
            key
        } else {
            allocate_tuple(smallvec![key], vm.heap)
        };
        Value::Ref(vm.heap.allocate(HeapData::GenericAlias(Self { origin, args })))
    }

    /// The value `__origin__` reports and a call dispatches to.
    ///
    /// `type` is a builtin function rather than a `Type` value in Monty, so
    /// `type[int]` hands back that function: `type[int].__origin__ is type`
    /// holds and `type[int](x)` reaches `type()`.
    pub(crate) fn origin_value(&self) -> Value {
        match self.origin {
            Type::Type => Value::Builtin(Builtins::Function(BuiltinsFunctions::Type)),
            origin => Value::Builtin(Builtins::Type(origin)),
        }
    }

    /// Invokes `on_child` for the heap id this alias owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = self.args {
            on_child(id);
        }
    }
}

/// Releases the args tuple of an alias abandoned before it reaches the heap;
/// a heap-stored alias is freed through [`HeapItem::py_dec_ref_ids`].
impl<C: ContainsHeap> DropWithContext<C> for GenericAlias {
    fn drop_with(self, ctx: &mut C) {
        self.args.drop_with(ctx);
    }
}

impl HeapItem for GenericAlias {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.args.py_dec_ref_ids(stack);
    }
}

impl<'h> HeapObjectRead<'h, GenericAlias> {
    /// The origin type and an owned ref to the `__args__` tuple.
    fn parts(&self, vm: &mut VM<'h>) -> (Type, Value) {
        let this = self.get(vm.heap);
        (this.origin, this.args.clone_with_heap(vm))
    }

    /// The alias's repr as a plain string, for error messages that name it.
    fn repr_string(&self, vm: &mut VM<'h>) -> RunResult<String> {
        let mut repr = String::new();
        self.py_repr_fmt(&mut repr, vm, &mut LazyHeapSet::default())?;
        Ok(repr)
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, GenericAlias> {
    /// `list[int](x)` is `list(x)`: the subscript is erased at runtime, so the
    /// arguments go straight to the origin type.
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let origin = self.get(vm.heap).origin_value();
        vm.call_function(&origin, args)
    }

    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::GenericAlias
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    /// Two aliases are equal when their origins and `__args__` are.
    ///
    /// Takes a recursion level like the repr: an argument can be a list that
    /// holds this alias, and comparing two such cycles raises `RecursionError`
    /// rather than overflowing the native stack.
    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::GenericAlias(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        let mut guard = vm.recursion_guard()?;
        let vm = &mut *guard;
        let (origin, args) = self.parts(vm);
        defer_drop!(args, vm);
        let (other_origin, other_args) = other.parts(vm);
        defer_drop!(other_args, vm);
        Ok(Some(origin == other_origin && args.py_eq(other_args, vm)?))
    }

    /// Combines the origin with the `__args__` tuple's hash, so `list[int]`
    /// is one dict key however many times it is written; an unhashable
    /// argument (`list[[1]]`) makes the alias unhashable, as in CPython.
    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let (origin, args) = self.parts(vm);
        defer_drop!(args, vm);
        let Some(args_hash) = args.py_hash(vm)? else {
            return Ok(None);
        };
        let mut hasher = AHasher::default();
        origin.hash(&mut hasher);
        args_hash.hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    /// `dict[str, int]`, `tuple[int, ...]`, `tuple[()]`.
    ///
    /// Takes a recursion level like the container reprs: an argument can be
    /// a list that holds this alias, and that cycle prints as `...`.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let Ok(mut guard) = vm.recursion_guard() else {
            return Ok(f.write_str("...")?);
        };
        let vm = &mut *guard;
        let (origin, args) = self.parts(vm);
        defer_drop!(args, vm);
        let Some(HeapReadOutput::Tuple(args)) = args.read_heap(vm) else {
            unreachable!("GenericAlias::args is always a tuple")
        };

        write!(f, "{}[", origin.name(vm.heap, vm.interns))?;
        let count = args.get(vm.heap).as_slice().len();
        if count == 0 {
            f.write_str("()")?;
        }
        for index in 0..count {
            if index > 0 {
                if repr_check_time(index, vm) {
                    f.write_str(", ...[timeout]")?;
                    break;
                }
                f.write_str(", ")?;
            }
            let item = args.clone_item(index, vm);
            defer_drop!(item, vm);
            repr_type_arg(item, f, vm, heap_ids)?;
        }
        Ok(f.write_char(']')?)
    }

    /// `__origin__`, `__args__` and `__parameters__` (always `()`, Monty
    /// having no `TypeVar`s); everything else resolves on the origin type, so
    /// `list[int].__name__` is `'list'` and unknown names report `list`.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let (origin, args) = self.parts(vm);
        defer_drop!(args, vm);
        match attr.static_string(vm.interns) {
            Some(StaticStrings::DunderOrigin) => Ok(Some(CallResult::Value(self.get(vm.heap).origin_value()))),
            Some(StaticStrings::DunderArgs) => Ok(Some(CallResult::Value(args.clone_with_heap(vm)))),
            Some(StaticStrings::DunderParameters) => Ok(Some(CallResult::Value(vm.heap.get_empty_tuple()))),
            _ => Value::Builtin(Builtins::Type(origin)).py_getattr(attr, vm).map(Some),
        }
    }

    /// `list[int].__origin__()` calls the attribute the alias itself carries;
    /// `dict[str, int].fromkeys(...)` and `list[int].__class_getitem__(str)`
    /// dispatch to the origin's classmethods, and `list[int].__name__()` to
    /// the origin's own attributes.
    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        if matches!(
            attr.static_string(vm.interns),
            Some(StaticStrings::DunderOrigin | StaticStrings::DunderArgs | StaticStrings::DunderParameters)
        ) {
            let Some(CallResult::Value(value)) = self.py_getattr(attr, vm)? else {
                unreachable!("the alias's own attributes are always plain values")
            };
            defer_drop!(value, vm);
            return vm.call_function(value, args);
        }
        let origin = self.get(vm.heap).origin;
        match attr {
            EitherStr::Interned(method_id) => origin.call_class_method(*method_id, args, vm),
            // Classmethod names are all interned, so a heap string never names one.
            EitherStr::Heap(name) => {
                args.drop_with(vm);
                Err(ExcType::attribute_error_type(&origin.name(vm.heap, vm.interns), name))
            }
        }
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Union::heap_or(self, other, vm)
    }

    fn py_ror_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Union::heap_ror(self, other, vm)
    }

    /// An alias has no type variables to fill, so `list[int][str]` fails as
    /// CPython's `__parameters__ == ()` case does.
    fn py_getitem(&self, _key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        let repr = self.repr_string(vm)?;
        Err(ExcType::type_error_not_generic_class(&repr))
    }
}

/// Writes one `__args__` item the way CPython's `ga_repr_item` does: a type
/// by its qualified name (`int`, `collections.deque`, a class's bare name),
/// `...` for `Ellipsis`, and the ordinary repr for anything else.
pub(crate) fn repr_type_arg(
    item: &Value,
    f: &mut impl Write,
    vm: &mut VM<'_>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    match item {
        Value::Ellipsis => Ok(f.write_str("...")?),
        Value::Builtin(Builtins::Type(ty)) => Ok(f.write_str(&ty.name(vm.heap, vm.interns))?),
        Value::Builtin(Builtins::ExcType(exc_type)) => Ok(f.write_str(exc_type.into())?),
        Value::Builtin(Builtins::Function(BuiltinsFunctions::Type)) => Ok(f.write_str("type")?),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Class(_) => Ok(f.write_str(&class_name(*id, vm.heap, vm.interns))?),
            HeapData::NamedTupleClass(class) => Ok(f.write_str(class.name(vm.interns))?),
            HeapData::HostClassType(class) => Ok(f.write_str(class.name(vm.interns))?),
            _ => item.py_repr_fmt(f, vm, heap_ids),
        },
        _ => item.py_repr_fmt(f, vm, heap_ids),
    }
}
