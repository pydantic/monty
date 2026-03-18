use std::{borrow::Cow, fmt::Write};

use ahash::AHashSet;

use crate::{
    ExcType, ResourceTracker,
    args::ArgValues,
    asyncio::{Coroutine, GatherFuture, GatherItem},
    bytecode::{CallResult, VM},
    exception_private::{RunError, SimpleException},
    heap::{DropWithHeap, Heap, HeapId, HeapItem, HeapReadOutput},
    intern::FunctionId,
    types::{
        Bytes, Dataclass, Dict, DictItemsView, DictKeysView, DictValuesView, FrozenSet, List, LongInt, Module,
        MontyIter, NamedTuple, Path, PyTrait, Range, ReMatch, RePattern, Set, Slice, Str, Tuple, Type,
        bytes::bytes_repr_fmt, dict_view::DictView, list::repr_sequence_fmt, str::string_repr_fmt,
        tuple::tuple_repr_fmt,
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
    pub(crate) fn is_gc_tracked(&self) -> bool {
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
    pub(crate) fn has_refs(&self) -> bool {
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
                gather.items.iter().any(|item| matches!(item, GatherItem::Coroutine(_)))
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
}

/// Thin wrapper around `Value` which is used in the `Cell` variant above.
///
/// The inner value is the cell's mutable payload.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub(crate) struct CellValue(pub(crate) Value);

impl std::ops::Deref for CellValue {
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
        std::mem::size_of::<Value>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.0.py_dec_ref_ids(stack);
    }
}

impl HeapItem for Closure {
    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.cells.len() * std::mem::size_of::<HeapId>()
            + self.defaults.len() * std::mem::size_of::<Value>()
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
        std::mem::size_of::<Self>() + self.defaults.len() * std::mem::size_of::<Value>()
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
        std::mem::size_of::<Self>() + self.arg().map_or(0, String::len)
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
        std::mem::size_of::<Self>() + self.namespace.len() * std::mem::size_of::<Value>()
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
        std::mem::size_of::<Self>()
            + self.items.len() * std::mem::size_of::<GatherItem>()
            + self.results.len() * std::mem::size_of::<Option<Value>>()
            + self.pending_calls.len() * std::mem::size_of::<crate::asyncio::CallId>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Decrement ref count for coroutine HeapIds
        for item in &self.items {
            if let GatherItem::Coroutine(id) = item {
                stack.push(*id);
            }
        }
        // Decrement ref count for result values that are heap references
        for result in self.results.iter_mut().flatten() {
            result.py_dec_ref_ids(stack);
        }
    }
}

impl HeapData {
    pub fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        match self {
            Self::Str(_) => Type::Str,
            Self::Bytes(_) => Type::Bytes,
            Self::List(_) => Type::List,
            Self::Tuple(_) => Type::Tuple,
            Self::NamedTuple(_) => Type::NamedTuple,
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
            Self::Exception(e) => e.py_type(),
            Self::Dataclass(_) => Type::Dataclass,
            Self::Iter(_) => Type::Iterator,
            Self::LongInt(_) => Type::Int,
            Self::Module(_) => Type::Module,
            Self::Coroutine(_) | Self::GatherFuture(_) => Type::Coroutine,
            Self::Path(_) => Type::Path,
            Self::ReMatch(_) => Type::ReMatch,
            Self::RePattern(_) => Type::RePattern,
        }
    }

    pub fn py_len(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> Option<usize> {
        match self {
            Self::Str(s) => Some(s.as_str().chars().count()),
            Self::Bytes(b) => Some(b.len()),
            Self::List(l) => Some(l.len()),
            Self::Tuple(t) => Some(t.as_slice().len()),
            Self::NamedTuple(nt) => Some(nt.items_len()),
            Self::Dict(d) => Some(d.len()),
            Self::DictKeysView(view) => Some(view.dict(&*vm.heap).len()),
            Self::DictItemsView(view) => Some(view.dict(&*vm.heap).len()),
            Self::DictValuesView(view) => Some(view.dict(&*vm.heap).len()),
            Self::Set(s) => Some(s.len()),
            Self::FrozenSet(fs) => Some(fs.len()),
            Self::Range(r) => Some(r.len()),
            _ => None,
        }
    }

    pub fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'_, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result {
        match self {
            Self::Str(s) => string_repr_fmt(s.as_str(), f),
            Self::Bytes(b) => bytes_repr_fmt(b.as_slice(), f),
            Self::List(l) => repr_sequence_fmt('[', ']', l.as_slice(), f, vm, heap_ids),
            Self::Tuple(t) => tuple_repr_fmt(t.as_slice(), f, vm, heap_ids),
            Self::NamedTuple(nt) => nt.py_repr_fmt(f, vm, heap_ids),
            Self::Dict(d) => d.py_repr_fmt(f, vm, heap_ids),
            Self::DictKeysView(view) => view.py_repr_fmt(f, vm, heap_ids),
            Self::DictItemsView(view) => view.py_repr_fmt(f, vm, heap_ids),
            Self::DictValuesView(view) => view.py_repr_fmt(f, vm, heap_ids),
            Self::Set(s) => s.repr_fmt(f, vm, heap_ids),
            Self::FrozenSet(fs) => fs.repr_fmt(f, vm, heap_ids),
            Self::Closure(closure) => vm.interns.get_function(closure.func_id).py_repr_fmt(f, vm.interns, 0),
            Self::FunctionDefaults(fd) => vm.interns.get_function(fd.func_id).py_repr_fmt(f, vm.interns, 0),
            Self::Cell(cell) => write!(f, "<cell: {} object>", cell.0.py_type(vm.heap)),
            Self::Range(r) => r.py_repr_fmt(f, vm, heap_ids),
            Self::Slice(s) => s.py_repr_fmt(f, vm, heap_ids),
            Self::Exception(e) => e.py_repr_fmt(f),
            Self::Dataclass(dc) => dc.py_repr_fmt(f, vm, heap_ids),
            Self::Iter(_) => write!(f, "<iterator>"),
            Self::LongInt(li) => write!(f, "{li}"),
            Self::Module(m) => write!(f, "<module '{}'>", vm.interns.get_str(m.name())),
            Self::Coroutine(coro) => {
                let func = vm.interns.get_function(coro.func_id);
                let name = vm.interns.get_str(func.name.name_id);
                write!(f, "<coroutine object {name}>")
            }
            Self::GatherFuture(gather) => write!(f, "<gather({})>", gather.item_count()),
            Self::Path(p) => p.py_repr_fmt(f, vm, heap_ids),
            Self::ReMatch(m) => m.py_repr_fmt(f, vm, heap_ids),
            Self::RePattern(p) => p.py_repr_fmt(f, vm, heap_ids),
            Self::ExtFunction(name) => write!(f, "<function '{name}' external>"),
        }
    }

    /// Returns the Python `repr()` string for this value.
    ///
    /// Convenience wrapper around `py_repr_fmt` that returns an owned string.
    fn py_repr(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> Cow<'static, str> {
        let mut s = String::new();
        let mut heap_ids = AHashSet::new();
        // Unwrap is safe: writing to String never fails
        self.py_repr_fmt(&mut s, vm, &mut heap_ids).unwrap();
        Cow::Owned(s)
    }

    pub fn py_str(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> Cow<'static, str> {
        match self {
            // Strings return their value directly without quotes
            Self::Str(s) => Cow::Owned(s.as_str().to_owned()),
            // LongInt returns its string representation
            Self::LongInt(li) => Cow::Owned(li.to_string()),
            // Exceptions return just the message (or empty string if no message)
            Self::Exception(e) => Cow::Owned(e.py_str()),
            // Paths return the path string without the PosixPath() wrapper
            Self::Path(p) => Cow::Owned(p.as_str().to_owned()),
            // All other types use repr
            _ => self.py_repr(vm),
        }
    }
}

impl<'h> PyTrait<'h> for HeapReadOutput<'h> {
    fn py_bool(&self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
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
            Self::Cell(_) => true,
            Self::Range(r) => r.py_bool(vm),
            Self::Slice(s) => s.py_bool(vm),
            Self::Exception(_) => true,
            Self::Dataclass(dc) => dc.py_bool(vm),
            Self::Iter(_) => true,
            Self::LongInt(li) => !li.get(vm.heap).is_zero(),
            Self::Module(_) => true,
            Self::Coroutine(_) => true,
            Self::GatherFuture(_) => true,
            Self::Path(p) => p.py_bool(vm),
            Self::ReMatch(m) => m.py_bool(vm),
            Self::RePattern(p) => p.py_bool(vm),
        }
    }

    fn py_call_attr(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        match self {
            HeapReadOutput::Str(s) => Ok(s.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::Bytes(b) => Ok(b.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::List(list) => Ok(list.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::Tuple(t) => Ok(t.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::Dict(dict) => Ok(dict.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::DictKeysView(view) => Ok(view.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::DictItemsView(view) => Ok(view.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::DictValuesView(view) => Ok(view.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::Set(s) => Ok(s.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::FrozenSet(fs) => Ok(fs.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::Dataclass(dc) => Ok(dc.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::Path(p) => Ok(p.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::Module(m) => Ok(m.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::ReMatch(m) => Ok(m.py_call_attr(self_id, vm, attr, args)?),
            HeapReadOutput::RePattern(p) => Ok(p.py_call_attr(self_id, vm, attr, args)?),
            // Types without methods — return AttributeError
            _ => {
                args.drop_with_heap(vm);
                let type_name = vm.heap.get(self_id).py_type(vm.heap);
                Err(ExcType::attribute_error(type_name, attr.as_str(vm.interns)))
            }
        }
    }

    fn py_type(&self, heap: &Heap<impl ResourceTracker>) -> Type {
        todo!()
    }

    fn py_len(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        todo!()
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, crate::ResourceError> {
        todo!()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result {
        todo!()
    }
}

impl HeapData {
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
            Self::Iter(iter) => iter.py_estimate_size(),
            Self::LongInt(li) => li.py_estimate_size(),
            Self::Module(m) => m.py_estimate_size(),
            Self::Coroutine(coro) => coro.py_estimate_size(),
            Self::GatherFuture(gather) => gather.py_estimate_size(),
            Self::Path(p) => p.py_estimate_size(),
            Self::ReMatch(m) => m.py_estimate_size(),
            Self::RePattern(p) => p.py_estimate_size(),
            Self::ExtFunction(s) => std::mem::size_of::<String>() + s.len(),
        }
    }
}
