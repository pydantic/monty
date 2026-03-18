use std::{
    borrow::Cow,
    fmt::Write,
    hash::{DefaultHasher, Hash, Hasher},
    mem::discriminant,
};

use ahash::AHashSet;
use num_integer::Integer;

use crate::{
    ExcType, ResourceError, ResourceTracker,
    args::ArgValues,
    asyncio::{Coroutine, GatherFuture, GatherItem},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{RunResult, SimpleException},
    heap::{Heap, HeapId, HeapItem},
    intern::{FunctionId, Interns},
    types::{
        Bytes, Dataclass, Dict, DictItemsView, DictKeysView, DictValuesView, FrozenSet, List, LongInt, Module,
        MontyIter, NamedTuple, Path, PyTrait, Range, ReMatch, RePattern, Set, Slice, Str, Tuple, Type,
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

    /// Re-cast this as `HeapDataMut` for mutation.
    ///
    /// This is an important part of the Heap invariants: we never allow `&mut HeapData`
    /// outside of the heap module to prevent heap data changing type during execution.
    pub(crate) fn to_mut(&mut self) -> HeapDataMut<'_> {
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

/// Mutable reference to `HeapData` inner values
#[derive(Debug)]
pub(crate) enum HeapDataMut<'a> {
    Str(&'a mut Str),
    Bytes(&'a mut Bytes),
    List(&'a mut List),
    Tuple(&'a mut Tuple),
    NamedTuple(&'a mut NamedTuple),
    Dict(&'a mut Dict),
    DictKeysView(&'a mut DictKeysView),
    DictItemsView(&'a mut DictItemsView),
    DictValuesView(&'a mut DictValuesView),
    Set(&'a mut Set),
    FrozenSet(&'a mut FrozenSet),
    Closure(&'a mut Closure),
    FunctionDefaults(&'a mut FunctionDefaults),
    /// A cell wrapping a single mutable value for closure support.
    ///
    /// Cells enable nonlocal variable access by providing a heap-allocated
    /// container that can be shared between a function and its nested functions.
    /// Both the outer function and inner function hold references to the same
    /// cell, allowing modifications to propagate across scope boundaries.
    Cell(&'a mut CellValue),
    /// A range object (e.g., `range(10)` or `range(1, 10, 2)`).
    ///
    /// Stored on the heap to keep `Value` enum small (16 bytes). Range objects
    /// are immutable and hashable.
    Range(&'a mut Range),
    /// A slice object (e.g., `slice(1, 10, 2)` or from `x[1:10:2]`).
    ///
    /// Stored on the heap to keep `Value` enum small. Slice objects represent
    /// start:stop:step indices for sequence slicing operations.
    Slice(&'a mut Slice),
    /// An exception instance (e.g., `ValueError('message')`).
    ///
    /// Stored on the heap to keep `Value` enum small (16 bytes). Exceptions
    /// are created when exception types are called or when `raise` is executed.
    Exception(&'a mut SimpleException),
    /// A dataclass instance with fields and method references.
    ///
    /// Contains a class name, a Dict of field name -> value mappings, and a set
    /// of method names that trigger external function calls when invoked.
    Dataclass(&'a mut Dataclass),
    /// An iterator for for-loop iteration and the `iter()` type constructor.
    ///
    /// Created by the `GetIter` opcode or `iter()` builtin, advanced by `ForIter`.
    /// Stores iteration state for lists, tuples, strings, ranges, dicts, and sets.
    Iter(&'a mut MontyIter),
    /// An arbitrary precision integer (LongInt).
    ///
    /// Stored on the heap to keep `Value` enum at 16 bytes. Python has one `int` type,
    /// so LongInt is an implementation detail - we use `Value::Int(i64)` for performance
    /// when values fit, and promote to LongInt on overflow. When LongInt results fit back
    /// in i64, they are demoted back to `Value::Int` for performance.
    LongInt(&'a mut LongInt),
    /// A Python module (e.g., `sys`, `typing`).
    ///
    /// Modules have a name and a dictionary of attributes. They are created by
    /// import statements and can have refs to other heap values in their attributes.
    Module(&'a mut Module),
    /// A coroutine object from an async function call.
    ///
    /// Contains pre-bound arguments and captured cells, ready to be awaited.
    /// When awaited, a new frame is pushed using the stored namespace.
    Coroutine(&'a mut Coroutine),
    /// A gather() result tracking multiple coroutines/tasks.
    ///
    /// Created by asyncio.gather() and spawns tasks when awaited.
    GatherFuture(&'a mut GatherFuture),
    /// A filesystem path from `pathlib.Path`.
    ///
    /// Stored on the heap to provide Python-compatible path operations.
    /// Pure methods (name, parent, etc.) are handled directly by the VM.
    /// I/O methods (exists, read_text, etc.) yield external function calls.
    Path(&'a mut Path),
    /// A regex match result from `re.match()`, `re.search()`, etc.
    ///
    /// Stores matched text, capture groups, and positions. All data is owned
    /// (no heap references), so reference counting is trivial.
    ReMatch(&'a mut ReMatch),
    /// A compiled regex pattern from `re.compile()`.
    ///
    /// Wraps a compiled regex with the original pattern string and flags.
    /// Custom serde serializes only the pattern and flags, recompiling on deserialize.
    RePattern(&'a mut RePattern),
    /// Reference to an external function where the name was not interned.
    ///
    /// Created when the host resolves a name lookup to a callable whose name
    /// does not match any interned string (e.g., the host returns a function
    /// with a different `__name__` than the variable it was assigned to).
    ExtFunction(&'a mut String),
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

impl HeapDataMut<'_> {
    /// Computes hash for immutable heap types that can be used as dict keys.
    ///
    /// Returns `Ok(Some(hash))` for immutable types (Str, Bytes, Tuple of hashables).
    /// Returns `Ok(None)` for mutable types (List, Dict) which cannot be dict keys.
    /// Returns `Err(ResourceError::Recursion)` if the recursion limit is exceeded
    /// while hashing deeply nested containers (e.g., tuples of tuples).
    ///
    /// This is called lazily when the value is first used as a dict key,
    /// avoiding unnecessary hash computation for values that are never used as keys.
    pub fn compute_hash_if_immutable(
        &self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> Result<Option<u64>, ResourceError> {
        match self {
            // Hash just the actual string or bytes content for consistency with Value::InternString/InternBytes
            // hence we don't include the discriminant
            Self::Str(s) => {
                let mut hasher = DefaultHasher::new();
                s.as_str().hash(&mut hasher);
                Ok(Some(hasher.finish()))
            }
            Self::Bytes(b) => {
                let mut hasher = DefaultHasher::new();
                b.as_slice().hash(&mut hasher);
                Ok(Some(hasher.finish()))
            }
            Self::FrozenSet(fs) => {
                // FrozenSet hash is XOR of element hashes (order-independent)
                // Recursion depth is checked inside compute_hash
                fs.compute_hash(heap, interns)
            }
            Self::Tuple(t) => {
                let token = heap.incr_recursion_depth()?;
                crate::defer_drop!(token, heap);
                let mut hasher = DefaultHasher::new();
                discriminant(self).hash(&mut hasher);
                // Tuple is hashable only if all elements are hashable
                for obj in t.as_slice() {
                    match obj.py_hash(heap, interns)? {
                        Some(h) => h.hash(&mut hasher),
                        None => return Ok(None),
                    }
                }
                Ok(Some(hasher.finish()))
            }
            Self::NamedTuple(nt) => {
                let token = heap.incr_recursion_depth()?;
                crate::defer_drop!(token, heap);
                let mut hasher = DefaultHasher::new();
                discriminant(self).hash(&mut hasher);
                // Hash only by elements (not type_name) to match equality semantics
                for obj in nt.as_vec() {
                    match obj.py_hash(heap, interns)? {
                        Some(h) => h.hash(&mut hasher),
                        None => return Ok(None),
                    }
                }
                Ok(Some(hasher.finish()))
            }
            Self::Closure(closure) => {
                let mut hasher = DefaultHasher::new();
                discriminant(self).hash(&mut hasher);
                // TODO, this is NOT proper hashing, we should somehow hash the function properly
                closure.func_id.hash(&mut hasher);
                Ok(Some(hasher.finish()))
            }
            Self::FunctionDefaults(fd) => {
                let mut hasher = DefaultHasher::new();
                discriminant(self).hash(&mut hasher);
                // TODO, this is NOT proper hashing, we should somehow hash the function properly
                fd.func_id.hash(&mut hasher);
                Ok(Some(hasher.finish()))
            }
            Self::Range(range) => {
                let mut hasher = DefaultHasher::new();
                discriminant(self).hash(&mut hasher);
                range.start.hash(&mut hasher);
                range.stop.hash(&mut hasher);
                range.step.hash(&mut hasher);
                Ok(Some(hasher.finish()))
            }
            // Dataclass hashability depends on the mutable flag
            // Recursion depth is checked inside compute_hash
            Self::Dataclass(dc) => dc.compute_hash(heap, interns),
            // Slices are immutable and hashable (like in CPython)
            Self::Slice(slice) => {
                let mut hasher = DefaultHasher::new();
                discriminant(self).hash(&mut hasher);
                slice.start.hash(&mut hasher);
                slice.stop.hash(&mut hasher);
                slice.step.hash(&mut hasher);
                Ok(Some(hasher.finish()))
            }
            // Path is immutable and hashable
            Self::Path(path) => {
                let mut hasher = DefaultHasher::new();
                discriminant(self).hash(&mut hasher);
                path.as_str().hash(&mut hasher);
                Ok(Some(hasher.finish()))
            }
            // LongInt is immutable and hashable
            Self::LongInt(li) => Ok(Some(li.hash())),
            // ExtFunction is hashable by name
            Self::ExtFunction(name) => {
                let mut hasher = DefaultHasher::new();
                discriminant(self).hash(&mut hasher);
                name.hash(&mut hasher);
                Ok(Some(hasher.finish()))
            }
            // other types cannot be hashed (Cell is handled specially in get_or_compute_hash)
            _ => Ok(None),
        }
    }
}

/// Shared dispatch macro for `PyTrait` methods on `HeapData` and `HeapDataMut`.
///
/// Both enums have identical variants (owned vs borrowed) and identical dispatch
/// logic. This macro eliminates the duplication by generating the match arms for
/// each method. The caller provides `self` and the method body for each variant.
macro_rules! impl_py_trait_dispatch {
    ($self_ty:ty) => {
        impl PyTrait for $self_ty {
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
                    Self::ReMatch(m) => m.py_type(heap),
                    Self::RePattern(p) => p.py_type(heap),
                }
            }

            fn py_len(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> Option<usize> {
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
                    Self::Range(r) => Some(r.len()),
                    // other types don't have length
                    _ => None,
                }
            }

            fn py_eq(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
                match (self, other) {
                    (Self::Str(a), Self::Str(b)) => a.py_eq(b, vm),
                    (Self::Bytes(a), Self::Bytes(b)) => a.py_eq(b, vm),
                    (Self::List(a), Self::List(b)) => a.py_eq(b, vm),
                    (Self::Tuple(a), Self::Tuple(b)) => a.py_eq(b, vm),
                    (Self::NamedTuple(a), Self::NamedTuple(b)) => a.py_eq(b, vm),
                    // NamedTuple can compare with Tuple by elements (matching CPython behavior)
                    (Self::NamedTuple(nt), Self::Tuple(t)) | (Self::Tuple(t), Self::NamedTuple(nt)) => {
                        let nt_items = nt.as_vec();
                        let t_items = t.as_slice();
                        if nt_items.len() != t_items.len() {
                            return Ok(false);
                        }
                        // Helper function pattern: acquire token, call helper, drop token.
                        // Cannot use defer_drop! here because the helper needs &mut VM.
                        let token = vm.heap.incr_recursion_depth()?;
                        defer_drop!(token, vm);
                        for (a, b) in nt_items.iter().zip(t_items.iter()) {
                            if !a.py_eq(b, vm)? {
                                return Ok(false);
                            }
                        }
                        Ok(true)
                    }
                    (Self::Dict(a), Self::Dict(b)) => a.py_eq(b, vm),
                    (Self::DictKeysView(a), Self::DictKeysView(b)) => a.py_eq(b, vm),
                    (Self::DictItemsView(a), Self::DictItemsView(b)) => a.py_eq(b, vm),
                    (Self::DictValuesView(_), Self::DictValuesView(_)) => Ok(false),
                    (Self::DictKeysView(a), Self::Set(b)) | (Self::Set(b), Self::DictKeysView(a)) => a.eq_set(b, vm),
                    (Self::DictKeysView(a), Self::FrozenSet(b)) | (Self::FrozenSet(b), Self::DictKeysView(a)) => {
                        a.eq_frozenset(b, vm)
                    }
                    (Self::DictItemsView(a), Self::Set(b)) | (Self::Set(b), Self::DictItemsView(a)) => a.eq_set(b, vm),
                    (Self::DictItemsView(a), Self::FrozenSet(b)) | (Self::FrozenSet(b), Self::DictItemsView(a)) => {
                        a.eq_frozenset(b, vm)
                    }
                    (Self::Set(a), Self::Set(b)) => a.py_eq(b, vm),
                    (Self::FrozenSet(a), Self::FrozenSet(b)) => a.py_eq(b, vm),
                    (Self::Closure(a), Self::Closure(b)) => Ok(a.func_id == b.func_id && a.cells == b.cells),
                    (Self::FunctionDefaults(a), Self::FunctionDefaults(b)) => Ok(a.func_id == b.func_id),
                    (Self::Range(a), Self::Range(b)) => a.py_eq(b, vm),
                    (Self::Dataclass(a), Self::Dataclass(b)) => a.py_eq(b, vm),
                    // LongInt equality
                    (Self::LongInt(a), Self::LongInt(b)) => Ok(a == b),
                    // Slice equality
                    (Self::Slice(a), Self::Slice(b)) => a.py_eq(b, vm),
                    // Path equality
                    (Self::Path(a), Self::Path(b)) => a.py_eq(b, vm),
                    // ReMatch objects are not comparable
                    (Self::ReMatch(a), Self::ReMatch(b)) => a.py_eq(b, vm),
                    // RePattern equality by pattern string and flags
                    (Self::RePattern(a), Self::RePattern(b)) => a.py_eq(b, vm),
                    // Cells, Exceptions, Iterators, Modules, and async types compare by identity only
                    // (handled at Value level via HeapId comparison)
                    (Self::Cell(_), Self::Cell(_))
                    | (Self::Exception(_), Self::Exception(_))
                    | (Self::Iter(_), Self::Iter(_))
                    | (Self::Module(_), Self::Module(_))
                    | (Self::Coroutine(_), Self::Coroutine(_))
                    | (Self::GatherFuture(_), Self::GatherFuture(_)) => Ok(false),
                    _ => Ok(false), // Different types are never equal
                }
            }

            fn py_cmp(
                &self,
                other: &Self,
                vm: &mut VM<'_, '_, impl ResourceTracker>,
            ) -> Result<Option<std::cmp::Ordering>, ResourceError> {
                match (self, other) {
                    (Self::Str(a), Self::Str(b)) => a.py_cmp(b, vm),
                    (Self::Bytes(a), Self::Bytes(b)) => a.py_cmp(b, vm),
                    (Self::Tuple(a), Self::Tuple(b)) => a.py_cmp(b, vm),
                    _ => Ok(None),
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
                    Self::ReMatch(m) => m.py_bool(vm),
                    Self::RePattern(p) => p.py_bool(vm),
                }
            }

            fn py_repr_fmt(
                &self,
                f: &mut impl Write,
                vm: &VM<'_, '_, impl ResourceTracker>,
                heap_ids: &mut AHashSet<HeapId>,
            ) -> std::fmt::Result {
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
                    Self::Closure(closure) => vm
                        .interns
                        .get_function(closure.func_id)
                        .py_repr_fmt(f, vm.interns, 0),
                    Self::FunctionDefaults(fd) => vm.interns.get_function(fd.func_id).py_repr_fmt(f, vm.interns, 0),
                    // Cell repr shows the contained value's type
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

            fn py_str(&self, vm: &VM<'_, '_, impl ResourceTracker>) -> Cow<'static, str> {
                match self {
                    // Strings return their value directly without quotes
                    Self::Str(s) => s.py_str(vm),
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

            fn py_add(
                &self,
                other: &Self,
                vm: &mut VM<'_, '_, impl ResourceTracker>,
            ) -> Result<Option<Value>, ResourceError> {
                match (self, other) {
                    (Self::Str(a), Self::Str(b)) => a.py_add(b, vm),
                    (Self::Bytes(a), Self::Bytes(b)) => a.py_add(b, vm),
                    (Self::List(a), Self::List(b)) => a.py_add(b, vm),
                    (Self::Tuple(a), Self::Tuple(b)) => a.py_add(b, vm),
                    (Self::Dict(a), Self::Dict(b)) => a.py_add(b, vm),
                    (Self::LongInt(a), Self::LongInt(b)) => {
                        let bi = a.inner() + b.inner();
                        Ok(LongInt::new(bi).into_value(vm.heap).map(Some)?)
                    }
                    // Cells and Dataclasses don't support arithmetic operations
                    _ => Ok(None),
                }
            }

            fn py_sub(
                &self,
                other: &Self,
                vm: &mut VM<'_, '_, impl ResourceTracker>,
            ) -> Result<Option<Value>, ResourceError> {
                match (self, other) {
                    (Self::Str(a), Self::Str(b)) => a.py_sub(b, vm),
                    (Self::Bytes(a), Self::Bytes(b)) => a.py_sub(b, vm),
                    (Self::List(a), Self::List(b)) => a.py_sub(b, vm),
                    (Self::Tuple(a), Self::Tuple(b)) => a.py_sub(b, vm),
                    (Self::Dict(a), Self::Dict(b)) => a.py_sub(b, vm),
                    (Self::Set(a), Self::Set(b)) => a.py_sub(b, vm),
                    (Self::FrozenSet(a), Self::FrozenSet(b)) => a.py_sub(b, vm),
                    (Self::LongInt(a), Self::LongInt(b)) => {
                        let bi = a.inner() - b.inner();
                        Ok(LongInt::new(bi).into_value(vm.heap).map(Some)?)
                    }
                    // Cells don't support arithmetic operations
                    _ => Ok(None),
                }
            }

            fn py_mod(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Option<Value>> {
                match (self, other) {
                    (Self::Str(a), Self::Str(b)) => a.py_mod(b, vm),
                    (Self::Bytes(a), Self::Bytes(b)) => a.py_mod(b, vm),
                    (Self::List(a), Self::List(b)) => a.py_mod(b, vm),
                    (Self::Tuple(a), Self::Tuple(b)) => a.py_mod(b, vm),
                    (Self::Dict(a), Self::Dict(b)) => a.py_mod(b, vm),
                    (Self::LongInt(a), Self::LongInt(b)) => {
                        if b.is_zero() {
                            Err(ExcType::zero_division().into())
                        } else {
                            let bi = a.inner().mod_floor(b.inner());
                            Ok(LongInt::new(bi).into_value(vm.heap).map(Some)?)
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
                vm: &mut VM<'_, '_, impl ResourceTracker>,
                self_id: Option<HeapId>,
            ) -> Result<bool, ResourceError> {
                match self {
                    Self::List(list) => list.py_iadd(other, vm, self_id),
                    Self::Dict(dict) => dict.py_iadd(other, vm, self_id),
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
                    Self::ReMatch(m) => m.py_call_attr(self_id, vm, attr, args),
                    Self::RePattern(p) => p.py_call_attr(self_id, vm, attr, args),
                    _ => Err(ExcType::attribute_error(
                        self.py_type(vm.heap),
                        attr.as_str(vm.interns),
                    )),
                }
            }

            fn py_getitem(&self, key: &Value, vm: &mut VM<'_, '_, impl ResourceTracker>) -> RunResult<Value> {
                match self {
                    Self::Str(s) => s.py_getitem(key, vm),
                    Self::Bytes(b) => b.py_getitem(key, vm),
                    Self::List(l) => l.py_getitem(key, vm),
                    Self::Tuple(t) => t.py_getitem(key, vm),
                    Self::NamedTuple(nt) => nt.py_getitem(key, vm),
                    Self::Dict(d) => d.py_getitem(key, vm),
                    Self::Range(r) => r.py_getitem(key, vm),
                    Self::ReMatch(m) => m.py_getitem(key, vm),
                    _ => Err(ExcType::type_error_not_sub(self.py_type(vm.heap))),
                }
            }

            fn py_setitem(
                &mut self,
                key: Value,
                value: Value,
                vm: &mut VM<'_, '_, impl ResourceTracker>,
            ) -> RunResult<()> {
                match self {
                    Self::Str(s) => s.py_setitem(key, value, vm),
                    Self::Bytes(b) => b.py_setitem(key, value, vm),
                    Self::List(l) => l.py_setitem(key, value, vm),
                    Self::Tuple(t) => t.py_setitem(key, value, vm),
                    Self::Dict(d) => d.py_setitem(key, value, vm),
                    _ => Err(ExcType::type_error_not_sub_assignment(self.py_type(vm.heap))),
                }
            }

            fn py_getattr(
                &self,
                attr: &EitherStr,
                vm: &mut VM<'_, '_, impl ResourceTracker>,
            ) -> RunResult<Option<CallResult>> {
                match self {
                    Self::Dataclass(dc) => dc.py_getattr(attr, vm),
                    Self::Module(m) => Ok(m.py_getattr(attr, vm.heap, vm.interns)),
                    Self::NamedTuple(nt) => nt.py_getattr(attr, vm),
                    Self::Slice(s) => s.py_getattr(attr, vm),
                    Self::Exception(exc) => exc.py_getattr(attr, vm.heap, vm.interns),
                    Self::Path(p) => p.py_getattr(attr, vm),
                    Self::ReMatch(m) => m.py_getattr(attr, vm),
                    Self::RePattern(p) => p.py_getattr(attr, vm),
                    // All other types don't support attribute access via py_getattr
                    _ => Ok(None),
                }
            }
        }
    };
}

impl_py_trait_dispatch!(HeapDataMut<'_>);
impl_py_trait_dispatch!(HeapData);

/// Shared dispatch macro for `HeapItem` methods on `HeapData` and `HeapDataMut`.
///
/// Dispatches `py_estimate_size` and `py_dec_ref_ids` to the inner type's
/// `HeapItem` implementation. For types without a dedicated `HeapItem` impl
/// (like `ExtFunction` wrapping `String`), the logic is inlined here.
macro_rules! impl_heap_item_dispatch {
    ($self_ty:ty) => {
        impl HeapItem for $self_ty {
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
                    Self::Closure(closure) => closure.py_dec_ref_ids(stack),
                    Self::FunctionDefaults(fd) => fd.py_dec_ref_ids(stack),
                    Self::Cell(cell) => cell.py_dec_ref_ids(stack),
                    Self::Dataclass(dc) => dc.py_dec_ref_ids(stack),
                    Self::Iter(iter) => iter.py_dec_ref_ids(stack),
                    Self::Module(m) => m.py_dec_ref_ids(stack),
                    Self::Coroutine(coro) => coro.py_dec_ref_ids(stack),
                    Self::GatherFuture(gather) => gather.py_dec_ref_ids(stack),
                    // Types with no nested heap references
                    _ => {}
                }
            }
        }
    };
}

impl_heap_item_dispatch!(HeapDataMut<'_>);
impl_heap_item_dispatch!(HeapData);
