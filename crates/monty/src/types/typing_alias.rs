//! `typing.Literal[...]` and `typing.Annotated[...]` — the runtime values of
//! the two subscriptable `typing` forms that are not unions.
//!
//! Both hold a tuple of arguments. A `Literal` records its choices, deduplicated
//! by value *and* type as CPython does (`Literal[1, True]` keeps both), and
//! compares as a set. An `Annotated` records the annotated type followed by its
//! metadata, exposes the type as `__origin__` and the rest as `__metadata__`,
//! and otherwise behaves like the type it wraps: `Annotated[int, 1]('5')` is
//! `int('5')`. Nothing inspects the arguments beyond that, so the forms are
//! inert data for `get_type_hints` and schema builders to read.

use std::{
    fmt::Write,
    hash::{Hash, Hasher},
};

use ahash::AHasher;
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::{
    args::ArgValues,
    builtins::Builtins,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, hash_one},
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::StaticStrings,
    types::{
        BuiltinType, LazyHeapSet, PyTrait, Type, Union, allocate_string, generic_alias::repr_type_arg,
        list::repr_check_time, tuple::allocate_tuple, union::tuple_items,
    },
    value::{EitherStr, Marker, Value},
};

/// Which `typing` form was subscripted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum TypingForm {
    Literal,
    Annotated,
}

impl TypingForm {
    /// The `typing` marker this form is subscripted from, which is what
    /// `Literal[1].__origin__` reports and `typing.Literal[...]` prints.
    fn marker(self) -> StaticStrings {
        match self {
            Self::Literal => StaticStrings::Literal,
            Self::Annotated => StaticStrings::Annotated,
        }
    }

    /// `type(Literal[1])` and `type(Annotated[int, 1])`: CPython's private
    /// `typing._LiteralGenericAlias` and `typing._AnnotatedAlias`.
    fn py_type(self) -> Type {
        match self {
            Self::Literal => Type::LiteralGenericAlias,
            Self::Annotated => Type::AnnotatedAlias,
        }
    }
}

/// A subscripted `typing.Literal` or `typing.Annotated`.
///
/// `args` is an OWNED ref to a tuple, so `py_dec_ref_ids` and
/// `for_each_child_id` must both release it. For a `Literal` it is `__args__`
/// itself; for an `Annotated` it is the annotated type followed by the
/// metadata, split into `__args__` and `__metadata__` on read.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TypingAlias {
    form: TypingForm,
    /// Always a `Value::Ref` to a `HeapData::Tuple`.
    args: Value,
}

impl TypingAlias {
    /// `typing.Literal[key]`: a tuple key lists the choices, anything else is
    /// the single choice. Nested literals flatten and duplicates (by value and
    /// type) drop, as in CPython. Takes ownership of `key`.
    pub(crate) fn literal(key: Value, vm: &mut VM<'_>) -> RunResult<Value> {
        let items = key_items(key, vm)?;
        defer_drop!(items, vm);
        let choices: Vec<Value> = Vec::new();
        defer_drop_mut!(choices, vm);
        for (index, item) in items.iter().enumerate() {
            vm.heap.tracker.check_memory_time_every(index)?;
            match nested_literal_args(item, vm)? {
                Some(nested) => {
                    defer_drop!(nested, vm);
                    for choice in nested {
                        push_unique_choice(choices, choice, vm)?;
                    }
                }
                None => push_unique_choice(choices, item, vm)?,
            }
        }
        let args = allocate_tuple(choices.iter().map(|c| c.clone_with_heap(vm)).collect(), vm.heap);
        Ok(Self::allocate(TypingForm::Literal, args, vm))
    }

    /// `typing.Annotated[key]`: the key must be a tuple of the annotated type
    /// and at least one metadata item. An annotated type that is itself an
    /// `Annotated` contributes its own type and metadata first, so
    /// `Annotated[Annotated[int, 1], 2]` is `Annotated[int, 1, 2]`. Takes
    /// ownership of `key`.
    pub(crate) fn annotated(key: Value, vm: &mut VM<'_>) -> RunResult<Value> {
        let items = key_items(key, vm)?;
        defer_drop!(items, vm);
        let [origin, metadata @ ..] = items.as_slice() else {
            return Err(ExcType::annotated_requires_two_args());
        };
        if metadata.is_empty() {
            return Err(ExcType::annotated_requires_two_args());
        }
        let mut args = SmallVec::new();
        // CPython's `_type_check` rejects a tuple as the type and stores `None` as `NoneType`.
        match origin {
            Value::None => args.push(Value::Builtin(Builtins::Type(BuiltinType::NoneType))),
            Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Tuple(_)) => {
                let repr = origin.py_repr(vm)?;
                defer_drop!(repr, vm);
                return Err(ExcType::annotated_type_required(
                    repr.as_str(vm).expect("py_repr returns str"),
                ));
            }
            Value::Ref(id)
                if let HeapData::TypingAlias(inner) = vm.heap.get(*id)
                    && inner.form == TypingForm::Annotated =>
            {
                let inner_args = inner.args(vm.heap);
                defer_drop!(inner_args, vm);
                args.extend(alias_items(inner_args, vm)?);
            }
            other => args.push(other.clone_with_heap(vm)),
        }
        args.extend(metadata.iter().map(|item| item.clone_with_heap(vm)));
        let args = allocate_tuple(args, vm.heap);
        Ok(Self::allocate(TypingForm::Annotated, args, vm))
    }

    /// Wraps an args tuple (owned) in a heap alias of `form`.
    fn allocate(form: TypingForm, args: Value, vm: &VM<'_>) -> Value {
        Value::Ref(vm.heap.allocate(HeapData::TypingAlias(Self { form, args })))
    }

    /// The Python type of this alias, without needing VM access.
    pub(crate) fn py_type(&self) -> Type {
        self.form.py_type()
    }

    /// Invokes `on_child` for the heap id this alias owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = self.args {
            on_child(id);
        }
    }

    /// An owned ref to the stored tuple.
    pub(crate) fn args(&self, heap: &impl ContainsHeap) -> Value {
        self.args.clone_with_heap(heap)
    }
}

/// Owned clones of a stored tuple's items; `args` comes from [`TypingAlias::args`].
fn alias_items(args: &Value, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
    let Some(items) = tuple_items(args, vm)? else {
        unreachable!("TypingAlias::args is always a tuple")
    };
    Ok(items)
}

/// The items a subscript key supplies: the tuple's own, or the key alone.
/// Takes ownership of `key`.
fn key_items(key: Value, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
    defer_drop!(key, vm);
    match tuple_items(key, vm)? {
        Some(items) => Ok(items),
        None => Ok(vec![key.clone_with_heap(vm)]),
    }
}

/// The choices of `value` when it is itself a `Literal`, to flatten into an
/// enclosing one; `None` for anything else.
fn nested_literal_args(value: &Value, vm: &mut VM<'_>) -> RunResult<Option<Vec<Value>>> {
    let Value::Ref(id) = value else { return Ok(None) };
    let args = match vm.heap.get(*id) {
        HeapData::TypingAlias(alias) if alias.form == TypingForm::Literal => alias.args(vm.heap),
        _ => return Ok(None),
    };
    defer_drop!(args, vm);
    alias_items(args, vm).map(Some)
}

/// Appends `choice` to `choices` unless an equal choice of the same type is
/// already there, so `Literal[1, True]` keeps both and `Literal[1, 1]` one.
fn push_unique_choice(choices: &mut Vec<Value>, choice: &Value, vm: &mut VM<'_>) -> RunResult<()> {
    for (index, existing) in choices.iter().enumerate() {
        vm.heap.tracker.check_memory_time_every(index)?;
        if same_choice(existing, choice, vm)? {
            return Ok(());
        }
    }
    choices.push(choice.clone_with_heap(vm));
    Ok(())
}

/// CPython compares literal choices as `(value, type(value))` pairs.
fn same_choice(a: &Value, b: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    Ok(a.py_type(vm) == b.py_type(vm) && a.py_eq(b, vm)?)
}

/// Releases the args tuple of an alias abandoned before it reaches the heap;
/// a heap-stored alias is freed through [`HeapItem::py_dec_ref_ids`].
impl<C: ContainsHeap> DropWithContext<C> for TypingAlias {
    fn drop_with(self, ctx: &mut C) {
        self.args.drop_with(ctx);
    }
}

impl HeapItem for TypingAlias {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.args.py_dec_ref_ids(stack);
    }
}

impl<'h> HeapObjectRead<'h, TypingAlias> {
    /// The form and owned clones of the stored tuple's items.
    fn parts(&self, vm: &mut VM<'h>) -> RunResult<(TypingForm, Vec<Value>)> {
        let this = self.get(vm.heap);
        let (form, args) = (this.form, this.args(vm.heap));
        defer_drop!(args, vm);
        Ok((form, alias_items(args, vm)?))
    }

    /// An `Annotated`'s annotated type (owned), the first stored item.
    fn annotated_origin(&self, vm: &mut VM<'h>) -> Value {
        let args = self.get(vm.heap).args(vm.heap);
        defer_drop!(args, vm);
        let Some(HeapReadOutput::Tuple(args)) = args.read_heap(vm) else {
            unreachable!("TypingAlias::args is always a tuple")
        };
        args.clone_item(0, vm)
    }

    /// The alias's repr as a plain string, for error messages that name it.
    fn repr_string(&self, vm: &mut VM<'h>) -> RunResult<String> {
        let mut repr = String::new();
        self.py_repr_fmt(&mut repr, vm, &mut LazyHeapSet::default())?;
        Ok(repr)
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, TypingAlias> {
    /// `Annotated[int, 1](x)` is `int(x)`; a `Literal` cannot be instantiated,
    /// as calling `typing.Literal` itself cannot.
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        match self.get(vm.heap).form {
            TypingForm::Annotated => {
                let origin = self.annotated_origin(vm);
                defer_drop!(origin, vm);
                vm.call_function(origin, args)
            }
            TypingForm::Literal => {
                args.drop_with(vm);
                Err(ExcType::cannot_instantiate_typing_form("Literal"))
            }
        }
    }

    fn py_type(&self, vm: &VM<'h>) -> Type {
        self.get(vm.heap).form.py_type()
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    /// Two literals are equal when they hold the same choices in any order,
    /// each matched by value and type; two annotated forms when their types
    /// and metadata are.
    ///
    /// Takes a recursion level like the repr: an argument can be a list that
    /// holds this alias, and comparing two such cycles raises `RecursionError`
    /// rather than overflowing the native stack.
    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::TypingAlias(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        let mut guard = vm.recursion_guard()?;
        let vm = &mut *guard;
        let (form, mine) = self.parts(vm)?;
        defer_drop!(mine, vm);
        let (other_form, theirs) = other.parts(vm)?;
        defer_drop!(theirs, vm);
        if form != other_form || mine.len() != theirs.len() {
            return Ok(Some(false));
        }
        match form {
            TypingForm::Annotated => {
                for (a, b) in mine.iter().zip(theirs.iter()) {
                    if !a.py_eq(b, vm)? {
                        return Ok(Some(false));
                    }
                }
                Ok(Some(true))
            }
            // Choices are deduplicated, so equal lengths plus containment one
            // way is set equality. Quadratic, so the inner scan polls the time limit.
            TypingForm::Literal => {
                for choice in mine {
                    let mut found = false;
                    for (index, candidate) in theirs.iter().enumerate() {
                        vm.heap.tracker.check_memory_time_every(index)?;
                        if same_choice(choice, candidate, vm)? {
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
        }
    }

    /// Agrees with `py_eq_impl`: order-independent over `(choice, type)` for a
    /// literal, positional for an annotated form. An unhashable argument
    /// (`Literal[[1]]`) makes the alias unhashable, as in CPython.
    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let (form, items) = self.parts(vm)?;
        defer_drop!(items, vm);
        let mut hasher = AHasher::default();
        form.hash(&mut hasher);
        let mut combined = items.len() as u64;
        for (index, item) in items.iter().enumerate() {
            vm.heap.tracker.check_memory_time_every(index)?;
            let Some(hash) = item.py_hash(vm)? else {
                return Ok(None);
            };
            match form {
                TypingForm::Literal => {
                    // Dicts and sets persist hashes, so the type takes part by name, never by discriminant.
                    let type_name = item.py_type(vm).name(vm.heap, vm.interns);
                    combined = combined.wrapping_add(hash_one((hash, type_name.as_ref())).raw());
                }
                TypingForm::Annotated => hash.hash(&mut hasher),
            }
        }
        combined.hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    /// `typing.Literal['a', 'b']`, `typing.Annotated[int, 'meta']`.
    ///
    /// A literal's choices and an annotated form's type print as generic
    /// alias arguments do (types by name); metadata prints with its plain
    /// repr, so `Annotated[int, str]` is `typing.Annotated[int, <class 'str'>]`.
    /// Takes a recursion level like the container reprs: an argument can be a
    /// list that holds this alias, and that cycle prints as `...`.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let Ok(mut guard) = vm.recursion_guard() else {
            return Ok(f.write_str("...")?);
        };
        let vm = &mut *guard;
        let form = self.get(vm.heap).form;
        let marker: &'static str = form.marker().into();
        write!(f, "typing.{marker}[")?;
        let args = self.get(vm.heap).args(vm.heap);
        defer_drop!(args, vm);
        let Some(HeapReadOutput::Tuple(args)) = args.read_heap(vm) else {
            unreachable!("TypingAlias::args is always a tuple")
        };
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
            if form == TypingForm::Annotated && index > 0 {
                item.py_repr_fmt(f, vm, heap_ids)?;
            } else {
                repr_type_arg(item, f, vm, heap_ids)?;
            }
        }
        Ok(f.write_char(']')?)
    }

    /// `__origin__`, `__args__`, `__parameters__` (always `()`), `__name__`
    /// and, on an `Annotated`, `__metadata__`. An annotated form resolves any
    /// other non-dunder name on its type, as CPython's alias does; a literal
    /// and the remaining dunders raise `AttributeError`.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let form = self.get(vm.heap).form;
        let value = match (attr.static_string(vm.interns), form) {
            (Some(StaticStrings::DunderOrigin), TypingForm::Literal) => Value::Marker(Marker(StaticStrings::Literal)),
            (Some(StaticStrings::DunderOrigin), TypingForm::Annotated) => self.annotated_origin(vm),
            (Some(StaticStrings::DunderArgs), TypingForm::Literal) => self.get(vm.heap).args(vm.heap),
            (Some(StaticStrings::DunderArgs), TypingForm::Annotated) => {
                allocate_tuple(smallvec![self.annotated_origin(vm)], vm.heap)
            }
            (Some(StaticStrings::DunderMetadata), TypingForm::Annotated) => {
                let (_, mut items) = self.parts(vm)?;
                let origin = items.remove(0);
                origin.drop_with(vm);
                allocate_tuple(SmallVec::from_vec(items), vm.heap)
            }
            (Some(StaticStrings::DunderParameters), _) => vm.heap.get_empty_tuple(),
            (Some(StaticStrings::DunderName), _) => {
                let name: &'static str = form.marker().into();
                allocate_string(name, vm.heap)
            }
            (_, TypingForm::Annotated) if !is_dunder(attr.as_str(vm.interns)) => {
                let origin = self.annotated_origin(vm);
                defer_drop!(origin, vm);
                return origin.py_getattr(attr, vm).map(Some);
            }
            _ => return Ok(None),
        };
        Ok(Some(CallResult::Value(value)))
    }

    /// `Annotated[dict, 1].fromkeys(...)` dispatches to the type's own methods;
    /// the alias's own attributes (`Literal[1].__args__()`) call what they hold.
    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let form = self.get(vm.heap).form;
        if form == TypingForm::Annotated && !is_dunder(attr.as_str(vm.interns)) {
            let origin = self.annotated_origin(vm);
            defer_drop!(origin, vm);
            return origin.py_value(vm).py_call_attr(vm, attr, args);
        }
        match self.py_getattr(attr, vm) {
            Ok(Some(CallResult::Value(value))) => {
                defer_drop!(value, vm);
                vm.call_function(value, args)
            }
            Ok(Some(_)) => unreachable!("the alias's own attributes are always plain values"),
            Ok(None) => Err(ExcType::attribute_error_method(form.py_type(), attr, args, vm)),
            Err(err) => {
                args.drop_with(vm);
                Err(err)
            }
        }
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Union::heap_or(self, other, vm)
    }

    fn py_ror_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Union::heap_ror(self, other, vm)
    }

    /// Neither form has type variables to fill, so `Literal[1][2]` fails as
    /// a generic alias does.
    fn py_getitem(&self, _key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        let repr = self.repr_string(vm)?;
        Err(ExcType::type_error_not_generic_class(&repr))
    }
}

/// Whether `name` is a `__dunder__`, which CPython's aliases never relay to
/// the annotated type.
fn is_dunder(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}
