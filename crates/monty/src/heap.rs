use std::{
    borrow::Cow,
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem::discriminant,
};

use ahash::AHashSet;
use num_integer::Integer;

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult, SimpleException},
    for_iterator::{ForIterator, IterState},
    intern::{FunctionId, Interns},
    resource::{ResourceError, ResourceTracker},
    types::{
        Bytes, Dataclass, Dict, FrozenSet, List, LongInt, Module, NamedTuple, PyTrait, Range, Set, Str, Tuple, Type,
        str::allocate_char,
    },
    value::{Attr, Value},
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

/// HeapData captures every runtime value that must live in the arena.
///
/// Each variant wraps a type that implements `AbstractValue`, providing
/// Python-compatible operations. The trait is manually implemented to dispatch
/// to the appropriate variant's implementation.
///
/// Note: The `Value` variant is special - it wraps boxed immediate values
/// that need heap identity (e.g., when `id()` is called on an int).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum HeapData {
    Str(Str),
    Bytes(Bytes),
    List(List),
    Tuple(Tuple),
    NamedTuple(NamedTuple),
    Dict(Dict),
    Set(Set),
    FrozenSet(FrozenSet),
    /// A closure: a function that captures variables from enclosing scopes.
    ///
    /// Contains a reference to the function definition, a vector of captured cell HeapIds,
    /// and evaluated default values (if any). When the closure is called, these cells are
    /// passed to the RunFrame for variable access. When the closure is dropped, we must
    /// decrement the ref count on each captured cell and each default value.
    Closure(FunctionId, Vec<HeapId>, Vec<Value>),
    /// A function with evaluated default parameter values (non-closure).
    ///
    /// Contains a reference to the function definition and the evaluated default values.
    /// When the function is called, defaults are cloned for missing optional parameters.
    /// When dropped, we must decrement the ref count on each default value.
    FunctionDefaults(FunctionId, Vec<Value>),
    /// A cell wrapping a single mutable value for closure support.
    ///
    /// Cells enable nonlocal variable access by providing a heap-allocated
    /// container that can be shared between a function and its nested functions.
    /// Both the outer function and inner function hold references to the same
    /// cell, allowing modifications to propagate across scope boundaries.
    Cell(Value),
    /// A range object (e.g., `range(10)` or `range(1, 10, 2)`).
    ///
    /// Stored on the heap to keep `Value` enum small (16 bytes). Range objects
    /// are immutable and hashable.
    Range(Range),
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
    /// An iterator for for-loop iteration.
    ///
    /// Created by the `GetIter` opcode, advanced by `ForIter`. Stores iteration
    /// state for lists, tuples, strings, ranges, dicts, and sets.
    Iterator(ForIterator),
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
    pub fn is_gc_tracked(&self) -> bool {
        matches!(
            self,
            Self::List(_)
                | Self::Tuple(_)
                | Self::NamedTuple(_)
                | Self::Dict(_)
                | Self::Set(_)
                | Self::FrozenSet(_)
                | Self::Closure(_, _, _)
                | Self::FunctionDefaults(_, _)
                | Self::Cell(_)
                | Self::Dataclass(_)
                | Self::Iterator(_)
                | Self::Module(_)
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
    pub fn has_refs(&self) -> bool {
        match self {
            Self::List(list) => list.contains_refs(),
            Self::Tuple(tuple) => tuple.contains_refs(),
            Self::NamedTuple(nt) => nt.contains_refs(),
            Self::Dict(dict) => dict.has_refs(),
            Self::Set(set) => set.has_refs(),
            Self::FrozenSet(fset) => fset.has_refs(),
            // Closures always have refs when they have captured cells (HeapIds)
            Self::Closure(_, cells, defaults) => {
                !cells.is_empty() || defaults.iter().any(|v| matches!(v, Value::Ref(_)))
            }
            Self::FunctionDefaults(_, defaults) => defaults.iter().any(|v| matches!(v, Value::Ref(_))),
            Self::Cell(value) => matches!(value, Value::Ref(_)),
            Self::Dataclass(dc) => dc.has_refs(),
            Self::Iterator(iter) => iter.has_refs(),
            Self::Module(m) => m.has_refs(),
            // Leaf types cannot have refs
            Self::Str(_) | Self::Bytes(_) | Self::Range(_) | Self::Exception(_) | Self::LongInt(_) => false,
        }
    }

    /// Computes hash for immutable heap types that can be used as dict keys.
    ///
    /// Returns Some(hash) for immutable types (Str, Bytes, Tuple of hashables).
    /// Returns None for mutable types (List, Dict) which cannot be dict keys.
    ///
    /// This is called lazily when the value is first used as a dict key,
    /// avoiding unnecessary hash computation for values that are never used as keys.
    fn compute_hash_if_immutable(&self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Option<u64> {
        match self {
            // Hash just the actual string or bytes content for consistency with Value::InternString/InternBytes
            // hence we don't include the discriminant
            Self::Str(s) => {
                let mut hasher = DefaultHasher::new();
                s.as_str().hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::Bytes(b) => {
                let mut hasher = DefaultHasher::new();
                b.as_slice().hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::FrozenSet(fs) => {
                // FrozenSet hash is XOR of element hashes (order-independent)
                fs.compute_hash(heap, interns)
            }
            Self::Tuple(t) => {
                let mut hasher = DefaultHasher::new();
                discriminant(self).hash(&mut hasher);
                // Tuple is hashable only if all elements are hashable
                for obj in t.as_vec() {
                    let h = obj.py_hash(heap, interns)?;
                    h.hash(&mut hasher);
                }
                Some(hasher.finish())
            }
            Self::NamedTuple(nt) => {
                let mut hasher = DefaultHasher::new();
                discriminant(self).hash(&mut hasher);
                // Hash only by elements (not type_name) to match equality semantics
                for obj in nt.as_vec() {
                    let h = obj.py_hash(heap, interns)?;
                    h.hash(&mut hasher);
                }
                Some(hasher.finish())
            }
            Self::Closure(f, _, _) | Self::FunctionDefaults(f, _) => {
                let mut hasher = DefaultHasher::new();
                discriminant(self).hash(&mut hasher);
                // TODO, this is NOT proper hashing, we should somehow hash the function properly
                f.hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::Range(range) => {
                let mut hasher = DefaultHasher::new();
                discriminant(self).hash(&mut hasher);
                range.start.hash(&mut hasher);
                range.stop.hash(&mut hasher);
                range.step.hash(&mut hasher);
                Some(hasher.finish())
            }
            // Dataclass hashability depends on the mutable flag
            Self::Dataclass(dc) => dc.compute_hash(heap, interns),
            // Mutable types, exceptions, iterators, and modules cannot be hashed
            // (Cell is handled specially in get_or_compute_hash)
            Self::List(_)
            | Self::Dict(_)
            | Self::Set(_)
            | Self::Cell(_)
            | Self::Exception(_)
            | Self::Iterator(_)
            | Self::Module(_) => None,
            // LongInt is immutable and hashable
            Self::LongInt(li) => Some(li.hash()),
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
            Self::Set(s) => s.py_type(heap),
            Self::FrozenSet(fs) => fs.py_type(heap),
            Self::Closure(_, _, _) | Self::FunctionDefaults(_, _) => Type::Function,
            Self::Cell(_) => Type::Cell,
            Self::Range(_) => Type::Range,
            Self::Exception(e) => e.py_type(),
            Self::Dataclass(dc) => dc.py_type(heap),
            Self::Iterator(_) => Type::Iterator,
            // LongInt is still `int` in Python - it's an implementation detail
            Self::LongInt(_) => Type::Int,
            Self::Module(_) => Type::Module,
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
            Self::Closure(_, _, _) | Self::FunctionDefaults(_, _) => 0,
            Self::Cell(v) => std::mem::size_of::<Value>() + v.py_estimate_size(),
            Self::Range(_) => std::mem::size_of::<Range>(),
            Self::Exception(e) => std::mem::size_of::<SimpleException>() + e.arg().map_or(0, String::len),
            Self::Dataclass(dc) => dc.py_estimate_size(),
            Self::Iterator(_) => std::mem::size_of::<ForIterator>(),
            Self::LongInt(li) => li.estimate_size(),
            Self::Module(m) => std::mem::size_of::<Module>() + m.attrs().py_estimate_size(),
        }
    }

    fn py_len(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Option<usize> {
        match self {
            Self::Str(s) => PyTrait::py_len(s, heap, interns),
            Self::Bytes(b) => PyTrait::py_len(b, heap, interns),
            Self::List(l) => PyTrait::py_len(l, heap, interns),
            Self::Tuple(t) => PyTrait::py_len(t, heap, interns),
            Self::NamedTuple(nt) => PyTrait::py_len(nt, heap, interns),
            Self::Dict(d) => PyTrait::py_len(d, heap, interns),
            Self::Set(s) => PyTrait::py_len(s, heap, interns),
            Self::FrozenSet(fs) => PyTrait::py_len(fs, heap, interns),
            Self::Range(r) => Some(r.len()),
            // Cells, Exceptions, Dataclasses, Iterators, LongInts, and Modules don't have length
            Self::Cell(_)
            | Self::Closure(_, _, _)
            | Self::FunctionDefaults(_, _)
            | Self::Exception(_)
            | Self::Dataclass(_)
            | Self::Iterator(_)
            | Self::LongInt(_)
            | Self::Module(_) => None,
        }
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        match (self, other) {
            (Self::Str(a), Self::Str(b)) => a.py_eq(b, heap, interns),
            (Self::Bytes(a), Self::Bytes(b)) => a.py_eq(b, heap, interns),
            (Self::List(a), Self::List(b)) => a.py_eq(b, heap, interns),
            (Self::Tuple(a), Self::Tuple(b)) => a.py_eq(b, heap, interns),
            (Self::NamedTuple(a), Self::NamedTuple(b)) => a.py_eq(b, heap, interns),
            // NamedTuple can compare with Tuple by elements (matching CPython behavior)
            (Self::NamedTuple(nt), Self::Tuple(t)) | (Self::Tuple(t), Self::NamedTuple(nt)) => {
                let nt_items = nt.as_vec();
                let t_items = t.as_vec();
                if nt_items.len() != t_items.len() {
                    return false;
                }
                nt_items
                    .iter()
                    .zip(t_items.iter())
                    .all(|(a, b)| a.py_eq(b, heap, interns))
            }
            (Self::Dict(a), Self::Dict(b)) => a.py_eq(b, heap, interns),
            (Self::Set(a), Self::Set(b)) => a.py_eq(b, heap, interns),
            (Self::FrozenSet(a), Self::FrozenSet(b)) => a.py_eq(b, heap, interns),
            (Self::Closure(a_id, a_cells, _), Self::Closure(b_id, b_cells, _)) => *a_id == *b_id && a_cells == b_cells,
            (Self::FunctionDefaults(a_id, _), Self::FunctionDefaults(b_id, _)) => *a_id == *b_id,
            (Self::Range(a), Self::Range(b)) => a.py_eq(b, heap, interns),
            (Self::Dataclass(a), Self::Dataclass(b)) => a.py_eq(b, heap, interns),
            // LongInt equality
            (Self::LongInt(a), Self::LongInt(b)) => a == b,
            // Cells, Exceptions, Iterators, and Modules compare by identity only (handled at Value level via HeapId comparison)
            (Self::Cell(_), Self::Cell(_))
            | (Self::Exception(_), Self::Exception(_))
            | (Self::Iterator(_), Self::Iterator(_))
            | (Self::Module(_), Self::Module(_)) => false,
            _ => false, // Different types are never equal
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
            Self::Closure(_, cells, defaults) => {
                // Decrement ref count for captured cells
                stack.extend(cells.iter().copied());
                // Decrement ref count for default values that are heap references
                for default in defaults.iter_mut() {
                    default.py_dec_ref_ids(stack);
                }
            }
            Self::FunctionDefaults(_, defaults) => {
                // Decrement ref count for default values that are heap references
                for default in defaults.iter_mut() {
                    default.py_dec_ref_ids(stack);
                }
            }
            Self::Cell(v) => v.py_dec_ref_ids(stack),
            Self::Dataclass(dc) => dc.py_dec_ref_ids(stack),
            Self::Iterator(iter) => iter.py_dec_ref_ids(stack),
            Self::Module(m) => m.py_dec_ref_ids(stack),
            // Range, Exception, and LongInt have no nested heap references
            Self::Range(_) | Self::Exception(_) | Self::LongInt(_) => {}
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
            Self::Closure(_, _, _) | Self::FunctionDefaults(_, _) => true,
            Self::Cell(_) => true, // Cells are always truthy
            Self::Range(r) => r.py_bool(heap, interns),
            Self::Exception(_) => true, // Exceptions are always truthy
            Self::Dataclass(dc) => dc.py_bool(heap, interns),
            Self::Iterator(_) => true, // Iterators are always truthy
            Self::LongInt(li) => !li.is_zero(),
            Self::Module(_) => true, // Modules are always truthy
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
            Self::Closure(f_id, _, _) | Self::FunctionDefaults(f_id, _) => {
                interns.get_function(*f_id).py_repr_fmt(f, interns, 0)
            }
            // Cell repr shows the contained value's type
            Self::Cell(v) => write!(f, "<cell: {} object>", v.py_type(heap)),
            Self::Range(r) => r.py_repr_fmt(f, heap, heap_ids, interns),
            Self::Exception(e) => e.py_repr_fmt(f),
            Self::Dataclass(dc) => dc.py_repr_fmt(f, heap, heap_ids, interns),
            Self::Iterator(_) => write!(f, "<iterator>"),
            Self::LongInt(li) => write!(f, "{li}"),
            Self::Module(m) => write!(f, "<module '{}'>", interns.get_str(m.name())),
        }
    }

    fn py_str(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Cow<'static, str> {
        match self {
            // Strings return their value directly without quotes
            Self::Str(s) => s.py_str(heap, interns),
            // LongInt returns its string representation
            Self::LongInt(li) => Cow::Owned(li.to_string()),
            // Exceptions return just the message (or empty string if no message)
            Self::Exception(e) => Cow::Owned(e.arg().cloned().unwrap_or_default()),
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
        attr: &Attr,
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
            _ => Err(ExcType::attribute_error(self.py_type(heap), attr.as_str(interns))),
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
            // LongInt is immutable and hashable
            // NamedTuple is immutable and hashable (like Tuple)
            HeapData::Str(_)
            | HeapData::Bytes(_)
            | HeapData::Tuple(_)
            | HeapData::NamedTuple(_)
            | HeapData::FrozenSet(_)
            | HeapData::Cell(_)
            | HeapData::Closure(_, _, _)
            | HeapData::FunctionDefaults(_, _)
            | HeapData::Range(_)
            | HeapData::LongInt(_) => Self::Unknown,
            // Dataclass hashability depends on the mutable flag
            HeapData::Dataclass(dc) => {
                if dc.is_frozen() {
                    Self::Unknown
                } else {
                    Self::Unhashable
                }
            }
            // Mutable containers, exceptions, iterators, and modules are unhashable
            HeapData::List(_)
            | HeapData::Dict(_)
            | HeapData::Set(_)
            | HeapData::Exception(_)
            | HeapData::Iterator(_)
            | HeapData::Module(_) => Self::Unhashable,
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
    refcount: usize,
    /// The payload data. Temporarily `None` while borrowed via `with_entry_mut`/`call_attr`.
    data: Option<HeapData>,
    /// Current hashing status / cached hash value
    hash_state: HashState,
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
}

impl<T: ResourceTracker + serde::Serialize> serde::Serialize for Heap<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Heap", 5)?;
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
        Self {
            entries: Vec::with_capacity(capacity),
            free_list: Vec::new(),
            tracker,
            may_have_cycles: false,
            allocations_since_gc: 0,
        }
    }

    /// Returns a reference to the resource tracker.
    pub fn tracker(&self) -> &T {
        &self.tracker
    }

    /// Returns a mutable reference to the resource tracker.
    pub fn tracker_mut(&mut self) -> &mut T {
        &mut self.tracker
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
            refcount: 1,
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

    /// Increments the reference count for an existing heap entry.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    pub fn inc_ref(&mut self, id: HeapId) {
        let value = self
            .entries
            .get_mut(id.index())
            .expect("Heap::inc_ref: slot missing")
            .as_mut()
            .expect("Heap::inc_ref: object already freed");
        value.refcount += 1;
    }

    /// Decrements the reference count and frees the value (plus children) once it hits zero.
    ///
    /// When an value is freed, its slot ID is added to the free list for reuse by
    /// future allocations. Uses recursion for child cleanup - avoiding repeated Vec
    /// allocations and benefiting from call stack locality.
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    pub fn dec_ref(&mut self, id: HeapId) {
        let slot = self.entries.get_mut(id.index()).expect("Heap::dec_ref: slot missing");
        let entry = slot.as_mut().expect("Heap::dec_ref: object already freed");
        if entry.refcount > 1 {
            entry.refcount -= 1;
        } else if let Some(value) = slot.take() {
            // refcount == 1, free the value and add slot to free list for reuse
            self.free_list.push(id);

            // Notify tracker of freed memory
            if let Some(ref data) = value.data {
                self.tracker.on_free(|| data.py_estimate_size());
            }

            // Collect child IDs and mark Values as Dereferenced (when ref-count-panic enabled)
            if let Some(mut data) = value.data {
                let mut child_ids = Vec::new();
                data.py_dec_ref_ids(&mut child_ids);
                drop(data);
                // Recursively decrement children
                for child_id in child_ids {
                    self.dec_ref(child_id);
                }
            }
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
    pub fn get_mut(&mut self, id: HeapId) -> &mut HeapData {
        self.entries
            .get_mut(id.index())
            .expect("Heap::get_mut: slot missing")
            .as_mut()
            .expect("Heap::get_mut: object already freed")
            .data
            .as_mut()
            .expect("Heap::get_mut: data currently borrowed")
    }

    /// Advances an iterator stored on the heap and returns the next value.
    ///
    /// This method uses a two-phase approach to avoid borrow conflicts:
    /// 1. Read iterator state (immutable borrow ends)
    /// 2. Based on state, get the value (may access other heap objects)
    /// 3. Update iterator index (mutable borrow)
    ///
    /// This is more efficient than `std::mem::replace` with a placeholder because
    /// it avoids creating and moving placeholder objects on every iteration.
    ///
    /// Returns `Ok(None)` when the iterator is exhausted.
    /// Returns `Err` for dict/set size changes or allocation failures.
    pub fn advance_iterator(&mut self, iter_id: HeapId, interns: &Interns) -> RunResult<Option<Value>> {
        // Phase 1: Get iterator state (immutable borrow ends after this block)
        let HeapData::Iterator(iter) = self.get(iter_id) else {
            panic!("advance_iterator: expected Iterator on heap");
        };
        let state = iter.iter_state();

        // Phase 2: Based on state, get the value and determine char_len for strings
        let (value, string_char_len) = match state {
            IterState::Exhausted => return Ok(None),
            IterState::Range(value) => (Value::Int(value), None),
            IterState::List { list_id, index } => {
                // Get item from list (immutable borrow)
                let HeapData::List(list) = self.get(list_id) else {
                    panic!("advance_iterator: expected List on heap");
                };
                // Check if list shrunk during iteration
                if index >= list.len() {
                    return Ok(None);
                }
                let item = list.as_vec()[index].copy_for_extend();

                // Inc refcount after borrow ends
                if let Value::Ref(id) = &item {
                    self.inc_ref(*id);
                }
                (item, None)
            }

            IterState::Tuple { tuple_id, index } => {
                let HeapData::Tuple(tuple) = self.get(tuple_id) else {
                    panic!("advance_iterator: expected Tuple on heap");
                };
                let item = tuple.as_vec()[index].copy_for_extend();
                if let Value::Ref(id) = &item {
                    self.inc_ref(*id);
                }
                (item, None)
            }

            IterState::NamedTuple { namedtuple_id, index } => {
                let HeapData::NamedTuple(namedtuple) = self.get(namedtuple_id) else {
                    panic!("advance_iterator: expected NamedTuple on heap");
                };
                let item = namedtuple.as_vec()[index].copy_for_extend();
                if let Value::Ref(id) = &item {
                    self.inc_ref(*id);
                }
                (item, None)
            }

            IterState::DictKeys {
                dict_id,
                index,
                expected_len,
            } => {
                let HeapData::Dict(dict) = self.get(dict_id) else {
                    panic!("advance_iterator: expected Dict on heap");
                };
                // Check for dict mutation
                if dict.len() != expected_len {
                    return Err(ExcType::runtime_error_dict_changed_size());
                }
                let key = dict.key_at(index).expect("index should be valid").copy_for_extend();
                if let Value::Ref(id) = &key {
                    self.inc_ref(*id);
                }
                (key, None)
            }

            IterState::IterStr { char, char_len } => {
                let value = allocate_char(char, self)?;
                (value, Some(char_len))
            }

            IterState::HeapBytes { bytes_id, index } => {
                let HeapData::Bytes(bytes) = self.get(bytes_id) else {
                    panic!("advance_iterator: expected Bytes on heap");
                };
                let byte_val = i64::from(bytes.as_slice()[index]);
                (Value::Int(byte_val), None)
            }

            IterState::InternBytes { bytes_id, index } => {
                let bytes = interns.get_bytes(bytes_id);
                (Value::Int(i64::from(bytes[index])), None)
            }

            IterState::Set {
                set_id,
                index,
                expected_len,
            } => {
                let HeapData::Set(set) = self.get(set_id) else {
                    panic!("advance_iterator: expected Set on heap");
                };
                // Check for set mutation
                if set.len() != expected_len {
                    return Err(ExcType::runtime_error_set_changed_size());
                }
                let item = set
                    .storage()
                    .value_at(index)
                    .expect("index should be valid")
                    .copy_for_extend();
                if let Value::Ref(id) = &item {
                    self.inc_ref(*id);
                }
                (item, None)
            }

            IterState::FrozenSet { frozenset_id, index } => {
                let HeapData::FrozenSet(frozenset) = self.get(frozenset_id) else {
                    panic!("advance_iterator: expected FrozenSet on heap");
                };
                let item = frozenset
                    .storage()
                    .value_at(index)
                    .expect("index should be valid")
                    .copy_for_extend();
                if let Value::Ref(id) = &item {
                    self.inc_ref(*id);
                }
                (item, None)
            }
        };

        // Phase 3: Advance the iterator
        let HeapData::Iterator(iter) = self.get_mut(iter_id) else {
            panic!("advance_iterator: expected Iterator on heap");
        };
        iter.advance(string_char_len);

        Ok(Some(value))
    }

    /// Returns or computes the hash for the heap entry at the given ID.
    ///
    /// Hashes are computed lazily on first use and then cached. Returns
    /// Some(hash) for immutable types (Str, Bytes, hashable Tuple), None
    /// for mutable types (List, Dict).
    ///
    /// # Panics
    /// Panics if the value ID is invalid or the value has already been freed.
    pub fn get_or_compute_hash(&mut self, id: HeapId, interns: &Interns) -> Option<u64> {
        let entry = self
            .entries
            .get_mut(id.index())
            .expect("Heap::get_or_compute_hash: slot missing")
            .as_mut()
            .expect("Heap::get_or_compute_hash: object already freed");

        match entry.hash_state {
            HashState::Unhashable => return None,
            HashState::Cached(hash) => return Some(hash),
            HashState::Unknown => {}
        }

        // Handle Cell specially - uses identity-based hashing (like Python cell objects)
        if let Some(HeapData::Cell(_)) = &entry.data {
            let mut hasher = DefaultHasher::new();
            id.hash(&mut hasher);
            let hash = hasher.finish();
            entry.hash_state = HashState::Cached(hash);
            return Some(hash);
        }

        // Compute hash lazily - need to temporarily take data to avoid borrow conflict
        let data = entry.data.take().expect("Heap::get_or_compute_hash: data borrowed");
        let hash = data.compute_hash_if_immutable(self, interns);

        // Restore data and cache the hash if computed
        let entry = self
            .entries
            .get_mut(id.index())
            .expect("Heap::get_or_compute_hash: slot missing after compute")
            .as_mut()
            .expect("Heap::get_or_compute_hash: object freed during compute");
        entry.data = Some(data);
        entry.hash_state = match hash {
            Some(value) => HashState::Cached(value),
            None => HashState::Unhashable,
        };
        hash
    }

    /// Calls an attribute on the heap entry at `id` while temporarily taking ownership
    /// of its payload so we can borrow the heap again inside the call. This avoids the
    /// borrow checker conflict that arises when attribute implementations also need
    /// mutable access to the heap (e.g. for refcounting).
    pub fn call_attr(&mut self, id: HeapId, attr: &Attr, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        // Take data out in a block so the borrow of self.entries ends
        let mut data = take_data!(self, id, "call_attr");

        let result = data.py_call_attr(self, attr, args, interns);

        // Restore data
        let entry = self
            .entries
            .get_mut(id.index())
            .expect("Heap::call_attr: slot missing")
            .as_mut()
            .expect("Heap::call_attr: object already freed");
        entry.data = Some(data);
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
        F: FnOnce(&mut Self, &mut HeapData) -> R,
    {
        // Take data out in a block so the borrow of self.entries ends
        let mut data = take_data!(self, id, "with_entry_mut");

        let result = f(self, &mut data);

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
    }

    /// Returns the number of live (non-freed) values on the heap.
    ///
    /// This is primarily used for testing to verify that all heap entries
    /// are accounted for in reference count tests.
    #[must_use]
    #[cfg(feature = "ref-count-return")]
    pub fn entry_count(&self) -> usize {
        self.entries.iter().filter(|o| o.is_some()).count()
    }

    /// Gets the value inside a cell, cloning it with proper refcount handling.
    ///
    /// Uses `clone_with_heap` to properly handle all value types including closures,
    /// which need their captured cell refcounts incremented.
    ///
    /// # Panics
    /// Panics if the ID is invalid, the value has been freed, or the entry is not a Cell.
    pub fn get_cell_value(&mut self, id: HeapId) -> Value {
        // Take the data out to avoid borrow conflicts when cloning
        let data = take_data!(self, id, "get_cell_value");

        let result = match &data {
            HeapData::Cell(v) => v.clone_with_heap(self),
            _ => panic!("Heap::get_cell_value: entry is not a Cell"),
        };

        // Restore data before returning
        restore_data!(self, id, data, "get_cell_value");

        result
    }

    /// Sets the value inside a cell, properly dropping the old value.
    ///
    /// # Panics
    /// Panics if the ID is invalid, the value has been freed, or the entry is not a Cell.
    pub fn set_cell_value(&mut self, id: HeapId, value: Value) {
        // Take the data out to avoid borrow conflicts
        let mut data = take_data!(self, id, "set_cell_value");

        match &mut data {
            HeapData::Cell(old_value) => {
                // Swap in the new value
                let old = std::mem::replace(old_value, value);
                // Restore data first, then drop old value
                restore_data!(self, id, data, "set_cell_value");
                old.drop_with_heap(self);
            }
            _ => panic!("Heap::set_cell_value: entry is not a Cell"),
        }
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
        // Take the source data temporarily
        let source_data = take_data!(self, source_id, "iadd_extend_list");

        if let HeapData::List(list) = &source_data {
            // Copy items and track which refs need incrementing
            let items: Vec<Value> = list.as_vec().iter().map(Value::copy_for_extend).collect();
            let ref_ids: Vec<HeapId> = items.iter().filter_map(Value::ref_id).collect();

            // Restore source data before mutating heap (inc_ref needs it)
            restore_data!(self, source_id, source_data, "iadd_extend_list");

            // Now increment refcounts
            for id in ref_ids {
                self.inc_ref(id);
            }

            // Extend destination
            dest.extend(items);
            true
        } else {
            // Not a list, restore and return false
            restore_data!(self, source_id, source_data, "iadd_extend_list");
            false
        }
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
        // Take the data out to avoid borrow conflicts
        let data = take_data!(self, id, "mult_sequence");

        match &data {
            HeapData::Str(s) => {
                let repeated = s.as_str().repeat(count);
                restore_data!(self, id, data, "mult_sequence");
                Ok(Some(Value::Ref(self.allocate(HeapData::Str(repeated.into()))?)))
            }
            HeapData::Bytes(b) => {
                let repeated = b.as_slice().repeat(count);
                restore_data!(self, id, data, "mult_sequence");
                Ok(Some(Value::Ref(self.allocate(HeapData::Bytes(repeated.into()))?)))
            }
            HeapData::List(list) => {
                if count == 0 {
                    restore_data!(self, id, data, "mult_sequence");
                    Ok(Some(Value::Ref(self.allocate(HeapData::List(List::new(Vec::new())))?)))
                } else {
                    // Copy items and track which refs need incrementing
                    let items: Vec<Value> = list.as_vec().iter().map(Value::copy_for_extend).collect();
                    let ref_ids: Vec<HeapId> = items.iter().filter_map(Value::ref_id).collect();
                    let original_len = items.len();

                    // Restore data before heap operations
                    restore_data!(self, id, data, "mult_sequence");

                    // Now increment refcounts for each copy we'll make
                    // We need (count) copies of each ref
                    for ref_id in &ref_ids {
                        for _ in 0..count {
                            self.inc_ref(*ref_id);
                        }
                    }

                    // Build the repeated list with overflow check
                    let capacity = original_len
                        .checked_mul(count)
                        .ok_or_else(ExcType::overflow_repeat_count)?;
                    let mut result = Vec::with_capacity(capacity);
                    for _ in 0..count {
                        for item in &items {
                            result.push(item.copy_for_extend());
                        }
                    }

                    // Manually forget the items vec to avoid Drop panic
                    // The values have been copied to result with proper refcounts
                    std::mem::forget(items);

                    Ok(Some(Value::Ref(self.allocate(HeapData::List(List::new(result)))?)))
                }
            }
            HeapData::Tuple(tuple) => {
                if count == 0 {
                    restore_data!(self, id, data, "mult_sequence");
                    Ok(Some(Value::Ref(
                        self.allocate(HeapData::Tuple(Tuple::new(Vec::new())))?,
                    )))
                } else {
                    // Copy items and track which refs need incrementing
                    let items: Vec<Value> = tuple.as_vec().iter().map(Value::copy_for_extend).collect();
                    let ref_ids: Vec<HeapId> = items.iter().filter_map(Value::ref_id).collect();
                    let original_len = items.len();

                    // Restore data before heap operations
                    restore_data!(self, id, data, "mult_sequence");

                    // Now increment refcounts for each copy we'll make
                    // We need (count) copies of each ref
                    for ref_id in &ref_ids {
                        for _ in 0..count {
                            self.inc_ref(*ref_id);
                        }
                    }

                    // Build the repeated tuple with overflow check
                    let capacity = original_len
                        .checked_mul(count)
                        .ok_or_else(ExcType::overflow_repeat_count)?;
                    let mut result = Vec::with_capacity(capacity);
                    for _ in 0..count {
                        for item in &items {
                            result.push(item.copy_for_extend());
                        }
                    }

                    // Manually forget the items vec to avoid Drop panic
                    std::mem::forget(items);

                    Ok(Some(Value::Ref(self.allocate(HeapData::Tuple(Tuple::new(result)))?)))
                }
            }
            _ => {
                // Dicts, Cells, Callables, Functions and Closures don't support multiplication
                restore_data!(self, id, data, "mult_sequence");
                Ok(None)
            }
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
    /// (typically from Namespaces). It marks all reachable objects starting
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

/// Collects child HeapIds from a HeapData value for GC traversal.
fn collect_child_ids(data: &HeapData, work_list: &mut Vec<HeapId>) {
    match data {
        // Leaf types with no heap references
        HeapData::Str(_) | HeapData::Bytes(_) | HeapData::Range(_) | HeapData::Exception(_) | HeapData::LongInt(_) => {}
        HeapData::List(list) => {
            // Skip iteration if no refs - major GC optimization for lists of primitives
            if !list.contains_refs() {
                return;
            }
            for value in list.as_vec() {
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
            for value in tuple.as_vec() {
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
        HeapData::Closure(_, cells, defaults) => {
            // Add captured cells to work list
            for cell_id in cells {
                work_list.push(*cell_id);
            }
            // Add default values that are heap references
            for default in defaults {
                if let Value::Ref(id) = default {
                    work_list.push(*id);
                }
            }
        }
        HeapData::FunctionDefaults(_, defaults) => {
            // Add default values that are heap references
            for default in defaults {
                if let Value::Ref(id) = default {
                    work_list.push(*id);
                }
            }
        }
        HeapData::Cell(value) => {
            // Cell can contain a reference to another heap value
            if let Value::Ref(id) = value {
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
        HeapData::Iterator(iter) => {
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
