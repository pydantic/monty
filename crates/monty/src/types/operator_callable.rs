//! `operator.itemgetter`, `operator.attrgetter` and `operator.methodcaller`.
//!
//! Each is a callable holding the items, attribute names or method name it was
//! built from. A call hands the work to a frozen Python function (see
//! [`crate::frozen`]), so subscripting, attribute lookup and the method call
//! take exactly the paths the syntax takes and a user `__getitem__` or
//! `__getattr__` may suspend.

use std::fmt::Write;

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::{
    args::{ArgValues, KwargsValues},
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, RunReentryGuard, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    frozen::FrozenFunction,
    hash::{HashValue, identity_hash},
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead},
    modules::operator::BinaryOp,
    types::{BuiltinType, LazyHeapSet, PyTrait, Type, list::repr_should_truncate, tuple::allocate_tuple},
    value::{EitherStr, VALUE_SIZE, Value},
};

/// Subscripts with each item and gathers the results into a tuple.
pub(crate) const ITEMGETTER_SOURCE: &str = r"
def __monty_operator_itemgetter(obj, items, tuple_):
    result = []
    for item in items:
        result.append(obj[item])
    return tuple_(result)
";

/// Follows one possibly dotted attribute name.
pub(crate) const ATTRGETTER_ONE_SOURCE: &str = r"
def __monty_operator_attrgetter(obj, name, getattr_):
    for part in name.split('.'):
        obj = getattr_(obj, part)
    return obj
";

/// Follows each possibly dotted attribute name and gathers the results into a tuple.
pub(crate) const ATTRGETTER_MANY_SOURCE: &str = r"
def __monty_operator_attrgetter_many(obj, names, getattr_, tuple_):
    result = []
    for name in names:
        value = obj
        for part in name.split('.'):
            value = getattr_(value, part)
        result.append(value)
    return tuple_(result)
";

/// Which of the three callables an [`OperatorCallable`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum OperatorKind {
    ItemGetter,
    AttrGetter,
    MethodCaller,
}

impl OperatorKind {
    /// The `type()` of an object of this kind.
    pub(crate) fn py_type(self) -> Type {
        match self {
            Self::ItemGetter => Type::ItemGetter,
            Self::AttrGetter => Type::AttrGetter,
            Self::MethodCaller => Type::MethodCaller,
        }
    }

    /// The name CPython uses in this kind's argument errors.
    fn name(self) -> &'static str {
        match self {
            Self::ItemGetter => "itemgetter",
            Self::AttrGetter => "attrgetter",
            Self::MethodCaller => "methodcaller",
        }
    }
}

/// An `itemgetter`, `attrgetter` or `methodcaller`.
///
/// Every value in `items` and both halves of every `keywords` pair are OWNED
/// refs held directly here, so `py_dec_ref_ids` and `for_each_child_id` must
/// enumerate all of them or the GC will under-trace and free a live object.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OperatorCallable {
    kind: OperatorKind,
    /// The items to subscript with, the attribute names to read, or for a
    /// `methodcaller` the method name followed by its bound positionals.
    items: Vec<Value>,
    /// A `methodcaller`'s bound keywords as `(name, value)` pairs, in the order given.
    keywords: Vec<(Value, Value)>,
}

impl OperatorCallable {
    /// `itemgetter(*items)`, `attrgetter(*attrs)` and `methodcaller(name, /, *args, **kwargs)`.
    pub(crate) fn init(kind: OperatorKind, vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let name = kind.name();
        let (positional, keywords) = if kind == OperatorKind::MethodCaller {
            args.into_parts()
        } else {
            args.reject_kwargs(name, vm.heap)?.into_parts()
        };
        let items: Vec<Value> = positional.collect();
        let keywords: Vec<(Value, Value)> = keywords.into_iter().collect();
        let callable = Self { kind, items, keywords };

        let error = match kind {
            OperatorKind::ItemGetter if callable.items.is_empty() => Some(arity_error(name, 0)),
            OperatorKind::AttrGetter if callable.items.is_empty() => Some(arity_error(name, 0)),
            OperatorKind::MethodCaller if callable.items.is_empty() => Some(ExcType::type_error(
                "methodcaller needs at least one argument, the method name",
            )),
            OperatorKind::AttrGetter
                if callable
                    .items
                    .iter()
                    .any(|item| item.as_str_heap(vm.heap, vm.interns).is_none()) =>
            {
                Some(ExcType::type_error("attribute name must be a string"))
            }
            OperatorKind::MethodCaller if callable.items[0].as_str_heap(vm.heap, vm.interns).is_none() => {
                Some(ExcType::type_error("method name must be a string"))
            }
            _ => None,
        };
        if let Some(error) = error {
            callable.drop_with(vm);
            return Err(error);
        }
        Ok(Value::Ref(vm.heap.allocate(HeapData::Operator(Box::new(callable)))))
    }

    /// Which callable this is.
    pub(crate) fn kind(&self) -> OperatorKind {
        self.kind
    }

    /// Invokes `on_child` for each heap id this callable owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for value in self.owned_values() {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
    }

    /// Every owned `Value`, in no particular order.
    fn owned_values(&self) -> impl Iterator<Item = &Value> {
        self.items
            .iter()
            .chain(self.keywords.iter().flat_map(|(name, value)| [name, value]))
    }

    /// Owned clones of `items[from..]`, preflighted so an over-budget call
    /// raises `MemoryError` rather than bursting past the allocator's limit.
    fn clone_items(&self, from: usize, heap: &impl ContainsHeap) -> RunResult<SmallVec<[Value; 2]>> {
        let items = &self.items[from..];
        heap.heap()
            .tracker
            .check_allocation(items.len().saturating_mul(VALUE_SIZE))?;
        Ok(items.iter().map(|item| item.clone_with_heap(heap)).collect())
    }
}

/// CPython's "expected 1 argument" error for a call or construction with `count`.
fn arity_error(name: &str, count: usize) -> RunError {
    ExcType::type_error(format!("{name} expected 1 argument, got {count}"))
}

/// Extracts the one object a getter is called on, releasing everything on error.
///
/// `itemgetter` and `attrgetter` reject keywords before counting arguments;
/// `methodcaller` counts first, as CPython does.
fn take_object(kind: OperatorKind, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let name = kind.name();
    let args = if kind == OperatorKind::MethodCaller {
        args
    } else {
        args.reject_kwargs(name, vm.heap)?
    };
    let (positional, keywords) = args.into_parts();
    let mut positional: Vec<Value> = positional.collect();
    if positional.len() != 1 {
        let count = positional.len();
        positional.drop_with(vm);
        keywords.drop_with(vm);
        return Err(arity_error(name, count));
    }
    if !keywords.is_empty() {
        positional.drop_with(vm);
        keywords.drop_with(vm);
        return Err(ExcType::type_error_no_kwargs(name));
    }
    keywords.drop_with(vm);
    Ok(positional.remove(0))
}

/// Releases the refs a callable owns, for one abandoned before it reaches the
/// heap; a heap-stored one is freed through [`HeapItem::py_dec_ref_ids`].
impl<C: ContainsHeap> DropWithContext<C> for OperatorCallable {
    fn drop_with(self, ctx: &mut C) {
        self.items.drop_with(ctx);
        self.keywords.drop_with(ctx);
    }
}

impl HeapItem for OperatorCallable {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for item in &mut self.items {
            item.py_dec_ref_ids(stack);
        }
        for (name, value) in &mut self.keywords {
            name.py_dec_ref_ids(stack);
            value.py_dec_ref_ids(stack);
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, OperatorCallable> {
    /// Applies the getter or caller to the one object passed.
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let kind = self.get(vm.heap).kind;
        let obj = take_object(kind, args, vm)?;
        let items = match self.get(vm.heap).clone_items(0, vm) {
            Ok(items) => items,
            Err(err) => {
                obj.drop_with(vm);
                return Err(err);
            }
        };
        let tuple_type = Value::Builtin(Builtins::Type(BuiltinType::Tuple));
        let getattr = Value::Builtin(Builtins::Function(BuiltinsFunctions::Getattr));
        match kind {
            OperatorKind::ItemGetter if items.len() == 1 => {
                let item = items.into_iter().next().expect("length checked");
                vm.call_frozen_exact(
                    FrozenFunction::OperatorBinary,
                    [Value::Int(BinaryOp::Getitem as i64), obj, item],
                )
            }
            OperatorKind::ItemGetter => vm.call_frozen_exact(
                FrozenFunction::OperatorItemgetterMany,
                [obj, allocate_tuple(items, vm.heap), tuple_type],
            ),
            OperatorKind::AttrGetter if items.len() == 1 => {
                let name = items.into_iter().next().expect("length checked");
                vm.call_frozen_exact(FrozenFunction::OperatorAttrgetterOne, [obj, name, getattr])
            }
            OperatorKind::AttrGetter => vm.call_frozen_exact(
                FrozenFunction::OperatorAttrgetterMany,
                [obj, allocate_tuple(items, vm.heap), getattr, tuple_type],
            ),
            OperatorKind::MethodCaller => {
                let mut items = items;
                let name = items.remove(0);
                let name_len = name.try_to_str_heap(vm.heap, vm.interns).map_or(0, str::len);
                let keyword_count = self.get(vm.heap).keywords.len();
                // The name copy and the keyword pairs are allocations of their own, so
                // they are preflighted as `clone_items` does the positional slots.
                if let Err(err) = vm
                    .heap
                    .tracker
                    .check_allocation(name_len.saturating_add(keyword_count.saturating_mul(2 * VALUE_SIZE)))
                {
                    name.drop_with(vm);
                    obj.drop_with(vm);
                    items.drop_with(vm);
                    return Err(err.into());
                }
                let attr = EitherStr::Heap(name.try_to_str_heap(vm.heap, vm.interns).unwrap_or_default().to_owned());
                name.drop_with(vm);
                let pairs: Vec<(Value, Value)> = self
                    .get(vm.heap)
                    .keywords
                    .iter()
                    .map(|(key, value)| (key.clone_with_heap(vm), value.clone_with_heap(vm)))
                    .collect();
                let args = ArgValues::from_parts(items.into_vec(), KwargsValues::Pairs(pairs));
                // Forwarding is a native call that can reach this callable again, so it
                // takes a re-entry level, as `functools.partial` does.
                if let Err(err) = vm.enter_run_reentry() {
                    obj.drop_with(vm);
                    args.drop_with(vm);
                    return Err(err.into());
                }
                let mut guard = RunReentryGuard::new(vm);
                let vm = &mut *guard;
                // A name the program has interned goes through `VM::call_attr`, which
                // owns the dunder dispatch (`__enter__`, `__exit__`) the syntax relies on.
                if let Some(name_id) = vm.interns.get_string_id_by_name(attr.as_str(vm.interns)) {
                    return vm.call_attr(obj, name_id, args);
                }
                defer_drop!(obj, vm);
                obj.py_value(vm).py_call_attr(vm, &attr, args)
            }
        }
    }

    fn py_type(&self, vm: &VM<'h>) -> Type {
        self.get(vm.heap).kind.py_type()
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    /// These hash by identity, as CPython's do: none defines `__eq__`.
    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    /// `operator.itemgetter('a', 1)`, `operator.methodcaller('m', 1, x=2)`.
    ///
    /// Takes a recursion level like the container reprs: an item can hold a
    /// list that holds this callable, and that cycle prints as `...`.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let Ok(mut guard) = vm.recursion_guard() else {
            return Ok(f.write_str("...")?);
        };
        let vm = &mut *guard;

        write!(f, "operator.{}(", self.get(vm.heap).kind.name())?;
        // One counter spans both loops so the poll cadence covers the whole
        // object, as `repr_items_fmt` does for a sequence.
        let mut count = 0;
        'items: {
            for index in 0..self.get(vm.heap).items.len() {
                if repr_should_truncate(count, vm)? {
                    f.write_str(", ...[timeout]")?;
                    break 'items;
                }
                if count > 0 {
                    f.write_str(", ")?;
                }
                count += 1;
                let item = self.get(vm.heap).items[index].clone_with_heap(vm);
                defer_drop!(item, vm);
                item.py_repr_fmt(f, vm, heap_ids)?;
            }
            for index in 0..self.get(vm.heap).keywords.len() {
                if repr_should_truncate(count, vm)? {
                    f.write_str(", ...[timeout]")?;
                    break 'items;
                }
                count += 1;
                let (name, value) = &self.get(vm.heap).keywords[index];
                let (name, value) = (name.clone_with_heap(vm), value.clone_with_heap(vm));
                defer_drop!(name, vm);
                defer_drop!(value, vm);
                f.write_str(", ")?;
                f.write_str(name.try_to_str_heap(vm.heap, vm.interns).unwrap_or("?"))?;
                f.write_str("=")?;
                value.py_repr_fmt(f, vm, heap_ids)?;
            }
        }
        Ok(f.write_char(')')?)
    }

    fn py_getattr(&self, _: &EitherStr, _: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        Ok(None)
    }
}
