use std::{
    borrow::Cow,
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
    ops::Deref,
};

use monty_types::ExcType;

use crate::{
    args::ArgValues,
    asyncio::{Awaiter, Coroutine, ExternalFuture, ExternalFutureState, GatherFuture, GatherState},
    bytecode::{CallResult, VM},
    exception_private::{ExcTypeExt, RunError, RunResult, SimpleException},
    expressions::CmpOperator,
    hash::{HashValue, identity_hash},
    heap::{DropWithContext, HeapId, HeapItem, HeapReadOutput},
    intern::FunctionId,
    modules::collections::defaultdict::defaultdict_missing,
    types::{LazyHeapSet, LongInt, PyTrait, Type, str::allocate_string},
    value::{EitherStr, Value},
};

macro_rules! heap_storage_type {
    (inline $payload:ty) => {
        $payload
    };
    (boxed $payload:ty) => {
        Box<$payload>
    };
}

/// Invokes a consumer macro with every concrete payload stored by the heap.
///
/// This is the single payload registry. Its order is append-only because
/// `HeapData`'s serde discriminants are snapshot data.
macro_rules! heap_payloads {
    ($consumer:ident) => {
        $consumer! {
            Str(inline $crate::types::Str),
            Bytes(inline $crate::types::Bytes),
            List(inline $crate::types::List),
            /// `collections.deque` — a double-ended queue with an optional `maxlen`.
            Deque(inline $crate::types::Deque),
            Tuple(inline $crate::types::Tuple),
            NamedTuple(boxed $crate::types::NamedTuple),
            /// A `collections.namedtuple` class object (the callable that builds instances).
            NamedTupleClass(boxed $crate::types::NamedTupleClass),
            Dict(inline $crate::types::Dict),
            DictKeysView(inline $crate::types::DictKeysView),
            DictItemsView(inline $crate::types::DictItemsView),
            DictValuesView(inline $crate::types::DictValuesView),
            Set(inline $crate::types::Set),
            FrozenSet(inline $crate::types::FrozenSet),
            Closure(inline $crate::heap_data::Closure),
            FunctionDefaults(inline $crate::heap_data::FunctionDefaults),
            /// A cell wrapping a single mutable value for closure support.
            Cell(inline $crate::heap_data::CellValue),
            /// A range object such as `range(1, 10, 2)`.
            Range(inline $crate::types::Range),
            /// A slice object such as `slice(1, 10, 2)`.
            Slice(inline $crate::types::Slice),
            /// An exception instance such as `ValueError('message')`.
            Exception(inline $crate::exception_private::SimpleException),
            /// A host-backed class instance (the heap form of the wire `ClassInstance`).
            HostClass(boxed $crate::types::HostClass),
            /// The lightweight type object `type(x)` materializes for a `HostClass`
            /// instance — the real class lives on the host, so this is a named
            /// stand-in (see `HostClassType`).
            HostClassType(boxed $crate::types::HostClassType),
            /// A user-defined class object created by a `class` statement.
            Class(boxed $crate::types::Class),
            /// An instance of a user-defined class.
            Instance(boxed $crate::types::Instance),
            /// A method bound to an instance.
            BoundMethod(inline $crate::types::BoundMethod),
            /// One `dataclasses.Field` held by a class's `__dataclass_fields__` dictionary.
            DataclassField(inline $crate::modules::dataclasses::DataclassField),
            /// A `list_iterator` object.
            ListIterator(inline $crate::types::list::ListIterator),
            /// A `_collections._deque_iterator` object.
            DequeIterator(inline $crate::types::deque::DequeIterator),
            /// A `tuple_iterator` object.
            TupleIterator(inline $crate::types::TupleIterator),
            /// A `str_ascii_iterator` or `str_iterator` object.
            StringIterator(inline $crate::types::StringIterator),
            /// A `bytes_iterator` object.
            BytesIterator(inline $crate::types::BytesIterator),
            /// A `range_iterator` object.
            RangeIterator(inline $crate::types::RangeIterator),
            /// A `dict_keyiterator` object.
            DictKeyIterator(inline $crate::types::DictKeyIterator),
            /// A `dict_itemiterator` object.
            DictItemIterator(inline $crate::types::DictItemIterator),
            /// A `dict_valueiterator` object.
            DictValueIterator(inline $crate::types::DictValueIterator),
            /// A `set_iterator` object.
            SetIterator(inline $crate::types::SetIterator),
            /// A `callable_iterator` object from `iter(callable, sentinel)`.
            CallableIterator(inline $crate::types::callable_iterator::CallableIterator),
            /// An arbitrary-precision integer used when a Python `int` does not fit in `i64`.
            LongInt(inline $crate::types::LongInt),
            /// A Python module and its attributes.
            Module(boxed $crate::types::Module),
            /// A coroutine object from an async function call.
            Coroutine(inline $crate::asyncio::Coroutine),
            /// An `asyncio.gather()` result tracking multiple coroutines or tasks.
            GatherFuture(boxed $crate::asyncio::GatherFuture),
            /// An external future driven by the host.
            ExternalFuture(boxed $crate::asyncio::ExternalFuture),
            /// A filesystem path from `pathlib.Path`.
            Path(inline $crate::types::Path),
            /// A path-backed file object returned by `open()`.
            OpenFile(boxed $crate::types::OpenFile),
            /// A compiled regular-expression pattern.
            RePattern(boxed $crate::types::RePattern),
            /// A regular-expression match result.
            ReMatch(boxed $crate::types::ReMatch),
            /// A reference to an external function supplied by the host.
            ExtFunction(inline $crate::types::ExtFunction),
            /// A `datetime.date` value.
            Date(inline $crate::types::date::Date),
            /// A `datetime.datetime` value.
            DateTime(inline $crate::types::datetime::DateTime),
            /// A `datetime.timedelta` value.
            TimeDelta(inline $crate::types::timedelta::TimeDelta),
            /// A fixed-offset `datetime.timezone` value.
            TimeZone(inline $crate::types::timezone::TimeZone),
            // Append-only: a mid-list insertion changes serde's discriminants for all
            // following variants and makes snapshots decode as the wrong payload type.
            /// Any `itertools` iterator (`count`, `repeat`, and others).
            Itertools(inline $crate::types::ItertoolsIter),
            /// The options of a `@dataclass`, held in `__dataclass_params__`.
            DataclassParams(inline $crate::modules::dataclasses::DataclassParams),
            /// A `datetime.time` value stored with narrow integer fields.
            ///
            /// Appended here rather than beside `DateTime` because this list is
            /// append-only (see the note above).
            Time(inline $crate::types::time::Time),
            /// A `functools.partial` object.
            Partial(boxed $crate::types::Partial),
        }
    };
}

pub(crate) use heap_payloads;

macro_rules! define_heap_data {
    ($(
        $(#[$meta:meta])*
        $variant:ident($storage:ident $payload:ty)
    ),* $(,)?) => {
        /// Every runtime value that can be stored in the heap arena.
        #[derive(Debug, serde::Serialize, serde::Deserialize)]
        pub(crate) enum HeapData {
            $(
                $(#[$meta])*
                $variant(heap_storage_type!($storage $payload)),
            )*
        }
    };
}

heap_payloads!(define_heap_data);

// `HeapData` is copied on every allocate and free. `Dict`, the largest hot
// variant, sets the payload ceiling; larger variants should remain boxed.
const _: () = assert!(mem::size_of::<HeapData>() <= 80);

impl HeapData {
    /// Returns whether this heap data type can participate in reference cycles.
    ///
    /// Only container types that can hold references to other heap objects need to be
    /// tracked for GC purposes. Leaf types like Str, Bytes, Range, and Exception cannot
    /// form cycles and should not count toward the GC allocation threshold.
    ///
    /// This optimization allows programs that allocate many leaf objects (like strings)
    /// to avoid triggering unnecessary GC cycles.
    ///
    /// Matched exhaustively so new variants must choose.
    #[inline]
    pub(crate) fn is_gc_tracked(&self) -> bool {
        match self {
            Self::Itertools(iter) => iter.is_gc_tracked(),
            Self::List(_)
            | Self::Deque(_)
            | Self::Tuple(_)
            | Self::NamedTuple(_)
            | Self::NamedTupleClass(_)
            | Self::Dict(_)
            | Self::DictKeysView(_)
            | Self::DictItemsView(_)
            | Self::DictValuesView(_)
            | Self::Set(_)
            | Self::FrozenSet(_)
            | Self::Closure(_)
            | Self::FunctionDefaults(_)
            | Self::Cell(_)
            | Self::HostClass(_)
            | Self::HostClassType(_)
            | Self::Class(_)
            | Self::Instance(_)
            | Self::BoundMethod(_)
            | Self::DataclassField(_)
            | Self::ListIterator(_)
            | Self::DequeIterator(_)
            | Self::TupleIterator(_)
            | Self::DictKeyIterator(_)
            | Self::DictItemIterator(_)
            | Self::DictValueIterator(_)
            | Self::SetIterator(_)
            | Self::CallableIterator(_)
            | Self::Module(_)
            | Self::Coroutine(_)
            | Self::GatherFuture(_)
            | Self::ExternalFuture(_)
            | Self::Partial(_) => true,
            // Leaf types, plus iterators whose heap refs only point at leaves and so
            // cannot close a cycle. Move one up if it gains a container-valued field.
            Self::Str(_)
            | Self::Bytes(_)
            | Self::Range(_)
            | Self::Slice(_)
            | Self::Exception(_)
            | Self::DataclassParams(_)
            | Self::StringIterator(_)
            | Self::BytesIterator(_)
            | Self::RangeIterator(_)
            | Self::LongInt(_)
            | Self::Path(_)
            | Self::OpenFile(_)
            | Self::RePattern(_)
            | Self::ReMatch(_)
            | Self::ExtFunction(_)
            | Self::Date(_)
            | Self::DateTime(_)
            | Self::Time(_)
            | Self::TimeDelta(_)
            | Self::TimeZone(_) => false,
        }
    }

    /// Whether calling a `Ref` to this heap data would succeed at dispatch.
    ///
    /// The one place the callable heap-variant set is listed; keep in sync with
    /// `VM::call_heap_callable`.
    #[must_use]
    pub(crate) fn is_callable(&self) -> bool {
        matches!(
            self,
            Self::Class(_)
                | Self::BoundMethod(_)
                | Self::Closure(_)
                | Self::FunctionDefaults(_)
                | Self::ExtFunction(_)
                | Self::Partial(_)
        )
    }

    /// Returns the Python `Type` for this heap data without requiring VM access.
    ///
    /// This is a lightweight alternative to the `PyTrait::py_type` dispatch on
    /// `HeapReadOutput`, useful in error messages and diagnostics where only a
    /// `&Heap` is available (not a full `&VM`).
    #[must_use]
    pub(crate) fn py_type(&self) -> Type {
        match self {
            Self::Str(_) => Type::Str,
            Self::Bytes(_) => Type::Bytes,
            Self::List(_) => Type::List,
            Self::Deque(_) => Type::Deque,
            Self::Tuple(_) | Self::NamedTuple(_) => Type::Tuple,
            Self::NamedTupleClass(_) => Type::Type,
            Self::Dict(_) => Type::Dict,
            Self::Partial(_) => Type::Partial,
            Self::DictKeysView(_) => Type::DictKeys,
            Self::DictItemsView(_) => Type::DictItems,
            Self::DictValuesView(_) => Type::DictValues,
            Self::Set(_) => Type::Set,
            Self::FrozenSet(_) => Type::FrozenSet,
            Self::Closure(_) | Self::FunctionDefaults(_) | Self::ExtFunction(_) => Type::Function,
            Self::Cell(_) => Type::Cell,
            Self::Range(_) => Type::Range,
            Self::Slice(_) => Type::Slice,
            Self::Exception(e) => Type::Exception(e.exc_type()),
            Self::HostClass(_) => Type::HostClass,
            Self::HostClassType(_) => Type::Type,
            // A class object's type is `type`; an instance's carries its class id.
            Self::Class(_) => Type::Type,
            Self::Instance(instance) => Type::Instance(instance.class()),
            Self::BoundMethod(_) => Type::Function,
            Self::DataclassField(_) => Type::DataclassField,
            Self::DataclassParams(_) => Type::DataclassParams,
            Self::LongInt(_) => Type::Int,
            Self::Module(_) => Type::Module,
            Self::Coroutine(_) | Self::GatherFuture(_) | Self::ExternalFuture(_) => Type::Coroutine,
            Self::Path(_) => Type::Path,
            Self::OpenFile(file) => file.file_type(),
            Self::RePattern(_) => Type::RePattern,
            Self::ReMatch(_) => Type::ReMatch,
            Self::Date(_) => Type::Date,
            Self::DateTime(_) => Type::DateTime,
            Self::Time(_) => Type::Time,
            Self::TimeDelta(_) => Type::TimeDelta,
            Self::TimeZone(_) => Type::TimeZone,
            Self::ListIterator(_) => Type::ListIterator,
            Self::DequeIterator(_) => Type::DequeIterator,
            Self::TupleIterator(_) => Type::TupleIterator,
            Self::StringIterator(iter) => iter.py_type(),
            Self::BytesIterator(_) => Type::BytesIterator,
            Self::RangeIterator(_) => Type::RangeIterator,
            Self::DictKeyIterator(_) => Type::DictKeyIterator,
            Self::DictItemIterator(_) => Type::DictItemIterator,
            Self::DictValueIterator(_) => Type::DictValueIterator,
            Self::SetIterator(_) => Type::SetIterator,
            Self::CallableIterator(_) => Type::CallableIterator,
            Self::Itertools(i) => i.py_type(),
        }
    }
}

/// Thin wrapper around `Value` which is used in the `Cell` variant above.
///
/// The inner value is the cell's mutable payload.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub(crate) struct CellValue(pub(crate) Value);

impl Deref for CellValue {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A closure: a function that captures variables from enclosing scopes.
///
/// Contains a reference to the function definition, a vector of captured cell HeapIds,
/// and evaluated default values (if any). When the closure is called, these cells are
/// passed to the RunFrame for variable access. When the closure is dropped, we must
/// decrement the ref count on each captured cell and each default value.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Closure {
    /// The function definition being captured.
    pub func_id: FunctionId,
    /// Captured cells from enclosing scopes.
    pub cells: Vec<HeapId>,
    /// Evaluated default parameter values (if any).
    pub defaults: Vec<Value>,
}

/// A function with evaluated default parameter values (non-closure).
///
/// Contains a reference to the function definition and the evaluated default values.
/// When the function is called, defaults are cloned for missing optional parameters.
/// When dropped, we must decrement the ref count on each default value.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct FunctionDefaults {
    /// The function definition being captured.
    pub func_id: FunctionId,
    /// Evaluated default parameter values (if any).
    pub defaults: Vec<Value>,
}

impl HeapItem for CellValue {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.0.py_dec_ref_ids(stack);
    }
}

impl HeapItem for Closure {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Decrement ref count for captured cells
        stack.extend(self.cells.iter().copied());
        // Decrement ref count for default values that are heap references
        for default in &mut self.defaults {
            default.py_dec_ref_ids(stack);
        }
    }
}

impl HeapItem for FunctionDefaults {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Decrement ref count for default values that are heap references
        for default in &mut self.defaults {
            default.py_dec_ref_ids(stack);
        }
    }
}

impl HeapItem for SimpleException {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Exceptions don't contain heap references
    }
}

impl HeapItem for LongInt {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // LongInt doesn't contain heap references
    }
}

impl HeapItem for Coroutine {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Decrement ref count for namespace values that are heap references
        for value in &mut self.namespace {
            value.py_dec_ref_ids(stack);
        }
    }
}

impl HeapItem for GatherFuture {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Decrement ref count for items the gather owns (every entry in
        // `items` is inc_ref'd at construction time).
        stack.extend(self.items.iter().copied());
        // Release per-state heap refs: in-flight slot results plus this
        // gather's own awaiter (if `GatherSlot`, it owns an inc_ref on the
        // outer gather), or the cached result list once the gather has
        // completed successfully. `Pending` and `Failed` carry no heap refs.
        match &mut self.state {
            GatherState::Awaited(awaited) => {
                if let Awaiter::GatherSlot { gather, .. } = &awaited.awaiter {
                    stack.push(*gather);
                }
                for result in awaited.results.iter_mut().flatten() {
                    result.py_dec_ref_ids(stack);
                }
            }
            GatherState::Completed(Value::Ref(id)) => stack.push(*id),
            GatherState::Pending | GatherState::Failed(_) | GatherState::Completed(_) => {}
        }
    }
}

impl HeapItem for ExternalFuture {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // `Pending { awaiter: Some(Awaiter::GatherSlot { gather, .. }) }`
        // owns an inc_ref on `gather` — release it when this entry is
        // freed. `Awaiter::Task` and `None` own nothing. `Resolved` owns
        // the cached value; `Failed` carries no heap refs.
        match &mut self.state {
            ExternalFutureState::Resolved(value) => value.py_dec_ref_ids(stack),
            ExternalFutureState::Pending {
                awaiter: Some(Awaiter::GatherSlot { gather, .. }),
            } => stack.push(*gather),
            ExternalFutureState::Pending {
                awaiter: None | Some(Awaiter::Task(_)),
            }
            | ExternalFutureState::Failed(_) => {}
        }
    }
}

macro_rules! heap_read_output_py_trait_forward {
    ($self:expr, |$value:ident| $body:expr, else $fallback:expr) => {
        match $self {
            Self::Str($value) => $body,
            Self::Bytes($value) => $body,
            Self::List($value) => $body,
            Self::Deque($value) => $body,
            Self::ListIterator($value) => $body,
            Self::DequeIterator($value) => $body,
            Self::TupleIterator($value) => $body,
            Self::StringIterator($value) => $body,
            Self::BytesIterator($value) => $body,
            Self::RangeIterator($value) => $body,
            Self::DictKeyIterator($value) => $body,
            Self::DictItemIterator($value) => $body,
            Self::DictValueIterator($value) => $body,
            Self::SetIterator($value) => $body,
            Self::CallableIterator($value) => $body,
            Self::Itertools($value) => $body,
            Self::Partial($value) => $body,
            Self::Tuple($value) => $body,
            Self::NamedTuple($value) => $body,
            Self::NamedTupleClass($value) => $body,
            Self::Dict($value) => $body,
            Self::DictKeysView($value) => $body,
            Self::DictItemsView($value) => $body,
            Self::DictValuesView($value) => $body,
            Self::Set($value) => $body,
            Self::FrozenSet($value) => $body,
            Self::Range($value) => $body,
            Self::Slice($value) => $body,
            Self::HostClass($value) => $body,
            Self::HostClassType($value) => $body,
            Self::Class($value) => $body,
            Self::Instance($value) => $body,
            Self::BoundMethod($value) => $body,
            Self::DataclassField($value) => $body,
            Self::DataclassParams($value) => $body,
            Self::LongInt($value) => $body,
            Self::Path($value) => $body,
            Self::OpenFile($value) => $body,
            Self::RePattern($value) => $body,
            Self::ReMatch($value) => $body,
            Self::Date($value) => $body,
            Self::DateTime($value) => $body,
            Self::Time($value) => $body,
            Self::TimeDelta($value) => $body,
            Self::TimeZone($value) => $body,
            Self::Closure(_)
            | Self::FunctionDefaults(_)
            | Self::ExtFunction(_)
            | Self::Cell(_)
            | Self::Exception(_)
            | Self::Module(_)
            | Self::Coroutine(_)
            | Self::GatherFuture(_)
            | Self::ExternalFuture(_) => $fallback,
        }
    };
}

/// Subscripts a heap value, routing a `defaultdict` miss through `__missing__`
/// and everything else — Counter included — to `py_getitem`.
///
/// A defaultdict's miss stores `factory()`, so that mutating case stays outside
/// [`PyTrait::py_getitem`]'s read-only interface while retaining the identity-aware
/// object handle supplied by `Value::py_getitem`.
pub(crate) fn heap_subscript<'h>(value: HeapReadOutput<'h>, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
    match value {
        HeapReadOutput::Dict(mut dict) if dict.get(vm.heap).is_defaultdict() => match dict.dict_get(key, vm)? {
            Some(value) => Ok(value),
            None => defaultdict_missing(&mut dict, key, vm),
        },
        value => value.py_getitem(key, vm),
    }
}

impl<'h> PyTrait<'h> for HeapReadOutput<'h> {
    /// Forwards so host class instances and named tuples name their real class.
    fn py_type_name(&self, vm: &VM<'h>) -> Cow<'h, str> {
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_type_name(vm),
            else self.py_type(vm).name(vm.heap, vm.interns)
        )
    }

    /// Delegates to the types defining their own `in`; the rest keep the trait
    /// default (`None`), leaving `Value::py_contains` to iterate or raise.
    fn py_contains_impl(&self, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_contains_impl(item, vm), else Ok(None))
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_bool(vm),
            else {
                match self {
                    Self::Closure(_)
                    | Self::FunctionDefaults(_)
                    | Self::ExtFunction(_)
                    | Self::Cell(_)
                    | Self::Exception(_)
                    | Self::Module(_)
                    | Self::Coroutine(_)
                    | Self::GatherFuture(_)
                    | Self::ExternalFuture(_) => Ok(true),
                    _ => unreachable!("py-trait variants handled by heap_read_output_py_trait_forward"),
                }
            }
        )
    }

    fn py_radd_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_radd_impl(other, vm), else Ok(None))
    }

    fn py_rsub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_rsub_impl(other, vm), else Ok(None))
    }

    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_mul_impl(other, vm), else Ok(None))
    }

    fn py_rmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_rmul_impl(other, vm), else Ok(None))
    }

    fn py_matmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_matmul_impl(other, vm), else Ok(None))
    }

    fn py_rmatmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_rmatmul_impl(other, vm), else Ok(None))
    }

    fn py_truediv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_truediv_impl(other, vm), else Ok(None))
    }

    fn py_rtruediv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_rtruediv_impl(other, vm), else Ok(None))
    }

    fn py_floordiv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_floordiv_impl(other, vm), else Ok(None))
    }

    fn py_rfloordiv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_rfloordiv_impl(other, vm), else Ok(None))
    }

    fn py_rmod_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_rmod_impl(other, vm), else Ok(None))
    }

    fn py_pow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_pow_impl(other, modulus, vm), else Ok(None))
    }

    fn py_rpow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_rpow_impl(other, modulus, vm), else Ok(None))
    }

    fn py_and_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_and_impl(other, vm), else Ok(None))
    }

    fn py_rand_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_rand_impl(other, vm), else Ok(None))
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_or_impl(other, vm), else Ok(None))
    }

    fn py_ror_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_ror_impl(other, vm), else Ok(None))
    }

    fn py_xor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_xor_impl(other, vm), else Ok(None))
    }

    fn py_rxor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_rxor_impl(other, vm), else Ok(None))
    }

    fn py_lshift_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_lshift_impl(other, vm), else Ok(None))
    }

    fn py_rlshift_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_rlshift_impl(other, vm), else Ok(None))
    }

    fn py_rshift_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_rshift_impl(other, vm), else Ok(None))
    }

    fn py_rrshift_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_rrshift_impl(other, vm), else Ok(None))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> Result<CallResult, RunError> {
        if let Self::Module(module) = self {
            Ok(module.py_call_attr(vm, attr, args)?)
        } else {
            heap_read_output_py_trait_forward!(
                self,
                |value| Ok(value.py_call_attr(vm, attr, args)?),
                else {
                    args.drop_with(vm);
                    let type_name = self.py_type_name(vm);
                    Err(ExcType::attribute_error(type_name, attr.as_str(vm.interns)))
                }
            )
        }
    }

    fn py_is_iterator(&self, vm: &VM<'h>) -> bool {
        match self {
            // A user-defined class is an iterator only if it defines `__next__`.
            Self::Instance(inst) => inst.py_is_iterator(vm),
            // Every built-in iterator is identified by its type, so there is no
            // list to keep in step with new iterator types here.
            other => other.py_type(vm).is_iterator(),
        }
    }

    fn py_is_iterable(&self, vm: &VM<'h>) -> bool {
        heap_read_output_py_trait_forward!(self, |value| value.py_is_iterable(vm), else false)
    }

    fn py_is_context_manager(&self, vm: &VM<'h>) -> bool {
        // Only types that implement the protocol return true; everything else
        // inherits the default `false`. The `with` statement gates `py_enter`
        // / `py_exit` on this check, so a real context manager whose
        // `__enter__` happens to raise `AttributeError` is no longer
        // misdiagnosed as "not a context manager".
        heap_read_output_py_trait_forward!(self, |value| value.py_is_context_manager(vm), else false)
    }

    fn py_enter(&mut self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        // Only types that override the trait default need explicit arms; all
        // others fall through to the catch-all `AttributeError`, matching how
        // `py_call_attr` is structured.
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_enter(vm),
            else { Err(ExcType::attribute_error(self.py_type_name(vm), "__enter__")) }
        )
    }

    fn py_exit(&mut self, vm: &mut VM<'h>, exc: Option<HeapId>) -> RunResult<CallResult> {
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_exit(vm, exc),
            else { Err(ExcType::attribute_error(self.py_type_name(vm), "__exit__")) }
        )
    }

    fn py_type(&self, vm: &VM<'h>) -> Type {
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_type(vm),
            else {
                match self {
                    Self::Closure(_) | Self::FunctionDefaults(_) | Self::ExtFunction(_) => Type::Function,
                    Self::Cell(_) => Type::Cell,
                    Self::Exception(e) => e.py_type(vm),
                    Self::Module(_) => Type::Module,
                    Self::Coroutine(_) | Self::GatherFuture(_) | Self::ExternalFuture(_) => Type::Coroutine,
                    _ => unreachable!("py-trait variants handled by heap_read_output_py_trait_forward"),
                }
            }
        )
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        heap_read_output_py_trait_forward!(self, |value| value.py_len(vm), else None)
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_eq_impl(other, vm),
            else {
                match self {
                    Self::Closure(a) => Ok(match other.read_heap(vm) {
                        Some(Self::Closure(b)) => {
                            let a = a.get(vm.heap);
                            let b = b.get(vm.heap);
                            Some(a.func_id == b.func_id && a.cells == b.cells)
                        }
                        _ => None,
                    }),
                    Self::FunctionDefaults(a) => Ok(match other.read_heap(vm) {
                        Some(Self::FunctionDefaults(b)) => Some(a.get(vm.heap).func_id == b.get(vm.heap).func_id),
                        _ => None,
                    }),
                    Self::ExtFunction(_)
                    | Self::Cell(_)
                    | Self::Exception(_)
                    | Self::Module(_)
                    | Self::Coroutine(_)
                    | Self::GatherFuture(_)
                    | Self::ExternalFuture(_) => Ok(None),
                    _ => unreachable!("py-trait variants handled by heap_read_output_py_trait_forward"),
                }
            }
        )
    }

    /// Dispatches hashing to per-type `PyTrait` implementations where possible.
    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_hash(vm),
            else {
                match self {
                    Self::Closure(c) => {
                        let mut hasher = DefaultHasher::new();
                        c.get(vm.heap).func_id.hash(&mut hasher);
                        Ok(Some(HashValue::new(hasher.finish())))
                    }
                    Self::FunctionDefaults(fd) => {
                        let mut hasher = DefaultHasher::new();
                        fd.get(vm.heap).func_id.hash(&mut hasher);
                        Ok(Some(HashValue::new(hasher.finish())))
                    }
                    Self::Cell(value) => Ok(Some(identity_hash(value.id()))),
                    Self::ExtFunction(value) => Ok(Some(identity_hash(value.id()))),
                    Self::Exception(_)
                    | Self::Module(_)
                    | Self::Coroutine(_)
                    | Self::GatherFuture(_)
                    | Self::ExternalFuture(_) => Ok(None),
                    _ => unreachable!("py-trait variants handled by heap_read_output_py_trait_forward"),
                }
            }
        )
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_repr_fmt(f, vm, heap_ids),
            else {
                match self {
                    Self::Closure(closure) => Ok(vm
                        .interns
                        .get_function(closure.get(vm.heap).func_id)
                        .py_repr_fmt(f, vm.interns, 0)?),
                    Self::FunctionDefaults(fd) => Ok(vm
                        .interns
                        .get_function(fd.get(vm.heap).func_id)
                        .py_repr_fmt(f, vm.interns, 0)?),
                    Self::Cell(cell) => Ok(write!(f, "<cell: {} object>", cell.get(vm.heap).0.py_type_name(vm))?),
                    Self::Exception(e) => Ok(e.get(vm.heap).py_repr_fmt(f)?),
                    Self::Module(m) => Ok(write!(f, "<module '{}'>", vm.interns.get_str(m.get(vm.heap).name()))?),
                    Self::Coroutine(coro) => {
                        let func = vm.interns.get_function(coro.get(vm.heap).func_id);
                        let name = vm.interns.get_str(func.name.name_id);
                        Ok(write!(f, "<coroutine object {name}>")?)
                    }
                    Self::GatherFuture(gather) => Ok(write!(f, "<gather({})>", gather.get(vm.heap).item_count())?),
                    Self::ExternalFuture(fut) => Ok(write!(
                        f,
                        "<coroutine external_future({})>",
                        fut.get(vm.heap).call_id.raw()
                    )?),
                    Self::ExtFunction(function) => {
                        Ok(write!(f, "<function '{}' external>", function.get(vm.heap).as_str())?)
                    }
                    _ => unreachable!("py-trait variants handled by heap_read_output_py_trait_forward"),
                }
            }
        )
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_str(vm),
            else {
                match self {
                    Self::Exception(e) => Ok(allocate_string(e.get(vm.heap).py_str(), vm.heap)),
                    _ => self.py_repr(vm),
                }
            }
        )
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_add_impl(other, vm), else Ok(None))
    }

    fn py_sub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_sub_impl(other, vm), else Ok(None))
    }

    fn py_mod_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_mod_impl(other, vm), else Ok(None))
    }

    fn py_index_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_index_impl(vm), else Ok(None))
    }

    fn py_neg_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_neg_impl(vm), else Ok(None))
    }

    fn py_pos_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_pos_impl(vm), else Ok(None))
    }

    fn py_iadd_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        heap_read_output_py_trait_forward!(self, |value| value.py_iadd_impl(other, vm), else Ok(false))
    }

    fn py_isub_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        heap_read_output_py_trait_forward!(self, |value| value.py_isub_impl(other, vm), else Ok(false))
    }

    fn py_iand_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        heap_read_output_py_trait_forward!(self, |value| value.py_iand_impl(other, vm), else Ok(false))
    }

    fn py_ior_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        heap_read_output_py_trait_forward!(self, |value| value.py_ior_impl(other, vm), else Ok(false))
    }

    fn py_cmp_op(&self, other: &Value, op: CmpOperator, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        heap_read_output_py_trait_forward!(self, |value| value.py_cmp_op(other, op, vm), else Ok(None))
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_getitem(key, vm),
            else { Err(ExcType::type_error_not_sub(&self.py_type_name(vm))) }
        )
    }

    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        heap_read_output_py_trait_forward!(
            self,
            |item| item.py_setitem(key, value, vm),
            else {
                key.drop_with(vm);
                value.drop_with(vm);
                Err(ExcType::type_error_not_sub_assignment(
                    &self.py_type_name(vm),
                ))
            }
        )
    }

    fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        heap_read_output_py_trait_forward!(
            self,
            |item| item.py_set_attr(name, value, vm),
            else {
                value.drop_with(vm);
                let type_name = self.py_type_name(vm);
                Err(ExcType::attribute_error_no_setattr(
                    &type_name,
                    name.as_str(vm.interns),
                ))
            }
        )
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_getattr(attr, vm),
            else {
                match self {
                    Self::Module(m) => Ok(m.py_getattr(attr, vm)),
                    Self::Exception(e) => Ok(e.py_getattr(attr, vm)),
                    _ => Ok(None),
                }
            }
        )
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        match self {
            Self::Str(value) => value.py_iter(vm),
            Self::Bytes(value) => value.py_iter(vm),
            Self::List(value) => value.py_iter(vm),
            Self::Deque(value) => value.py_iter(vm),
            Self::ListIterator(value) => value.py_iter(vm),
            Self::DequeIterator(value) => value.py_iter(vm),
            Self::TupleIterator(value) => value.py_iter(vm),
            Self::StringIterator(value) => value.py_iter(vm),
            Self::BytesIterator(value) => value.py_iter(vm),
            Self::RangeIterator(value) => value.py_iter(vm),
            Self::DictKeyIterator(value) => value.py_iter(vm),
            Self::DictItemIterator(value) => value.py_iter(vm),
            Self::DictValueIterator(value) => value.py_iter(vm),
            Self::SetIterator(value) => value.py_iter(vm),
            Self::CallableIterator(value) => value.py_iter(vm),
            Self::Itertools(value) => value.py_iter(vm),
            Self::Tuple(value) => value.py_iter(vm),
            Self::NamedTuple(value) => value.py_iter(vm),
            Self::Dict(value) => value.py_iter(vm),
            Self::DictKeysView(value) => value.py_iter(vm),
            Self::DictItemsView(value) => value.py_iter(vm),
            Self::DictValuesView(value) => value.py_iter(vm),
            Self::Set(value) => value.py_iter(vm),
            Self::FrozenSet(value) => value.py_iter(vm),
            Self::Range(value) => value.py_iter(vm),
            Self::Slice(value) => value.py_iter(vm),
            Self::HostClass(value) => value.py_iter(vm),
            Self::HostClassType(value) => value.py_iter(vm),
            Self::Class(value) => value.py_iter(vm),
            Self::Instance(value) => value.py_iter(vm),
            Self::BoundMethod(value) => value.py_iter(vm),
            Self::DataclassField(value) => value.py_iter(vm),
            Self::DataclassParams(value) => value.py_iter(vm),
            Self::Path(value) => value.py_iter(vm),
            Self::OpenFile(value) => value.py_iter(vm),
            Self::ReMatch(value) => value.py_iter(vm),
            Self::RePattern(value) => value.py_iter(vm),
            Self::Date(value) => value.py_iter(vm),
            Self::DateTime(value) => value.py_iter(vm),
            Self::Time(value) => value.py_iter(vm),
            Self::TimeDelta(value) => value.py_iter(vm),
            Self::TimeZone(value) => value.py_iter(vm),
            Self::NamedTupleClass(_)
            | Self::Closure(_)
            | Self::FunctionDefaults(_)
            | Self::ExtFunction(_)
            | Self::Partial(_)
            | Self::Cell(_)
            | Self::Exception(_)
            | Self::LongInt(_)
            | Self::Module(_)
            | Self::Coroutine(_)
            | Self::GatherFuture(_)
            | Self::ExternalFuture(_) => Err(ExcType::type_error_not_iterable(&self.py_type_name(vm))),
        }
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        match self {
            Self::Str(value) => value.py_next(vm),
            Self::Bytes(value) => value.py_next(vm),
            Self::List(value) => value.py_next(vm),
            Self::ListIterator(value) => value.py_next(vm),
            Self::DequeIterator(value) => value.py_next(vm),
            Self::TupleIterator(value) => value.py_next(vm),
            Self::StringIterator(value) => value.py_next(vm),
            Self::BytesIterator(value) => value.py_next(vm),
            Self::RangeIterator(value) => value.py_next(vm),
            Self::DictKeyIterator(value) => value.py_next(vm),
            Self::DictItemIterator(value) => value.py_next(vm),
            Self::DictValueIterator(value) => value.py_next(vm),
            Self::SetIterator(value) => value.py_next(vm),
            Self::CallableIterator(value) => value.py_next(vm),
            Self::Itertools(value) => value.py_next(vm),
            Self::Tuple(value) => value.py_next(vm),
            Self::NamedTuple(value) => value.py_next(vm),
            Self::Dict(value) => value.py_next(vm),
            Self::DictKeysView(value) => value.py_next(vm),
            Self::DictItemsView(value) => value.py_next(vm),
            Self::DictValuesView(value) => value.py_next(vm),
            Self::Set(value) => value.py_next(vm),
            Self::FrozenSet(value) => value.py_next(vm),
            Self::Range(value) => value.py_next(vm),
            Self::Slice(value) => value.py_next(vm),
            Self::HostClass(value) => value.py_next(vm),
            Self::HostClassType(value) => value.py_next(vm),
            Self::Class(value) => value.py_next(vm),
            Self::Instance(value) => value.py_next(vm),
            Self::BoundMethod(value) => value.py_next(vm),
            Self::DataclassField(value) => value.py_next(vm),
            Self::DataclassParams(value) => value.py_next(vm),
            Self::Path(value) => value.py_next(vm),
            Self::OpenFile(value) => value.py_next(vm),
            Self::ReMatch(value) => value.py_next(vm),
            Self::RePattern(value) => value.py_next(vm),
            Self::Date(value) => value.py_next(vm),
            Self::DateTime(value) => value.py_next(vm),
            Self::Time(value) => value.py_next(vm),
            Self::TimeDelta(value) => value.py_next(vm),
            Self::TimeZone(value) => value.py_next(vm),
            other => Err(ExcType::type_error_not_iterator(&other.py_type_name(vm))),
        }
    }
}
