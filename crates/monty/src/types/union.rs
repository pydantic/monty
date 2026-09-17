//! `typing.Union` — the runtime value of `int | None`, `str | list[int]`,
//! `typing.Union[int, str]` and `typing.Optional[int]`.
//!
//! A union is an unordered, deduplicated set of members: `int | str == str | int`,
//! `int | int` is `int` itself, and `(int | str) | bytes` flattens to three
//! members. `isinstance` accepts one and tests each member in turn. Nothing
//! else is checked at construction — as in CPython 3.14, `Union[int, 1]` is
//! `int | 1`.

use std::fmt::Write;

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::{
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, hash_one},
    heap::{
        ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapPayload,
        HeapReadOutput,
    },
    intern::StaticStrings,
    types::{LazyHeapSet, PyTrait, Type, generic_alias::repr_type_arg, list::repr_check_time, tuple::allocate_tuple},
    value::{EitherStr, VALUE_SIZE, Value},
};

/// A union of two or more members, such as `int | None`.
///
/// `args` is an OWNED ref to the `__args__` tuple, so `py_dec_ref_ids` and
/// `for_each_child_id` must both release it. Members are flattened and
/// deduplicated in first-seen order, `None` stored as `NoneType`; a union
/// never nests another union and never has fewer than two members, since
/// [`Union::from_members`] returns a lone member itself.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Union {
    /// The `__args__` tuple, always a `Value::Ref` to a `HeapData::Tuple`.
    args: Value,
}

/// How a value takes part in `|`: CPython's `type.__or__` accepts types,
/// `None`, generic aliases and unions, and a union's own `__or__` accepts
/// anything at all.
#[derive(PartialEq, Eq)]
enum Operand {
    Union,
    /// A type-like value with an `__or__`: a builtin type, exception type,
    /// class object, generic alias or `typing` form.
    TypeLike,
    /// `None`, unionable but with no `__or__` of its own, so `None | None`
    /// is not a union.
    NoneValue,
}

impl Union {
    /// `lhs | rhs` when either side makes it a union; `None` when `|` means
    /// something else for these operands (ints, sets, dict merging, ...).
    ///
    /// Called from both the direct and reflected `|` paths, so `None | int`
    /// and `1 | (int | str)` reach it with the operands in source order.
    pub(crate) fn try_or(lhs: &Value, rhs: &Value, vm: &mut VM<'_>) -> RunResult<Option<Value>> {
        let (lhs_kind, rhs_kind) = (operand_kind(lhs, vm), operand_kind(rhs, vm));
        let unionable = match (lhs_kind, rhs_kind) {
            (Some(Operand::Union), _) | (_, Some(Operand::Union)) => true,
            (Some(l), Some(r)) => l != Operand::NoneValue || r != Operand::NoneValue,
            _ => false,
        };
        if unionable {
            let members = vec![lhs.clone_with_heap(vm), rhs.clone_with_heap(vm)];
            Self::from_members(members, vm).map(Some)
        } else {
            Ok(None)
        }
    }

    /// `typing.Union[key]`: a tuple key supplies the members, anything else is
    /// the single member. Takes ownership of `key`.
    pub(crate) fn subscript(key: Value, vm: &mut VM<'_>) -> RunResult<Value> {
        // Guarded because the item clone can fail its allocation preflight,
        // and `key` must be released on that path too.
        let mut key_guard = DropGuard::new(key, vm);
        let items = {
            let (key, vm) = key_guard.as_parts_mut();
            tuple_items(key, vm)?
        };
        let members = match items {
            Some(items) => {
                drop(key_guard);
                items
            }
            None => vec![key_guard.into_inner()],
        };
        if members.is_empty() {
            Err(ExcType::union_of_no_types())
        } else {
            Self::from_members(members, vm)
        }
    }

    /// `typing.Optional[key]` is `key | None`. Takes ownership of `key`.
    pub(crate) fn optional(key: Value, vm: &mut VM<'_>) -> RunResult<Value> {
        // CPython rejects a tuple as a whole, naming it in the error.
        if matches!(&key, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Tuple(_))) {
            defer_drop!(key, vm);
            let repr = key.py_repr(vm)?;
            defer_drop!(repr, vm);
            Err(ExcType::optional_requires_single_type(repr.to_str(vm)?))
        } else {
            Self::from_members(vec![key, Value::None], vm)
        }
    }

    /// Builds the union of `members`, flattening nested unions and dropping
    /// duplicates; a single surviving member is returned as itself, so
    /// `int | int` is `int`. Takes ownership of `members`.
    ///
    /// Deduplication is a linear scan per member, so a union built from a
    /// huge tuple is quadratic; the loop polls the time limit rather than
    /// running to completion.
    fn from_members(members: Vec<Value>, vm: &mut VM<'_>) -> RunResult<Value> {
        defer_drop!(members, vm);
        let mut guard = DropGuard::new(Vec::new(), vm);
        let (flat, vm) = guard.as_parts_mut();
        for (index, member) in members.iter().enumerate() {
            vm.heap.tracker.check_memory_time_every(index)?;
            let member = member.clone_with_heap(vm);
            defer_drop!(member, vm);
            match tuple_items_of_union(member, vm)? {
                Some(nested) => {
                    defer_drop!(nested, vm);
                    for nested_member in nested {
                        push_unique(flat, nested_member, vm)?;
                    }
                }
                None => push_unique(flat, member, vm)?,
            }
        }
        let (mut flat, vm) = guard.into_parts();
        if flat.len() == 1 {
            Ok(flat.remove(0))
        } else {
            let args = allocate_tuple(SmallVec::from_vec(flat), vm.heap);
            Ok(Value::Ref(vm.heap.allocate(HeapData::Union(Self { args }))))
        }
    }

    /// Invokes `on_child` for the heap id this union owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = self.args {
            on_child(id);
        }
    }

    /// An owned ref to the `__args__` tuple, for `isinstance` to test each
    /// member of.
    pub(crate) fn args(&self, heap: &impl ContainsHeap) -> Value {
        self.args.clone_with_heap(heap)
    }

    /// `py_or_impl` for the heap types that take part in unions (classes,
    /// generic aliases, unions): `this | other`.
    pub(crate) fn heap_or<'h>(
        this: &HeapObjectRead<'h, impl HeapPayload>,
        other: &Value,
        vm: &mut VM<'h>,
    ) -> RunResult<Option<Value>> {
        let this = this.clone_value(vm.heap);
        defer_drop!(this, vm);
        Self::try_or(this, other, vm)
    }

    /// `py_ror_impl` counterpart of [`Union::heap_or`]: `other | this`, reached
    /// when the left operand has no `|` of its own (`None | Foo`, `1 | (int | str)`).
    pub(crate) fn heap_ror<'h>(
        this: &HeapObjectRead<'h, impl HeapPayload>,
        other: &Value,
        vm: &mut VM<'h>,
    ) -> RunResult<Option<Value>> {
        let this = this.clone_value(vm.heap);
        defer_drop!(this, vm);
        Self::try_or(other, this, vm)
    }
}

/// Appends `member` to `flat` unless an equal member is already there.
/// `None` is stored as `NoneType`, which is what `__args__` reports.
fn push_unique(flat: &mut Vec<Value>, member: &Value, vm: &mut VM<'_>) -> RunResult<()> {
    let member = match member {
        Value::None => Value::Builtin(Builtins::Type(Type::NoneType)),
        other => other.clone_with_heap(vm),
    };
    defer_drop!(member, vm);
    // The scan is linear per member, so a wide union polls the time limit here
    // rather than only in the outer construction loop.
    for (index, existing) in flat.iter().enumerate() {
        vm.heap.tracker.check_memory_time_every(index)?;
        if existing.py_eq(member, vm)? {
            return Ok(());
        }
    }
    flat.push(member.clone_with_heap(vm));
    Ok(())
}

/// Classifies `value` for [`Union::try_or`]; `None` for a value `|` never
/// unions with (an instance, a number, a string, ...).
fn operand_kind(value: &Value, vm: &VM<'_>) -> Option<Operand> {
    match value {
        Value::None => Some(Operand::NoneValue),
        Value::Builtin(Builtins::Type(_) | Builtins::ExcType(_) | Builtins::Function(BuiltinsFunctions::Type)) => {
            Some(Operand::TypeLike)
        }
        // `typing.Any`, `typing.List` and the other forms all define `__or__`.
        Value::Marker(marker) if marker.py_type() != Type::TextIOWrapper => Some(Operand::TypeLike),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Union(_) => Some(Operand::Union),
            HeapData::Class(_)
            | HeapData::NamedTupleClass(_)
            | HeapData::HostClassType(_)
            | HeapData::GenericAlias(_) => Some(Operand::TypeLike),
            _ => None,
        },
        _ => None,
    }
}

/// Owned clones of a tuple's items, or `None` when `value` is not a plain
/// tuple (a namedtuple key is one member, as in CPython). Preflights the
/// clone like `clone_all_pairs`.
fn tuple_items(value: &Value, vm: &mut VM<'_>) -> RunResult<Option<Vec<Value>>> {
    let Value::Ref(id) = value else { return Ok(None) };
    let HeapData::Tuple(tuple) = vm.heap.get(*id) else {
        return Ok(None);
    };
    let items = tuple.as_slice();
    vm.heap
        .tracker
        .check_allocation(items.len().saturating_mul(VALUE_SIZE))?;
    Ok(Some(items.iter().map(|item| item.clone_with_heap(vm.heap)).collect()))
}

/// Owned clones of a union's members, or `None` when `value` is not a union.
fn tuple_items_of_union(value: &Value, vm: &mut VM<'_>) -> RunResult<Option<Vec<Value>>> {
    let Value::Ref(id) = value else { return Ok(None) };
    let HeapData::Union(union) = vm.heap.get(*id) else {
        return Ok(None);
    };
    let args = union.args(vm.heap);
    defer_drop!(args, vm);
    tuple_items(args, vm)
}

/// Releases the args tuple of a union abandoned before it reaches the heap;
/// a heap-stored union is freed through [`HeapItem::py_dec_ref_ids`].
impl<C: ContainsHeap> DropWithContext<C> for Union {
    fn drop_with(self, ctx: &mut C) {
        self.args.drop_with(ctx);
    }
}

impl HeapItem for Union {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.args.py_dec_ref_ids(stack);
    }
}

impl<'h> HeapObjectRead<'h, Union> {
    /// The union's repr as a plain string, for error messages that name it.
    fn repr_string(&self, vm: &mut VM<'h>) -> RunResult<String> {
        let mut repr = String::new();
        self.py_repr_fmt(&mut repr, vm, &mut LazyHeapSet::default())?;
        Ok(repr)
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Union> {
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::Union
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    /// Two unions are equal when they have the same members in any order.
    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::Union(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        let args = self.get(vm.heap).args(vm.heap);
        defer_drop!(args, vm);
        let other_args = other.get(vm.heap).args(vm.heap);
        defer_drop!(other_args, vm);
        let Some(mine) = tuple_items(args, vm)? else {
            unreachable!("Union::args is always a tuple")
        };
        defer_drop!(mine, vm);
        let Some(theirs) = tuple_items(other_args, vm)? else {
            unreachable!("Union::args is always a tuple")
        };
        defer_drop!(theirs, vm);
        if mine.len() != theirs.len() {
            return Ok(Some(false));
        }
        // Members are deduplicated, so equal lengths plus containment one way
        // is set equality. Quadratic, so the inner scan polls the time limit.
        for member in mine {
            let mut found = false;
            for (index, candidate) in theirs.iter().enumerate() {
                vm.heap.tracker.check_memory_time_every(index)?;
                if member.py_eq(candidate, vm)? {
                    found = true;
                    break;
                }
            }
            if !found {
                return Ok(Some(false));
            }
        }
        Ok(Some(true))
    }

    /// Order-independent, so it agrees with `py_eq_impl`; an unhashable
    /// member (`int | list[[1]]`) makes the union unhashable.
    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let args = self.get(vm.heap).args(vm.heap);
        defer_drop!(args, vm);
        let Some(members) = tuple_items(args, vm)? else {
            unreachable!("Union::args is always a tuple")
        };
        defer_drop!(members, vm);
        let mut combined = members.len() as u64;
        for (index, member) in members.iter().enumerate() {
            vm.heap.tracker.check_memory_time_every(index)?;
            let Some(hash) = member.py_hash(vm)? else {
                return Ok(None);
            };
            combined = combined.wrapping_add(hash_one(hash).raw());
        }
        Ok(Some(HashValue::new(combined)))
    }

    /// `int | None`, `list[int] | str | None`.
    ///
    /// Members render as in a generic alias, except that `NoneType` prints
    /// as `None`.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let Ok(mut guard) = vm.recursion_guard() else {
            return Ok(f.write_str("...")?);
        };
        let vm = &mut *guard;
        let args = self.get(vm.heap).args(vm.heap);
        defer_drop!(args, vm);
        let Some(HeapReadOutput::Tuple(args)) = args.read_heap(vm) else {
            unreachable!("Union::args is always a tuple")
        };
        let count = args.get(vm.heap).as_slice().len();
        for index in 0..count {
            if index > 0 {
                if repr_check_time(index, vm) {
                    f.write_str(" | ...[timeout]")?;
                    break;
                }
                f.write_str(" | ")?;
            }
            let member = args.clone_item(index, vm);
            defer_drop!(member, vm);
            if matches!(member, Value::Builtin(Builtins::Type(Type::NoneType))) {
                f.write_str("None")?;
            } else {
                repr_type_arg(member, f, vm, heap_ids)?;
            }
        }
        Ok(())
    }

    /// `__args__`, `__origin__` (the `typing.Union` type) and `__parameters__`
    /// (always `()`); nothing is delegated, so any other name is an
    /// `AttributeError` on the union.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        match attr.static_string(vm.interns) {
            Some(StaticStrings::DunderArgs) => Ok(Some(CallResult::Value(self.get(vm.heap).args(vm.heap)))),
            Some(StaticStrings::DunderOrigin) => {
                Ok(Some(CallResult::Value(Value::Builtin(Builtins::Type(Type::Union)))))
            }
            Some(StaticStrings::DunderParameters) => Ok(Some(CallResult::Value(vm.heap.get_empty_tuple()))),
            _ => Ok(None),
        }
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Union::heap_or(self, other, vm)
    }

    fn py_ror_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Union::heap_ror(self, other, vm)
    }

    /// A union has no type variables to fill, so `(int | str)[bytes]` fails
    /// as it does for a generic alias.
    fn py_getitem(&self, _key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        let repr = self.repr_string(vm)?;
        Err(ExcType::type_error_not_generic_class(&repr))
    }
}
