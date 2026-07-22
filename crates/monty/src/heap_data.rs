use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
    ops::Deref,
};

use monty_types::{ExcType, ResourceError};

use crate::{
    args::ArgValues,
    asyncio::{Awaiter, Coroutine, ExternalFuture, ExternalFutureState, GatherFuture, GatherState, awaited_state_size},
    bytecode::{CallResult, VM},
    exception_private::{ExcTypeExt, RunError, RunResult, SimpleException},
    hash::{HashValue, identity_hash},
    heap::{DropWithContext, HeapId, HeapItem, HeapReadOutput},
    intern::FunctionId,
    types::{
        BoundMethod, Bytes, BytesIterator, Class, Dataclass, Dict, DictItemIterator, DictItemsView, DictKeyIterator,
        DictKeysView, DictValueIterator, DictValuesView, ExtFunction, FrozenSet, Instance, LazyHeapSet, List, LongInt,
        Module, NamedTuple, OpenFile, Path, PyTrait, Range, RangeIterator, ReMatch, RePattern, Set, SetIterator, Slice,
        Str, StringIterator, Tuple, TupleIterator, Type, callable_iterator::CallableIterator, date, datetime,
        list::ListIterator, str::allocate_string, timedelta, timezone,
    },
    value::{EitherStr, Value},
};

/// HeapData captures every runtime value that must live in the arena.
///
/// Each variant wraps a type that implements `PyTrait`, providing
/// Python-compatible operations. The trait is manually implemented to dispatch
/// to the appropriate variant's implementation.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum HeapData {
    Str(Str),
    Bytes(Bytes),
    List(List),
    Tuple(Tuple),
    NamedTuple(NamedTuple),
    Dict(Dict),
    DictKeysView(DictKeysView),
    DictItemsView(DictItemsView),
    DictValuesView(DictValuesView),
    Set(Set),
    FrozenSet(FrozenSet),
    Closure(Closure),
    FunctionDefaults(FunctionDefaults),
    /// A cell wrapping a single mutable value for closure support.
    ///
    /// Cells enable nonlocal variable access by providing a heap-allocated
    /// container that can be shared between a function and its nested functions.
    /// Both the outer function and inner function hold references to the same
    /// cell, allowing modifications to propagate across scope boundaries.
    Cell(CellValue),
    /// A range object (e.g., `range(10)` or `range(1, 10, 2)`).
    ///
    /// Stored on the heap to keep `Value` enum small (16 bytes). Range objects
    /// are immutable and hashable.
    Range(Range),
    /// A slice object (e.g., `slice(1, 10, 2)` or from `x[1:10:2]`).
    ///
    /// Stored on the heap to keep `Value` enum small. Slice objects represent
    /// start:stop:step indices for sequence slicing operations.
    Slice(Slice),
    /// An exception instance (e.g., `ValueError('message')`).
    ///
    /// Stored on the heap to keep `Value` enum small (16 bytes). Exceptions
    /// are created when exception types are called or when `raise` is executed.
    Exception(SimpleException),
    /// A dataclass instance with fields and method references.
    ///
    /// Contains a class name, a Dict of field name -> value mappings, and a set
    /// of method names that trigger external function calls when invoked.
    Dataclass(Dataclass),
    /// A user-defined class object created by `class Foo: ...`.
    ///
    /// Holds the class name and a namespace of methods + class variables. Its own
    /// `HeapId` is the type identity used by `type()`/`isinstance`.
    Class(Class),
    /// An instance of a user-defined class.
    ///
    /// Holds a reference to its `Class` and an `attrs` dict (the instance `__dict__`).
    Instance(Instance),
    /// A method bound to an instance, produced by `obj.method` without calling it.
    BoundMethod(BoundMethod),
    /// `list_iterator` object.
    ListIterator(ListIterator),
    /// `tuple_iterator` object.
    TupleIterator(TupleIterator),
    /// `str_ascii_iterator` or `str_iterator` object.
    StringIterator(StringIterator),
    /// `bytes_iterator` object.
    BytesIterator(BytesIterator),
    /// `range_iterator` object.
    RangeIterator(RangeIterator),
    /// `dict_keyiterator` object.
    DictKeyIterator(DictKeyIterator),
    /// `dict_itemiterator` object.
    DictItemIterator(DictItemIterator),
    /// `dict_valueiterator` object.
    DictValueIterator(DictValueIterator),
    /// `set_iterator` object.
    SetIterator(SetIterator),
    /// `callable_iterator` object, from `iter(callable, sentinel)`
    CallableIterator(CallableIterator),
    /// An arbitrary precision integer (LongInt).
    ///
    /// Stored on the heap to keep `Value` enum at 16 bytes. Python has one `int` type,
    /// so LongInt is an implementation detail - we use `Value::Int(i64)` for performance
    /// when values fit, and promote to LongInt on overflow. When LongInt results fit back
    /// in i64, they are demoted back to `Value::Int` for performance.
    LongInt(LongInt),
    /// A Python module (e.g., `sys`, `typing`).
    ///
    /// Modules have a name and a dictionary of attributes. They are created by
    /// import statements and can have refs to other heap values in their attributes.
    Module(Module),
    /// A coroutine object from an async function call.
    ///
    /// Contains pre-bound arguments and captured cells, ready to be awaited.
    /// When awaited, a new frame is pushed using the stored namespace.
    Coroutine(Coroutine),
    /// A gather() result tracking multiple coroutines/tasks.
    ///
    /// Created by asyncio.gather() and spawns tasks when awaited.
    GatherFuture(GatherFuture),
    /// An external future driven by the host.
    ///
    /// Created when the host returns `ExtFunctionResult::Future(call_id)`.
    /// Holds its own state machine (`Pending`/`Resolved`/`Failed`) so
    /// re-await yields cached results, matching CPython's Future semantics.
    ExternalFuture(ExternalFuture),
    /// A filesystem path from `pathlib.Path`.
    ///
    /// Stored on the heap to provide Python-compatible path operations.
    /// Pure methods (name, parent, etc.) are handled directly by the VM.
    /// I/O methods (exists, read_text, etc.) yield external function calls.
    Path(Path),
    /// A path-backed file object returned by the `open()` builtin.
    ///
    /// The object stores only virtual path and mode state.  Reads and writes are
    /// full-file OS calls; no native file descriptor is kept while Monty runs.
    OpenFile(OpenFile),
    /// A compiled regex pattern from `re.compile()`.
    ///
    /// Contains the original pattern string, flags, and compiled regex engine.
    /// Leaf type: no heap references, not GC-tracked.
    RePattern(Box<RePattern>),
    /// A regex match result from a successful regex operation.
    ///
    /// Contains the matched text, capture groups, positions, and input string.
    /// Leaf type: no heap references, not GC-tracked.
    ReMatch(ReMatch),
    /// Reference to an external function supplied by the host or synthesized for a call.
    ExtFunction(ExtFunction),
    /// A `datetime.date` value stored with `chrono::NaiveDate`.
    Date(date::Date),
    /// A `datetime.datetime` value stored with chrono primitives.
    DateTime(datetime::DateTime),
    /// A `datetime.timedelta` duration value stored with `chrono::TimeDelta`.
    TimeDelta(timedelta::TimeDelta),
    /// A fixed-offset `datetime.timezone` value.
    TimeZone(timezone::TimeZone),
}

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
            Self::List(_)
            | Self::Tuple(_)
            | Self::NamedTuple(_)
            | Self::Dict(_)
            | Self::DictKeysView(_)
            | Self::DictItemsView(_)
            | Self::DictValuesView(_)
            | Self::Set(_)
            | Self::FrozenSet(_)
            | Self::Closure(_)
            | Self::FunctionDefaults(_)
            | Self::Cell(_)
            | Self::Dataclass(_)
            | Self::Class(_)
            | Self::Instance(_)
            | Self::BoundMethod(_)
            | Self::ListIterator(_)
            | Self::TupleIterator(_)
            | Self::DictKeyIterator(_)
            | Self::DictItemIterator(_)
            | Self::DictValueIterator(_)
            | Self::SetIterator(_)
            | Self::CallableIterator(_)
            | Self::Module(_)
            | Self::Coroutine(_)
            | Self::GatherFuture(_)
            | Self::ExternalFuture(_) => true,
            // Leaf types, plus iterators whose heap refs only point at leaves and so
            // cannot close a cycle. Move one up if it gains a container-valued field.
            Self::Str(_)
            | Self::Bytes(_)
            | Self::Range(_)
            | Self::Slice(_)
            | Self::Exception(_)
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
            Self::Class(_) | Self::BoundMethod(_) | Self::Closure(_) | Self::FunctionDefaults(_) | Self::ExtFunction(_)
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
            Self::Tuple(_) | Self::NamedTuple(_) => Type::Tuple,
            Self::Dict(_) => Type::Dict,
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
            Self::Dataclass(_) => Type::Dataclass,
            // A class object's type is `type`; an instance's carries its class id.
            Self::Class(_) => Type::Type,
            Self::Instance(instance) => Type::Instance(instance.class()),
            Self::BoundMethod(_) => Type::Function,
            Self::LongInt(_) => Type::Int,
            Self::Module(_) => Type::Module,
            Self::Coroutine(_) | Self::GatherFuture(_) | Self::ExternalFuture(_) => Type::Coroutine,
            Self::Path(_) => Type::Path,
            Self::OpenFile(file) => file.file_type(),
            Self::RePattern(_) => Type::RePattern,
            Self::ReMatch(_) => Type::ReMatch,
            Self::Date(_) => Type::Date,
            Self::DateTime(_) => Type::DateTime,
            Self::TimeDelta(_) => Type::TimeDelta,
            Self::TimeZone(_) => Type::TimeZone,
            Self::ListIterator(_) => Type::ListIterator,
            Self::TupleIterator(_) => Type::TupleIterator,
            Self::StringIterator(iter) => iter.py_type(),
            Self::BytesIterator(_) => Type::BytesIterator,
            Self::RangeIterator(_) => Type::RangeIterator,
            Self::DictKeyIterator(_) => Type::DictKeyIterator,
            Self::DictItemIterator(_) => Type::DictItemIterator,
            Self::DictValueIterator(_) => Type::DictValueIterator,
            Self::SetIterator(_) => Type::SetIterator,
            Self::CallableIterator(_) => Type::CallableIterator,
        }
    }

    pub fn py_estimate_size(&self) -> usize {
        match self {
            Self::Str(s) => s.py_estimate_size(),
            Self::Bytes(b) => b.py_estimate_size(),
            Self::List(l) => l.py_estimate_size(),
            Self::Tuple(t) => t.py_estimate_size(),
            Self::NamedTuple(nt) => nt.py_estimate_size(),
            Self::Dict(d) => d.py_estimate_size(),
            Self::DictKeysView(view) => view.py_estimate_size(),
            Self::DictItemsView(view) => view.py_estimate_size(),
            Self::DictValuesView(view) => view.py_estimate_size(),
            Self::Set(s) => s.py_estimate_size(),
            Self::FrozenSet(fs) => fs.py_estimate_size(),
            Self::Closure(closure) => closure.py_estimate_size(),
            Self::FunctionDefaults(fd) => fd.py_estimate_size(),
            Self::Cell(cell) => cell.py_estimate_size(),
            Self::Range(r) => r.py_estimate_size(),
            Self::Slice(s) => s.py_estimate_size(),
            Self::Exception(e) => e.py_estimate_size(),
            Self::Dataclass(dc) => dc.py_estimate_size(),
            Self::Class(class) => class.py_estimate_size(),
            Self::Instance(instance) => instance.py_estimate_size(),
            Self::BoundMethod(bm) => bm.py_estimate_size(),
            Self::LongInt(li) => li.py_estimate_size(),
            Self::Module(m) => m.py_estimate_size(),
            Self::Coroutine(coro) => coro.py_estimate_size(),
            Self::GatherFuture(gather) => gather.py_estimate_size(),
            Self::ExternalFuture(fut) => fut.py_estimate_size(),
            Self::Path(p) => p.py_estimate_size(),
            Self::OpenFile(file) => file.py_estimate_size(),
            Self::ReMatch(m) => m.py_estimate_size(),
            Self::RePattern(p) => p.py_estimate_size(),
            Self::ExtFunction(function) => function.py_estimate_size(),
            Self::Date(d) => d.py_estimate_size(),
            Self::DateTime(d) => d.py_estimate_size(),
            Self::TimeDelta(d) => d.py_estimate_size(),
            Self::TimeZone(d) => d.py_estimate_size(),
            Self::ListIterator(d) => d.py_estimate_size(),
            Self::TupleIterator(d) => d.py_estimate_size(),
            Self::StringIterator(d) => d.py_estimate_size(),
            Self::BytesIterator(d) => d.py_estimate_size(),
            Self::RangeIterator(d) => d.py_estimate_size(),
            Self::DictKeyIterator(d) => d.py_estimate_size(),
            Self::DictItemIterator(d) => d.py_estimate_size(),
            Self::DictValueIterator(d) => d.py_estimate_size(),
            Self::SetIterator(d) => d.py_estimate_size(),
            Self::CallableIterator(d) => d.py_estimate_size(),
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
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Value>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.0.py_dec_ref_ids(stack);
    }
}

impl HeapItem for Closure {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
            + self.cells.len() * mem::size_of::<HeapId>()
            + self.defaults.len() * mem::size_of::<Value>()
    }

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
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.defaults.len() * mem::size_of::<Value>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Decrement ref count for default values that are heap references
        for default in &mut self.defaults {
            default.py_dec_ref_ids(stack);
        }
    }
}

impl HeapItem for SimpleException {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.arg().map_or(0, String::len) + self.data().estimate_size()
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Exceptions don't contain heap references
    }
}

impl HeapItem for LongInt {
    fn py_estimate_size(&self) -> usize {
        self.estimate_size()
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // LongInt doesn't contain heap references
    }
}

impl HeapItem for Coroutine {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.namespace.len() * mem::size_of::<Value>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Decrement ref count for namespace values that are heap references
        for value in &mut self.namespace {
            value.py_dec_ref_ids(stack);
        }
    }
}

impl HeapItem for GatherFuture {
    fn py_estimate_size(&self) -> usize {
        let state_size = match &self.state {
            GatherState::Awaited(awaited) => awaited_state_size(&awaited.pending_children, &awaited.results),
            GatherState::Pending | GatherState::Completed(_) | GatherState::Failed(_) => 0,
        };
        mem::size_of::<Self>() + self.items.len() * mem::size_of::<HeapId>() + state_size
    }

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
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
    }

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
            Self::ListIterator($value) => $body,
            Self::TupleIterator($value) => $body,
            Self::StringIterator($value) => $body,
            Self::BytesIterator($value) => $body,
            Self::RangeIterator($value) => $body,
            Self::DictKeyIterator($value) => $body,
            Self::DictItemIterator($value) => $body,
            Self::DictValueIterator($value) => $body,
            Self::SetIterator($value) => $body,
            Self::CallableIterator($value) => $body,
            Self::Tuple($value) => $body,
            Self::NamedTuple($value) => $body,
            Self::Dict($value) => $body,
            Self::DictKeysView($value) => $body,
            Self::DictItemsView($value) => $body,
            Self::DictValuesView($value) => $body,
            Self::Set($value) => $body,
            Self::FrozenSet($value) => $body,
            Self::Range($value) => $body,
            Self::Slice($value) => $body,
            Self::Dataclass($value) => $body,
            Self::Class($value) => $body,
            Self::Instance($value) => $body,
            Self::BoundMethod($value) => $body,
            Self::LongInt($value) => $body,
            Self::Path($value) => $body,
            Self::OpenFile($value) => $body,
            Self::RePattern($value) => $body,
            Self::ReMatch($value) => $body,
            Self::Date($value) => $body,
            Self::DateTime($value) => $body,
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

impl<'h> PyTrait<'h> for HeapReadOutput<'h> {
    fn py_bool(&self, vm: &mut VM<'h>) -> bool {
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
                    | Self::ExternalFuture(_) => true,
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

    fn py_call_attr(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        if let Self::Module(module) = self {
            Ok(module.py_call_attr(self_id, vm, attr, args)?)
        } else {
            heap_read_output_py_trait_forward!(
                self,
                |value| Ok(value.py_call_attr(self_id, vm, attr, args)?),
                else {
                    args.drop_with(vm);
                    let type_name = vm.heap.read(self_id).py_type(vm).name(vm.heap, vm.interns);
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

    fn py_enter(&mut self, self_id: HeapId, vm: &mut VM<'h>) -> RunResult<CallResult> {
        // Only types that override the trait default need explicit arms; all
        // others fall through to the catch-all `AttributeError`, matching how
        // `py_call_attr` is structured.
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_enter(self_id, vm),
            else { Err(ExcType::attribute_error(self.py_type(vm).name(vm.heap, vm.interns), "__enter__")) }
        )
    }

    fn py_exit(&mut self, self_id: HeapId, vm: &mut VM<'h>, exc: Option<HeapId>) -> RunResult<CallResult> {
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_exit(self_id, vm, exc),
            else { Err(ExcType::attribute_error(self.py_type(vm).name(vm.heap, vm.interns), "__exit__")) }
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
    fn py_hash(&self, self_id: HeapId, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_hash(self_id, vm),
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
                    Self::Cell(_) | Self::ExtFunction(_) => Ok(Some(identity_hash(self_id))),
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
                    Self::Exception(e) => Ok(allocate_string(e.get(vm.heap).py_str(), vm.heap)?),
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

    fn py_iadd_impl(&mut self, other: &Value, vm: &mut VM<'h>, self_id: Option<HeapId>) -> Result<bool, ResourceError> {
        heap_read_output_py_trait_forward!(self, |value| value.py_iadd_impl(other, vm, self_id), else Ok(false))
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        heap_read_output_py_trait_forward!(
            self,
            |value| value.py_getitem(key, vm),
            else { Err(ExcType::type_error_not_sub(&self.py_type(vm).name(vm.heap, vm.interns))) }
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
                    &self.py_type(vm).name(vm.heap, vm.interns),
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
                    Self::Exception(e) => e.py_getattr(attr, vm),
                    _ => Ok(None),
                }
            }
        )
    }

    fn py_iter(&self, self_id: Option<HeapId>, vm: &mut VM<'h>) -> RunResult<Value> {
        match self {
            Self::Str(value) => value.py_iter(self_id, vm),
            Self::Bytes(value) => value.py_iter(self_id, vm),
            Self::List(value) => value.py_iter(self_id, vm),
            Self::ListIterator(value) => value.py_iter(self_id, vm),
            Self::TupleIterator(value) => value.py_iter(self_id, vm),
            Self::StringIterator(value) => value.py_iter(self_id, vm),
            Self::BytesIterator(value) => value.py_iter(self_id, vm),
            Self::RangeIterator(value) => value.py_iter(self_id, vm),
            Self::DictKeyIterator(value) => value.py_iter(self_id, vm),
            Self::DictItemIterator(value) => value.py_iter(self_id, vm),
            Self::DictValueIterator(value) => value.py_iter(self_id, vm),
            Self::SetIterator(value) => value.py_iter(self_id, vm),
            Self::CallableIterator(value) => value.py_iter(self_id, vm),
            Self::Tuple(value) => value.py_iter(self_id, vm),
            Self::NamedTuple(value) => value.py_iter(self_id, vm),
            Self::Dict(value) => value.py_iter(self_id, vm),
            Self::DictKeysView(value) => value.py_iter(self_id, vm),
            Self::DictItemsView(value) => value.py_iter(self_id, vm),
            Self::DictValuesView(value) => value.py_iter(self_id, vm),
            Self::Set(value) => value.py_iter(self_id, vm),
            Self::FrozenSet(value) => value.py_iter(self_id, vm),
            Self::Range(value) => value.py_iter(self_id, vm),
            Self::Slice(value) => value.py_iter(self_id, vm),
            Self::Dataclass(value) => value.py_iter(self_id, vm),
            Self::Class(value) => value.py_iter(self_id, vm),
            Self::Instance(value) => value.py_iter(self_id, vm),
            Self::BoundMethod(value) => value.py_iter(self_id, vm),
            Self::Path(value) => value.py_iter(self_id, vm),
            Self::OpenFile(value) => value.py_iter(self_id, vm),
            Self::ReMatch(value) => value.py_iter(self_id, vm),
            Self::RePattern(value) => value.py_iter(self_id, vm),
            Self::Date(value) => value.py_iter(self_id, vm),
            Self::DateTime(value) => value.py_iter(self_id, vm),
            Self::TimeDelta(value) => value.py_iter(self_id, vm),
            Self::TimeZone(value) => value.py_iter(self_id, vm),
            Self::Closure(_)
            | Self::FunctionDefaults(_)
            | Self::ExtFunction(_)
            | Self::Cell(_)
            | Self::Exception(_)
            | Self::LongInt(_)
            | Self::Module(_)
            | Self::Coroutine(_)
            | Self::GatherFuture(_)
            | Self::ExternalFuture(_) => Err(ExcType::type_error_not_iterable(
                &self.py_type(vm).name(vm.heap, vm.interns),
            )),
        }
    }

    fn py_next(&mut self, self_id: Option<HeapId>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        match self {
            Self::Str(value) => value.py_next(self_id, vm),
            Self::Bytes(value) => value.py_next(self_id, vm),
            Self::List(value) => value.py_next(self_id, vm),
            Self::ListIterator(value) => value.py_next(self_id, vm),
            Self::TupleIterator(value) => value.py_next(self_id, vm),
            Self::StringIterator(value) => value.py_next(self_id, vm),
            Self::BytesIterator(value) => value.py_next(self_id, vm),
            Self::RangeIterator(value) => value.py_next(self_id, vm),
            Self::DictKeyIterator(value) => value.py_next(self_id, vm),
            Self::DictItemIterator(value) => value.py_next(self_id, vm),
            Self::DictValueIterator(value) => value.py_next(self_id, vm),
            Self::SetIterator(value) => value.py_next(self_id, vm),
            Self::CallableIterator(value) => value.py_next(self_id, vm),
            Self::Tuple(value) => value.py_next(self_id, vm),
            Self::NamedTuple(value) => value.py_next(self_id, vm),
            Self::Dict(value) => value.py_next(self_id, vm),
            Self::DictKeysView(value) => value.py_next(self_id, vm),
            Self::DictItemsView(value) => value.py_next(self_id, vm),
            Self::DictValuesView(value) => value.py_next(self_id, vm),
            Self::Set(value) => value.py_next(self_id, vm),
            Self::FrozenSet(value) => value.py_next(self_id, vm),
            Self::Range(value) => value.py_next(self_id, vm),
            Self::Slice(value) => value.py_next(self_id, vm),
            Self::Dataclass(value) => value.py_next(self_id, vm),
            Self::Class(value) => value.py_next(self_id, vm),
            Self::Instance(value) => value.py_next(self_id, vm),
            Self::BoundMethod(value) => value.py_next(self_id, vm),
            Self::Path(value) => value.py_next(self_id, vm),
            Self::OpenFile(value) => value.py_next(self_id, vm),
            Self::ReMatch(value) => value.py_next(self_id, vm),
            Self::RePattern(value) => value.py_next(self_id, vm),
            Self::Date(value) => value.py_next(self_id, vm),
            Self::DateTime(value) => value.py_next(self_id, vm),
            Self::TimeDelta(value) => value.py_next(self_id, vm),
            Self::TimeZone(value) => value.py_next(self_id, vm),
            other => Err(ExcType::type_error_not_iterator(
                &other.py_type(vm).name(vm.heap, vm.interns),
            )),
        }
    }
}
