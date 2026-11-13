//! `types.GenericAlias` — the runtime value of `list[int]`, `dict[str, int]`,
//! `tuple[int, ...]` and the other parameterized builtin types — and, sharing
//! the type, CPython's `typing._GenericAlias`: `typing.List[int]`,
//! `ClassVar[int]`, `Annotated[int, 'meta']` and the other subscripted
//! `typing` special forms.
//!
//! An alias records which type or form was subscripted and with what, and
//! otherwise behaves like the type it parameterizes: calling `list[int](x)`
//! calls `list(x)`, and any attribute other than `__origin__`, `__args__` and
//! `__parameters__` is looked up on the origin. Nothing inspects the
//! arguments, so `list[int]` and `list[3.5]` are equally valid, as in CPython.

use std::{
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
};

use ahash::{AHashMap, AHasher};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::{
    args::{ArgValues, FromArgs},
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    hash::HashValue,
    heap::{
        ContainsHeap, DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput,
    },
    intern::StaticStrings,
    types::{
        BuiltinType, LazyHeapSet, PyTrait, Type, Union,
        instance::class_name,
        list::repr_should_truncate,
        set::unhashable_type_name,
        tuple::{TupleVec, allocate_tuple},
    },
    value::{EitherStr, Marker, VALUE_SIZE, Value},
};

/// The origin of `typing.Literal[...]`, whose values compare as a set.
const LITERAL: AliasOrigin = AliasOrigin::Special(Marker(StaticStrings::Literal));

/// The origin of `typing.Callable[[params], result]`, stored flattened as `(params..., result)`.
const CALLABLE: AliasOrigin = AliasOrigin::Special(Marker(StaticStrings::Callable));

/// What an alias parameterizes: a builtin type (`list[int]`) or a `typing`
/// special form (`typing.List[int]`, `ClassVar[int]`), CPython's
/// `types.GenericAlias` and `typing._GenericAlias` in one type. Untagged so a
/// builtin origin dumps as the bare type it always did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum AliasOrigin {
    /// A builtin type satisfying [`Type::has_class_getitem`]; never a class object.
    Builtin(BuiltinType),
    /// A `typing` marker satisfying [`Marker::is_generic_form`].
    Special(Marker),
}

/// A parameterized builtin type such as `list[int]`, or `typing` form such as `List[int]`.
///
/// `args` is an OWNED ref to the `__args__` tuple — the subscript itself when
/// it was a tuple (`dict[str, int]`), otherwise a one-item tuple wrapping it —
/// so `py_dec_ref_ids` and `for_each_child_id` must both release it. For
/// `Annotated[T, *metadata]` it holds `T` and the metadata together, which
/// `__args__` and `__metadata__` split.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GenericAlias {
    /// The subscripted type or form, `list` in `list[int]`.
    origin: AliasOrigin,
    /// The subscript as a tuple, always a `Value::Ref` to a `HeapData::Tuple`.
    args: Value,
}

impl GenericAlias {
    /// Builds `origin[key]`, taking ownership of `key`.
    ///
    /// `origin` must satisfy [`Type::has_class_getitem`]; callers raise the
    /// `type 'int' is not subscriptable` error for the rest.
    pub(crate) fn subscript(origin: BuiltinType, key: Value, vm: &mut VM<'_>) -> Value {
        debug_assert!(Type::from(origin).has_class_getitem(), "{origin} is not a generic type");
        Self::allocate(AliasOrigin::Builtin(origin), key, vm)
    }

    /// Builds `typing.Form[key]` for a form satisfying [`Marker::is_generic_form`],
    /// taking ownership of `key`, with the argument checks CPython makes.
    pub(crate) fn subscript_special(form: Marker, key: Value, vm: &mut VM<'_>) -> RunResult<Value> {
        debug_assert!(form.is_generic_form(), "{form:?} is not subscriptable");
        let mut key_guard = DropGuard::new(key, vm);
        let (key, vm) = key_guard.as_parts();
        let count = match key {
            Value::Ref(id) if let HeapData::Tuple(items) = vm.heap.get(*id) => items.as_slice().len(),
            _ => 1,
        };
        match form.0 {
            StaticStrings::Annotated if count < 2 => {
                return Err(ExcType::type_error(
                    "Annotated[...] should be used with at least two arguments (a type and an annotation).",
                ));
            }
            StaticStrings::ClassVar | StaticStrings::FinalType if count != 1 => {
                let repr = key.py_repr(vm)?;
                defer_drop!(repr, vm);
                return Err(ExcType::type_error(format!(
                    "typing.{} accepts only single type. Got {}.",
                    <&str>::from(form.0),
                    repr.as_str(vm).unwrap_or_default()
                )));
            }
            StaticStrings::Callable if count != 2 => {
                return Err(ExcType::type_error(
                    "Callable must be used as Callable[[arg, ...], result].",
                ));
            }
            _ => {}
        }
        let (key, vm) = key_guard.into_parts();
        // `typing` forms normalise a `None` argument to `NoneType`, as CPython's
        // `_type_check` does — except `Literal` values and `Annotated` metadata.
        let key = match form.0 {
            StaticStrings::Literal => literal_params(key, vm)?,
            StaticStrings::Annotated => flatten_annotated(none_to_none_type(key, 1, vm)?, vm)?,
            StaticStrings::Callable => none_to_none_type(callable_params(key, vm)?, usize::MAX, vm)?,
            _ => none_to_none_type(key, usize::MAX, vm)?,
        };
        Ok(Self::allocate(AliasOrigin::Special(form), key, vm))
    }

    /// Allocates the alias, wrapping a non-tuple `key` as a one-item `__args__`.
    fn allocate(origin: AliasOrigin, key: Value, vm: &mut VM<'_>) -> Value {
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

    /// The subscripted type or form.
    pub(crate) fn origin(&self) -> AliasOrigin {
        self.origin
    }

    /// The value `__origin__` reports and a call dispatches to, as an owned value.
    ///
    /// `type` is a builtin function rather than a `Type` value in Monty, so
    /// `type[int]` hands back that function: `type[int].__origin__ is type`
    /// holds and `type[int](x)` reaches `type()`. A `typing` alias of a builtin
    /// (`typing.List[int]`) reports that builtin; `Annotated[T, ...]` reports
    /// `T`; the other forms report themselves.
    pub(crate) fn origin_value(&self, heap: &Heap) -> Value {
        match self.origin {
            AliasOrigin::Builtin(BuiltinType::Type) => Value::Builtin(Builtins::Function(BuiltinsFunctions::Type)),
            AliasOrigin::Builtin(origin) => Value::Builtin(Builtins::Type(origin)),
            AliasOrigin::Special(form) => match form.0 {
                StaticStrings::ListType => Value::Builtin(Builtins::Type(BuiltinType::List)),
                StaticStrings::DictType => Value::Builtin(Builtins::Type(BuiltinType::Dict)),
                StaticStrings::TupleType => Value::Builtin(Builtins::Type(BuiltinType::Tuple)),
                StaticStrings::SetType => Value::Builtin(Builtins::Type(BuiltinType::Set)),
                StaticStrings::FrozenSet => Value::Builtin(Builtins::Type(BuiltinType::FrozenSet)),
                StaticStrings::Type => Value::Builtin(Builtins::Function(BuiltinsFunctions::Type)),
                StaticStrings::Annotated => self.annotated_type(heap),
                _ => Value::Marker(form),
            },
        }
    }

    /// Whether this is `Annotated[T, *metadata]`.
    fn is_annotated(&self) -> bool {
        matches!(self.origin, AliasOrigin::Special(Marker(StaticStrings::Annotated)))
    }

    /// The `T` of `Annotated[T, *metadata]`, owned.
    fn annotated_type(&self, heap: &Heap) -> Value {
        let Value::Ref(id) = self.args else {
            unreachable!("GenericAlias::args is always a tuple")
        };
        let HeapData::Tuple(items) = heap.get(id) else {
            unreachable!("GenericAlias::args is always a tuple")
        };
        items.as_slice()[0].clone_with_heap(heap)
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
    /// The origin and an owned ref to the stored args tuple.
    fn parts(&self, vm: &mut VM<'h>) -> (AliasOrigin, Value) {
        let this = self.get(vm.heap);
        (this.origin, this.args.clone_with_heap(vm))
    }

    /// `__args__`, owned: the stored tuple, or for `Annotated` its type alone.
    fn dunder_args(&self, vm: &mut VM<'h>) -> Value {
        let this = self.get(vm.heap);
        if this.is_annotated() {
            let item = this.annotated_type(vm.heap);
            allocate_tuple(smallvec![item], vm.heap)
        } else {
            this.args.clone_with_heap(vm)
        }
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
    /// arguments go straight to the origin type. A `typing` form refuses, with
    /// CPython's message for a bare generic, an abstract class or a special form.
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let this = self.get(vm.heap);
        if let AliasOrigin::Special(form) = this.origin
            && !this.is_annotated()
        {
            args.drop_with(vm);
            let name: &str = form.0.into();
            let message = match form.0 {
                StaticStrings::ListType
                | StaticStrings::DictType
                | StaticStrings::TupleType
                | StaticStrings::SetType
                | StaticStrings::FrozenSet
                | StaticStrings::Type => {
                    format!(
                        "Type {name} cannot be instantiated; use {}() instead",
                        name.to_lowercase()
                    )
                }
                StaticStrings::Callable => abstract_class_error(name, "'__call__'"),
                StaticStrings::Sequence => abstract_class_error(name, "'__getitem__', '__len__'"),
                StaticStrings::Mapping => abstract_class_error(name, "'__getitem__', '__iter__', '__len__'"),
                StaticStrings::Iterable => abstract_class_error(name, "'__iter__'"),
                StaticStrings::IteratorType => abstract_class_error(name, "'__next__'"),
                StaticStrings::Generator => abstract_class_error(name, "'send', 'throw'"),
                _ => format!("Cannot instantiate typing.{name}"),
            };
            return Err(ExcType::type_error(message));
        }
        let origin = this.origin_value(vm.heap);
        defer_drop!(origin, vm);
        vm.call_function(origin, args)
    }

    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::GenericAlias
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    /// Two aliases are equal when their origins and `__args__` are; `Literal`
    /// compares its values as a set, as CPython does.
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
        if origin != other_origin {
            Ok(Some(false))
        } else if origin == LITERAL {
            literal_eq(args, other_args, vm).map(Some)
        } else {
            args.py_eq(other_args, vm).map(Some)
        }
    }

    /// Combines the origin with the `__args__` tuple's hash, so `list[int]`
    /// is one dict key however many times it is written; an unhashable
    /// argument (`list[[1]]`) makes the alias unhashable, as in CPython.
    /// `Literal` hashes its values in any order, matching its equality.
    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let (origin, args) = self.parts(vm);
        defer_drop!(args, vm);
        let args_hash = if origin == LITERAL {
            literal_hash(args, vm)?
        } else {
            args.py_hash(vm)?
        };
        let Some(args_hash) = args_hash else {
            return Ok(None);
        };
        let mut hasher = AHasher::default();
        // Dicts and sets persist hashes: a builtin origin hashes by Type's
        // original discriminant, a form by its name, which never move.
        match origin {
            AliasOrigin::Builtin(origin) => Type::from(origin).hash(&mut hasher),
            AliasOrigin::Special(form) => <&str>::from(form.0).hash(&mut hasher),
        }
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

        match origin {
            AliasOrigin::Builtin(origin) => write!(f, "{}[", Type::from(origin).name(vm.heap, vm.interns))?,
            AliasOrigin::Special(form) => {
                form.py_repr_fmt(f)?;
                f.write_char('[')?;
            }
        }
        let count = args.get(vm.heap).as_slice().len();
        if count == 0 {
            f.write_str("()")?;
        }
        // `Callable` stores `(params..., result)` and prints the parameter list.
        let (list_end, list_only) = if origin == CALLABLE && count > 0 {
            let first = args.clone_item(0, vm);
            defer_drop!(first, vm);
            if count == 2 && matches!(first, Value::Ellipsis) {
                (0, false)
            } else {
                f.write_char('[')?;
                (count - 1, count == 1)
            }
        } else {
            (0, false)
        };
        if list_only {
            f.write_str("], ")?;
        }
        for index in 0..count {
            if index > 0 {
                if repr_should_truncate(index, vm)? {
                    f.write_str(", ...[timeout]")?;
                    break;
                }
                f.write_str(if index == list_end { "], " } else { ", " })?;
            }
            let item = args.clone_item(index, vm);
            defer_drop!(item, vm);
            if !repr_list_arg(item, f, vm, heap_ids)? {
                repr_type_arg(item, f, vm, heap_ids)?;
            }
        }
        Ok(f.write_char(']')?)
    }

    /// `__origin__`, `__args__`, `__parameters__` (always `()`, Monty having
    /// no `TypeVar`s) and, for `Annotated`, `__metadata__`; everything else
    /// resolves on a builtin origin type, so `list[int].__name__` is `'list'`
    /// and unknown names report `list`, and is an error on a `typing` form.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let origin = self.get(vm.heap).origin;
        match attr.static_string(vm.interns) {
            Some(StaticStrings::DunderOrigin) => Ok(Some(CallResult::Value(self.get(vm.heap).origin_value(vm.heap)))),
            Some(StaticStrings::DunderArgs) => Ok(Some(CallResult::Value(self.dunder_args(vm)))),
            Some(StaticStrings::DunderParameters) => Ok(Some(CallResult::Value(vm.heap.get_empty_tuple()))),
            Some(StaticStrings::DunderMetadata) if self.get(vm.heap).is_annotated() => {
                let (_, args) = self.parts(vm);
                defer_drop!(args, vm);
                let Some(HeapReadOutput::Tuple(args)) = args.read_heap(vm) else {
                    unreachable!("GenericAlias::args is always a tuple")
                };
                let count = args.get(vm.heap).as_slice().len() - 1;
                vm.heap.tracker.check_allocation(count.saturating_mul(VALUE_SIZE))?;
                let metadata = args.get(vm.heap).as_slice()[1..]
                    .iter()
                    .map(|item| item.clone_with_heap(vm.heap))
                    .collect();
                Ok(Some(CallResult::Value(allocate_tuple(metadata, vm.heap))))
            }
            _ => match origin {
                AliasOrigin::Builtin(origin) => Value::Builtin(Builtins::Type(origin)).py_getattr(attr, vm).map(Some),
                // CPython's `_BaseGenericAlias.__getattr__`: a dunder raises with
                // the bare name, anything else is looked up on the origin.
                AliasOrigin::Special(_) => {
                    let name = attr.as_str(vm.interns);
                    if is_dunder(name) {
                        Err(SimpleException::new_msg(ExcType::AttributeError, name).into())
                    } else {
                        let origin = self.get(vm.heap).origin_value(vm.heap);
                        defer_drop!(origin, vm);
                        match origin {
                            // The forms standing in for a `collections.abc` class
                            // report it as the class CPython forwards to.
                            Value::Marker(form) if is_abc_form(*form) => {
                                Err(ExcType::attribute_error_type(form.0.into(), name))
                            }
                            // A special form (`ClassVar`, `Literal`, `NoReturn`) raises the
                            // bare name too, as CPython's `_SpecialForm.__getattr__` does.
                            Value::Marker(form) if form.py_type() == Type::SpecialForm => {
                                Err(SimpleException::new_msg(ExcType::AttributeError, name).into())
                            }
                            _ => origin.py_getattr(attr, vm).map(Some),
                        }
                    }
                }
            },
        }
    }

    /// `list[int].__origin__()` calls the attribute the alias itself carries;
    /// `dict[str, int].fromkeys(...)` and `list[int].__class_getitem__(str)`
    /// dispatch to the origin's classmethods, and `list[int].__name__()` to
    /// the origin's own attributes.
    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let own_attribute = matches!(
            attr.static_string(vm.interns),
            Some(
                StaticStrings::DunderOrigin
                    | StaticStrings::DunderArgs
                    | StaticStrings::DunderParameters
                    | StaticStrings::DunderMetadata
            )
        );
        let origin = self.get(vm.heap).origin_value(vm.heap);
        defer_drop!(origin, vm);
        match self.get(vm.heap).origin {
            AliasOrigin::Builtin(origin) if !own_attribute => Type::from(origin).call_class_method(attr, args, vm),
            // A `typing` form's origin answers the call itself: a host class's
            // lookup may suspend for the host, which only a call-attr carries. A
            // form's own markers take the lookup path below, for its errors.
            AliasOrigin::Special(_)
                if !own_attribute && !is_dunder(attr.as_str(vm.interns)) && !matches!(origin, Value::Marker(_)) =>
            {
                origin.py_value(vm).py_call_attr(vm, attr, args)
            }
            _ => {
                // The guard releases `args` when the attribute lookup fails.
                let mut args_guard = DropGuard::new(args, vm);
                let (_, vm) = args_guard.as_parts_mut();
                let Some(CallResult::Value(value)) = self.py_getattr(attr, vm)? else {
                    unreachable!("the alias's own attributes and a dunder's error are plain values")
                };
                let (args, vm) = args_guard.into_parts();
                defer_drop!(value, vm);
                vm.call_function(value, args)
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

/// Replaces `None` with `NoneType` in the first `limit` items of a subscript
/// (`key` itself when it is not a tuple), taking and returning ownership. The
/// copy is preflighted like any container copy.
fn none_to_none_type(key: Value, limit: usize, vm: &mut VM<'_>) -> RunResult<Value> {
    let none_type = || Value::Builtin(Builtins::Type(BuiltinType::NoneType));
    // The reader on `key` must be released before `key`, possibly the last
    // reference to a fresh tuple, is dropped below.
    let converted = {
        let Some(HeapReadOutput::Tuple(items)) = key.read_heap(vm) else {
            return Ok(if matches!(key, Value::None) { none_type() } else { key });
        };
        let count = items.get(vm.heap).as_slice().len();
        if let Err(err) = vm.heap.tracker.check_allocation(count.saturating_mul(VALUE_SIZE)) {
            drop(items);
            key.drop_with(vm);
            return Err(err.into());
        }
        let mut converted = smallvec![];
        for index in 0..count {
            let item = items.clone_item(index, vm);
            converted.push(if index < limit && matches!(item, Value::None) {
                none_type()
            } else {
                item
            });
        }
        converted
    };
    key.drop_with(vm);
    Ok(allocate_tuple(converted, vm.heap))
}

/// `Annotated[Annotated[T, a], b]` is `Annotated[T, a, b]`: nested metadata
/// flattens, innermost first, as CPython's `Annotated.__class_getitem__` does.
/// Takes and returns ownership of the args tuple.
fn flatten_annotated(key: Value, vm: &mut VM<'_>) -> RunResult<Value> {
    defer_drop!(key, vm);
    // The readers on `key` must be released before it can be dropped.
    let flattened = {
        let Some(HeapReadOutput::Tuple(items)) = key.read_heap(vm) else {
            unreachable!("Annotated takes at least two arguments")
        };
        let first = items.clone_item(0, vm);
        defer_drop!(first, vm);
        match first.read_heap(vm) {
            Some(HeapReadOutput::GenericAlias(inner)) if inner.get(vm.heap).is_annotated() => {
                let (_, inner_args) = inner.parts(vm);
                defer_drop!(inner_args, vm);
                let Some(HeapReadOutput::Tuple(inner_items)) = inner_args.read_heap(vm) else {
                    unreachable!("GenericAlias::args is always a tuple")
                };
                let count = items.get(vm.heap).as_slice().len();
                let inner_count = inner_items.get(vm.heap).as_slice().len();
                // One preflight for the whole buffer: the inner alias's args
                // plus this subscript's metadata.
                vm.heap
                    .tracker
                    .check_allocation((inner_count + count - 1).saturating_mul(VALUE_SIZE))?;
                let mut flattened = TupleVec::with_capacity(inner_count + count - 1);
                for index in 0..inner_count {
                    flattened.push(inner_items.clone_item(index, vm));
                }
                for index in 1..count {
                    flattened.push(items.clone_item(index, vm));
                }
                Some(flattened)
            }
            _ => None,
        }
    };
    Ok(match flattened {
        Some(flattened) => allocate_tuple(flattened, vm.heap),
        None => key.clone_with_heap(vm),
    })
}

/// `Literal`'s parameters as CPython stores them: a nested `Literal` is
/// flattened into its values and duplicates are dropped, where `1` and `True`
/// are distinct values. Takes ownership of `key` and returns the args tuple.
fn literal_params(key: Value, vm: &mut VM<'_>) -> RunResult<Value> {
    defer_drop!(key, vm);
    let written = if matches!(key.read_heap(vm), Some(HeapReadOutput::Tuple(_))) {
        tuple_items(key, vm)?
    } else {
        smallvec![key.clone_with_heap(vm)]
    };
    // One level of flattening: a nested `Literal` was normalised when it was
    // built. The flattened buffer and the deduplicated one it feeds are
    // preflighted together; values are taken out of `written` one at a time and
    // `flattened` is guarded from the start, so a refused copy leaks nothing.
    defer_drop_mut!(written, vm);
    let mut total = 0usize;
    for value in written.iter() {
        total = total.saturating_add(nested_literal_len(value, vm).unwrap_or(1));
    }
    vm.heap.tracker.check_allocation(total.saturating_mul(2 * VALUE_SIZE))?;
    let mut flattened_guard = DropGuard::new(TupleVec::with_capacity(total), vm);
    let (flattened, vm) = flattened_guard.as_parts_mut();
    for index in 0..written.len() {
        vm.heap.tracker.check_time_every(index)?;
        let value = mem::replace(&mut written[index], Value::None);
        let nested = match value.read_heap(vm) {
            Some(HeapReadOutput::GenericAlias(alias)) if alias.get(vm.heap).origin == LITERAL => {
                Some(alias.parts(vm).1)
            }
            _ => None,
        };
        match nested {
            Some(args) => {
                defer_drop!(args, vm);
                value.drop_with(vm);
                flattened.extend(tuple_items(args, vm)?);
            }
            None => flattened.push(value),
        }
    }
    let (flattened, vm) = flattened_guard.into_parts();
    // Values are taken out one at a time, so the guard still releases the rest
    // when a comparison fails. Hashable values are bucketed by type and hash,
    // keeping deduplication linear; unhashable ones are compared one by one.
    defer_drop_mut!(flattened, vm);
    let mut params_guard = DropGuard::new(TupleVec::new(), vm);
    let (params, vm) = params_guard.as_parts_mut();
    let mut buckets: AHashMap<(Type, u64), SmallVec<[usize; 1]>> = AHashMap::new();
    let mut unhashable: Vec<usize> = Vec::new();
    for index in 0..flattened.len() {
        vm.heap.tracker.check_time_every(index)?;
        let value = mem::replace(&mut flattened[index], Value::None);
        let mut value_guard = DropGuard::new(value, vm);
        let (value, vm) = value_guard.as_parts_mut();
        let bucket = value.py_hash(vm)?.map(|hash| (value.py_type(vm), hash.raw()));
        let candidates = match &bucket {
            Some(key) => buckets.get(key).map_or(&[][..], SmallVec::as_slice),
            None => unhashable.as_slice(),
        };
        let mut duplicate = false;
        for &candidate in candidates {
            if same_literal_value(&params[candidate], value, vm)? {
                duplicate = true;
                break;
            }
        }
        if !duplicate {
            let (value, _) = value_guard.into_parts();
            let position = params.len();
            params.push(value);
            match bucket {
                Some(key) => buckets.entry(key).or_default().push(position),
                None => unhashable.push(position),
            }
        }
    }
    let (params, vm) = params_guard.into_parts();
    Ok(allocate_tuple(params, vm.heap))
}

/// How many values a nested `Literal` contributes when flattened; `None` for
/// any other value, which contributes itself.
fn nested_literal_len(value: &Value, vm: &VM<'_>) -> Option<usize> {
    let Value::Ref(id) = value else { return None };
    let HeapData::GenericAlias(alias) = vm.heap.get(*id) else {
        return None;
    };
    if alias.origin != LITERAL {
        return None;
    }
    let Value::Ref(args) = alias.args else {
        unreachable!("GenericAlias::args is always a tuple")
    };
    let HeapData::Tuple(items) = vm.heap.get(args) else {
        unreachable!("GenericAlias::args is always a tuple")
    };
    Some(items.as_slice().len())
}

/// Whether two `Literal` values are one parameter: equal and of one type, so
/// `Literal[1]` and `Literal[True]` differ, as in CPython.
fn same_literal_value(a: &Value, b: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    Ok(a.py_type(vm) == b.py_type(vm) && a.py_eq(b, vm)?)
}

/// `Literal` equality: the same values in any order, both sides already
/// deduplicated. An unhashable value raises the `TypeError` CPython's set
/// comparison does.
fn literal_eq(args: &Value, other_args: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    let items = tuple_items(args, vm)?;
    defer_drop!(items, vm);
    let others = tuple_items(other_args, vm)?;
    defer_drop!(others, vm);
    // Bucket the other side by type and hash, so the comparison is linear.
    let mut buckets: AHashMap<(Type, u64), SmallVec<[usize; 1]>> = AHashMap::new();
    for (index, item) in items.iter().chain(others.iter()).enumerate() {
        vm.heap.tracker.check_time_every(index)?;
        let Some(hash) = item.py_hash(vm)? else {
            let unhashable_type = unhashable_type_name(item, vm)?;
            return Err(ExcType::type_error_unhashable_set_element("tuple", &unhashable_type));
        };
        if index >= items.len() {
            buckets
                .entry((item.py_type(vm), hash.raw()))
                .or_default()
                .push(index - items.len());
        }
    }
    if items.len() != others.len() {
        return Ok(false);
    }
    for (index, item) in items.iter().enumerate() {
        vm.heap.tracker.check_time_every(index)?;
        let Some(hash) = item.py_hash(vm)? else {
            unreachable!("checked above")
        };
        let mut found = false;
        for &candidate in buckets
            .get(&(item.py_type(vm), hash.raw()))
            .map_or(&[][..], SmallVec::as_slice)
        {
            if same_literal_value(item, &others[candidate], vm)? {
                found = true;
                break;
            }
        }
        if !found {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `Literal`'s argument hash: each value's hash paired with its type, as the
/// equality pairs them, combined in any order; `None` when a value is unhashable.
fn literal_hash(args: &Value, vm: &mut VM<'_>) -> RunResult<Option<HashValue>> {
    let items = tuple_items(args, vm)?;
    defer_drop!(items, vm);
    let mut combined: u64 = 0;
    for (index, item) in items.iter().enumerate() {
        vm.heap.tracker.check_time_every(index)?;
        let Some(hash) = item.py_hash(vm)? else {
            return Ok(None);
        };
        let mut hasher = AHasher::default();
        item.py_type(vm).hash(&mut hasher);
        hash.hash(&mut hasher);
        combined = combined.wrapping_add(hasher.finish());
    }
    Ok(Some(HashValue::new(combined)))
}

/// `Callable[[a, b], r]` stores `(a, b, r)`, as CPython's `_CallableGenericAlias`
/// does: a parameter list flattens, while `...` and a single bare parameter
/// stand alone. Takes ownership of the `(params, result)` pair; the copy of
/// the list is preflighted like any container copy.
fn callable_params(key: Value, vm: &mut VM<'_>) -> RunResult<Value> {
    defer_drop!(key, vm);
    let Some(HeapReadOutput::Tuple(pair)) = key.read_heap(vm) else {
        unreachable!("Callable takes a parameter list and a result")
    };
    let params = pair.clone_item(0, vm);
    defer_drop!(params, vm);
    let mut flattened = if let Some(HeapReadOutput::List(list)) = params.read_heap(vm) {
        let count = list.get(vm.heap).as_slice().len();
        vm.heap.tracker.check_allocation(count.saturating_mul(VALUE_SIZE))?;
        (0..count).map(|index| list.clone_item(index, vm)).collect::<TupleVec>()
    } else {
        smallvec![params.clone_with_heap(vm)]
    };
    flattened.push(pair.clone_item(1, vm));
    Ok(allocate_tuple(flattened, vm.heap))
}

/// Whether `name` is a `__dunder__`, which CPython's alias never forwards to its origin.
fn is_dunder(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}

/// Whether `form` stands in for a `collections.abc` class in CPython
/// (`typing.Callable[...]` is `collections.abc.Callable[...]`).
fn is_abc_form(form: Marker) -> bool {
    matches!(
        form.0,
        StaticStrings::Callable
            | StaticStrings::Sequence
            | StaticStrings::Mapping
            | StaticStrings::Iterable
            | StaticStrings::IteratorType
            | StaticStrings::Generator
    )
}

/// Owned clones of a tuple's items, preflighted like any container copy.
fn tuple_items(tuple: &Value, vm: &mut VM<'_>) -> RunResult<TupleVec> {
    let Some(HeapReadOutput::Tuple(items)) = tuple.read_heap(vm) else {
        unreachable!("GenericAlias::args is always a tuple")
    };
    let count = items.get(vm.heap).as_slice().len();
    vm.heap.tracker.check_allocation(count.saturating_mul(VALUE_SIZE))?;
    Ok((0..count).map(|index| items.clone_item(index, vm)).collect())
}

/// CPython's `TypeError` for instantiating one of `collections.abc`'s classes.
fn abstract_class_error(name: &str, methods: &str) -> String {
    let noun = if methods.contains(',') { "methods" } else { "method" };
    format!("Can't instantiate abstract class {name} without an implementation for abstract {noun} {methods}")
}

/// Frozen helper for `typing.get_type_hints`: `alias_with(alias, args, include_extras)`
/// rebuilds `alias` around the resolved `args` tuple (the alias itself when they
/// are unchanged), and strips `Annotated[T, ...]` to `T` unless `include_extras`.
pub(crate) fn call_alias_with(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let AliasWithArgs {
        alias,
        resolved,
        include_extras,
    } = AliasWithArgs::from_args(args, vm)?;
    defer_drop!(alias, vm);
    defer_drop!(resolved, vm);
    let Some(HeapReadOutput::GenericAlias(generic)) = alias.read_heap(vm) else {
        unreachable!("frozen code passes a generic alias")
    };
    let (origin, args) = generic.parts(vm);
    defer_drop!(args, vm);
    if generic.get(vm.heap).is_annotated() {
        let Some(HeapReadOutput::Tuple(resolved_items)) = resolved.read_heap(vm) else {
            unreachable!("frozen code passes the resolved args as a tuple")
        };
        let inner = resolved_items.clone_item(0, vm);
        if !matches!(include_extras, Value::Bool(true)) {
            return Ok(inner);
        }
        let Some(HeapReadOutput::Tuple(stored)) = args.read_heap(vm) else {
            unreachable!("GenericAlias::args is always a tuple")
        };
        defer_drop!(inner, vm);
        let count = stored.get(vm.heap).as_slice().len();
        vm.heap.tracker.check_allocation(count.saturating_mul(VALUE_SIZE))?;
        let mut items = smallvec![inner.clone_with_heap(vm)];
        for index in 1..count {
            items.push(stored.clone_item(index, vm));
        }
        let key = allocate_tuple(items, vm.heap);
        return Ok(GenericAlias::allocate(origin, key, vm));
    }
    if resolved.py_eq(args, vm)? {
        Ok(alias.clone_with_heap(vm))
    } else {
        Ok(GenericAlias::allocate(origin, resolved.clone_with_heap(vm), vm))
    }
}

/// Arguments of the private `alias_with` helper.
#[derive(FromArgs)]
#[from_args(name = "alias_with")]
struct AliasWithArgs {
    #[from_args(pos_only)]
    alias: Value,
    #[from_args(pos_only)]
    resolved: Value,
    #[from_args(pos_only)]
    include_extras: Value,
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
        Value::Builtin(Builtins::Type(ty)) => Ok(f.write_str(&Type::from(*ty).name(vm.heap, vm.interns))?),
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

/// Writes a list argument (`Callable[[int], str]`) element by element, each as
/// [`repr_type_arg`] writes it; `false` when `item` is not a list. As CPython's
/// `ga_repr` only looks inside a top-level list, a list nested deeper takes its
/// ordinary repr, whose cycle protection keeps a self-referential list finite.
fn repr_list_arg(item: &Value, f: &mut impl Write, vm: &mut VM<'_>, heap_ids: &mut LazyHeapSet) -> RunResult<bool> {
    let Some(HeapReadOutput::List(list)) = item.read_heap(vm) else {
        return Ok(false);
    };
    f.write_char('[')?;
    for index in 0..list.get(vm.heap).as_slice().len() {
        if index > 0 {
            if repr_should_truncate(index, vm)? {
                f.write_str(", ...[timeout]")?;
                break;
            }
            f.write_str(", ")?;
        }
        let element = list.clone_item(index, vm);
        defer_drop!(element, vm);
        repr_type_arg(element, f, vm, heap_ids)?;
    }
    f.write_char(']')?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;

    #[test]
    fn legacy_origin_dump_loads_unchanged() {
        #[derive(Serialize)]
        struct LegacyAlias {
            origin: Type,
            args: Value,
        }

        // Include a type after Instance, where the two enums' indices differ.
        for origin in [Type::List, Type::ItertoolsChain] {
            let legacy = LegacyAlias {
                origin,
                args: Value::None,
            };
            let bytes = minicbor_serde::to_vec(&legacy).unwrap();
            let alias: GenericAlias = minicbor_serde::from_slice(&bytes).unwrap();

            assert_eq!(
                alias.origin,
                AliasOrigin::Builtin(BuiltinType::try_from(origin).unwrap())
            );
            assert_eq!(minicbor_serde::to_vec(&alias).unwrap(), bytes);
        }
    }
}
