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
    bytecode::VM,
    exception_private::{RunResult, SimpleException},
    heap::{Heap, HeapId},
    intern::{FunctionId, Interns},
    types::{
        AttrCallResult, Bytes, Dataclass, Dict, FrozenSet, List, LongInt, Module, MontyIter, NamedTuple, Path, PyTrait,
        Range, Set, Slice, Str, Tuple, Type,
    },
    value::{EitherStr, Value},
};

/// Mutable reference to `HeapData` inner values
#[derive(Debug)]
pub(crate) enum HeapDataMut<'a> {
    Str(&'a mut Str),
    Bytes(&'a mut Bytes),
    List(&'a mut List),
    Tuple(&'a mut Tuple),
    NamedTuple(&'a mut NamedTuple),
    Dict(&'a mut Dict),
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
            // Mutable types, exceptions, iterators, modules, and async types cannot be hashed
            // (Cell is handled specially in get_or_compute_hash)
            Self::List(_)
            | Self::Dict(_)
            | Self::Set(_)
            | Self::Cell(_)
            | Self::Exception(_)
            | Self::Iter(_)
            | Self::Module(_)
            | Self::Coroutine(_)
            | Self::GatherFuture(_) => Ok(None),
            // LongInt is immutable and hashable
            Self::LongInt(li) => Ok(Some(li.hash())),
        }
    }
}

/// Manual implementation of AbstractValue dispatch for HeapData.
///
/// This provides efficient dispatch without boxing overhead by matching on
/// the enum variant and delegating to the inner type's implementation.
impl PyTrait for HeapDataMut<'_> {
    fn py_type(&self, heap: &Heap<impl ResourceTracker>) -> Type {
        match self {
            Self::Str(s) => s.py_type(heap),
            Self::Bytes(b) => b.py_type(heap),
            Self::List(l) => l.py_type(heap),
            Self::Tuple(t) => t.py_type(heap),
            Self::NamedTuple(nt) => nt.py_type(heap),
            Self::Dict(d) => d.py_type(heap),
            Self::Set(s) => s.py_type(heap),
            Self::FrozenSet(fs) => fs.py_type(heap),
            Self::Closure(_) | Self::FunctionDefaults(_) => Type::Function,
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
                std::mem::size_of::<Coroutine>()
                    + coro.namespace.len() * std::mem::size_of::<Value>()
                    + coro.frame_cells.len() * std::mem::size_of::<HeapId>()
            }
            Self::GatherFuture(gather) => {
                std::mem::size_of::<GatherFuture>()
                    + gather.items.len() * std::mem::size_of::<crate::asyncio::GatherItem>()
                    + gather.results.len() * std::mem::size_of::<Option<Value>>()
                    + gather.pending_calls.len() * std::mem::size_of::<crate::asyncio::CallId>()
            }
            Self::Path(p) => p.py_estimate_size(),
        }
    }

    fn py_len(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Option<usize> {
        match self {
            Self::Str(s) => s.py_len(heap, interns),
            Self::Bytes(b) => b.py_len(heap, interns),
            Self::List(l) => l.py_len(heap, interns),
            Self::Tuple(t) => t.py_len(heap, interns),
            Self::NamedTuple(nt) => nt.py_len(heap, interns),
            Self::Dict(d) => d.py_len(heap, interns),
            Self::Set(s) => s.py_len(heap, interns),
            Self::FrozenSet(fs) => fs.py_len(heap, interns),
            Self::Range(r) => Some(r.len()),
            // Cells, Slices, Exceptions, Dataclasses, Iterators, LongInts, Modules, Paths, and async types don't have length
            Self::Cell(_)
            | Self::Closure(_)
            | Self::FunctionDefaults(_)
            | Self::Slice(_)
            | Self::Exception(_)
            | Self::Dataclass(_)
            | Self::Iter(_)
            | Self::LongInt(_)
            | Self::Module(_)
            | Self::Coroutine(_)
            | Self::GatherFuture(_)
            | Self::Path(_) => None,
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
                // Decrement ref count for frame cells
                stack.extend(coro.frame_cells.iter().copied());
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
            // Range, Slice, Exception, LongInt, and Path have no nested heap references
            Self::Range(_) | Self::Slice(_) | Self::Exception(_) | Self::LongInt(_) | Self::Path(_) => {}
        }
    }

    fn py_bool(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        match self {
            Self::Str(s) => s.py_bool(heap, interns),
            Self::Bytes(b) => b.py_bool(heap, interns),
            Self::List(l) => l.py_bool(heap, interns),
            Self::Tuple(t) => t.py_bool(heap, interns),
            Self::NamedTuple(nt) => nt.py_bool(heap, interns),
            Self::Dict(d) => d.py_bool(heap, interns),
            Self::Set(s) => s.py_bool(heap, interns),
            Self::FrozenSet(fs) => fs.py_bool(heap, interns),
            Self::Closure(_) | Self::FunctionDefaults(_) => true,
            Self::Cell(_) => true, // Cells are always truthy
            Self::Range(r) => r.py_bool(heap, interns),
            Self::Slice(s) => s.py_bool(heap, interns),
            Self::Exception(_) => true, // Exceptions are always truthy
            Self::Dataclass(dc) => dc.py_bool(heap, interns),
            Self::Iter(_) => true, // Iterators are always truthy
            Self::LongInt(li) => !li.is_zero(),
            Self::Module(_) => true,       // Modules are always truthy
            Self::Coroutine(_) => true,    // Coroutines are always truthy
            Self::GatherFuture(_) => true, // GatherFutures are always truthy
            Self::Path(p) => p.py_bool(heap, interns),
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
        other: Value,
        heap: &mut Heap<impl ResourceTracker>,
        self_id: Option<HeapId>,
        interns: &Interns,
    ) -> Result<bool, crate::resource::ResourceError> {
        match self {
            Self::Str(s) => s.py_iadd(other, heap, self_id, interns),
            Self::Bytes(b) => b.py_iadd(other, heap, self_id, interns),
            Self::List(l) => l.py_iadd(other, heap, self_id, interns),
            Self::Tuple(t) => t.py_iadd(other, heap, self_id, interns),
            Self::Dict(d) => d.py_iadd(other, heap, self_id, interns),
            _ => {
                // Drop other if it's a Ref (ensure proper refcounting for unsupported types)
                other.drop_with_heap(heap);
                Ok(false)
            }
        }
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        match self {
            Self::Str(s) => s.py_call_attr(heap, attr, args, interns),
            Self::Bytes(b) => b.py_call_attr(heap, attr, args, interns),
            Self::List(l) => l.py_call_attr(heap, attr, args, interns),
            Self::Tuple(t) => t.py_call_attr(heap, attr, args, interns),
            Self::Dict(d) => d.py_call_attr(heap, attr, args, interns),
            Self::Set(s) => s.py_call_attr(heap, attr, args, interns),
            Self::FrozenSet(fs) => fs.py_call_attr(heap, attr, args, interns),
            Self::Dataclass(dc) => dc.py_call_attr(heap, attr, args, interns),
            Self::Path(p) => p.py_call_attr(heap, attr, args, interns),
            _ => Err(ExcType::attribute_error(self.py_type(heap), attr.as_str(interns))),
        }
    }

    fn py_call_attr_raw(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<AttrCallResult> {
        match self {
            // List intercepts sort for key function support via PrintWriter
            Self::List(l) => l.py_call_attr_raw(self_id, vm, attr, args),
            // Dataclass detects public method calls and returns MethodCall
            Self::Dataclass(dc) => dc.py_call_attr_raw(self_id, vm, attr, args),
            // Path has special handling for OS calls (exists, read_text, etc.)
            Self::Path(p) => p.py_call_attr_raw(self_id, vm, attr, args),
            // Module has special handling for OS calls (os.getenv, etc.)
            Self::Module(m) => m.py_call_attr_raw(self_id, vm, attr, args),
            // All other types use the default implementation (wrap py_call_attr)
            _ => self
                .py_call_attr(vm.heap, attr, args, vm.interns)
                .map(AttrCallResult::Value),
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
    ) -> RunResult<Option<AttrCallResult>> {
        match self {
            Self::Dataclass(dc) => dc.py_getattr(attr, heap, interns),
            Self::Module(m) => Ok(m.py_getattr(attr, heap, interns)),
            Self::NamedTuple(nt) => nt.py_getattr(attr, heap, interns),
            Self::Slice(s) => s.py_getattr(attr, heap, interns),
            Self::Exception(exc) => exc.py_getattr(attr, heap, interns),
            Self::Path(p) => p.py_getattr(attr, heap, interns),
            // All other types don't support attribute access via py_getattr
            _ => Ok(None),
        }
    }
}
