use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem::discriminant,
};

use ahash::AHashSet;

use crate::{
    builtins::Builtins,
    exception_private::{exc_err_fmt, ExcType, RunError, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{BytesId, ExtFunctionId, FunctionId, Interns, StringId},
    resource::ResourceTracker,
    types::{bytes::bytes_repr_fmt, str::string_repr_fmt, PyTrait, Type},
};

/// Bitwise operation type for `py_bitwise`.
#[derive(Debug, Clone, Copy)]
pub enum BitwiseOp {
    And,
    Or,
    Xor,
    LShift,
    RShift,
}

impl BitwiseOp {
    /// Returns the operator symbol for error messages.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::And => "&",
            Self::Or => "|",
            Self::Xor => "^",
            Self::LShift => "<<",
            Self::RShift => ">>",
        }
    }
}

/// Primary value type representing Python objects at runtime.
///
/// This enum uses a hybrid design: small immediate values (Int, Bool, None) are stored
/// inline, while heap-allocated values (List, Str, Dict, etc.) are stored in the arena
/// and referenced via `Ref(HeapId)`.
///
/// NOTE: `Clone` is intentionally NOT derived. Use `clone_with_heap()` for heap values
/// or `clone_immediate()` for immediate values only. Direct cloning via `.clone()` would
/// bypass reference counting and cause memory leaks.
///
/// NOTE: it's important to keep this size small to minimize memory overhead!
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum Value {
    // Immediate values (stored inline, no heap allocation)
    Undefined,
    Ellipsis,
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// An interned string literal. The StringId references the string in the Interns table.
    /// To get the actual string content, use `interns.get(string_id)`.
    InternString(StringId),
    /// An interned bytes literal. The BytesId references the bytes in the Interns table.
    /// To get the actual bytes content, use `interns.get_bytes(bytes_id)`.
    InternBytes(BytesId),
    /// A builtin function or exception type
    Builtin(Builtins),
    /// A function defined in the module (not a closure, doesn't capture any variables)
    DefFunction(FunctionId),
    /// Reference to an external function defined on the host
    ExtFunction(ExtFunctionId),

    // Heap-allocated values (stored in arena)
    Ref(HeapId),

    /// Sentinel value indicating this Value was properly cleaned up via `drop_with_heap`.
    /// Only exists when `ref-count-panic` feature is enabled. Used to verify reference counting
    /// correctness - if a `Ref` variant is dropped without calling `drop_with_heap`, the
    /// Drop impl will panic.
    #[cfg(feature = "ref-count-panic")]
    Dereferenced,
}

/// Drop implementation that panics if a `Ref` variant is dropped without calling `drop_with_heap`.
/// This helps catch reference counting bugs during development/testing.
/// Only enabled when the `ref-count-panic` feature is active.
#[cfg(feature = "ref-count-panic")]
impl Drop for Value {
    fn drop(&mut self) {
        if let Self::Ref(id) = self {
            panic!("Value::Ref({id:?}) dropped without calling drop_with_heap() - this is a reference counting bug");
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl PyTrait for Value {
    fn py_type(&self, heap: &Heap<impl ResourceTracker>) -> Type {
        match self {
            Self::Undefined => panic!("Cannot get type of undefined value"),
            Self::Ellipsis => Type::Ellipsis,
            Self::None => Type::NoneType,
            Self::Bool(_) => Type::Bool,
            Self::Int(_) => Type::Int,
            Self::Float(_) => Type::Float,
            Self::InternString(_) => Type::Str,
            Self::InternBytes(_) => Type::Bytes,
            Self::Builtin(c) => c.py_type(),
            Self::DefFunction(_) | Self::ExtFunction(_) => Type::Function,
            Self::Ref(id) => heap.get(*id).py_type(heap),
            #[cfg(feature = "ref-count-panic")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    /// Returns 0 for Value since immediate values are stack-allocated.
    ///
    /// Heap-allocated values (Ref variants) have their size tracked when
    /// the HeapData is allocated, not here.
    fn py_estimate_size(&self) -> usize {
        // Value is stack-allocated; heap data is sized separately when allocated
        0
    }

    fn py_len(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Option<usize> {
        match self {
            // Count Unicode characters, not bytes, to match Python semantics
            Self::InternString(string_id) => Some(interns.get_str(*string_id).chars().count()),
            Self::InternBytes(bytes_id) => Some(interns.get_bytes(*bytes_id).len()),
            Self::Ref(id) => heap.get(*id).py_len(heap, interns),
            _ => None,
        }
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        match (self, other) {
            (Self::Undefined, _) => false,
            (_, Self::Undefined) => false,
            (Self::Int(v1), Self::Int(v2)) => v1 == v2,
            (Self::Bool(v1), Self::Bool(v2)) => v1 == v2,
            (Self::Bool(v1), Self::Int(v2)) => i64::from(*v1) == *v2,
            (Self::Int(v1), Self::Bool(v2)) => *v1 == i64::from(*v2),
            (Self::Float(v1), Self::Float(v2)) => v1 == v2,
            (Self::Int(v1), Self::Float(v2)) => (*v1 as f64) == *v2,
            (Self::Float(v1), Self::Int(v2)) => *v1 == (*v2 as f64),
            (Self::Bool(v1), Self::Float(v2)) => (i64::from(*v1) as f64) == *v2,
            (Self::Float(v1), Self::Bool(v2)) => *v1 == (i64::from(*v2) as f64),
            (Self::None, Self::None) => true,

            // For interned interns, compare by StringId first (fast path for same interned string)
            (Self::InternString(s1), Self::InternString(s2)) => s1 == s2,
            // for strings we need to account for the fact they might be either interned or not
            (Self::InternString(string_id), Self::Ref(id2)) => {
                if let HeapData::Str(s2) = heap.get(*id2) {
                    interns.get_str(*string_id) == s2.as_str()
                } else {
                    false
                }
            }
            (Self::Ref(id1), Self::InternString(string_id)) => {
                if let HeapData::Str(s1) = heap.get(*id1) {
                    s1.as_str() == interns.get_str(*string_id)
                } else {
                    false
                }
            }

            // For interned bytes, compare by content (bytes are not deduplicated unlike interns)
            (Self::InternBytes(b1), Self::InternBytes(b2)) => {
                // Fast path: same BytesId means same content
                b1 == b2 || interns.get_bytes(*b1) == interns.get_bytes(*b2)
            }
            // same for bytes
            (Self::InternBytes(bytes_id), Self::Ref(id2)) => {
                if let HeapData::Bytes(b2) = heap.get(*id2) {
                    interns.get_bytes(*bytes_id) == b2.as_slice()
                } else {
                    false
                }
            }
            (Self::Ref(id1), Self::InternBytes(bytes_id)) => {
                if let HeapData::Bytes(b1) = heap.get(*id1) {
                    b1.as_slice() == interns.get_bytes(*bytes_id)
                } else {
                    false
                }
            }

            (Self::Ref(id1), Self::Ref(id2)) => {
                if *id1 == *id2 {
                    return true;
                }
                // Need to use with_two for proper borrow management
                heap.with_two(*id1, *id2, |heap, left, right| left.py_eq(right, heap, interns))
            }

            // Builtins equality - just check the enums are equal
            (Self::Builtin(b1), Self::Builtin(b2)) => b1 == b2,
            (Self::DefFunction(f1), Self::DefFunction(f2)) => f1 == f2,

            _ => false,
        }
    }

    fn py_cmp(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Option<Ordering> {
        match (self, other) {
            (Self::Int(s), Self::Int(o)) => s.partial_cmp(o),
            (Self::Float(s), Self::Float(o)) => s.partial_cmp(o),
            (Self::Int(s), Self::Float(o)) => (*s as f64).partial_cmp(o),
            (Self::Float(s), Self::Int(o)) => s.partial_cmp(&(*o as f64)),
            (Self::Bool(s), _) => Self::Int(i64::from(*s)).py_cmp(other, heap, interns),
            (_, Self::Bool(s)) => self.py_cmp(&Self::Int(i64::from(*s)), heap, interns),
            (Self::InternString(s1), Self::InternString(s2)) => interns.get_str(*s1).partial_cmp(interns.get_str(*s2)),
            (Self::InternBytes(b1), Self::InternBytes(b2)) => {
                interns.get_bytes(*b1).partial_cmp(interns.get_bytes(*b2))
            }
            // Ref comparison requires heap context, not supported in PartialOrd
            (Self::Ref(_), Self::Ref(_)) => None,
            _ => None,
        }
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Self::Ref(id) = self {
            stack.push(*id);
            // Mark as Dereferenced to prevent Drop panic
            #[cfg(feature = "ref-count-panic")]
            self.dec_ref_forget();
        }
    }

    fn py_bool(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        match self {
            Self::Undefined => false,
            Self::Ellipsis => true,
            Self::None => false,
            Self::Bool(b) => *b,
            Self::Int(v) => *v != 0,
            Self::Float(f) => *f != 0.0,
            Self::Builtin(_) => true,                            // Builtins are always truthy
            Self::DefFunction(_) | Self::ExtFunction(_) => true, // same
            Self::InternString(string_id) => !interns.get_str(*string_id).is_empty(),
            Self::InternBytes(bytes_id) => !interns.get_bytes(*bytes_id).is_empty(),
            Self::Ref(id) => heap.get(*id).py_bool(heap, interns),
            #[cfg(feature = "ref-count-panic")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
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
            Self::Undefined => f.write_str("Undefined"),
            Self::Ellipsis => f.write_str("Ellipsis"),
            Self::None => f.write_str("None"),
            Self::Bool(true) => f.write_str("True"),
            Self::Bool(false) => f.write_str("False"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => {
                let s = v.to_string();
                if s.contains('.') {
                    f.write_str(&s)
                } else {
                    write!(f, "{s}.0")
                }
            }
            Self::Builtin(b) => b.py_repr_fmt(f),
            Self::DefFunction(f_id) => interns.get_function(*f_id).py_repr_fmt(f, interns, 0),
            Self::ExtFunction(f_id) => {
                write!(f, "<function '{}' external>", interns.get_external_function_name(*f_id))
            }
            Self::InternString(string_id) => string_repr_fmt(interns.get_str(*string_id), f),
            Self::InternBytes(bytes_id) => bytes_repr_fmt(interns.get_bytes(*bytes_id), f),
            Self::Ref(id) => {
                if heap_ids.contains(id) {
                    // Cycle detected - write type-specific placeholder following Python semantics
                    match heap.get(*id) {
                        HeapData::List(_) => f.write_str("[...]"),
                        HeapData::Tuple(_) => f.write_str("(...)"),
                        HeapData::Dict(_) => f.write_str("{...}"),
                        // Other types don't typically have cycles, but handle gracefully
                        _ => f.write_str("..."),
                    }
                } else {
                    heap_ids.insert(*id);
                    let result = heap.get(*id).py_repr_fmt(f, heap, heap_ids, interns);
                    heap_ids.remove(id);
                    result
                }
            }
            #[cfg(feature = "ref-count-panic")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    fn py_str(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Cow<'static, str> {
        match self {
            Self::InternString(string_id) => interns.get_str(*string_id).to_owned().into(),
            Self::Ref(id) => heap.get(*id).py_str(heap, interns),
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
            (Self::Int(v1), Self::Int(v2)) => Ok(Some(Self::Int(v1 + v2))),
            (Self::Float(v1), Self::Float(v2)) => Ok(Some(Self::Float(v1 + v2))),
            (Self::Ref(id1), Self::Ref(id2)) => {
                heap.with_two(*id1, *id2, |heap, left, right| left.py_add(right, heap, interns))
            }
            (Self::InternString(s1), Self::InternString(s2)) => {
                let concat = format!("{}{}", interns.get_str(*s1), interns.get_str(*s2));
                Ok(Some(Self::Ref(heap.allocate(HeapData::Str(concat.into()))?)))
            }
            // for strings we need to account for the fact they might be either interned or not
            (Self::InternString(string_id), Self::Ref(id2)) => {
                if let HeapData::Str(s2) = heap.get(*id2) {
                    let concat = format!("{}{}", interns.get_str(*string_id), s2.as_str());
                    Ok(Some(Self::Ref(heap.allocate(HeapData::Str(concat.into()))?)))
                } else {
                    Ok(None)
                }
            }
            (Self::Ref(id1), Self::InternString(string_id)) => {
                if let HeapData::Str(s1) = heap.get(*id1) {
                    let concat = format!("{}{}", s1.as_str(), interns.get_str(*string_id));
                    Ok(Some(Self::Ref(heap.allocate(HeapData::Str(concat.into()))?)))
                } else {
                    Ok(None)
                }
            }
            // same for bytes
            (Self::InternBytes(b1), Self::InternBytes(b2)) => {
                let bytes1 = interns.get_bytes(*b1);
                let bytes2 = interns.get_bytes(*b2);
                let mut b = Vec::with_capacity(bytes1.len() + bytes2.len());
                b.extend_from_slice(bytes1);
                b.extend_from_slice(bytes2);
                Ok(Some(Self::Ref(heap.allocate(HeapData::Bytes(b.into()))?)))
            }
            (Self::InternBytes(bytes_id), Self::Ref(id2)) => {
                if let HeapData::Bytes(b2) = heap.get(*id2) {
                    let bytes1 = interns.get_bytes(*bytes_id);
                    let mut b = Vec::with_capacity(bytes1.len() + b2.len());
                    b.extend_from_slice(bytes1);
                    b.extend_from_slice(b2);
                    Ok(Some(Self::Ref(heap.allocate(HeapData::Bytes(b.into()))?)))
                } else {
                    Ok(None)
                }
            }
            (Self::Ref(id1), Self::InternBytes(bytes_id)) => {
                if let HeapData::Bytes(b1) = heap.get(*id1) {
                    let bytes2 = interns.get_bytes(*bytes_id);
                    let mut b = Vec::with_capacity(b1.len() + bytes2.len());
                    b.extend_from_slice(b1);
                    b.extend_from_slice(bytes2);
                    Ok(Some(Self::Ref(heap.allocate(HeapData::Bytes(b.into()))?)))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn py_sub(
        &self,
        other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<Option<Self>, crate::resource::ResourceError> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Ok(Some(Self::Int(v1 - v2))),
            _ => Ok(None),
        }
    }

    fn py_mod(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Self::Int(v1 % v2)),
            (Self::Float(v1), Self::Float(v2)) => Some(Self::Float(v1 % v2)),
            (Self::Float(v1), Self::Int(v2)) => Some(Self::Float(v1 % (*v2 as f64))),
            (Self::Int(v1), Self::Float(v2)) => Some(Self::Float((*v1 as f64) % v2)),
            _ => None,
        }
    }

    fn py_mod_eq(&self, other: &Self, right_value: i64) -> Option<bool> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(v1 % v2 == right_value),
            (Self::Float(v1), Self::Float(v2)) => Some(v1 % v2 == right_value as f64),
            (Self::Float(v1), Self::Int(v2)) => Some(v1 % (*v2 as f64) == right_value as f64),
            (Self::Int(v1), Self::Float(v2)) => Some((*v1 as f64) % v2 == right_value as f64),
            _ => None,
        }
    }

    fn py_iadd(
        &mut self,
        other: Self,
        heap: &mut Heap<impl ResourceTracker>,
        _self_id: Option<HeapId>,
        interns: &Interns,
    ) -> Result<bool, crate::resource::ResourceError> {
        match (&self, &other) {
            (Self::Int(v1), Self::Int(v2)) => {
                *self = Self::Int(*v1 + v2);
                Ok(true)
            }
            (Self::Float(v1), Self::Float(v2)) => {
                *self = Self::Float(*v1 + *v2);
                Ok(true)
            }
            (Self::InternString(s1), Self::InternString(s2)) => {
                let concat = format!("{}{}", interns.get_str(*s1), interns.get_str(*s2));
                *self = Self::Ref(heap.allocate(HeapData::Str(concat.into()))?);
                Ok(true)
            }
            (Self::InternString(string_id), Self::Ref(id2)) => {
                let result = if let HeapData::Str(s2) = heap.get(*id2) {
                    let concat = format!("{}{}", interns.get_str(*string_id), s2.as_str());
                    *self = Self::Ref(heap.allocate(HeapData::Str(concat.into()))?);
                    true
                } else {
                    false
                };
                // Drop the other value - we've consumed it
                other.drop_with_heap(heap);
                Ok(result)
            }
            (Self::Ref(id1), Self::InternString(string_id)) => {
                if let HeapData::Str(s1) = heap.get_mut(*id1) {
                    s1.as_string_mut().push_str(interns.get_str(*string_id));
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            // same for bytes
            (Self::InternBytes(b1), Self::InternBytes(b2)) => {
                let bytes1 = interns.get_bytes(*b1);
                let bytes2 = interns.get_bytes(*b2);
                let mut b = Vec::with_capacity(bytes1.len() + bytes2.len());
                b.extend_from_slice(bytes1);
                b.extend_from_slice(bytes2);
                *self = Self::Ref(heap.allocate(HeapData::Bytes(b.into()))?);
                Ok(true)
            }
            (Self::InternBytes(bytes_id), Self::Ref(id2)) => {
                let result = if let HeapData::Bytes(b2) = heap.get(*id2) {
                    let bytes1 = interns.get_bytes(*bytes_id);
                    let mut b = Vec::with_capacity(bytes1.len() + b2.len());
                    b.extend_from_slice(bytes1);
                    b.extend_from_slice(b2);
                    *self = Self::Ref(heap.allocate(HeapData::Bytes(b.into()))?);
                    true
                } else {
                    false
                };
                // Drop the other value - we've consumed it
                other.drop_with_heap(heap);
                Ok(result)
            }
            (Self::Ref(id1), Self::InternBytes(bytes_id)) => {
                if let HeapData::Bytes(b1) = heap.get_mut(*id1) {
                    b1.as_vec_mut().extend_from_slice(interns.get_bytes(*bytes_id));
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            (Self::Ref(id), Self::Ref(_)) => {
                heap.with_entry_mut(*id, |heap, data| data.py_iadd(other, heap, Some(*id), interns))
            }
            _ => {
                // Drop other if it's a Ref (ensure proper refcounting for unsupported type combinations)
                other.drop_with_heap(heap);
                Ok(false)
            }
        }
    }

    fn py_mult(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<Value>> {
        match (self, other) {
            // Numeric multiplication
            (Self::Int(a), Self::Int(b)) => {
                // Use checked_mul to handle overflow, fall back to float
                match a.checked_mul(*b) {
                    Some(result) => Ok(Some(Self::Int(result))),
                    None => Ok(Some(Self::Float(*a as f64 * *b as f64))),
                }
            }
            (Self::Float(a), Self::Float(b)) => Ok(Some(Self::Float(a * b))),
            (Self::Int(a), Self::Float(b)) => Ok(Some(Self::Float(*a as f64 * b))),
            (Self::Float(a), Self::Int(b)) => Ok(Some(Self::Float(a * *b as f64))),

            // Bool numeric multiplication (True=1, False=0)
            (Self::Bool(a), Self::Int(b)) => {
                let a_int = i64::from(*a);
                Ok(Some(Self::Int(a_int * b)))
            }
            (Self::Int(a), Self::Bool(b)) => {
                let b_int = i64::from(*b);
                Ok(Some(Self::Int(a * b_int)))
            }
            (Self::Bool(a), Self::Float(b)) => {
                let a_float = if *a { 1.0 } else { 0.0 };
                Ok(Some(Self::Float(a_float * b)))
            }
            (Self::Float(a), Self::Bool(b)) => {
                let b_float = if *b { 1.0 } else { 0.0 };
                Ok(Some(Self::Float(a * b_float)))
            }
            (Self::Bool(a), Self::Bool(b)) => {
                let result = i64::from(*a) * i64::from(*b);
                Ok(Some(Self::Int(result)))
            }

            // String repetition: "ab" * 3 or 3 * "ab"
            (Self::InternString(s), Self::Int(n)) | (Self::Int(n), Self::InternString(s)) => {
                let count = i64_to_repeat_count(*n)?;
                let result = interns.get_str(*s).repeat(count);
                Ok(Some(Self::Ref(heap.allocate(HeapData::Str(result.into()))?)))
            }

            // Bytes repetition: b"ab" * 3 or 3 * b"ab"
            (Self::InternBytes(b), Self::Int(n)) | (Self::Int(n), Self::InternBytes(b)) => {
                let count = i64_to_repeat_count(*n)?;
                let result: Vec<u8> = interns.get_bytes(*b).repeat(count);
                Ok(Some(Self::Ref(heap.allocate(HeapData::Bytes(result.into()))?)))
            }

            // Heap string repetition: heap_str * int or int * heap_str
            (Self::Ref(id), Self::Int(n)) | (Self::Int(n), Self::Ref(id)) => {
                let count = i64_to_repeat_count(*n)?;
                heap.mult_sequence(*id, count)
            }

            _ => Ok(None),
        }
    }

    fn py_div(&self, other: &Self, _heap: &mut Heap<impl ResourceTracker>) -> RunResult<Option<Value>> {
        match (self, other) {
            // True division always returns float
            (Self::Int(a), Self::Int(b)) => {
                if *b == 0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float(*a as f64 / *b as f64)))
                }
            }
            (Self::Float(a), Self::Float(b)) => {
                if *b == 0.0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float(a / b)))
                }
            }
            (Self::Int(a), Self::Float(b)) => {
                if *b == 0.0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float(*a as f64 / b)))
                }
            }
            (Self::Float(a), Self::Int(b)) => {
                if *b == 0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float(a / *b as f64)))
                }
            }
            // Bool division (True=1, False=0)
            (Self::Bool(a), Self::Int(b)) => {
                if *b == 0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float(f64::from(*a) / *b as f64)))
                }
            }
            (Self::Int(a), Self::Bool(b)) => {
                if *b {
                    Ok(Some(Self::Float(*a as f64))) // a / 1 = a
                } else {
                    Err(ExcType::zero_division().into())
                }
            }
            (Self::Bool(a), Self::Float(b)) => {
                if *b == 0.0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float(f64::from(*a) / b)))
                }
            }
            (Self::Float(a), Self::Bool(b)) => {
                if *b {
                    Ok(Some(Self::Float(*a))) // a / 1.0 = a
                } else {
                    Err(ExcType::zero_division().into())
                }
            }
            (Self::Bool(a), Self::Bool(b)) => {
                if *b {
                    Ok(Some(Self::Float(f64::from(*a)))) // a / 1 = a
                } else {
                    Err(ExcType::zero_division().into())
                }
            }
            _ => Ok(None),
        }
    }

    fn py_floordiv(&self, other: &Self, _heap: &mut Heap<impl ResourceTracker>) -> RunResult<Option<Value>> {
        match (self, other) {
            // Floor division: int // int returns int
            (Self::Int(a), Self::Int(b)) => {
                if *b == 0 {
                    Err(ExcType::zero_division().into())
                } else {
                    // Python floor division rounds toward negative infinity
                    // div_euclid doesn't match Python semantics, so compute manually
                    let d = a / b;
                    let r = a % b;
                    // If there's a remainder and signs differ, round down (toward -∞)
                    let result = if r != 0 && (*a < 0) != (*b < 0) { d - 1 } else { d };
                    Ok(Some(Self::Int(result)))
                }
            }
            // Float floor division returns float
            (Self::Float(a), Self::Float(b)) => {
                if *b == 0.0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float((a / b).floor())))
                }
            }
            (Self::Int(a), Self::Float(b)) => {
                if *b == 0.0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float((*a as f64 / b).floor())))
                }
            }
            (Self::Float(a), Self::Int(b)) => {
                if *b == 0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float((a / *b as f64).floor())))
                }
            }
            // Bool floor division (True=1, False=0)
            (Self::Bool(a), Self::Int(b)) => {
                if *b == 0 {
                    Err(ExcType::zero_division().into())
                } else {
                    let a_int = i64::from(*a);
                    // Use same floor division logic as Int // Int
                    let d = a_int / b;
                    let r = a_int % b;
                    let result = if r != 0 && (a_int < 0) != (*b < 0) { d - 1 } else { d };
                    Ok(Some(Self::Int(result)))
                }
            }
            (Self::Int(a), Self::Bool(b)) => {
                if *b {
                    Ok(Some(Self::Int(*a))) // a // 1 = a
                } else {
                    Err(ExcType::zero_division().into())
                }
            }
            (Self::Bool(a), Self::Float(b)) => {
                if *b == 0.0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float((f64::from(*a) / b).floor())))
                }
            }
            (Self::Float(a), Self::Bool(b)) => {
                if *b {
                    Ok(Some(Self::Float(a.floor()))) // a // 1.0 = floor(a)
                } else {
                    Err(ExcType::zero_division().into())
                }
            }
            (Self::Bool(a), Self::Bool(b)) => {
                if *b {
                    Ok(Some(Self::Int(i64::from(*a)))) // a // 1 = a
                } else {
                    Err(ExcType::zero_division().into())
                }
            }
            _ => Ok(None),
        }
    }

    fn py_pow(&self, other: &Self, _heap: &mut Heap<impl ResourceTracker>) -> RunResult<Option<Value>> {
        match (self, other) {
            (Self::Int(base), Self::Int(exp)) => {
                if *base == 0 && *exp < 0 {
                    Err(ExcType::zero_pow_negative().into())
                } else if *exp >= 0 {
                    // Positive exponent: try to return int, fall back to float on overflow
                    // Note: exp > u32::MAX would overflow, so we use float for large exponents
                    if let Ok(exp_u32) = u32::try_from(*exp) {
                        match base.checked_pow(exp_u32) {
                            Some(result) => Ok(Some(Self::Int(result))),
                            None => Ok(Some(Self::Float((*base as f64).powf(*exp as f64)))),
                        }
                    } else {
                        Ok(Some(Self::Float((*base as f64).powf(*exp as f64))))
                    }
                } else {
                    // Negative exponent: return float
                    // Use powi if exp fits in i32, otherwise use powf
                    if let Ok(exp_i32) = i32::try_from(*exp) {
                        Ok(Some(Self::Float((*base as f64).powi(exp_i32))))
                    } else {
                        Ok(Some(Self::Float((*base as f64).powf(*exp as f64))))
                    }
                }
            }
            (Self::Float(base), Self::Float(exp)) => {
                if *base == 0.0 && *exp < 0.0 {
                    Err(ExcType::zero_pow_negative().into())
                } else {
                    Ok(Some(Self::Float(base.powf(*exp))))
                }
            }
            (Self::Int(base), Self::Float(exp)) => {
                if *base == 0 && *exp < 0.0 {
                    Err(ExcType::zero_pow_negative().into())
                } else {
                    Ok(Some(Self::Float((*base as f64).powf(*exp))))
                }
            }
            (Self::Float(base), Self::Int(exp)) => {
                if *base == 0.0 && *exp < 0 {
                    Err(ExcType::zero_pow_negative().into())
                } else if let Ok(exp_i32) = i32::try_from(*exp) {
                    // Use powi if exp fits in i32
                    Ok(Some(Self::Float(base.powi(exp_i32))))
                } else {
                    // Fall back to powf for exponents outside i32 range
                    Ok(Some(Self::Float(base.powf(*exp as f64))))
                }
            }
            // Bool power operations (True=1, False=0)
            (Self::Bool(base), Self::Int(exp)) => {
                let base_int = i64::from(*base);
                if base_int == 0 && *exp < 0 {
                    Err(ExcType::zero_pow_negative().into())
                } else if *exp >= 0 {
                    // Positive exponent: 1**n=1, 0**n=0 (for n>0), 0**0=1
                    if let Ok(exp_u32) = u32::try_from(*exp) {
                        match base_int.checked_pow(exp_u32) {
                            Some(result) => Ok(Some(Self::Int(result))),
                            None => Ok(Some(Self::Float((base_int as f64).powf(*exp as f64)))),
                        }
                    } else {
                        Ok(Some(Self::Float((base_int as f64).powf(*exp as f64))))
                    }
                } else {
                    // Negative exponent: return float (1**-n=1.0)
                    if let Ok(exp_i32) = i32::try_from(*exp) {
                        Ok(Some(Self::Float((base_int as f64).powi(exp_i32))))
                    } else {
                        Ok(Some(Self::Float((base_int as f64).powf(*exp as f64))))
                    }
                }
            }
            (Self::Int(base), Self::Bool(exp)) => {
                // n ** True = n, n ** False = 1
                if *exp {
                    Ok(Some(Self::Int(*base)))
                } else {
                    Ok(Some(Self::Int(1)))
                }
            }
            (Self::Bool(base), Self::Float(exp)) => {
                let base_float = f64::from(*base);
                if base_float == 0.0 && *exp < 0.0 {
                    Err(ExcType::zero_pow_negative().into())
                } else {
                    Ok(Some(Self::Float(base_float.powf(*exp))))
                }
            }
            (Self::Float(base), Self::Bool(exp)) => {
                // base ** True = base, base ** False = 1.0
                if *exp {
                    Ok(Some(Self::Float(*base)))
                } else {
                    Ok(Some(Self::Float(1.0)))
                }
            }
            (Self::Bool(base), Self::Bool(exp)) => {
                // True ** True = 1, True ** False = 1, False ** True = 0, False ** False = 1
                let base_int = i64::from(*base);
                let exp_int = i64::from(*exp);
                if exp_int == 0 {
                    Ok(Some(Self::Int(1))) // anything ** 0 = 1
                } else {
                    Ok(Some(Self::Int(base_int))) // base ** 1 = base
                }
            }
            _ => Ok(None),
        }
    }

    fn py_getitem(&self, key: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Self> {
        match self {
            Self::Ref(id) => {
                // Need to take entry out to allow mutable heap access
                let id = *id;
                heap.with_entry_mut(id, |heap, data| data.py_getitem(key, heap, interns))
            }
            _ => Err(ExcType::type_error_not_sub(self.py_type(heap))),
        }
    }

    fn py_setitem(
        &mut self,
        key: Self,
        value: Self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<()> {
        match self {
            Self::Ref(id) => {
                let id = *id;
                heap.with_entry_mut(id, |heap, data| data.py_setitem(key, value, heap, interns))
            }
            _ => Err(ExcType::type_error(&format!(
                "'{}' object does not support item assignment",
                self.py_type(heap)
            ))),
        }
    }
}

impl Value {
    /// Returns a stable, unique identifier for this value.
    ///
    /// Should match Python's `id()` function conceptually.
    ///
    /// For immediate values (Int, Float, Builtins), this computes a deterministic ID
    /// based on the value's hash, avoiding heap allocation. This means `id(5) == id(5)` will
    /// return True (unlike CPython for large integers outside the interning range).
    ///
    /// Singletons (None, True, False, etc.) return IDs from a dedicated tagged range.
    /// Interned strings/bytes use their interner index for stable identity.
    /// Heap-allocated values (Ref) reuse their `HeapId` inside the heap-tagged range.
    pub fn id(&self) -> usize {
        match self {
            // Singletons have fixed tagged IDs
            Self::Undefined => singleton_id(SingletonSlot::Undefined),
            Self::Ellipsis => singleton_id(SingletonSlot::Ellipsis),
            Self::None => singleton_id(SingletonSlot::None),
            Self::Bool(b) => {
                if *b {
                    singleton_id(SingletonSlot::True)
                } else {
                    singleton_id(SingletonSlot::False)
                }
            }
            // Interned strings/bytes use their index directly - the index is the stable identifier
            Self::InternString(string_id) => INTERN_STR_ID_TAG | (string_id.index() & INTERN_STR_ID_MASK),
            Self::InternBytes(bytes_id) => INTERN_BYTES_ID_TAG | (bytes_id.index() & INTERN_BYTES_ID_MASK),
            // Already heap-allocated (includes Range and Exception), return id within a dedicated tag range
            Self::Ref(id) => heap_tagged_id(*id),
            // Value-based IDs for immediate types (no heap allocation!)
            Self::Int(v) => int_value_id(*v),
            Self::Float(v) => float_value_id(*v),
            Self::Builtin(c) => builtin_value_id(*c),
            Self::DefFunction(f_id) => function_value_id(*f_id),
            Self::ExtFunction(f_id) => ext_function_value_id(*f_id),
            #[cfg(feature = "ref-count-panic")]
            Self::Dereferenced => panic!("Cannot get id of Dereferenced object"),
        }
    }

    pub fn ref_id(&self) -> Option<HeapId> {
        match self {
            Self::Ref(id) => Some(*id),
            _ => None,
        }
    }

    /// Equivalent of Python's `is` operator.
    ///
    /// Compares value identity by comparing their IDs.
    pub fn is(&self, other: &Self) -> bool {
        self.id() == other.id()
    }

    /// Computes the hash value for this value, used for dict keys.
    ///
    /// Returns Some(hash) for hashable types (immediate values and immutable heap types).
    /// Returns None for unhashable types (list, dict).
    ///
    /// For heap-allocated values (Ref variant), this computes the hash lazily
    /// on first use and caches it for subsequent calls.
    ///
    /// The `interns` parameter is needed for InternString/InternBytes to look up
    /// their actual content and hash it consistently with equivalent heap Str/Bytes.
    pub fn py_hash(&self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Option<u64> {
        // strings bytes and heap allocated values have their own hashing logic
        match self {
            // Hash just the actual string or bytes content for consistency with heap Str/Bytes
            // hence we don't include the discriminant
            Self::InternString(string_id) => {
                let mut hasher = DefaultHasher::new();
                interns.get_str(*string_id).hash(&mut hasher);
                return Some(hasher.finish());
            }
            Self::InternBytes(bytes_id) => {
                let mut hasher = DefaultHasher::new();
                interns.get_bytes(*bytes_id).hash(&mut hasher);
                return Some(hasher.finish());
            }
            // For heap-allocated values (includes Range and Exception), compute hash lazily and cache it
            Self::Ref(id) => return heap.get_or_compute_hash(*id, interns),
            _ => {}
        }

        let mut hasher = DefaultHasher::new();
        // hash based on discriminant to avoid collisions with different types
        discriminant(self).hash(&mut hasher);
        match self {
            // Immediate values can be hashed directly
            Self::Undefined | Self::Ellipsis | Self::None => {}
            Self::Bool(b) => b.hash(&mut hasher),
            Self::Int(i) => i.hash(&mut hasher),
            // Hash the bit representation of float for consistency
            Self::Float(f) => f.to_bits().hash(&mut hasher),
            Self::Builtin(b) => b.hash(&mut hasher),
            // Hash functions based on function ID
            Self::DefFunction(f_id) => f_id.hash(&mut hasher),
            Self::ExtFunction(f_id) => f_id.hash(&mut hasher),
            Self::InternString(_) | Self::InternBytes(_) | Self::Ref(_) => unreachable!("covered above"),
            #[cfg(feature = "ref-count-panic")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
        Some(hasher.finish())
    }

    /// TODO this doesn't have many tests!!! also doesn't cover bytes
    /// Checks if `item` is contained in `self` (the container).
    ///
    /// Implements Python's `in` operator for various container types:
    /// - List/Tuple: linear search with equality
    /// - Dict: key lookup
    /// - Set/FrozenSet: element lookup
    /// - Str: substring search
    pub fn py_contains(
        &self,
        item: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<bool> {
        match self {
            Self::Ref(heap_id) => {
                // Use with_entry_mut to temporarily take ownership of the container.
                // This allows iterating over container elements while calling py_eq
                // (which needs &mut Heap for comparing nested heap values).
                heap.with_entry_mut(*heap_id, |heap, data| match data {
                    HeapData::List(el) => Ok(el.as_vec().iter().any(|i| item.py_eq(i, heap, interns))),
                    HeapData::Tuple(el) => Ok(el.as_vec().iter().any(|i| item.py_eq(i, heap, interns))),
                    HeapData::Dict(dict) => dict.get(item, heap, interns).map(|m| m.is_some()),
                    HeapData::Set(set) => set.contains(item, heap, interns),
                    HeapData::FrozenSet(fset) => fset.contains(item, heap, interns),
                    HeapData::Str(s) => str_contains(s.as_str(), item, heap, interns),
                    other => {
                        let type_name = other.py_type(heap);
                        Err(ExcType::type_error(&format!(
                            "argument of type '{type_name}' is not iterable"
                        )))
                    }
                })
            }
            Self::InternString(string_id) => {
                let container_str = interns.get_str(*string_id);
                str_contains(container_str, item, heap, interns)
            }
            _ => {
                let type_name = self.py_type(heap);
                Err(ExcType::type_error(&format!(
                    "argument of type '{type_name}' is not iterable"
                )))
            }
        }
    }

    /// Gets an attribute from this value.
    ///
    /// Currently only Dataclass objects support attribute access.
    /// Returns AttributeError for other types.
    pub fn py_get_attr(
        &self,
        name_id: StringId,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Self> {
        let attr_name = interns.get_str(name_id);

        if let Self::Ref(heap_id) = self {
            let heap_id = *heap_id;
            let is_dataclass = matches!(heap.get(heap_id), HeapData::Dataclass(_));

            if is_dataclass {
                let name_value = Self::InternString(name_id);
                heap.with_entry_mut(heap_id, |heap, data| {
                    if let HeapData::Dataclass(dc) = data {
                        match dc.get_attr(&name_value, heap, interns) {
                            Ok(Some(value)) => Ok(value.clone_with_heap(heap)),
                            Ok(None) => {
                                // Use the dataclass's actual name for the error message
                                Err(ExcType::attribute_error_not_found(dc.name(), attr_name))
                            }
                            Err(e) => Err(e),
                        }
                    } else {
                        unreachable!("type changed during borrow")
                    }
                })
            } else {
                let type_name = heap.get(heap_id).py_type(heap);
                Err(ExcType::attribute_error(type_name, attr_name))
            }
        } else {
            let type_name = self.py_type(heap);
            Err(ExcType::attribute_error(type_name, attr_name))
        }
    }

    /// Sets an attribute on this value.
    ///
    /// Currently only Dataclass objects support attribute setting.
    /// Returns AttributeError for other types.
    ///
    /// Takes ownership of `value` and drops it on error.
    /// On success, drops the old attribute value if one existed.
    pub fn py_set_attr(
        &self,
        name_id: StringId,
        value: Self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<()> {
        let attr_name = interns.get_str(name_id);

        if let Self::Ref(heap_id) = self {
            let heap_id = *heap_id;
            let is_dataclass = matches!(heap.get(heap_id), HeapData::Dataclass(_));

            if is_dataclass {
                let name_value = Self::InternString(name_id);
                heap.with_entry_mut(heap_id, |heap, data| {
                    if let HeapData::Dataclass(dc) = data {
                        match dc.set_attr(name_value, value, heap, interns) {
                            Ok(old_value) => {
                                if let Some(old) = old_value {
                                    old.drop_with_heap(heap);
                                }
                                Ok(())
                            }
                            Err(e) => Err(e),
                        }
                    } else {
                        unreachable!("type changed during borrow")
                    }
                })
            } else {
                let type_name = heap.get(heap_id).py_type(heap);
                value.drop_with_heap(heap);
                Err(ExcType::attribute_error_no_setattr(type_name, attr_name))
            }
        } else {
            let type_name = self.py_type(heap);
            value.drop_with_heap(heap);
            Err(ExcType::attribute_error_no_setattr(type_name, attr_name))
        }
    }

    pub fn as_int(&self) -> RunResult<i64> {
        match self {
            Self::Int(i) => Ok(*i),
            // TODO use self.type
            _ => exc_err_fmt!(ExcType::TypeError; "'{self:?}' object cannot be interpreted as an integer"),
        }
    }

    /// Performs a binary bitwise operation on two values.
    ///
    /// Python only supports bitwise operations on integers (and bools, which coerce to int).
    /// Returns a `TypeError` if either operand is not an integer or bool.
    ///
    /// For shift operations:
    /// - Negative shift counts raise `ValueError`
    /// - Left shifts > 63 raise `OverflowError`
    /// - Right shifts > 63 return 0 (or -1 for negative numbers)
    pub fn py_bitwise(&self, other: &Self, op: BitwiseOp, heap: &Heap<impl ResourceTracker>) -> Result<Self, RunError> {
        // Capture types for error messages
        let lhs_type = self.py_type(heap);
        let rhs_type = other.py_type(heap);

        // Get integer values from lhs and rhs
        let lhs_int = match self {
            Self::Int(i) => Some(*i),
            Self::Bool(b) => Some(i64::from(*b)),
            _ => None,
        };
        let rhs_int = match other {
            Self::Int(i) => Some(*i),
            Self::Bool(b) => Some(i64::from(*b)),
            _ => None,
        };

        if let (Some(l), Some(r)) = (lhs_int, rhs_int) {
            let result = match op {
                BitwiseOp::And => l & r,
                BitwiseOp::Or => l | r,
                BitwiseOp::Xor => l ^ r,
                BitwiseOp::LShift => {
                    // Python raises ValueError for negative shift, OverflowError for too large
                    if r < 0 {
                        return Err(ExcType::value_error_negative_shift_count());
                    }
                    // Limit shift to avoid overflow
                    if r > 63 {
                        return Err(ExcType::overflow_shift_count());
                    }
                    l << r
                }
                BitwiseOp::RShift => {
                    if r < 0 {
                        return Err(ExcType::value_error_negative_shift_count());
                    }
                    // Large right shifts just give 0 or -1 for negative numbers
                    if r > 63 {
                        if l < 0 {
                            -1
                        } else {
                            0
                        }
                    } else {
                        l >> r
                    }
                }
            };
            Ok(Self::Int(result))
        } else {
            Err(ExcType::binary_type_error(op.as_str(), lhs_type, rhs_type))
        }
    }

    /// Clones an value with proper heap reference counting.
    ///
    /// For immediate values (Int, Bool, None, etc.), this performs a simple copy.
    /// For heap-allocated values (Ref variant), this increments the reference count
    /// and returns a new reference to the same heap value.
    ///
    /// # Important
    /// This method MUST be used instead of the derived `Clone` implementation to ensure
    /// proper reference counting. Using `.clone()` directly will bypass reference counting
    /// and cause memory leaks or double-frees.
    #[must_use]
    pub fn clone_with_heap(&self, heap: &mut Heap<impl ResourceTracker>) -> Self {
        match self {
            Self::Ref(id) => {
                heap.inc_ref(*id);
                Self::Ref(*id)
            }
            // Immediate values can be copied without heap interaction
            other => other.clone_immediate(),
        }
    }

    /// Drops an value, decrementing its heap reference count if applicable.
    ///
    /// For immediate values, this is a no-op. For heap-allocated values (Ref variant),
    /// this decrements the reference count and frees the value (and any children) when
    /// the count reaches zero. For Closure variants, this decrements ref counts on all
    /// captured cells.
    ///
    /// # Important
    /// This method MUST be called before overwriting a namespace slot or discarding
    /// a value to prevent memory leaks.
    ///
    /// With `ref-count-panic` enabled, `Ref` variants are replaced with `Dereferenced` and
    /// the original is forgotten to prevent the Drop impl from panicking. Non-Ref variants
    /// are left unchanged since they don't trigger the Drop panic.
    #[cfg_attr(not(feature = "ref-count-return"), expect(unused_mut))]
    pub fn drop_with_heap(mut self, heap: &mut Heap<impl ResourceTracker>) {
        #[cfg(feature = "ref-count-panic")]
        {
            let old = std::mem::replace(&mut self, Self::Dereferenced);
            if let Self::Ref(id) = &old {
                heap.dec_ref(*id);
                std::mem::forget(old);
            }
        }
        #[cfg(not(feature = "ref-count-panic"))]
        if let Self::Ref(id) = self {
            heap.dec_ref(id);
        }
    }

    /// Internal helper for copying immediate values without heap interaction.
    ///
    /// This method should only be called by `clone_with_heap()` for immediate values.
    /// Attempting to clone a Ref variant will panic.
    pub fn clone_immediate(&self) -> Self {
        match self {
            Self::Ref(_) => panic!("Ref clones must go through clone_with_heap to maintain refcounts"),
            #[cfg(feature = "ref-count-panic")]
            Self::Dereferenced => panic!("Cannot clone Dereferenced object"),
            _ => self.copy_for_extend(),
        }
    }

    /// Creates a shallow copy of this Value without incrementing reference counts.
    ///
    /// IMPORTANT: For Ref variants, this copies the ValueId but does NOT increment
    /// the reference count. The caller MUST call `heap.inc_ref()` separately for any
    /// Ref variants to maintain correct reference counting.
    ///
    /// For Closure variants, this copies without incrementing cell ref counts.
    /// The caller MUST increment ref counts on the captured cells separately.
    ///
    /// This is useful when you need to copy Objects from a borrowed heap context
    /// and will increment refcounts in a separate step.
    pub(crate) fn copy_for_extend(&self) -> Self {
        match self {
            Self::Undefined => Self::Undefined,
            Self::Ellipsis => Self::Ellipsis,
            Self::None => Self::None,
            Self::Bool(b) => Self::Bool(*b),
            Self::Int(v) => Self::Int(*v),
            Self::Float(v) => Self::Float(*v),
            Self::Builtin(b) => Self::Builtin(*b),
            Self::DefFunction(f) => Self::DefFunction(*f),
            Self::ExtFunction(f) => Self::ExtFunction(*f),
            Self::InternString(s) => Self::InternString(*s),
            Self::InternBytes(b) => Self::InternBytes(*b),
            Self::Ref(id) => Self::Ref(*id), // Caller must increment refcount!
            #[cfg(feature = "ref-count-panic")]
            Self::Dereferenced => panic!("Cannot copy Dereferenced object"),
        }
    }

    /// Mark as Dereferenced to prevent Drop panic
    ///
    /// This should be called from `py_dec_ref_ids` methods only
    #[cfg(feature = "ref-count-panic")]
    pub fn dec_ref_forget(&mut self) {
        let old = std::mem::replace(self, Self::Dereferenced);
        std::mem::forget(old);
    }

    /// Converts the value into a keyword string representation if possible.
    ///
    /// Returns `Some(KeywordStr)` for `InternString` values or heap `str`
    /// objects, otherwise returns `None`.
    pub fn as_either_str<T: ResourceTracker>(&self, heap: &mut Heap<T>) -> Option<EitherStr> {
        match self {
            Self::InternString(id) => Some(EitherStr::Interned(*id)),
            Self::Ref(heap_id) => match heap.get(*heap_id) {
                HeapData::Str(s) => Some(EitherStr::Heap(s.as_str().to_owned())),
                _ => None,
            },
            _ => None,
        }
    }
}

/// Interned or heap-owned string identifier.
#[derive(Debug, Clone)]
pub enum EitherStr {
    /// Interned string identifier (cheap comparisons and no allocation).
    Interned(StringId),
    /// Heap-owned string extracted from a `str` object.
    Heap(String),
}

impl EitherStr {
    /// Returns the keyword as a str slice for error messages or comparisons.
    pub fn as_str<'a>(&'a self, interns: &'a Interns) -> &'a str {
        match self {
            Self::Interned(id) => interns.get_str(*id),
            Self::Heap(s) => s.as_str(),
        }
    }

    /// Checks whether this keyword matches the given interned identifier.
    pub fn matches(&self, target: StringId, interns: &Interns) -> bool {
        match self {
            Self::Interned(id) => *id == target,
            Self::Heap(s) => s == interns.get_str(target),
        }
    }
}

/// Attribute names for accessing fields and methods on objects.
///
/// Uses `StringId` for interned attribute names (parsed at compile time) and
/// `Other(String)` for runtime-constructed names (e.g., from `getattr()`).
///
/// Known method names (append, get, keys, etc.) are pre-interned with stable
/// `StringId` values - see `intern.rs` for the `ATTR_*` constants.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum Attr {
    /// Interned attribute name (compile-time constant or parsed identifier).
    ///
    /// Compare against `ATTR_*` constants from `intern.rs` for known methods.
    Interned(StringId),

    /// Runtime-constructed attribute name (rare, e.g., from `getattr()`).
    Other(String),
}

impl Attr {
    /// Returns the attribute name as a string reference.
    pub fn as_str<'a>(&'a self, interns: &'a Interns) -> &'a str {
        match self {
            Self::Interned(id) => interns.get_str(*id),
            Self::Other(name) => name,
        }
    }

    /// Returns the `StringId` if this is an interned attribute.
    #[inline]
    pub fn string_id(&self) -> Option<StringId> {
        match self {
            Self::Interned(id) => Some(*id),
            Self::Other(_) => None,
        }
    }
}

/// High-bit tag reserved for literal singletons (None, Ellipsis, booleans).
const SINGLETON_ID_TAG: usize = 1usize << (usize::BITS - 1);
/// High-bit tag reserved for interned string `id()` values.
const INTERN_STR_ID_TAG: usize = 1usize << (usize::BITS - 2);
/// High-bit tag reserved for interned bytes `id()` values to avoid colliding with any other space.
const INTERN_BYTES_ID_TAG: usize = 1usize << (usize::BITS - 3);
/// High-bit tag reserved for heap-backed `HeapId`s.
const HEAP_ID_TAG: usize = 1usize << (usize::BITS - 4);

/// Mask that keeps pointer-derived bits below the bytes tag bit.
const INTERN_BYTES_ID_MASK: usize = INTERN_BYTES_ID_TAG - 1;
/// Mask that keeps pointer-derived bits below the string tag bit.
const INTERN_STR_ID_MASK: usize = INTERN_STR_ID_TAG - 1;
/// Mask that keeps per-singleton offsets below the singleton tag bit.
const SINGLETON_ID_MASK: usize = SINGLETON_ID_TAG - 1;
/// Mask that keeps heap value IDs below the heap tag bit.
const HEAP_ID_MASK: usize = HEAP_ID_TAG - 1;

/// High-bit tag for Int value-based IDs (no heap allocation needed).
const INT_ID_TAG: usize = 1usize << (usize::BITS - 5);
/// High-bit tag for Float value-based IDs.
const FLOAT_ID_TAG: usize = 1usize << (usize::BITS - 6);
/// High-bit tag for Callable value-based IDs.
const BUILTIN_ID_TAG: usize = 1usize << (usize::BITS - 7);
/// High-bit tag for Function value-based IDs.
const FUNCTION_ID_TAG: usize = 1usize << (usize::BITS - 8);
/// High-bit tag for External Function value-based IDs.
const EXTFUNCTION_ID_TAG: usize = 1usize << (usize::BITS - 9);

/// Masks for value-based ID tags (keep bits below the tag bit).
const INT_ID_MASK: usize = INT_ID_TAG - 1;
const FLOAT_ID_MASK: usize = FLOAT_ID_TAG - 1;
const BUILTIN_ID_MASK: usize = BUILTIN_ID_TAG - 1;
const FUNCTION_ID_MASK: usize = FUNCTION_ID_TAG - 1;
const EXTFUNCTION_ID_MASK: usize = EXTFUNCTION_ID_TAG - 1;

/// Enumerates singleton literal slots so we can issue stable `id()` values without heap allocation.
#[repr(usize)]
#[derive(Copy, Clone)]
enum SingletonSlot {
    Undefined = 0,
    Ellipsis = 1,
    None = 2,
    False = 3,
    True = 4,
}

/// Returns the fully tagged `id()` value for the requested singleton literal.
#[inline]
const fn singleton_id(slot: SingletonSlot) -> usize {
    SINGLETON_ID_TAG | ((slot as usize) & SINGLETON_ID_MASK)
}

/// Converts a heap `HeapId` into its tagged `id()` value, ensuring it never collides with other spaces.
#[inline]
pub fn heap_tagged_id(heap_id: HeapId) -> usize {
    HEAP_ID_TAG | (heap_id.index() & HEAP_ID_MASK)
}

/// Computes a deterministic ID for an i64 integer value.
/// Uses the value's hash combined with a type tag to ensure uniqueness across types.
#[inline]
fn int_value_id(value: i64) -> usize {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    let hash_u64 = hasher.finish();
    // Mask to usize range before conversion to handle 32-bit platforms
    let masked = hash_u64 & (usize::MAX as u64);
    let hash_usize = usize::try_from(masked).expect("masked value fits in usize");
    INT_ID_TAG | (hash_usize & INT_ID_MASK)
}

/// Computes a deterministic ID for an f64 float value.
/// Uses the bit representation's hash for consistency (handles NaN, infinities, etc.).
#[inline]
fn float_value_id(value: f64) -> usize {
    let mut hasher = DefaultHasher::new();
    value.to_bits().hash(&mut hasher);
    let hash_u64 = hasher.finish();
    // Mask to usize range before conversion to handle 32-bit platforms
    let masked = hash_u64 & (usize::MAX as u64);
    let hash_usize = usize::try_from(masked).expect("masked value fits in usize");
    FLOAT_ID_TAG | (hash_usize & FLOAT_ID_MASK)
}

/// Computes a deterministic ID for a builtin based on its discriminant.
#[inline]
fn builtin_value_id(b: Builtins) -> usize {
    let mut hasher = DefaultHasher::new();
    discriminant(&b).hash(&mut hasher);
    match &b {
        Builtins::Function(f) => discriminant(f).hash(&mut hasher),
        Builtins::ExcType(exc) => discriminant(exc).hash(&mut hasher),
        Builtins::Type(t) => discriminant(t).hash(&mut hasher),
    }
    let hash_u64 = hasher.finish();
    // Mask to usize range before conversion to handle 32-bit platforms
    let masked = hash_u64 & (usize::MAX as u64);
    let hash_usize = usize::try_from(masked).expect("masked value fits in usize");
    BUILTIN_ID_TAG | (hash_usize & BUILTIN_ID_MASK)
}

/// Computes a deterministic ID for a function based on its id.
#[inline]
fn function_value_id(f_id: FunctionId) -> usize {
    FUNCTION_ID_TAG | (f_id.index() & FUNCTION_ID_MASK)
}

/// Computes a deterministic ID for an external function based on its id.
#[inline]
fn ext_function_value_id(f_id: ExtFunctionId) -> usize {
    EXTFUNCTION_ID_TAG | (f_id.index() & EXTFUNCTION_ID_MASK)
}

/// Converts an i64 repeat count to usize, handling negative values and overflow.
///
/// Returns 0 for negative values (Python treats negative repeat counts as 0).
/// Returns `OverflowError` if the value exceeds `usize::MAX`.
#[inline]
fn i64_to_repeat_count(n: i64) -> RunResult<usize> {
    if n <= 0 {
        Ok(0)
    } else {
        usize::try_from(n).map_err(|_| ExcType::overflow_repeat_count().into())
    }
}

/// Helper for substring containment check in strings.
///
/// Called by `py_contains` when the container is a string.
/// The item must also be a string (either interned or heap-allocated).
fn str_contains(
    container_str: &str,
    item: &Value,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<bool> {
    match item {
        Value::InternString(item_id) => {
            let item_str = interns.get_str(*item_id);
            Ok(container_str.contains(item_str))
        }
        Value::Ref(item_heap_id) => {
            if let HeapData::Str(item_str) = heap.get(*item_heap_id) {
                Ok(container_str.contains(item_str.as_str()))
            } else {
                Err(ExcType::type_error("'in <str>' requires string as left operand"))
            }
        }
        _ => Err(ExcType::type_error("'in <str>' requires string as left operand")),
    }
}
