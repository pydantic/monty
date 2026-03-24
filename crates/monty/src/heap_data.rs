use std::{borrow::Cow, fmt::Write};

use ahash::AHashSet;
use num_integer::Integer;

use crate::{
    ExcType, ResourceTracker,
    args::ArgValues,
    asyncio::{Coroutine, GatherFuture, GatherItem},
    bytecode::{CallResult, VM},
    exception_private::{RunError, RunResult, SimpleException},
    heap::{DropWithHeap, HeapId, HeapItem, HeapReadOutput},
    intern::FunctionId,
    types::{
        Bytes, Dataclass, Dict, DictItemsView, DictKeysView, DictValuesView, FrozenSet, List, LongInt, Module,
        MontyIter, NamedTuple, Path, PyTrait, Range, ReMatch, RePattern, Set, Slice, Str, Tuple, Type,
        dict_view::DictView,
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
                let type_name = vm.heap.read(self_id).py_type(vm);
                Err(ExcType::attribute_error(type_name, attr.as_str(vm.interns)))
            }
        }
    }

    fn py_type(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        match self {
            Self::Str(s) => s.py_type(vm),
            Self::Bytes(b) => b.py_type(vm),
            Self::List(l) => l.py_type(vm),
            Self::Tuple(t) => t.py_type(vm),
            Self::NamedTuple(nt) => nt.py_type(vm),
            Self::Dict(d) => d.py_type(vm),
            Self::DictKeysView(v) => v.py_type(vm),
            Self::DictItemsView(v) => v.py_type(vm),
            Self::DictValuesView(v) => v.py_type(vm),
            Self::Set(s) => s.py_type(vm),
            Self::FrozenSet(fs) => fs.py_type(vm),
            Self::Closure(_) | Self::FunctionDefaults(_) | Self::ExtFunction(_) => Type::Function,
            Self::Cell(_) => Type::Cell,
            Self::Range(r) => r.py_type(vm),
            Self::Slice(s) => s.py_type(vm),
            Self::Exception(e) => e.py_type(vm),
            Self::Dataclass(dc) => dc.py_type(vm),
            Self::Iter(_) => Type::Iterator,
            Self::LongInt(_) => Type::Int,
            Self::Module(_) => Type::Module,
            Self::Coroutine(_) | Self::GatherFuture(_) => Type::Coroutine,
            Self::Path(p) => p.py_type(vm),
            Self::ReMatch(re) => re.py_type(vm),
            Self::RePattern(p) => p.py_type(vm),
        }
    }

    fn py_len(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        match self {
            Self::Str(s) => s.py_len(vm),
            Self::Bytes(b) => b.py_len(vm),
            Self::List(l) => l.py_len(vm),
            Self::Tuple(t) => t.py_len(vm),
            Self::NamedTuple(nt) => nt.py_len(vm),
            Self::Dict(d) => d.py_len(vm),
            Self::DictKeysView(view) => view.py_len(vm),
            Self::DictItemsView(view) => view.py_len(vm),
            Self::DictValuesView(view) => view.py_len(vm),
            Self::Set(s) => s.py_len(vm),
            Self::FrozenSet(fs) => fs.py_len(vm),
            Self::Range(r) => r.py_len(vm),
            Self::Slice(s) => s.py_len(vm),
            Self::Dataclass(dc) => dc.py_len(vm),
            Self::ReMatch(m) => m.py_len(vm),
            Self::RePattern(p) => p.py_len(vm),
            // Types without length — return None
            _ => None,
        }
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, crate::ResourceError> {
        match (self, other) {
            // Simple types: compare with shared borrows (no &mut VM needed)
            (HeapReadOutput::Str(a), HeapReadOutput::Str(b)) => Ok(a.get(vm.heap).as_str() == b.get(vm.heap).as_str()),
            (HeapReadOutput::Bytes(a), HeapReadOutput::Bytes(b)) => {
                Ok(a.get(vm.heap).as_slice() == b.get(vm.heap).as_slice())
            }
            (HeapReadOutput::LongInt(a), HeapReadOutput::LongInt(b)) => Ok(a.get(vm.heap) == b.get(vm.heap)),
            (HeapReadOutput::Closure(a), HeapReadOutput::Closure(b)) => {
                let a = a.get(vm.heap);
                let b = b.get(vm.heap);
                Ok(a.func_id == b.func_id && a.cells == b.cells)
            }
            (HeapReadOutput::FunctionDefaults(a), HeapReadOutput::FunctionDefaults(b)) => {
                Ok(a.get(vm.heap).func_id == b.get(vm.heap).func_id)
            }
            (HeapReadOutput::Range(a), HeapReadOutput::Range(b)) => {
                // Range::py_eq is pure data comparison — inline to avoid
                // borrow conflict with the &mut VM signature
                let a = a.get(vm.heap);
                let b = b.get(vm.heap);
                let len_a = a.len();
                if len_a != b.len() {
                    Ok(false)
                } else if len_a == 0 {
                    Ok(true)
                } else {
                    Ok(a.start == b.start && a.step == b.step)
                }
            }
            // Container types: use HeapRead-specific comparison methods
            (HeapReadOutput::List(a), HeapReadOutput::List(b)) => a.py_eq(b, vm),
            (HeapReadOutput::Tuple(a), HeapReadOutput::Tuple(b)) => a.py_eq(b, vm),
            // Container types with HeapRead eq methods
            (HeapReadOutput::Dict(a), HeapReadOutput::Dict(b)) => a.py_eq(b, vm),
            (HeapReadOutput::Set(a), HeapReadOutput::Set(b)) => a.py_eq(b, vm),
            (HeapReadOutput::FrozenSet(a), HeapReadOutput::FrozenSet(b)) => a.py_eq(b, vm),
            // NamedTuple: element-wise comparison via HeapRead clone_item
            (HeapReadOutput::NamedTuple(a), HeapReadOutput::NamedTuple(b)) => a.py_eq(b, vm),
            // NamedTuple/Tuple cross-type comparison
            (HeapReadOutput::NamedTuple(nt), HeapReadOutput::Tuple(t))
            | (HeapReadOutput::Tuple(t), HeapReadOutput::NamedTuple(nt)) => nt.eq_tuple(t, vm),
            // DictKeysView comparisons — copy view to local, pass HeapRead directly
            (HeapReadOutput::DictKeysView(a), HeapReadOutput::DictKeysView(b)) => {
                let a_view = DictKeysView::new(a.get(vm.heap).dict_id());
                let b_view = DictKeysView::new(b.get(vm.heap).dict_id());
                a_view.eq_view(b_view, vm)
            }
            (HeapReadOutput::DictKeysView(a), HeapReadOutput::Set(b)) => {
                let view = DictKeysView::new(a.get(vm.heap).dict_id());
                view.eq_set(b, vm)
            }
            (HeapReadOutput::Set(b), HeapReadOutput::DictKeysView(a)) => {
                let view = DictKeysView::new(a.get(vm.heap).dict_id());
                view.eq_set(b, vm)
            }
            (HeapReadOutput::DictKeysView(a), HeapReadOutput::FrozenSet(b)) => {
                let view = DictKeysView::new(a.get(vm.heap).dict_id());
                view.eq_frozenset(b, vm)
            }
            (HeapReadOutput::FrozenSet(b), HeapReadOutput::DictKeysView(a)) => {
                let view = DictKeysView::new(a.get(vm.heap).dict_id());
                view.eq_frozenset(b, vm)
            }
            // DictItemsView comparisons
            (HeapReadOutput::DictItemsView(a), HeapReadOutput::DictItemsView(b)) => {
                let a_view = DictItemsView::new(a.get(vm.heap).dict_id());
                let b_view = DictItemsView::new(b.get(vm.heap).dict_id());
                a_view.eq_view(b_view, vm)
            }
            (HeapReadOutput::DictItemsView(a), HeapReadOutput::Set(b)) => {
                let view = DictItemsView::new(a.get(vm.heap).dict_id());
                view.eq_set(b, vm)
            }
            (HeapReadOutput::Set(b), HeapReadOutput::DictItemsView(a)) => {
                let view = DictItemsView::new(a.get(vm.heap).dict_id());
                view.eq_set(b, vm)
            }
            (HeapReadOutput::DictItemsView(a), HeapReadOutput::FrozenSet(b)) => {
                let view = DictItemsView::new(a.get(vm.heap).dict_id());
                view.eq_frozenset(b, vm)
            }
            (HeapReadOutput::FrozenSet(b), HeapReadOutput::DictItemsView(a)) => {
                let view = DictItemsView::new(a.get(vm.heap).dict_id());
                view.eq_frozenset(b, vm)
            }
            (HeapReadOutput::Dataclass(a), HeapReadOutput::Dataclass(b)) => {
                if a.get(vm.heap).name(vm.interns) != b.get(vm.heap).name(vm.interns) {
                    return Ok(false);
                }
                a.attrs().py_eq(&b.attrs(), vm)
            }
            // Pure data comparisons (no VM needed)
            (HeapReadOutput::Slice(a), HeapReadOutput::Slice(b)) => {
                let a = a.get(vm.heap);
                let b = b.get(vm.heap);
                Ok(a.start == b.start && a.stop == b.stop && a.step == b.step)
            }
            (HeapReadOutput::Path(a), HeapReadOutput::Path(b)) => Ok(a.get(vm.heap) == b.get(vm.heap)),
            (HeapReadOutput::RePattern(a), HeapReadOutput::RePattern(b)) => Ok(a.get(vm.heap) == b.get(vm.heap)),
            // Identity-only types (handled by HeapId comparison above)
            (HeapReadOutput::ReMatch(_), HeapReadOutput::ReMatch(_))
            | (HeapReadOutput::Cell(_), HeapReadOutput::Cell(_))
            | (HeapReadOutput::Exception(_), HeapReadOutput::Exception(_))
            | (HeapReadOutput::Iter(_), HeapReadOutput::Iter(_))
            | (HeapReadOutput::Module(_), HeapReadOutput::Module(_))
            | (HeapReadOutput::Coroutine(_), HeapReadOutput::Coroutine(_))
            | (HeapReadOutput::GatherFuture(_), HeapReadOutput::GatherFuture(_))
            | (HeapReadOutput::DictValuesView(_), HeapReadOutput::DictValuesView(_)) => Ok(false),
            // Different types are never equal
            _ => Ok(false),
        }
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        match self {
            Self::Str(s) => s.py_repr_fmt(f, vm, heap_ids),
            Self::Bytes(b) => b.py_repr_fmt(f, vm, heap_ids),
            Self::List(l) => l.py_repr_fmt(f, vm, heap_ids),
            Self::Tuple(t) => t.py_repr_fmt(f, vm, heap_ids),
            Self::NamedTuple(nt) => nt.py_repr_fmt(f, vm, heap_ids),
            Self::Dict(d) => d.py_repr_fmt(f, vm, heap_ids),
            Self::DictKeysView(view) => view.py_repr_fmt(f, vm, heap_ids),
            Self::DictItemsView(view) => view.py_repr_fmt(f, vm, heap_ids),
            Self::DictValuesView(view) => view.py_repr_fmt(f, vm, heap_ids),
            Self::Set(s) => s.py_repr_fmt(f, vm, heap_ids),
            Self::FrozenSet(fs) => fs.py_repr_fmt(f, vm, heap_ids),
            Self::Closure(closure) => Ok(vm
                .interns
                .get_function(closure.get(vm.heap).func_id)
                .py_repr_fmt(f, vm.interns, 0)?),
            Self::FunctionDefaults(fd) => Ok(vm
                .interns
                .get_function(fd.get(vm.heap).func_id)
                .py_repr_fmt(f, vm.interns, 0)?),
            Self::Cell(cell) => Ok(write!(f, "<cell: {} object>", cell.get(vm.heap).0.py_type(vm))?),
            Self::Range(r) => r.py_repr_fmt(f, vm, heap_ids),
            Self::Slice(s) => s.py_repr_fmt(f, vm, heap_ids),
            Self::Exception(e) => Ok(e.get(vm.heap).py_repr_fmt(f)?),
            Self::Dataclass(dc) => dc.py_repr_fmt(f, vm, heap_ids),
            Self::Iter(_) => Ok(write!(f, "<iterator>")?),
            Self::LongInt(li) => {
                let li = li.get(vm.heap);
                li.check_str_digits_limit()?;
                Ok(write!(f, "{li}")?)
            }
            Self::Module(m) => Ok(write!(f, "<module '{}'>", vm.interns.get_str(m.get(vm.heap).name()))?),
            Self::Coroutine(coro) => {
                let func = vm.interns.get_function(coro.get(vm.heap).func_id);
                let name = vm.interns.get_str(func.name.name_id);
                Ok(write!(f, "<coroutine object {name}>")?)
            }
            Self::GatherFuture(gather) => Ok(write!(f, "<gather({})>", gather.get(vm.heap).item_count())?),
            Self::Path(p) => p.py_repr_fmt(f, vm, heap_ids),
            Self::ReMatch(m) => m.py_repr_fmt(f, vm, heap_ids),
            Self::RePattern(p) => p.py_repr_fmt(f, vm, heap_ids),
            Self::ExtFunction(name) => Ok(write!(f, "<function '{}' external>", name.get(vm.heap))?),
        }
    }

    fn py_str(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> RunResult<Cow<'static, str>> {
        match self {
            // Strings return their value directly without quotes
            Self::Str(s) => Ok(Cow::Owned(s.get(vm.heap).as_str().to_owned())),
            // LongInt returns its string representation
            Self::LongInt(li) => {
                let li = li.get(vm.heap);
                li.check_str_digits_limit()?;
                Ok(Cow::Owned(li.to_string()))
            }
            // Exceptions return just the message (or empty string if no message)
            Self::Exception(e) => Ok(Cow::Owned(e.get(vm.heap).py_str())),
            // Paths return the path string without the PosixPath() wrapper
            Self::Path(p) => Ok(Cow::Owned(p.get(vm.heap).as_str().to_owned())),
            // All other types use repr
            _ => self.py_repr(vm),
        }
    }

    fn py_add(
        &self,
        other: &Self,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<Option<Value>, crate::ResourceError> {
        match (self, other) {
            (HeapReadOutput::Str(a), HeapReadOutput::Str(b)) => {
                let concat = format!("{}{}", a.get(vm.heap).as_str(), b.get(vm.heap).as_str());
                Ok(Some(Value::Ref(vm.heap.allocate(HeapData::Str(concat.into()))?)))
            }
            (HeapReadOutput::Bytes(a), HeapReadOutput::Bytes(b)) => {
                let a_bytes = a.get(vm.heap).as_slice();
                let b_bytes = b.get(vm.heap).as_slice();
                let mut result = Vec::with_capacity(a_bytes.len() + b_bytes.len());
                result.extend_from_slice(a_bytes);
                result.extend_from_slice(b_bytes);
                Ok(Some(Value::Ref(vm.heap.allocate(HeapData::Bytes(result.into()))?)))
            }
            (HeapReadOutput::List(a), HeapReadOutput::List(b)) => a.py_add(b, vm),
            (HeapReadOutput::Tuple(a), HeapReadOutput::Tuple(b)) => a.py_add(b, vm),
            (HeapReadOutput::LongInt(a), HeapReadOutput::LongInt(b)) => {
                let bi = a.get(vm.heap).inner() + b.get(vm.heap).inner();
                Ok(LongInt::new(bi).into_value(vm.heap).map(Some)?)
            }
            _ => Ok(None),
        }
    }

    fn py_sub(
        &self,
        other: &Self,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<Option<Value>, crate::ResourceError> {
        match (self, other) {
            (HeapReadOutput::LongInt(a), HeapReadOutput::LongInt(b)) => {
                let bi = a.get(vm.heap).inner() - b.get(vm.heap).inner();
                Ok(LongInt::new(bi).into_value(vm.heap).map(Some)?)
            }
            _ => Ok(None),
        }
    }

    fn py_mod(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Option<Value>> {
        match (self, other) {
            (HeapReadOutput::LongInt(a), HeapReadOutput::LongInt(b)) => {
                if b.get(vm.heap).is_zero() {
                    Err(ExcType::zero_division().into())
                } else {
                    let bi = a.get(vm.heap).inner().mod_floor(b.get(vm.heap).inner());
                    Ok(LongInt::new(bi).into_value(vm.heap).map(Some)?)
                }
            }
            _ => Ok(None),
        }
    }

    fn py_iadd(
        &mut self,
        other: &Value,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        self_id: Option<HeapId>,
    ) -> Result<bool, crate::ResourceError> {
        match self {
            HeapReadOutput::List(list) => list.py_iadd(other, vm, self_id),
            _ => Ok(false),
        }
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        match self {
            Self::Str(s) => s.py_getitem(key, vm),
            Self::Bytes(b) => b.py_getitem(key, vm),
            Self::List(l) => l.py_getitem(key, vm),
            Self::Tuple(t) => t.py_getitem(key, vm),
            Self::NamedTuple(nt) => nt.py_getitem(key, vm),
            Self::Dict(d) => d.py_getitem(key, vm),
            Self::Range(r) => r.py_getitem(key, vm),
            Self::ReMatch(m) => m.py_getitem(key, vm),
            _ => Err(ExcType::type_error_not_sub(self.py_type(vm))),
        }
    }

    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<()> {
        match self {
            Self::List(l) => l.py_setitem(key, value, vm),
            Self::Dict(d) => d.py_setitem(key, value, vm),
            _ => {
                key.drop_with_heap(vm);
                value.drop_with_heap(vm);
                Err(ExcType::type_error_not_sub_assignment(self.py_type(vm)))
            }
        }
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        match self {
            Self::Str(s) => s.py_getattr(attr, vm),
            Self::Bytes(b) => b.py_getattr(attr, vm),
            Self::List(l) => l.py_getattr(attr, vm),
            Self::Tuple(t) => t.py_getattr(attr, vm),
            Self::NamedTuple(nt) => nt.py_getattr(attr, vm),
            Self::Dict(d) => d.py_getattr(attr, vm),
            Self::DictKeysView(view) => view.py_getattr(attr, vm),
            Self::DictItemsView(view) => view.py_getattr(attr, vm),
            Self::DictValuesView(view) => view.py_getattr(attr, vm),
            Self::Set(s) => s.py_getattr(attr, vm),
            Self::FrozenSet(fs) => fs.py_getattr(attr, vm),
            Self::Range(r) => r.py_getattr(attr, vm),
            Self::Slice(s) => s.py_getattr(attr, vm),
            Self::Dataclass(dc) => dc.py_getattr(attr, vm),
            Self::ReMatch(m) => m.py_getattr(attr, vm),
            Self::RePattern(p) => p.py_getattr(attr, vm),
            Self::Module(m) => Ok(m.py_getattr(attr, vm)),
            Self::Exception(e) => e.py_getattr(attr, vm),
            Self::Path(p) => p.py_getattr(attr, vm),
            _ => Ok(None),
        }
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
