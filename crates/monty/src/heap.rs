use std::{
    borrow::Cow,
    cell::Cell,
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem::size_of,
    vec,
};

use ahash::AHashSet;
use num_integer::Integer;
use smallvec::SmallVec;

// Re-export items moved to `heap_traits` so that `crate::heap::HeapGuard` etc. continue
// to resolve (used by the `defer_drop!` macros and throughout the codebase).
pub(crate) use crate::heap_traits::{ContainsHeap, DropWithHeap, HeapGuard, ImmutableHeapGuard};
use crate::{
    args::ArgValues,
    asyncio::{Coroutine, GatherFuture, GatherItem},
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunResult, SimpleException},
    heap_data::{CellValue, Closure, FunctionDefaults, HeapDataMut},
    intern::Interns,
    resource::{ResourceError, ResourceTracker, check_mult_size, check_repeat_size},
    types::{
        Bytes, Dataclass, Dict, DictItemsView, DictKeysView, DictValuesView, FrozenSet, List, LongInt, Module,
        MontyIter, NamedTuple, Path, PyTrait, Range, ReMatch, RePattern, Set, Slice, Str, Tuple, Type, allocate_tuple,
    },
    value::{EitherStr, Value},
};

/// Unique identifier for values stored inside the heap arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct HeapId(usize);

impl HeapId {
    /// Returns the raw index value.
    #[inline]
    pub fn index(self) -> usize {
        self.0
    }
}

/// The empty tuple is a singleton which is allocated at startup.
const EMPTY_TUPLE_ID: HeapId = HeapId(0);

/// HeapData captures every runtime value that must live in the arena.
///
/// Each variant wraps a type that implements `AbstractValue`, providing
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
    /// An iterator for for-loop iteration and the `iter()` type constructor.
    ///
    /// Created by the `GetIter` opcode or `iter()` builtin, advanced by `ForIter`.
    /// Stores iteration state for lists, tuples, strings, ranges, dicts, and sets.
    Iter(MontyIter),
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
    /// A filesystem path from `pathlib.Path`.
    ///
    /// Stored on the heap to provide Python-compatible path operations.
    /// Pure methods (name, parent, etc.) are handled directly by the VM.
    /// I/O methods (exists, read_text, etc.) yield external function calls.
    Path(Path),
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
    /// Reference to an external function whose name was not found in the intern table.
    ///
    /// Created when the host resolves a `NameLookup` to a callable whose name does not
    /// match any interned string (e.g., the host returns a function with a different
    /// `__name__` than the variable it was assigned to). When called, the VM yields
    /// `FrameExit::ExternalCall` with an `EitherStr::Heap` containing this name.
    ExtFunction(String),
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
    #[inline]
    fn is_gc_tracked(&self) -> bool {
        matches!(
            self,
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
                | Self::Iter(_)
                | Self::Module(_)
                | Self::Coroutine(_)
                | Self::GatherFuture(_)
        )
    }

    /// Returns whether this heap data currently contains any heap references (`Value::Ref`).
    ///
    /// Used during allocation to determine if this data could create reference cycles.
    /// When true, `mark_potential_cycle()` should be called to enable GC.
    ///
    /// Note: This is separate from `is_gc_tracked()` - a container may be GC-tracked
    /// (capable of holding refs) but not currently contain any refs.
    #[inline]
    fn has_refs(&self) -> bool {
        match self {
            Self::List(list) => list.contains_refs(),
            Self::Tuple(tuple) => tuple.contains_refs(),
            Self::NamedTuple(nt) => nt.contains_refs(),
            Self::Dict(dict) => dict.has_refs(),
            Self::DictKeysView(_) | Self::DictItemsView(_) | Self::DictValuesView(_) => true,
            Self::Set(set) => set.has_refs(),
            Self::FrozenSet(fset) => fset.has_refs(),
            // Closures always have refs when they have captured cells (HeapIds)
            Self::Closure(closure) => {
                !closure.cells.is_empty() || closure.defaults.iter().any(|v| matches!(v, Value::Ref(_)))
            }
            Self::FunctionDefaults(fd) => fd.defaults.iter().any(|v| matches!(v, Value::Ref(_))),
            Self::Cell(cell) => matches!(&cell.0, Value::Ref(_)),
            Self::Dataclass(dc) => dc.has_refs(),
            Self::Iter(iter) => iter.has_refs(),
            Self::Module(m) => m.has_refs(),
            // Coroutines have refs from namespace values (params, cell/free vars)
            Self::Coroutine(coro) => coro.namespace.iter().any(|v| matches!(v, Value::Ref(_))),
            // GatherFutures have refs from coroutine items and results
            Self::GatherFuture(gather) => {
                gather
                    .items
                    .iter()
                    .any(|item| matches!(item, crate::asyncio::GatherItem::Coroutine(_)))
                    || gather
                        .results
                        .iter()
                        .any(|r| r.as_ref().is_some_and(|v| matches!(v, Value::Ref(_))))
            }
            // Leaf types cannot have refs
            _ => false,
        }
    }

    /// Returns true if this heap data is a coroutine.
    #[inline]
    pub fn is_coroutine(&self) -> bool {
        matches!(self, Self::Coroutine(_))
    }

    /// Re-cast this as `HeapDataMut` for mutation.
    ///
    /// This is an important part of the Heap invariants: we never allow `&mut HeapData` to
    /// outside of this module to prevent heap data changing type during execution.
    fn to_mut(&mut self) -> HeapDataMut<'_> {
        match self {
            Self::Str(s) => HeapDataMut::Str(s),
            Self::Bytes(b) => HeapDataMut::Bytes(b),
            Self::List(l) => HeapDataMut::List(l),
            Self::Tuple(t) => HeapDataMut::Tuple(t),
            Self::NamedTuple(nt) => HeapDataMut::NamedTuple(nt),
            Self::Dict(d) => HeapDataMut::Dict(d),
            Self::DictKeysView(view) => HeapDataMut::DictKeysView(view),
            Self::DictItemsView(view) => HeapDataMut::DictItemsView(view),
            Self::DictValuesView(view) => HeapDataMut::DictValuesView(view),
            Self::Set(s) => HeapDataMut::Set(s),
            Self::FrozenSet(fs) => HeapDataMut::FrozenSet(fs),
            Self::Closure(closure) => HeapDataMut::Closure(closure),
            Self::FunctionDefaults(fd) => HeapDataMut::FunctionDefaults(fd),
            Self::Cell(cell) => HeapDataMut::Cell(cell),
            Self::Range(r) => HeapDataMut::Range(r),
            Self::Slice(s) => HeapDataMut::Slice(s),
            Self::Exception(e) => HeapDataMut::Exception(e),
            Self::Dataclass(dc) => HeapDataMut::Dataclass(dc),
            Self::Iter(iter) => HeapDataMut::Iter(iter),
            Self::LongInt(li) => HeapDataMut::LongInt(li),
            Self::Module(m) => HeapDataMut::Module(m),
            Self::Coroutine(coro) => HeapDataMut::Coroutine(coro),
            Self::GatherFuture(gather) => HeapDataMut::GatherFuture(gather),
            Self::Path(p) => HeapDataMut::Path(p),
            Self::ReMatch(m) => HeapDataMut::ReMatch(m),
            Self::RePattern(p) => HeapDataMut::RePattern(p),
            Self::ExtFunction(s) => HeapDataMut::ExtFunction(s),
        }
    }
}

/// Manual implementation of AbstractValue dispatch for HeapData.
///
/// This provides efficient dispatch without boxing overhead by matching on
/// the enum variant and delegating to the inner type's implementation.
impl PyTrait for HeapData {
    fn py_type(&self, heap: &Heap<impl ResourceTracker>) -> Type {
        match self {
            Self::Str(s) => s.py_type(heap),
            Self::Bytes(b) => b.py_type(heap),
            Self::List(l) => l.py_type(heap),
            Self::Tuple(t) => t.py_type(heap),
            Self::NamedTuple(nt) => nt.py_type(heap),
            Self::Dict(d) => d.py_type(heap),
            Self::DictKeysView(view) => view.py_type(heap),
            Self::DictItemsView(view) => view.py_type(heap),
            Self::DictValuesView(view) => view.py_type(heap),
            Self::Set(s) => s.py_type(heap),
            Self::FrozenSet(fs) => fs.py_type(heap),
            Self::Closure(_) | Self::FunctionDefaults(_) | Self::ExtFunction(_) => Type::Function,
            Self::Cell(_) => Type::Cell,
            Self::Range(_) => Type::Range,
            Self::Slice(_) => Type::Slice,
            Self::Exception(e) => e.py_type(),
            Self::Dataclass(dc) => dc.py_type(heap),
            Self::Iter(_) => Type::Iterator,
            // LongInt is still `int` in Python - it's an implementation detail
            Self::LongInt(_) => Type::Int,
            Self::Module(_) => Type::Module,
            Self::Coroutine(_) | Self::GatherFuture(_) => Type::Coroutine,
            Self::Path(p) => p.py_type(heap),
            Self::RePattern(p) => p.py_type(heap),
            Self::ReMatch(m) => m.py_type(heap),
        }
    }

    fn py_estimate_size(&self) -> usize {
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
            // TODO: should include size of captured cells and defaults
            Self::Closure(_) | Self::FunctionDefaults(_) => 0,
            Self::Cell(cell) => std::mem::size_of::<Value>() + cell.0.py_estimate_size(),
            Self::Range(_) => std::mem::size_of::<Range>(),
            Self::Slice(s) => s.py_estimate_size(),
            Self::Exception(e) => std::mem::size_of::<SimpleException>() + e.arg().map_or(0, String::len),
            Self::Dataclass(dc) => dc.py_estimate_size(),
            Self::Iter(_) => std::mem::size_of::<MontyIter>(),
            Self::LongInt(li) => li.estimate_size(),
            Self::Module(m) => std::mem::size_of::<Module>() + m.attrs().py_estimate_size(),
            Self::Coroutine(coro) => {
                std::mem::size_of::<Coroutine>() + coro.namespace.len() * std::mem::size_of::<Value>()
            }
            Self::GatherFuture(gather) => {
                std::mem::size_of::<GatherFuture>()
                    + gather.items.len() * std::mem::size_of::<crate::asyncio::GatherItem>()
                    + gather.results.len() * std::mem::size_of::<Option<Value>>()
                    + gather.pending_calls.len() * std::mem::size_of::<crate::asyncio::CallId>()
            }
            Self::Path(p) => p.py_estimate_size(),
            Self::RePattern(p) => p.py_estimate_size(),
            Self::ReMatch(m) => m.py_estimate_size(),
            Self::ExtFunction(s) => std::mem::size_of::<String>() + s.len(),
        }
    }

    fn py_len(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> Option<usize> {
        match self {
            Self::Str(s) => PyTrait::py_len(s, vm),
            Self::Bytes(b) => PyTrait::py_len(b, vm),
            Self::List(l) => PyTrait::py_len(l, vm),
            Self::Tuple(t) => PyTrait::py_len(t, vm),
            Self::NamedTuple(nt) => PyTrait::py_len(nt, vm),
            Self::Dict(d) => PyTrait::py_len(d, vm),
            Self::DictKeysView(view) => PyTrait::py_len(view, vm),
            Self::DictItemsView(view) => PyTrait::py_len(view, vm),
            Self::DictValuesView(view) => PyTrait::py_len(view, vm),
            Self::Set(s) => PyTrait::py_len(s, vm),
            Self::FrozenSet(fs) => PyTrait::py_len(fs, vm),
            Self::Range(r) => Some(r.len()),
            // other types don't have length
            _ => None,
        }
    }

    fn py_eq(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> Result<bool, ResourceError> {
        match (self, other) {
            (Self::Str(a), Self::Str(b)) => a.py_eq(b, heap, interns),
            (Self::Bytes(a), Self::Bytes(b)) => a.py_eq(b, heap, interns),
            (Self::List(a), Self::List(b)) => a.py_eq(b, heap, interns),
            (Self::Tuple(a), Self::Tuple(b)) => a.py_eq(b, heap, interns),
            (Self::NamedTuple(a), Self::NamedTuple(b)) => a.py_eq(b, heap, interns),
            // NamedTuple can compare with Tuple by elements (matching CPython behavior)
            (Self::NamedTuple(nt), Self::Tuple(t)) | (Self::Tuple(t), Self::NamedTuple(nt)) => {
                let nt_items = nt.as_vec();
                let t_items = t.as_slice();
                if nt_items.len() != t_items.len() {
                    return Ok(false);
                }
                let token = heap.incr_recursion_depth()?;
                crate::defer_drop!(token, heap);
                for (a, b) in nt_items.iter().zip(t_items.iter()) {
                    if !a.py_eq(b, heap, interns)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (Self::Dict(a), Self::Dict(b)) => a.py_eq(b, heap, interns),
            (Self::DictKeysView(a), Self::DictKeysView(b)) => a.py_eq(b, heap, interns),
            (Self::DictItemsView(a), Self::DictItemsView(b)) => a.py_eq(b, heap, interns),
            (Self::DictValuesView(_), Self::DictValuesView(_)) => Ok(false),
            (Self::DictKeysView(a), Self::Set(b)) | (Self::Set(b), Self::DictKeysView(a)) => a.eq_set(b, heap, interns),
            (Self::DictKeysView(a), Self::FrozenSet(b)) | (Self::FrozenSet(b), Self::DictKeysView(a)) => {
                a.eq_frozenset(b, heap, interns)
            }
            (Self::DictItemsView(a), Self::Set(b)) | (Self::Set(b), Self::DictItemsView(a)) => {
                a.eq_set(b, heap, interns)
            }
            (Self::DictItemsView(a), Self::FrozenSet(b)) | (Self::FrozenSet(b), Self::DictItemsView(a)) => {
                a.eq_frozenset(b, heap, interns)
            }
            (Self::Set(a), Self::Set(b)) => a.py_eq(b, heap, interns),
            (Self::FrozenSet(a), Self::FrozenSet(b)) => a.py_eq(b, heap, interns),
            (Self::Closure(a), Self::Closure(b)) => Ok(a.func_id == b.func_id && a.cells == b.cells),
            (Self::FunctionDefaults(a), Self::FunctionDefaults(b)) => Ok(a.func_id == b.func_id),
            (Self::Range(a), Self::Range(b)) => a.py_eq(b, heap, interns),
            (Self::Dataclass(a), Self::Dataclass(b)) => a.py_eq(b, heap, interns),
            // LongInt equality
            (Self::LongInt(a), Self::LongInt(b)) => Ok(a == b),
            // Slice equality
            (Self::Slice(a), Self::Slice(b)) => a.py_eq(b, heap, interns),
            // Path equality
            (Self::Path(a), Self::Path(b)) => a.py_eq(b, heap, interns),
            // RePattern equality
            (Self::RePattern(a), Self::RePattern(b)) => a.py_eq(b, heap, interns),
            // ReMatch equality
            (Self::ReMatch(a), Self::ReMatch(b)) => a.py_eq(b, heap, interns),
            // Cells, Exceptions, Iterators, Modules, and async types compare by identity only (handled at Value level via HeapId comparison)
            (Self::Cell(_), Self::Cell(_))
            | (Self::Exception(_), Self::Exception(_))
            | (Self::Iter(_), Self::Iter(_))
            | (Self::Module(_), Self::Module(_))
            | (Self::Coroutine(_), Self::Coroutine(_))
            | (Self::GatherFuture(_), Self::GatherFuture(_)) => Ok(false),
            _ => Ok(false), // Different types are never equal
        }
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        match self {
            Self::Str(s) => s.py_dec_ref_ids(stack),
            Self::Bytes(b) => b.py_dec_ref_ids(stack),
            Self::List(l) => l.py_dec_ref_ids(stack),
            Self::Tuple(t) => t.py_dec_ref_ids(stack),
            Self::NamedTuple(nt) => nt.py_dec_ref_ids(stack),
            Self::Dict(d) => d.py_dec_ref_ids(stack),
            Self::DictKeysView(view) => view.py_dec_ref_ids(stack),
            Self::DictItemsView(view) => view.py_dec_ref_ids(stack),
            Self::DictValuesView(view) => view.py_dec_ref_ids(stack),
            Self::Set(s) => s.py_dec_ref_ids(stack),
            Self::FrozenSet(fs) => fs.py_dec_ref_ids(stack),
            Self::Closure(closure) => {
                // Decrement ref count for captured cells
                stack.extend(closure.cells.iter().copied());
                // Decrement ref count for default values that are heap references
                for default in &mut closure.defaults {
                    default.py_dec_ref_ids(stack);
                }
            }
            Self::FunctionDefaults(fd) => {
                // Decrement ref count for default values that are heap references
                for default in &mut fd.defaults {
                    default.py_dec_ref_ids(stack);
                }
            }
            Self::Cell(cell) => cell.0.py_dec_ref_ids(stack),
            Self::Dataclass(dc) => dc.py_dec_ref_ids(stack),
            Self::Iter(iter) => iter.py_dec_ref_ids(stack),
            Self::Module(m) => m.py_dec_ref_ids(stack),
            Self::Coroutine(coro) => {
                // Decrement ref count for namespace values that are heap references
                for value in &mut coro.namespace {
                    value.py_dec_ref_ids(stack);
                }
            }
            Self::GatherFuture(gather) => {
                // Decrement ref count for coroutine HeapIds
                for item in &gather.items {
                    if let GatherItem::Coroutine(id) = item {
                        stack.push(*id);
                    }
                }
                // Decrement ref count for result values that are heap references
                for result in gather.results.iter_mut().flatten() {
                    result.py_dec_ref_ids(stack);
                }
            }
            // other types have no nested heap references
            _ => {}
        }
    }

    fn py_bool(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> bool {
        match self {
            Self::Str(s) => s.py_bool(vm),
            Self::Bytes(b) => b.py_bool(vm),
            Self::List(l) => l.py_bool(vm),
            Self::Tuple(t) => t.py_bool(vm),
            Self::NamedTuple(nt) => nt.py_bool(vm),
            Self::Dict(d) => d.py_bool(vm),
            Self::DictKeysView(view) => view.py_bool(vm),
            Self::DictItemsView(view) => view.py_bool(vm),
            Self::DictValuesView(view) => view.py_bool(vm),
            Self::Set(s) => s.py_bool(vm),
            Self::FrozenSet(fs) => fs.py_bool(vm),
            Self::Closure(_) | Self::FunctionDefaults(_) | Self::ExtFunction(_) => true,
            Self::Cell(_) => true, // Cells are always truthy
            Self::Range(r) => r.py_bool(vm),
            Self::Slice(s) => s.py_bool(vm),
            Self::Exception(_) => true, // Exceptions are always truthy
            Self::Dataclass(dc) => dc.py_bool(vm),
            Self::Iter(_) => true, // Iterators are always truthy
            Self::LongInt(li) => !li.is_zero(),
            Self::Module(_) => true,       // Modules are always truthy
            Self::Coroutine(_) => true,    // Coroutines are always truthy
            Self::GatherFuture(_) => true, // GatherFutures are always truthy
            Self::Path(p) => p.py_bool(vm),
            Self::RePattern(_) => true, // RePattern objects are always truthy
            Self::ReMatch(_) => true,   // ReMatch objects are always truthy
        }
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result {
        match self {
            Self::Str(s) => s.py_repr_fmt(f, heap, heap_ids, interns),
            Self::Bytes(b) => b.py_repr_fmt(f, heap, heap_ids, interns),
            Self::List(l) => l.py_repr_fmt(f, heap, heap_ids, interns),
            Self::Tuple(t) => t.py_repr_fmt(f, heap, heap_ids, interns),
            Self::NamedTuple(nt) => nt.py_repr_fmt(f, heap, heap_ids, interns),
            Self::Dict(d) => d.py_repr_fmt(f, heap, heap_ids, interns),
            Self::DictKeysView(view) => view.py_repr_fmt(f, heap, heap_ids, interns),
            Self::DictItemsView(view) => view.py_repr_fmt(f, heap, heap_ids, interns),
            Self::DictValuesView(view) => view.py_repr_fmt(f, heap, heap_ids, interns),
            Self::Set(s) => s.py_repr_fmt(f, heap, heap_ids, interns),
            Self::FrozenSet(fs) => fs.py_repr_fmt(f, heap, heap_ids, interns),
            Self::Closure(closure) => interns.get_function(closure.func_id).py_repr_fmt(f, interns, 0),
            Self::FunctionDefaults(fd) => interns.get_function(fd.func_id).py_repr_fmt(f, interns, 0),
            // Cell repr shows the contained value's type
            Self::Cell(cell) => write!(f, "<cell: {} object>", cell.0.py_type(heap)),
            Self::Range(r) => r.py_repr_fmt(f, heap, heap_ids, interns),
            Self::Slice(s) => s.py_repr_fmt(f, heap, heap_ids, interns),
            Self::Exception(e) => e.py_repr_fmt(f),
            Self::Dataclass(dc) => dc.py_repr_fmt(f, heap, heap_ids, interns),
            Self::Iter(_) => write!(f, "<iterator>"),
            Self::LongInt(li) => write!(f, "{li}"),
            Self::Module(m) => write!(f, "<module '{}'>", interns.get_str(m.name())),
            Self::Coroutine(coro) => {
                let func = interns.get_function(coro.func_id);
                let name = interns.get_str(func.name.name_id);
                write!(f, "<coroutine object {name}>")
            }
            Self::GatherFuture(gather) => write!(f, "<gather({})>", gather.item_count()),
            Self::Path(p) => p.py_repr_fmt(f, heap, heap_ids, interns),
            Self::RePattern(p) => p.py_repr_fmt(f, heap, heap_ids, interns),
            Self::ReMatch(m) => m.py_repr_fmt(f, heap, heap_ids, interns),
            Self::ExtFunction(name) => write!(f, "<function '{name}' external>"),
        }
    }

    fn py_str(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Cow<'static, str> {
        match self {
            // Strings return their value directly without quotes
            Self::Str(s) => s.py_str(heap, interns),
            // LongInt returns its string representation
            Self::LongInt(li) => Cow::Owned(li.to_string()),
            // Exceptions return just the message (or empty string if no message)
            Self::Exception(e) => Cow::Owned(e.py_str()),
            // Paths return the path string without the PosixPath() wrapper
            Self::Path(p) => Cow::Owned(p.as_str().to_owned()),
            // All other types use repr
            _ => self.py_repr(heap, interns),
        }
    }

    fn py_add(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> Result<Option<Value>, crate::resource::ResourceError> {
        match (self, other) {
            (Self::Str(a), Self::Str(b)) => a.py_add(b, heap, interns),
            (Self::Bytes(a), Self::Bytes(b)) => a.py_add(b, heap, interns),
            (Self::List(a), Self::List(b)) => a.py_add(b, heap, interns),
            (Self::Tuple(a), Self::Tuple(b)) => a.py_add(b, heap, interns),
            (Self::Dict(a), Self::Dict(b)) => a.py_add(b, heap, interns),
            (Self::LongInt(a), Self::LongInt(b)) => {
                let bi = a.inner() + b.inner();
                Ok(LongInt::new(bi).into_value(heap).map(Some)?)
            }
            // Cells and Dataclasses don't support arithmetic operations
            _ => Ok(None),
        }
    }

    fn py_sub(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<Option<Value>, crate::resource::ResourceError> {
        match (self, other) {
            (Self::Str(a), Self::Str(b)) => a.py_sub(b, heap),
            (Self::Bytes(a), Self::Bytes(b)) => a.py_sub(b, heap),
            (Self::List(a), Self::List(b)) => a.py_sub(b, heap),
            (Self::Tuple(a), Self::Tuple(b)) => a.py_sub(b, heap),
            (Self::Dict(a), Self::Dict(b)) => a.py_sub(b, heap),
            (Self::Set(a), Self::Set(b)) => a.py_sub(b, heap),
            (Self::FrozenSet(a), Self::FrozenSet(b)) => a.py_sub(b, heap),
            (Self::LongInt(a), Self::LongInt(b)) => {
                let bi = a.inner() - b.inner();
                Ok(LongInt::new(bi).into_value(heap).map(Some)?)
            }
            // Cells don't support arithmetic operations
            _ => Ok(None),
        }
    }

    fn py_mod(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> crate::exception_private::RunResult<Option<Value>> {
        match (self, other) {
            (Self::Str(a), Self::Str(b)) => a.py_mod(b, heap),
            (Self::Bytes(a), Self::Bytes(b)) => a.py_mod(b, heap),
            (Self::List(a), Self::List(b)) => a.py_mod(b, heap),
            (Self::Tuple(a), Self::Tuple(b)) => a.py_mod(b, heap),
            (Self::Dict(a), Self::Dict(b)) => a.py_mod(b, heap),
            (Self::LongInt(a), Self::LongInt(b)) => {
                if b.is_zero() {
                    Err(crate::exception_private::ExcType::zero_division().into())
                } else {
                    let bi = a.inner().mod_floor(b.inner());
                    Ok(LongInt::new(bi).into_value(heap).map(Some)?)
                }
            }
            // Cells don't support arithmetic operations
            _ => Ok(None),
        }
    }

    fn py_mod_eq(&self, other: &Self, right_value: i64) -> Option<bool> {
        match (self, other) {
            (Self::Str(a), Self::Str(b)) => a.py_mod_eq(b, right_value),
            (Self::Bytes(a), Self::Bytes(b)) => a.py_mod_eq(b, right_value),
            (Self::List(a), Self::List(b)) => a.py_mod_eq(b, right_value),
            (Self::Tuple(a), Self::Tuple(b)) => a.py_mod_eq(b, right_value),
            (Self::Dict(a), Self::Dict(b)) => a.py_mod_eq(b, right_value),
            // Cells don't support arithmetic operations
            _ => None,
        }
    }

    fn py_iadd(
        &mut self,
        other: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        self_id: Option<HeapId>,
        interns: &Interns,
    ) -> Result<bool, crate::resource::ResourceError> {
        match self {
            Self::List(l) => l.py_iadd(other, heap, self_id, interns),
            Self::Dict(d) => d.py_iadd(other, heap, self_id, interns),
            _ => Ok(false),
        }
    }

    fn py_call_attr(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        match self {
            Self::Str(s) => s.py_call_attr(self_id, vm, attr, args),
            Self::Bytes(b) => b.py_call_attr(self_id, vm, attr, args),
            Self::List(l) => l.py_call_attr(self_id, vm, attr, args),
            Self::Tuple(t) => t.py_call_attr(self_id, vm, attr, args),
            Self::Dict(d) => d.py_call_attr(self_id, vm, attr, args),
            Self::DictKeysView(view) => view.py_call_attr(self_id, vm, attr, args),
            Self::DictItemsView(view) => view.py_call_attr(self_id, vm, attr, args),
            Self::DictValuesView(view) => view.py_call_attr(self_id, vm, attr, args),
            Self::Set(s) => s.py_call_attr(self_id, vm, attr, args),
            Self::FrozenSet(fs) => fs.py_call_attr(self_id, vm, attr, args),
            Self::Dataclass(dc) => dc.py_call_attr(self_id, vm, attr, args),
            Self::Path(p) => p.py_call_attr(self_id, vm, attr, args),
            Self::Module(m) => m.py_call_attr(self_id, vm, attr, args),
            Self::RePattern(p) => p.py_call_attr(self_id, vm, attr, args),
            Self::ReMatch(m) => m.py_call_attr(self_id, vm, attr, args),
            _ => Err(ExcType::attribute_error(self.py_type(vm.heap), attr.as_str(vm.interns))),
        }
    }

    fn py_getitem(&self, key: &Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
        match self {
            Self::Str(s) => s.py_getitem(key, heap, interns),
            Self::Bytes(b) => b.py_getitem(key, heap, interns),
            Self::List(l) => l.py_getitem(key, heap, interns),
            Self::Tuple(t) => t.py_getitem(key, heap, interns),
            Self::NamedTuple(nt) => nt.py_getitem(key, heap, interns),
            Self::Dict(d) => d.py_getitem(key, heap, interns),
            Self::Range(r) => r.py_getitem(key, heap, interns),
            _ => Err(ExcType::type_error_not_sub(self.py_type(heap))),
        }
    }

    fn py_setitem(
        &mut self,
        key: Value,
        value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<()> {
        match self {
            Self::Str(s) => s.py_setitem(key, value, heap, interns),
            Self::Bytes(b) => b.py_setitem(key, value, heap, interns),
            Self::List(l) => l.py_setitem(key, value, heap, interns),
            Self::Tuple(t) => t.py_setitem(key, value, heap, interns),
            Self::Dict(d) => d.py_setitem(key, value, heap, interns),
            _ => Err(ExcType::type_error_not_sub_assignment(self.py_type(heap))),
        }
    }

    fn py_getattr(
        &self,
        attr: &EitherStr,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<CallResult>> {
        match self {
            Self::Dataclass(dc) => dc.py_getattr(attr, heap, interns),
            Self::Module(m) => Ok(m.py_getattr(attr, heap, interns)),
            Self::NamedTuple(nt) => nt.py_getattr(attr, heap, interns),
            Self::Slice(s) => s.py_getattr(attr, heap, interns),
            Self::Exception(exc) => exc.py_getattr(attr, heap, interns),
            Self::Path(p) => p.py_getattr(attr, heap, interns),
            Self::RePattern(p) => p.py_getattr(attr, heap, interns),
            Self::ReMatch(m) => m.py_getattr(attr, heap, interns),
            // All other types don't support attribute access via py_getattr
            _ => Ok(None),
        }
    }
}

/// Hash caching state stored alongside each heap entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum HashState {
    /// Hash has not yet been computed but the value might be hashable.
    Unknown,
    /// Cached hash value for immutable types that have been hashed at least once.
    Cached(u64),
    /// Value is unhashable (mutable types or tuples containing unhashables).
    Unhashable,
}

impl HashState {
    fn for_data(data: &HeapData) -> Self {
        match data {
            // Cells are hashable by identity (like all Python objects without __hash__ override)
            // FrozenSet is immutable and hashable
            // Range is immutable and hashable
            // Slice is immutable and hashable (like in CPython)
            // LongInt is immutable and hashable
            // NamedTuple is immutable and hashable (like Tuple)
            HeapData::Str(_)
            | HeapData::Bytes(_)
            | HeapData::Tuple(_)
            | HeapData::NamedTuple(_)
            | HeapData::FrozenSet(_)
            | HeapData::Cell(_)
            | HeapData::Closure(_)
            | HeapData::FunctionDefaults(_)
            | HeapData::Range(_)
            | HeapData::Slice(_)
            | HeapData::LongInt(_) => Self::Unknown,
            // Dataclass hashability depends on the mutable flag
            HeapData::Dataclass(dc) => {
                if dc.is_frozen() {
                    Self::Unknown
                } else {
                    Self::Unhashable
                }
            }
            // Path is immutable and hashable
            HeapData::Path(_) => Self::Unknown,
            // ExtFunction is hashable (by identity, like closures)
            HeapData::ExtFunction(_) => Self::Unknown,
            // other types are unhashable
            _ => Self::Unhashable,
        }
    }
}

/// A single entry inside the heap arena, storing refcount, payload, and hash metadata.
///
/// The `hash_state` field tracks whether the heap entry is hashable and, if so,
/// caches the computed hash. Mutable types (List, Dict) start as `Unhashable` and
/// will raise TypeError if used as dict keys.
///
/// The `data` field is an Option to support temporary borrowing: when methods like
/// `with_entry_mut` or `call_attr` need mutable access to both the data and the heap,
/// they can `.take()` the data out (leaving `None`), pass `&mut Heap` to user code,
/// then restore the data. This avoids unsafe code while keeping `refcount` accessible
/// for `inc_ref`/`dec_ref` during the borrow.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct HeapValue {
    refcount: Cell<usize>,
    /// The payload data. Temporarily `None` while borrowed via `with_entry_mut`/`call_attr`.
    data: Option<HeapData>,
    /// Current hashing status / cached hash value
    hash_state: HashState,
}

/// Zero-size token returned by [`Heap::incr_recursion_depth`].
///
/// Represents one level of recursion depth that must be released when the
/// recursive operation completes. There are two ways to release the token:
///
/// - **`DropWithHeap`** — for `&mut Heap` paths (e.g., `py_eq`). Compatible with
///   `defer_drop!` and `HeapGuard` for automatic cleanup on all code paths.
/// - **`DropWithImmutableHeap`** — for `&Heap` paths (e.g., `py_repr_fmt`) where
///   only shared access is available. Compatible with `defer_drop_immutable_heap!`
///   and `ImmutableHeapGuard`.
#[derive(Debug)]
pub(crate) struct RecursionToken(());

impl DropWithHeap for RecursionToken {
    #[inline]
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        heap.heap().decr_recursion_depth();
    }
}

/// Reference-counted arena that backs all heap-only runtime values.
///
/// Uses a free list to reuse slots from freed values, keeping memory usage
/// constant for long-running loops that repeatedly allocate and free values.
/// When an value is freed via `dec_ref`, its slot ID is added to the free list.
/// New allocations pop from the free list when available, otherwise append.
///
/// Generic over `T: ResourceTracker` to support different resource tracking strategies.
/// When `T = NoLimitTracker` (the default), all resource checks compile away to no-ops.
///
/// Serialization requires `T: Serialize` and `T: Deserialize`. Custom serde implementation
/// handles the Drop constraint by using `std::mem::take` during serialization.
#[derive(Debug)]
pub(crate) struct Heap<T: ResourceTracker> {
    entries: Vec<Option<HeapValue>>,
    /// IDs of freed slots available for reuse. Populated by `dec_ref`, consumed by `allocate`.
    free_list: Vec<HeapId>,
    /// Resource tracker for enforcing limits and scheduling GC.
    tracker: T,
    /// True if reference cycles may exist. Set when a container stores a Ref,
    /// cleared after GC completes. When false, GC can skip mark-sweep entirely.
    may_have_cycles: bool,
    /// Number of GC applicable allocations since the last GC.
    allocations_since_gc: u32,
    /// Current recursion depth — incremented on function calls and data structure traversals.
    ///
    /// Uses `Cell` for interior mutability so that methods with only `&Heap`
    /// (like `py_repr_fmt`) can still increment/decrement the depth counter.
    recursion_depth: Cell<usize>,
}

impl<T: ResourceTracker + serde::Serialize> serde::Serialize for Heap<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Heap", 6)?;
        state.serialize_field("entries", &self.entries)?;
        state.serialize_field("free_list", &self.free_list)?;
        state.serialize_field("tracker", &self.tracker)?;
        state.serialize_field("may_have_cycles", &self.may_have_cycles)?;
        state.serialize_field("allocations_since_gc", &self.allocations_since_gc)?;
        state.end()
    }
}

impl<'de, T: ResourceTracker + serde::Deserialize<'de>> serde::Deserialize<'de> for Heap<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct HeapFields<T> {
            entries: Vec<Option<HeapValue>>,
            free_list: Vec<HeapId>,
            tracker: T,
            may_have_cycles: bool,
            allocations_since_gc: u32,
        }
        let fields = HeapFields::<T>::deserialize(deserializer)?;
        Ok(Self {
            entries: fields.entries,
            free_list: fields.free_list,
            tracker: fields.tracker,
            may_have_cycles: fields.may_have_cycles,
            allocations_since_gc: fields.allocations_since_gc,
            recursion_depth: Cell::new(0),
        })
    }
}

macro_rules! take_data {
    ($self:ident, $id:expr, $func_name:literal) => {
        $self
            .entries
            .get_mut($id.index())
            .expect(concat!("Heap::", $func_name, ": slot missing"))
            .as_mut()
            .expect(concat!("Heap::", $func_name, ": object already freed"))
            .data
            .take()
            .expect(concat!("Heap::", $func_name, ": data already borrowed"))
    };
}

macro_rules! restore_data {
    ($self:ident, $id:expr, $new_data:expr, $func_name:literal) => {{
        let entry = $self
            .entries
            .get_mut($id.index())
            .expect(concat!("Heap::", $func_name, ": slot missing"))
            .as_mut()
            .expect(concat!("Heap::", $func_name, ": object already freed"));
        entry.data = Some($new_data);
    }};
}

/// GC interval - run GC every 100,000 applicable allocations.
///
/// This is intentionally infrequent to minimize overhead while still
/// eventually collecting reference cycles.
const GC_INTERVAL: u32 = 100_000;

impl<T: ResourceTracker> Heap<T> {
    /// Creates a new heap with the given resource tracker.
    ///
    /// Use this to create heaps with custom resource limits or GC scheduling.
    pub fn new(capacity: usize, tracker: T) -> Self {
        let mut this = Self {
            entries: Vec::with_capacity(capacity),
            free_list: Vec::new(),
            tracker,
            may_have_cycles: false,
            allocations_since_gc: 0,
            recursion_depth: Cell::new(0),
        };
        // TBC: should the empty tuple contribute to the resource limits?
        // If not, can just place it in `entries` directly without going through `allocate()`.
        let empty_tuple = this
            .allocate(HeapData::Tuple(Tuple::default()))
            .expect("Failed to allocate empty tuple singleton");
        debug_assert_eq!(empty_tuple, EMPTY_TUPLE_ID);
        this
    }

    /// Returns a reference to the resource tracker.
    pub fn tracker(&self) -> &T {
        &self.tracker
    }

    /// Returns a mutable reference to the resource tracker.
    pub fn tracker_mut(&mut self) -> &mut T {
        &mut self.tracker
    }

    /// Checks whether the configured time limit has been exceeded.
    ///
    /// Delegates to the resource tracker's `check_time()`. For `NoLimitTracker`,
    /// this is inlined as a no-op with zero runtime cost. For `LimitTracker`,
    /// it compares elapsed time against the configured `max_duration_secs`.
    ///
    /// Call this inside Rust-side loops (builtins, sort, iterator collection)
    /// that execute within a single bytecode instruction and would otherwise
    /// bypass the VM's per-instruction timeout check.
    #[inline]
    pub fn check_time(&self) -> Result<(), ResourceError> {
        self.tracker.check_time()
    }

    /// Increments the recursion depth and checks the limit via the `ResourceTracker`.
    ///
    /// Returns `Ok(RecursionToken)` if within limits. The caller must ensure the
    /// token is released on all code paths — either via `defer_drop!`/`HeapGuard`
    /// (for `&mut Heap` contexts) or via `RecursionToken::release()` (for `&Heap` contexts).
    ///
    /// Returns `Err(ResourceError::Recursion)` if the limit would be exceeded.
    #[inline]
    pub fn incr_recursion_depth(&self) -> Result<RecursionToken, ResourceError> {
        let depth = self.recursion_depth.get();
        self.tracker.check_recursion_depth(depth)?;
        self.recursion_depth.set(depth + 1);
        Ok(RecursionToken(()))
    }

    /// Increments the recursion depth, returning `Some(RecursionToken)` if within
    /// limits, or `None` if the limit is exceeded.
    ///
    /// Use this in repr-like contexts where exceeding the limit should produce
    /// truncated output (e.g., `[...]`) rather than an error.
    #[inline]
    pub fn incr_recursion_depth_for_repr(&self) -> Option<RecursionToken> {
        self.incr_recursion_depth().ok()
    }

    /// Decrements the recursion depth.
    ///
    /// Called internally by `RecursionToken` — prefer releasing the token
    /// rather than calling this directly.
    #[inline]
    pub(crate) fn decr_recursion_depth(&self) {
        let depth = self.recursion_depth.get();
        debug_assert!(depth > 0, "decr_recursion_depth called when depth is 0");
        self.recursion_depth.set(depth - 1);
    }

    /// Returns the current recursion depth.
    ///
    /// Used during async task switching to compute a task's depth contribution
    /// before adjusting the global counter.
    pub(crate) fn get_recursion_depth(&self) -> usize {
        self.recursion_depth.get()
    }

    /// Sets the recursion depth to an explicit value.
    ///
    /// Used after deserialization to restore the recursion depth to match
    /// the number of active (non-global) namespace frames that were serialized.
    /// Also used during async task switching to subtract/add a task's depth
    /// contribution when switching away from/to that task.
    pub(crate) fn set_recursion_depth(&self, depth: usize) {
        self.recursion_depth.set(depth);
    }

    /// Number of entries in the heap
    pub fn size(&self) -> usize {
        self.entries.len()
    }

    /// Marks that a reference cycle may exist in the heap.
    ///
    /// Call this when a container (list, dict, tuple, etc.) stores a reference
    /// to another heap object. This enables the GC to skip mark-sweep entirely
    /// when no cycles are possible.
    #[inline]
    pub fn mark_potential_cycle(&mut self) {
        self.may_have_cycles = true;
    }

    /// Returns the number of GC-tracked allocations since the last garbage collection.
    ///
    /// This counter increments for each allocation of a GC-tracked type (List, Dict, etc.)
    /// and resets to 0 when `collect_garbage` runs. Useful for testing GC behavior.
    #[cfg(feature = "ref-count-return")]
    pub fn get_allocations_since_gc(&self) -> u32 {
        self.allocations_since_gc
    }

    /// Allocates a new heap entry.
    ///
    /// Returns `Err(ResourceError)` if allocation would exceed configured limits.
    /// Use this when you need to handle resource limit errors gracefully.
    ///
    /// Only GC-tracked types (containers that can hold references) count toward the
    /// GC allocation threshold. Leaf types like strings don't trigger GC.
    ///
    /// When allocating a container that contains heap references, marks potential
    /// cycles to enable garbage collection.
    pub fn allocate(&mut self, data: HeapData) -> Result<HeapId, ResourceError> {
        self.tracker.on_allocate(|| data.py_estimate_size())?;
        if data.is_gc_tracked() {
            self.allocations_since_gc = self.allocations_since_gc.wrapping_add(1);
            // Mark potential cycles if this container has heap references.
            // This is essential for types like Dict where setitem doesn't call
            // mark_potential_cycle() - the allocation is the only place to detect refs.
            if data.has_refs() {
                self.may_have_cycles = true;
            }
        }

        let hash_state = HashState::for_data(&data);
        let new_entry = HeapValue {
            refcount: Cell::new(1),
            data: Some(data),
            hash_state,
        };

        let id = if let Some(id) = self.free_list.pop() {
            // Reuse a freed slot
            self.entries[id.index()] = Some(new_entry);
            id
        } else {
            // No free slots, append new entry
            let id = self.entries.len();
            self.entries.push(Some(new_entry));
            HeapId(id)
        };

        Ok(id)
    }

    /// Returns the singleton empty tuple.
    ///
    /// In Python, `() is ()` is always `True` because empty tuples are interned.
    /// This method provides the same optimization by returning the same `HeapId`
    /// for all empty tuple allocations.
    ///
    /// The returned `Value` has its reference count incremented, so the caller
    /// owns a reference and must call `dec_ref` when done.
    pub fn get_empty_tuple(&mut self) -> Value {
        // Return existing singleton with incremented refcount
        self.inc_ref(EMPTY_TUPLE_ID);
        Value::Ref(EMPTY_TUPLE_ID)
    }

    /// Increments the reference count for an existing heap entry.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    pub fn inc_ref(&self, id: HeapId) {
        let value = self
            .entries
            .get(id.index())
            .expect("Heap::inc_ref: slot missing")
            .as_ref()
            .expect("Heap::inc_ref: object already freed");
        value.refcount.update(|r| r + 1);
    }

    /// Decrements the reference count and frees the value (plus children) once it hits zero.
    ///
    /// Uses an iterative work stack instead of recursion to avoid Rust stack overflow
    /// when freeing deeply nested containers (e.g., a list nested 10,000 levels deep).
    /// This is analogous to CPython's "trashcan" mechanism for safe deallocation.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    pub fn dec_ref(&mut self, id: HeapId) {
        let mut current_id = id;
        let mut work_stack = Vec::new();
        loop {
            let slot = self
                .entries
                .get_mut(current_id.index())
                .expect("Heap::dec_ref: slot missing");
            let entry = slot.as_mut().expect("Heap::dec_ref: object already freed");
            if entry.refcount.get() > 1 {
                entry.refcount.update(|r| r - 1);
            } else if let Some(value) = slot.take() {
                // refcount == 1, free the value and add slot to free list for reuse
                self.free_list.push(current_id);

                // Notify tracker of freed memory
                if let Some(ref data) = value.data {
                    self.tracker.on_free(|| data.py_estimate_size());
                }

                // Collect child IDs and push onto work stack for iterative processing
                if let Some(mut data) = value.data {
                    data.py_dec_ref_ids(&mut work_stack);
                    drop(data);
                }
            }

            let Some(next_id) = work_stack.pop() else {
                break;
            };
            current_id = next_id;
        }
    }

    /// Returns an immutable reference to the heap data stored at the given ID.
    ///
    /// # Panics
    /// Panics if the value ID is invalid, the value has already been freed,
    /// or the data is currently borrowed via `with_entry_mut`/`call_attr`.
    #[must_use]
    pub fn get(&self, id: HeapId) -> &HeapData {
        self.entries
            .get(id.index())
            .expect("Heap::get: slot missing")
            .as_ref()
            .expect("Heap::get: object already freed")
            .data
            .as_ref()
            .expect("Heap::get: data currently borrowed")
    }

    /// Returns a mutable reference to the heap data stored at the given ID.
    ///
    /// # Panics
    /// Panics if the value ID is invalid, the value has already been freed,
    /// or the data is currently borrowed via `with_entry_mut`/`call_attr`.
    pub fn get_mut(&mut self, id: HeapId) -> HeapDataMut<'_> {
        self.entries
            .get_mut(id.index())
            .expect("Heap::get_mut: slot missing")
            .as_mut()
            .expect("Heap::get_mut: object already freed")
            .data
            .as_mut()
            .expect("Heap::get_mut: data currently borrowed")
            .to_mut()
    }

    /// Returns or computes the hash for the heap entry at the given ID.
    ///
    /// Hashes are computed lazily on first use and then cached. Returns
    /// `Ok(Some(hash))` for immutable types, `Ok(None)` for mutable types,
    /// or `Err(ResourceError::Recursion)` if the recursion limit is exceeded.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    pub fn get_or_compute_hash(&mut self, id: HeapId, interns: &Interns) -> Result<Option<u64>, ResourceError> {
        let entry = self
            .entries
            .get_mut(id.index())
            .expect("Heap::get_or_compute_hash: slot missing")
            .as_mut()
            .expect("Heap::get_or_compute_hash: object already freed");

        match entry.hash_state {
            HashState::Unhashable => return Ok(None),
            HashState::Cached(hash) => return Ok(Some(hash)),
            HashState::Unknown => {}
        }

        // Handle Cell specially - uses identity-based hashing (like Python cell objects)
        if let Some(HeapData::Cell(_)) = &entry.data {
            let mut hasher = DefaultHasher::new();
            id.hash(&mut hasher);
            let hash = hasher.finish();
            entry.hash_state = HashState::Cached(hash);
            return Ok(Some(hash));
        }

        // Compute hash lazily - need to temporarily take data to avoid borrow conflict.
        // IMPORTANT: data must be restored to the entry on ALL paths (including errors)
        // to avoid dropping HeapData containing Value::Ref without proper cleanup.
        let mut data = entry.data.take().expect("Heap::get_or_compute_hash: data borrowed");
        let hash = data.to_mut().compute_hash_if_immutable(self, interns);

        // Restore data before handling the result
        let entry = self
            .entries
            .get_mut(id.index())
            .expect("Heap::get_or_compute_hash: slot missing after compute")
            .as_mut()
            .expect("Heap::get_or_compute_hash: object freed during compute");
        entry.data = Some(data);

        // Now handle the result and cache if successful
        let hash = hash?;
        entry.hash_state = match hash {
            Some(value) => HashState::Cached(value),
            None => HashState::Unhashable,
        };
        Ok(hash)
    }

    /// Calls an attribute on the heap entry, returning an `CallResult` that may signal
    /// OS, external, or method calls.
    ///
    /// Temporarily takes ownership of the payload to avoid borrow conflicts when attribute
    /// implementations also need mutable heap access (e.g. for refcounting).
    ///
    /// Returns `CallResult` which may be:
    /// - `Value(v)` - Method completed synchronously with value `v`
    /// - `OsCall(func, args)` - Method needs OS operation; VM should yield to host
    /// - `ExternalCall(id, args)` - Method needs external function call
    /// - `MethodCall(name, args)` - Dataclass method call; VM should yield to host
    pub fn call_attr(vm: &mut VM<'_, '_, T>, id: HeapId, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        // Take data out so the borrow of self.entries ends
        let heap = &mut *vm.heap;
        let mut data = take_data!(heap, id, "call_attr");

        let result = data.py_call_attr(id, vm, attr, args);

        // Restore data
        let heap = &mut *vm.heap;
        restore_data!(heap, id, data, "call_attr");
        result
    }

    /// Gives mutable access to a heap entry while allowing reentrant heap usage
    /// inside the closure (e.g. to read other values or allocate results).
    ///
    /// The data is temporarily taken from the heap entry, so the closure can safely
    /// mutate both the entry data and the heap (e.g. to allocate new values).
    /// The data is automatically restored after the closure completes.
    pub fn with_entry_mut<F, R>(&mut self, id: HeapId, f: F) -> R
    where
        F: FnOnce(&mut Self, HeapDataMut) -> R,
    {
        // Take data out in a block so the borrow of self.entries ends
        let mut data = take_data!(self, id, "with_entry_mut");

        let result = f(self, data.to_mut());

        // Restore data
        restore_data!(self, id, data, "with_entry_mut");
        result
    }

    /// Temporarily takes ownership of two heap entries so their data can be borrowed
    /// simultaneously while still permitting mutable access to the heap (e.g. to
    /// allocate results). Automatically restores both entries after the closure
    /// finishes executing.
    pub fn with_two<F, R>(&mut self, left: HeapId, right: HeapId, f: F) -> R
    where
        F: FnOnce(&mut Self, &HeapData, &HeapData) -> R,
    {
        if left == right {
            // Same value - take data once and pass it twice
            let data = take_data!(self, left, "with_two");

            let result = f(self, &data, &data);

            restore_data!(self, left, data, "with_two");
            result
        } else {
            // Different values - take both
            let left_data = take_data!(self, left, "with_two (left)");
            let right_data = take_data!(self, right, "with_two (right)");

            let result = f(self, &left_data, &right_data);

            // Restore in reverse order
            restore_data!(self, right, right_data, "with_two (right)");
            restore_data!(self, left, left_data, "with_two (left)");
            result
        }
    }

    /// Returns the reference count for the heap entry at the given ID.
    ///
    /// This is primarily used for testing reference counting behavior.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    #[must_use]
    #[cfg(feature = "ref-count-return")]
    pub fn get_refcount(&self, id: HeapId) -> usize {
        self.entries
            .get(id.index())
            .expect("Heap::get_refcount: slot missing")
            .as_ref()
            .expect("Heap::get_refcount: object already freed")
            .refcount
            .get()
    }

    /// Returns the number of live (non-freed) values on the heap.
    ///
    /// This is primarily used for testing to verify that all heap entries
    /// are accounted for in reference count tests.
    ///
    /// Excludes the empty tuple singleton since it's an internal optimization
    /// detail that persists even when not explicitly used by user code.
    #[must_use]
    #[cfg(feature = "ref-count-return")]
    pub fn entry_count(&self) -> usize {
        // 1.. to skip index 0 which is the empty tuple singleton
        self.entries[1..].iter().filter(|o| o.is_some()).count()
    }

    /// Helper for List in-place add: extends the destination vec with items from a heap list.
    ///
    /// This method exists to work around borrow checker limitations when List::py_iadd
    /// needs to read from one heap entry while extending another. By keeping both
    /// the read and the refcount increments within Heap's impl block, we can use the
    /// take/restore pattern to avoid the lifetime propagation issues.
    ///
    /// Returns `true` if successful, `false` if the source ID is not a List.
    pub fn iadd_extend_list(&mut self, source_id: HeapId, dest: &mut Vec<Value>) -> bool {
        if let HeapData::List(list) = self.get(source_id) {
            let items: Vec<Value> = list.as_slice().iter().map(|v| v.clone_with_heap(self)).collect();
            dest.extend(items);
            true
        } else {
            false
        }
    }

    /// Multiplies a heap-allocated value by an `i64`.
    ///
    /// If `id` refers to a `LongInt`, performs integer multiplication with a size
    /// pre-check. Otherwise, treats `id` as a sequence and `int_val` as the repeat
    /// count. This avoids multiple `heap.get()` calls by looking up the data once.
    ///
    /// Returns `Ok(None)` if the heap entry is neither a LongInt nor a sequence type.
    pub fn mult_ref_by_i64(&mut self, id: HeapId, int_val: i64) -> RunResult<Option<Value>> {
        if let HeapData::LongInt(li) = self.get(id) {
            check_mult_size(li.bits(), i64_bits(int_val), &self.tracker)?;
            let result = LongInt::new(li.inner().clone()) * LongInt::from(int_val);
            Ok(Some(result.into_value(self)?))
        } else {
            let count = i64_to_repeat_count(int_val)?;
            self.mult_sequence(id, count)
        }
    }

    /// Multiplies two heap-allocated values.
    ///
    /// Returns Ok(None) for unsupported type combinations.
    pub fn mult_heap_values(&mut self, id1: HeapId, id2: HeapId) -> RunResult<Option<Value>> {
        let (seq_id, count) = match (self.get(id1), self.get(id2)) {
            (HeapData::LongInt(a), HeapData::LongInt(b)) => {
                check_mult_size(a.bits(), b.bits(), &self.tracker)?;
                let result = LongInt::new(a.inner() * b.inner());
                return Ok(Some(result.into_value(self)?));
            }
            (HeapData::LongInt(li), _) => {
                let count = longint_to_repeat_count(li)?;
                (id2, count)
            }
            (_, HeapData::LongInt(li)) => {
                let count = longint_to_repeat_count(li)?;
                (id1, count)
            }
            _ => return Ok(None),
        };

        self.mult_sequence(seq_id, count)
    }

    /// Multiplies (repeats) a sequence by an integer count.
    ///
    /// This method handles sequence repetition for Python's `*` operator when applied
    /// to sequences (str, bytes, list, tuple). It creates a new heap-allocated sequence
    /// with the elements repeated `count` times.
    ///
    /// # Arguments
    /// * `id` - HeapId of the sequence to repeat
    /// * `count` - Number of times to repeat (0 returns empty sequence)
    ///
    /// # Returns
    /// * `Ok(Some(Value))` - The new repeated sequence
    /// * `Ok(None)` - If the heap entry is not a sequence type
    /// * `Err` - If allocation fails due to resource limits
    pub fn mult_sequence(&mut self, id: HeapId, count: usize) -> RunResult<Option<Value>> {
        match self.get(id) {
            HeapData::Str(s) => {
                check_repeat_size(s.len(), count, &self.tracker)?;
                Ok(Some(Value::Ref(
                    self.allocate(HeapData::Str(s.as_str().repeat(count).into()))?,
                )))
            }
            HeapData::Bytes(b) => {
                check_repeat_size(b.len(), count, &self.tracker)?;
                Ok(Some(Value::Ref(
                    self.allocate(HeapData::Bytes(b.as_slice().repeat(count).into()))?,
                )))
            }
            HeapData::List(list) => {
                check_repeat_size(list.len().saturating_mul(size_of::<Value>()), count, &self.tracker)?;
                let mut result = Vec::with_capacity(list.as_slice().len() * count);
                for _ in 0..count {
                    result.extend(list.as_slice().iter().map(|v| v.clone_with_heap(self)));
                    self.check_time()?;
                }
                Ok(Some(Value::Ref(self.allocate(HeapData::List(List::new(result)))?)))
            }
            HeapData::Tuple(tuple) => {
                if count == 0 {
                    return Ok(Some(self.get_empty_tuple()));
                }
                check_repeat_size(
                    tuple.as_slice().len().saturating_mul(size_of::<Value>()),
                    count,
                    &self.tracker,
                )?;
                let mut result = SmallVec::with_capacity(tuple.as_slice().len() * count);
                for _ in 0..count {
                    result.extend(tuple.as_slice().iter().map(|v| v.clone_with_heap(self)));
                    self.check_time()?;
                }
                Ok(Some(allocate_tuple(result, self)?))
            }
            _ => Ok(None),
        }
    }

    /// Returns whether garbage collection should run.
    ///
    /// True if reference cycles count exist in the heap
    /// and the number of allocations since the last GC exceeds the interval.
    #[inline]
    pub fn should_gc(&self) -> bool {
        self.may_have_cycles && self.allocations_since_gc >= GC_INTERVAL
    }

    /// Runs mark-sweep garbage collection to free unreachable cycles.
    ///
    /// This method takes a closure that provides an iterator of root HeapIds
    /// (typically from the VM's globals and stack). It marks all reachable objects starting
    /// from roots, then sweeps (frees) any unreachable objects.
    ///
    /// This is necessary because reference counting alone cannot free cycles
    /// where objects reference each other but are unreachable from the program.
    ///
    /// # Caller Responsibility
    /// The caller should check `should_gc()` before calling this method.
    /// If no cycles are possible, the caller can skip GC entirely.
    ///
    /// # Arguments
    /// * `root` - HeapIds that are roots
    pub fn collect_garbage(&mut self, root: Vec<HeapId>) {
        // Mark phase: collect all reachable IDs using BFS
        // Use Vec<bool> instead of HashSet for O(1) operations without hashing overhead
        let mut reachable: Vec<bool> = vec![false; self.entries.len()];
        let mut work_list: Vec<HeapId> = root;

        while let Some(id) = work_list.pop() {
            let idx = id.index();
            // Skip if out of bounds or already visited
            if idx >= reachable.len() || reachable[idx] {
                continue;
            }
            reachable[idx] = true;

            // Add children to work list
            if let Some(Some(entry)) = self.entries.get(idx)
                && let Some(ref data) = entry.data
            {
                collect_child_ids(data, &mut work_list);
            }
        }

        // Sweep phase: free unreachable values
        for (id, value) in self.entries.iter_mut().enumerate() {
            if reachable[id] {
                continue;
            }

            // This entry is unreachable - free it
            if let Some(value) = value.take() {
                // Notify tracker of freed memory
                if let Some(ref data) = value.data {
                    self.tracker.on_free(|| data.py_estimate_size());
                }

                self.free_list.push(HeapId(id));

                // Mark Values as Dereferenced when ref-count-panic is enabled
                #[cfg(feature = "ref-count-panic")]
                if let Some(mut data) = value.data {
                    data.py_dec_ref_ids(&mut Vec::new());
                }
            }
        }

        // Reset cycle flag after GC - cycles have been collected
        self.may_have_cycles = false;
        self.allocations_since_gc = 0;
    }
}

/// Computes the number of significant bits in an `i64`.
///
/// Returns 0 for zero, otherwise returns the position of the highest set bit
/// plus one. Uses unsigned absolute value to handle negative numbers correctly.
fn i64_bits(value: i64) -> u64 {
    if value == 0 {
        0
    } else {
        u64::from(64 - value.unsigned_abs().leading_zeros())
    }
}

/// Converts an `i64` repeat count to `usize` for sequence repetition.
///
/// Returns 0 for negative values (Python treats negative repeat counts as 0).
/// Returns `OverflowError` if the value exceeds `usize::MAX`.
fn i64_to_repeat_count(n: i64) -> RunResult<usize> {
    if n <= 0 {
        Ok(0)
    } else {
        usize::try_from(n).map_err(|_| ExcType::overflow_repeat_count().into())
    }
}

/// Converts a `LongInt` repeat count to `usize` for sequence repetition.
///
/// Returns 0 for negative values (Python treats negative repeat counts as 0).
/// Returns `OverflowError` if the value exceeds `usize::MAX`.
fn longint_to_repeat_count(li: &LongInt) -> RunResult<usize> {
    if li.is_negative() {
        Ok(0)
    } else if let Some(count) = li.to_usize() {
        Ok(count)
    } else {
        Err(ExcType::overflow_repeat_count().into())
    }
}

/// Collects child HeapIds from a HeapData value for GC traversal.
fn collect_child_ids(data: &HeapData, work_list: &mut Vec<HeapId>) {
    match data {
        HeapData::List(list) => {
            // Skip iteration if no refs - major GC optimization for lists of primitives
            if !list.contains_refs() {
                return;
            }
            for value in list.as_slice() {
                if let Value::Ref(id) = value {
                    work_list.push(*id);
                }
            }
        }
        HeapData::Tuple(tuple) => {
            // Skip iteration if no refs - GC optimization for tuples of primitives
            if !tuple.contains_refs() {
                return;
            }
            for value in tuple.as_slice() {
                if let Value::Ref(id) = value {
                    work_list.push(*id);
                }
            }
        }
        HeapData::NamedTuple(nt) => {
            // Skip iteration if no refs - GC optimization for namedtuples of primitives
            if !nt.contains_refs() {
                return;
            }
            for value in nt.as_vec() {
                if let Value::Ref(id) = value {
                    work_list.push(*id);
                }
            }
        }
        HeapData::Dict(dict) => {
            // Skip iteration if no refs - major GC optimization for dicts of primitives
            if !dict.has_refs() {
                return;
            }
            for (k, v) in dict {
                if let Value::Ref(id) = k {
                    work_list.push(*id);
                }
                if let Value::Ref(id) = v {
                    work_list.push(*id);
                }
            }
        }
        HeapData::DictKeysView(view) => {
            work_list.push(view.dict_id());
        }
        HeapData::DictItemsView(view) => {
            work_list.push(view.dict_id());
        }
        HeapData::DictValuesView(view) => {
            work_list.push(view.dict_id());
        }
        HeapData::Set(set) => {
            for value in set.storage().iter() {
                if let Value::Ref(id) = value {
                    work_list.push(*id);
                }
            }
        }
        HeapData::FrozenSet(frozenset) => {
            for value in frozenset.storage().iter() {
                if let Value::Ref(id) = value {
                    work_list.push(*id);
                }
            }
        }
        HeapData::Closure(closure) => {
            // Add captured cells to work list
            for cell_id in &closure.cells {
                work_list.push(*cell_id);
            }
            // Add default values that are heap references
            for default in &closure.defaults {
                if let Value::Ref(id) = default {
                    work_list.push(*id);
                }
            }
        }
        HeapData::FunctionDefaults(fd) => {
            // Add default values that are heap references
            for default in &fd.defaults {
                if let Value::Ref(id) = default {
                    work_list.push(*id);
                }
            }
        }
        HeapData::Cell(cell) => {
            // Cell can contain a reference to another heap value
            if let Value::Ref(id) = &cell.0 {
                work_list.push(*id);
            }
        }
        HeapData::Dataclass(dc) => {
            // Dataclass attrs are stored in a Dict - iterate through entries
            for (k, v) in dc.attrs() {
                if let Value::Ref(id) = k {
                    work_list.push(*id);
                }
                if let Value::Ref(id) = v {
                    work_list.push(*id);
                }
            }
        }
        HeapData::Iter(iter) => {
            // Iterator holds a reference to the iterable being iterated
            if let Value::Ref(id) = iter.value() {
                work_list.push(*id);
            }
        }
        HeapData::Module(m) => {
            // Module attrs can contain references to heap values
            if !m.has_refs() {
                return;
            }
            for (k, v) in m.attrs() {
                if let Value::Ref(id) = k {
                    work_list.push(*id);
                }
                if let Value::Ref(id) = v {
                    work_list.push(*id);
                }
            }
        }
        HeapData::Coroutine(coro) => {
            // Add namespace values that are heap references
            for value in &coro.namespace {
                if let Value::Ref(id) = value {
                    work_list.push(*id);
                }
            }
        }
        HeapData::GatherFuture(gather) => {
            // Add coroutine HeapIds to work list
            for item in &gather.items {
                if let GatherItem::Coroutine(coro_id) = item {
                    work_list.push(*coro_id);
                }
            }
            // Add result values that are heap references
            for result in gather.results.iter().flatten() {
                if let Value::Ref(id) = result {
                    work_list.push(*id);
                }
            }
        }
        // Leaf types with no heap references
        _ => {}
    }
}

/// Drop implementation for Heap that marks all contained Objects as Dereferenced
/// before dropping to prevent panics when the `ref-count-panic` feature is enabled.
#[cfg(feature = "ref-count-panic")]
impl<T: ResourceTracker> Drop for Heap<T> {
    fn drop(&mut self) {
        // Mark all contained Objects as Dereferenced before dropping.
        // We use py_dec_ref_ids for this since it handles the marking
        // (we ignore the collected IDs since we're dropping everything anyway).
        let mut dummy_stack = Vec::new();
        for value in self.entries.iter_mut().flatten() {
            if let Some(data) = &mut value.data {
                data.py_dec_ref_ids(&mut dummy_stack);
            }
        }
    }
}
