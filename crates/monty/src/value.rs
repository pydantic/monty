use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::hash_map::DefaultHasher,
    fmt::{self, Write},
    hash::{Hash, Hasher},
    mem::discriminant,
    str::FromStr,
};

use ahash::AHashSet;
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{ToPrimitive, Zero};

use crate::{
    asyncio::CallId,
    builtins::Builtins,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId},
    intern::{BytesId, ExtFunctionId, FunctionId, Interns, LongIntId, StaticStrings, StringId},
    modules::ModuleFunctions,
    resource::{LARGE_RESULT_THRESHOLD, ResourceTracker},
    types::{
        AttrCallResult, LongInt, Property, PyTrait, Str, Type,
        bytes::{bytes_repr_fmt, get_byte_at_index, get_bytes_slice},
        path,
        str::{allocate_char, get_char_at_index, get_str_slice, string_repr_fmt},
    },
};

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
    /// An interned long integer literal. The `LongIntId` references the `BigInt` in the Interns table.
    /// Used for integer literals exceeding i64 range. Converted to heap-allocated `LongInt` on load.
    InternLongInt(LongIntId),
    /// A builtin function or exception type
    Builtin(Builtins),
    /// A function from a module (not a global builtin).
    /// Module functions require importing a module to access (e.g., `asyncio.gather`).
    ModuleFunction(ModuleFunctions),
    /// A function defined in the module (not a closure, doesn't capture any variables)
    DefFunction(FunctionId),
    /// Reference to an external function defined on the host
    ExtFunction(ExtFunctionId),
    /// A marker value representing special objects like sys.stdout/stderr.
    /// These exist but have minimal functionality in the sandboxed environment.
    Marker(Marker),
    /// A property descriptor that computes its value when accessed.
    /// When retrieved via `py_getattr`, the property's getter is invoked.
    Property(Property),
    /// A pending external function call result.
    ///
    /// Created when the host calls `run_pending()` instead of `run(result)` for an
    /// external function call. The CallId correlates with the call that created it.
    /// When awaited, blocks the task until the host provides a result via `resume()`.
    ///
    /// ExternalFutures follow single-shot semantics like coroutines - awaiting an
    /// already-awaited ExternalFuture raises RuntimeError.
    ExternalFuture(CallId),

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
            Self::Int(_) | Self::InternLongInt(_) => Type::Int,
            Self::Float(_) => Type::Float,
            Self::InternString(_) => Type::Str,
            Self::InternBytes(_) => Type::Bytes,
            Self::Builtin(c) => c.py_type(),
            Self::ModuleFunction(_) => Type::BuiltinFunction,
            Self::DefFunction(_) | Self::ExtFunction(_) => Type::Function,
            Self::Marker(m) => m.py_type(),
            Self::Property(_) => Type::Property,
            Self::ExternalFuture(_) => Type::Coroutine,
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

            // Int == LongInt comparison
            (Self::Int(a), Self::Ref(id)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    BigInt::from(*a) == *li.inner()
                } else {
                    false
                }
            }
            // LongInt == Int comparison
            (Self::Ref(id), Self::Int(b)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    *li.inner() == BigInt::from(*b)
                } else {
                    false
                }
            }

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
            // Module functions equality
            (Self::ModuleFunction(mf1), Self::ModuleFunction(mf2)) => mf1 == mf2,
            (Self::DefFunction(f1), Self::DefFunction(f2)) => f1 == f2,
            // Markers compare equal if they're the same variant
            (Self::Marker(m1), Self::Marker(m2)) => m1 == m2,
            // Properties compare equal if they're the same variant
            (Self::Property(p1), Self::Property(p2)) => p1 == p2,

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
            // Int vs LongInt comparison
            (Self::Int(a), Self::Ref(id)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    BigInt::from(*a).partial_cmp(li.inner())
                } else {
                    None
                }
            }
            // LongInt vs Int comparison
            (Self::Ref(id), Self::Int(b)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    li.inner().partial_cmp(&BigInt::from(*b))
                } else {
                    None
                }
            }
            // LongInt vs LongInt comparison
            (Self::Ref(id1), Self::Ref(id2)) => {
                let is_longint1 = matches!(heap.get(*id1), HeapData::LongInt(_));
                let is_longint2 = matches!(heap.get(*id2), HeapData::LongInt(_));
                if is_longint1 && is_longint2 {
                    heap.with_two(*id1, *id2, |_heap, left, right| {
                        if let (HeapData::LongInt(a), HeapData::LongInt(b)) = (left, right) {
                            a.inner().partial_cmp(b.inner())
                        } else {
                            None
                        }
                    })
                } else {
                    None
                }
            }
            (Self::InternString(s1), Self::InternString(s2)) => interns.get_str(*s1).partial_cmp(interns.get_str(*s2)),
            (Self::InternBytes(b1), Self::InternBytes(b2)) => {
                interns.get_bytes(*b1).partial_cmp(interns.get_bytes(*b2))
            }
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
            // InternLongInt is always truthy (if it were zero, it would fit in i64)
            Self::InternLongInt(_) => true,
            Self::Builtin(_) | Self::ModuleFunction(_) => true, // Builtins are always truthy
            Self::DefFunction(_) | Self::ExtFunction(_) => true, // Functions are always truthy
            Self::Marker(_) => true,                            // Markers are always truthy
            Self::Property(_) => true,                          // Properties are always truthy
            Self::ExternalFuture(_) => true,                    // ExternalFutures are always truthy
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
            Self::InternLongInt(long_int_id) => write!(f, "{}", interns.get_long_int(*long_int_id)),
            Self::Float(v) => {
                let s = v.to_string();
                if s.contains('.') {
                    f.write_str(&s)
                } else {
                    write!(f, "{s}.0")
                }
            }
            Self::Builtin(b) => b.py_repr_fmt(f),
            Self::ModuleFunction(mf) => mf.py_repr_fmt(f, self.id()),
            Self::DefFunction(f_id) => interns.get_function(*f_id).py_repr_fmt(f, interns, self.id()),
            Self::ExtFunction(f_id) => {
                write!(f, "<function '{}' external>", interns.get_external_function_name(*f_id))
            }
            Self::InternString(string_id) => string_repr_fmt(interns.get_str(*string_id), f),
            Self::InternBytes(bytes_id) => bytes_repr_fmt(interns.get_bytes(*bytes_id), f),
            Self::Marker(m) => m.py_repr_fmt(f),
            Self::Property(p) => write!(f, "<property {p:?}>"),
            Self::ExternalFuture(call_id) => write!(f, "<coroutine external_future({})>", call_id.raw()),
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
            // Int + Int with overflow detection
            (Self::Int(a), Self::Int(b)) => {
                if let Some(result) = a.checked_add(*b) {
                    Ok(Some(Self::Int(result)))
                } else {
                    // Overflow - promote to LongInt
                    let li = LongInt::from(*a) + LongInt::from(*b);
                    li.into_value(heap).map(Some)
                }
            }
            // Int + LongInt
            (Self::Int(a), Self::Ref(id)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    let result = LongInt::from(*a) + LongInt::new(li.inner().clone());
                    result.into_value(heap).map(Some)
                } else {
                    Ok(None)
                }
            }
            // LongInt + Int
            (Self::Ref(id), Self::Int(b)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    let result = LongInt::new(li.inner().clone()) + LongInt::from(*b);
                    result.into_value(heap).map(Some)
                } else {
                    Ok(None)
                }
            }
            (Self::Float(v1), Self::Float(v2)) => Ok(Some(Self::Float(v1 + v2))),
            // Int + Float and Float + Int
            (Self::Int(a), Self::Float(b)) => Ok(Some(Self::Float(*a as f64 + b))),
            (Self::Float(a), Self::Int(b)) => Ok(Some(Self::Float(a + *b as f64))),
            (Self::Ref(id1), Self::Ref(id2)) => {
                // Check if both are LongInts
                let is_longint1 = matches!(heap.get(*id1), HeapData::LongInt(_));
                let is_longint2 = matches!(heap.get(*id2), HeapData::LongInt(_));
                if is_longint1 && is_longint2 {
                    heap.with_two(*id1, *id2, |heap, left, right| {
                        if let (HeapData::LongInt(a), HeapData::LongInt(b)) = (left, right) {
                            let result = LongInt::new(a.inner() + b.inner());
                            result.into_value(heap).map(Some)
                        } else {
                            Ok(None)
                        }
                    })
                } else {
                    heap.with_two(*id1, *id2, |heap, left, right| left.py_add(right, heap, interns))
                }
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
        heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<Option<Self>, crate::resource::ResourceError> {
        match (self, other) {
            // Int - Int with overflow detection
            (Self::Int(a), Self::Int(b)) => {
                if let Some(result) = a.checked_sub(*b) {
                    Ok(Some(Self::Int(result)))
                } else {
                    // Overflow - promote to LongInt
                    let li = LongInt::from(*a) - LongInt::from(*b);
                    li.into_value(heap).map(Some)
                }
            }
            // Int - LongInt
            (Self::Int(a), Self::Ref(id)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    let result = LongInt::from(*a) - LongInt::new(li.inner().clone());
                    result.into_value(heap).map(Some)
                } else {
                    Ok(None)
                }
            }
            // LongInt - Int
            (Self::Ref(id), Self::Int(b)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    let result = LongInt::new(li.inner().clone()) - LongInt::from(*b);
                    result.into_value(heap).map(Some)
                } else {
                    Ok(None)
                }
            }
            // LongInt - LongInt
            (Self::Ref(id1), Self::Ref(id2)) => {
                let is_longint1 = matches!(heap.get(*id1), HeapData::LongInt(_));
                let is_longint2 = matches!(heap.get(*id2), HeapData::LongInt(_));
                if is_longint1 && is_longint2 {
                    heap.with_two(*id1, *id2, |heap, left, right| {
                        if let (HeapData::LongInt(a), HeapData::LongInt(b)) = (left, right) {
                            let result = LongInt::new(a.inner() - b.inner());
                            result.into_value(heap).map(Some)
                        } else {
                            Ok(None)
                        }
                    })
                } else {
                    Ok(None)
                }
            }
            // Float - Float
            (Self::Float(a), Self::Float(b)) => Ok(Some(Self::Float(a - b))),
            // Int - Float and Float - Int
            (Self::Int(a), Self::Float(b)) => Ok(Some(Self::Float(*a as f64 - b))),
            (Self::Float(a), Self::Int(b)) => Ok(Some(Self::Float(a - *b as f64))),
            _ => Ok(None),
        }
    }

    fn py_mod(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Option<Self>> {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => {
                if *b == 0 {
                    Err(ExcType::zero_division().into())
                } else {
                    // Python modulo: result has the same sign as divisor (b)
                    // Standard remainder (%) in Rust has same sign as dividend (a)
                    // We need to adjust when signs differ and remainder is non-zero
                    let r = *a % *b;
                    let result = if r != 0 && (*a < 0) != (*b < 0) { r + *b } else { r };
                    Ok(Some(Self::Int(result)))
                }
            }
            // Int % LongInt
            (Self::Int(a), Self::Ref(id)) => {
                // Clone to avoid borrow conflict with heap mutation
                let b_clone = if let HeapData::LongInt(li) = heap.get(*id) {
                    if li.is_zero() {
                        return Err(ExcType::zero_division().into());
                    }
                    li.inner().clone()
                } else {
                    return Ok(None);
                };
                let bi = BigInt::from(*a).mod_floor(&b_clone);
                Ok(Some(LongInt::new(bi).into_value(heap)?))
            }
            // LongInt % Int
            (Self::Ref(id), Self::Int(b)) => {
                if *b == 0 {
                    return Err(ExcType::zero_division().into());
                }
                // Clone to avoid borrow conflict with heap mutation
                let a_clone = if let HeapData::LongInt(li) = heap.get(*id) {
                    li.inner().clone()
                } else {
                    return Ok(None);
                };
                let bi = a_clone.mod_floor(&BigInt::from(*b));
                Ok(Some(LongInt::new(bi).into_value(heap)?))
            }
            // LongInt % LongInt
            (Self::Ref(id1), Self::Ref(id2)) => {
                let is_longint1 = matches!(heap.get(*id1), HeapData::LongInt(_));
                let is_longint2 = matches!(heap.get(*id2), HeapData::LongInt(_));
                if is_longint1 && is_longint2 {
                    // Check for zero division first
                    if matches!(heap.get(*id2), HeapData::LongInt(li) if li.is_zero()) {
                        return Err(ExcType::zero_division().into());
                    }
                    Ok(heap.with_two(*id1, *id2, |heap, left, right| {
                        if let (HeapData::LongInt(a), HeapData::LongInt(b)) = (left, right) {
                            let bi = a.inner().mod_floor(b.inner());
                            LongInt::new(bi).into_value(heap).map(Some)
                        } else {
                            Ok(None)
                        }
                    })?)
                } else {
                    Ok(None)
                }
            }
            (Self::Float(v1), Self::Float(v2)) => {
                if *v2 == 0.0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float(v1 % v2)))
                }
            }
            (Self::Float(v1), Self::Int(v2)) => {
                if *v2 == 0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float(v1 % (*v2 as f64))))
                }
            }
            (Self::Int(v1), Self::Float(v2)) => {
                if *v2 == 0.0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float((*v1 as f64) % v2)))
                }
            }
            _ => Ok(None),
        }
    }

    fn py_mod_eq(&self, other: &Self, right_value: i64) -> Option<bool> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => {
                // Use Python's modulo semantics (result has same sign as divisor)
                let r = *v1 % *v2;
                let result = if r != 0 && (*v1 < 0) != (*v2 < 0) { r + *v2 } else { r };
                Some(result == right_value)
            }
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
                if let Some(result) = v1.checked_add(*v2) {
                    *self = Self::Int(result);
                } else {
                    // Overflow - promote to LongInt
                    let li = LongInt::from(*v1) + LongInt::from(*v2);
                    *self = li.into_value(heap)?;
                }
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
            // Numeric multiplication with overflow promotion to LongInt
            (Self::Int(a), Self::Int(b)) => {
                if let Some(result) = a.checked_mul(*b) {
                    Ok(Some(Self::Int(result)))
                } else {
                    // Overflow - promote to LongInt
                    let li = LongInt::from(*a) * LongInt::from(*b);
                    Ok(Some(li.into_value(heap)?))
                }
            }
            // Int * LongInt
            (Self::Int(a), Self::Ref(id)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    // Check size before computing to prevent DoS
                    let a_bits = i64_bits(*a);
                    let b_bits = li.bits();
                    if let Some(estimated) = LongInt::estimate_mult_bytes(a_bits, b_bits)
                        && estimated > LARGE_RESULT_THRESHOLD
                    {
                        heap.tracker().check_large_result(estimated)?;
                    }
                    let result = LongInt::from(*a) * LongInt::new(li.inner().clone());
                    Ok(Some(result.into_value(heap)?))
                } else {
                    // Check for sequence repetition
                    let count = i64_to_repeat_count(*a)?;
                    heap.mult_sequence(*id, count)
                }
            }
            // LongInt * Int
            (Self::Ref(id), Self::Int(b)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    // Check size before computing to prevent DoS
                    let a_bits = li.bits();
                    let b_bits = i64_bits(*b);
                    if let Some(estimated) = LongInt::estimate_mult_bytes(a_bits, b_bits)
                        && estimated > LARGE_RESULT_THRESHOLD
                    {
                        heap.tracker().check_large_result(estimated)?;
                    }
                    let result = LongInt::new(li.inner().clone()) * LongInt::from(*b);
                    Ok(Some(result.into_value(heap)?))
                } else {
                    // Check for sequence repetition
                    let count = i64_to_repeat_count(*b)?;
                    heap.mult_sequence(*id, count)
                }
            }
            // LongInt * LongInt or sequence * LongInt
            (Self::Ref(id1), Self::Ref(id2)) => {
                let is_longint1 = matches!(heap.get(*id1), HeapData::LongInt(_));
                let is_longint2 = matches!(heap.get(*id2), HeapData::LongInt(_));
                if is_longint1 && is_longint2 {
                    // LongInt * LongInt - get bits for size check
                    let a_bits = if let HeapData::LongInt(li) = heap.get(*id1) {
                        li.bits()
                    } else {
                        0
                    };
                    let b_bits = if let HeapData::LongInt(li) = heap.get(*id2) {
                        li.bits()
                    } else {
                        0
                    };
                    // Check size before computing to prevent DoS
                    if let Some(estimated) = LongInt::estimate_mult_bytes(a_bits, b_bits)
                        && estimated > LARGE_RESULT_THRESHOLD
                    {
                        heap.tracker().check_large_result(estimated)?;
                    }
                    Ok(heap.with_two(*id1, *id2, |heap, left, right| {
                        if let (HeapData::LongInt(a), HeapData::LongInt(b)) = (left, right) {
                            let result = LongInt::new(a.inner() * b.inner());
                            result.into_value(heap).map(Some)
                        } else {
                            Ok(None)
                        }
                    })?)
                } else if is_longint2 {
                    // sequence * LongInt - get the repeat count from LongInt
                    let count = if let HeapData::LongInt(li) = heap.get(*id2) {
                        longint_to_repeat_count(li)?
                    } else {
                        return Ok(None);
                    };
                    heap.mult_sequence(*id1, count)
                } else if is_longint1 {
                    // LongInt * sequence - get the repeat count from LongInt
                    let count = if let HeapData::LongInt(li) = heap.get(*id1) {
                        longint_to_repeat_count(li)?
                    } else {
                        return Ok(None);
                    };
                    heap.mult_sequence(*id2, count)
                } else {
                    Ok(None)
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

            // String repetition with LongInt: "ab" * bigint or bigint * "ab"
            (Self::InternString(s), Self::Ref(id)) | (Self::Ref(id), Self::InternString(s)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    let count = longint_to_repeat_count(li)?;
                    let result = interns.get_str(*s).repeat(count);
                    Ok(Some(Self::Ref(heap.allocate(HeapData::Str(result.into()))?)))
                } else {
                    Ok(None)
                }
            }

            // Bytes repetition with LongInt: b"ab" * bigint or bigint * b"ab"
            (Self::InternBytes(b), Self::Ref(id)) | (Self::Ref(id), Self::InternBytes(b)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    let count = longint_to_repeat_count(li)?;
                    let result: Vec<u8> = interns.get_bytes(*b).repeat(count);
                    Ok(Some(Self::Ref(heap.allocate(HeapData::Bytes(result.into()))?)))
                } else {
                    Ok(None)
                }
            }

            _ => Ok(None),
        }
    }

    fn py_div(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<Value>> {
        match (self, other) {
            // True division always returns float
            (Self::Int(a), Self::Int(b)) => {
                if *b == 0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float(*a as f64 / *b as f64)))
                }
            }
            // Int / LongInt
            (Self::Int(a), Self::Ref(id)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    if li.is_zero() {
                        Err(ExcType::zero_division().into())
                    } else {
                        // Convert both to f64 for division
                        let a_f64 = *a as f64;
                        let b_f64 = li.to_f64().unwrap_or(f64::INFINITY);
                        Ok(Some(Self::Float(a_f64 / b_f64)))
                    }
                } else {
                    Ok(None)
                }
            }
            // LongInt / Int
            (Self::Ref(id), Self::Int(b)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    if *b == 0 {
                        Err(ExcType::zero_division().into())
                    } else {
                        // Convert both to f64 for division
                        let a_f64 = li.to_f64().unwrap_or(f64::INFINITY);
                        let b_f64 = *b as f64;
                        Ok(Some(Self::Float(a_f64 / b_f64)))
                    }
                } else {
                    Ok(None)
                }
            }
            // LongInt / LongInt or LongInt / Float or Float / LongInt
            (Self::Ref(id1), Self::Ref(id2)) => {
                let is_longint1 = matches!(heap.get(*id1), HeapData::LongInt(_));
                let is_longint2 = matches!(heap.get(*id2), HeapData::LongInt(_));
                if is_longint1 && is_longint2 {
                    // Check for zero division first
                    if matches!(heap.get(*id2), HeapData::LongInt(li) if li.is_zero()) {
                        return Err(ExcType::zero_division().into());
                    }
                    Ok(
                        heap.with_two(*id1, *id2, |_heap, left, right| -> RunResult<Option<Self>> {
                            if let (HeapData::LongInt(a), HeapData::LongInt(b)) = (left, right) {
                                let a_f64 = a.to_f64().unwrap_or(f64::INFINITY);
                                let b_f64 = b.to_f64().unwrap_or(f64::INFINITY);
                                Ok(Some(Self::Float(a_f64 / b_f64)))
                            } else {
                                Ok(None)
                            }
                        })?,
                    )
                } else {
                    Ok(None)
                }
            }
            // LongInt / Float
            (Self::Ref(id), Self::Float(b)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    if *b == 0.0 {
                        Err(ExcType::zero_division().into())
                    } else {
                        let a_f64 = li.to_f64().unwrap_or(f64::INFINITY);
                        Ok(Some(Self::Float(a_f64 / b)))
                    }
                } else {
                    Ok(None)
                }
            }
            // Float / LongInt
            (Self::Float(a), Self::Ref(id)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    if li.is_zero() {
                        Err(ExcType::zero_division().into())
                    } else {
                        let b_f64 = li.to_f64().unwrap_or(f64::INFINITY);
                        Ok(Some(Self::Float(a / b_f64)))
                    }
                } else {
                    Ok(None)
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
            _ => {
                // Check for Path / (str or Path) - path concatenation
                if let Self::Ref(id) = self
                    && matches!(heap.get(*id), HeapData::Path(_))
                {
                    return path::path_div(*id, other, heap, interns);
                }
                Ok(None)
            }
        }
    }

    fn py_floordiv(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Option<Value>> {
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
            // Int // LongInt
            (Self::Int(a), Self::Ref(id)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    if li.is_zero() {
                        Err(ExcType::zero_division().into())
                    } else {
                        let bi = BigInt::from(*a).div_floor(li.inner());
                        Ok(Some(LongInt::new(bi).into_value(heap)?))
                    }
                } else {
                    Ok(None)
                }
            }
            // LongInt // Int
            (Self::Ref(id), Self::Int(b)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    if *b == 0 {
                        Err(ExcType::zero_division().into())
                    } else {
                        let bi = li.inner().div_floor(&BigInt::from(*b));
                        Ok(Some(LongInt::new(bi).into_value(heap)?))
                    }
                } else {
                    Ok(None)
                }
            }
            // LongInt // LongInt
            (Self::Ref(id1), Self::Ref(id2)) => {
                let is_longint1 = matches!(heap.get(*id1), HeapData::LongInt(_));
                let is_longint2 = matches!(heap.get(*id2), HeapData::LongInt(_));
                if is_longint1 && is_longint2 {
                    // Check for zero division first
                    if matches!(heap.get(*id2), HeapData::LongInt(li) if li.is_zero()) {
                        return Err(ExcType::zero_division().into());
                    }
                    Ok(heap.with_two(*id1, *id2, |heap, left, right| {
                        if let (HeapData::LongInt(a), HeapData::LongInt(b)) = (left, right) {
                            let bi = a.inner().div_floor(b.inner());
                            LongInt::new(bi).into_value(heap).map(Some)
                        } else {
                            Ok(None)
                        }
                    })?)
                } else {
                    Ok(None)
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

    fn py_pow(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Option<Value>> {
        match (self, other) {
            (Self::Int(base), Self::Int(exp)) => {
                if *base == 0 && *exp < 0 {
                    Err(ExcType::zero_negative_power())
                } else if *exp >= 0 {
                    // Positive exponent: try to return int, promote to LongInt on overflow
                    if let Ok(exp_u32) = u32::try_from(*exp) {
                        if let Some(result) = base.checked_pow(exp_u32) {
                            Ok(Some(Self::Int(result)))
                        } else {
                            // Overflow - promote to LongInt
                            // Check size before computing to prevent DoS
                            check_pow_size(i64_bits(*base), u64::from(exp_u32), heap)?;
                            let bi = BigInt::from(*base).pow(exp_u32);
                            Ok(Some(LongInt::new(bi).into_value(heap)?))
                        }
                    } else {
                        // exp > u32::MAX - use BigInt with modpow-style exponentiation
                        // For very large exponents, we still need LongInt
                        // Safety: exp >= 0 is guaranteed by the outer if condition
                        #[expect(clippy::cast_sign_loss)]
                        let exp_u64 = *exp as u64;
                        // Check size before computing to prevent DoS
                        check_pow_size(i64_bits(*base), exp_u64, heap)?;
                        let bi = bigint_pow(BigInt::from(*base), exp_u64);
                        Ok(Some(LongInt::new(bi).into_value(heap)?))
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
            // LongInt ** Int
            (Self::Ref(id), Self::Int(exp)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    if li.is_zero() && *exp < 0 {
                        Err(ExcType::zero_negative_power())
                    } else if *exp >= 0 {
                        // Use BigInt pow for positive exponents
                        if let Ok(exp_u32) = u32::try_from(*exp) {
                            // Check size before computing to prevent DoS
                            check_pow_size(li.bits(), u64::from(exp_u32), heap)?;
                            let bi = li.inner().pow(exp_u32);
                            Ok(Some(LongInt::new(bi).into_value(heap)?))
                        } else {
                            // Safety: exp >= 0 is guaranteed by the outer if condition
                            #[expect(clippy::cast_sign_loss)]
                            let exp_u64 = *exp as u64;
                            // Check size before computing to prevent DoS
                            check_pow_size(li.bits(), exp_u64, heap)?;
                            let bi = bigint_pow(li.inner().clone(), exp_u64);
                            Ok(Some(LongInt::new(bi).into_value(heap)?))
                        }
                    } else {
                        // Negative exponent: return float (LongInt base becomes 0.0 for large values)
                        if let Some(base_f64) = li.to_f64() {
                            if let Ok(exp_i32) = i32::try_from(*exp) {
                                Ok(Some(Self::Float(base_f64.powi(exp_i32))))
                            } else {
                                Ok(Some(Self::Float(base_f64.powf(*exp as f64))))
                            }
                        } else {
                            // Base too large for f64, result approaches 0
                            Ok(Some(Self::Float(0.0)))
                        }
                    }
                } else {
                    Ok(None)
                }
            }
            // Int ** LongInt (only small positive exponents make sense)
            (Self::Int(base), Self::Ref(id)) => {
                if let HeapData::LongInt(li) = heap.get(*id) {
                    if *base == 0 && li.is_negative() {
                        Err(ExcType::zero_negative_power())
                    } else if !li.is_negative() {
                        // For very large exponents, most results are huge or 0/1
                        // Check for x ** 0 = 1 first (including 0 ** 0 = 1)
                        if li.is_zero() {
                            Ok(Some(Self::Int(1)))
                        } else if *base == 0 {
                            Ok(Some(Self::Int(0)))
                        } else if *base == 1 {
                            Ok(Some(Self::Int(1)))
                        } else if *base == -1 {
                            // (-1) ** n = 1 if n is even, -1 if n is odd
                            let is_even = (li.inner() % 2i32).is_zero();
                            Ok(Some(Self::Int(if is_even { 1 } else { -1 })))
                        } else if let Some(exp_u32) = li.to_u32() {
                            // Reasonable exponent size
                            if let Some(result) = base.checked_pow(exp_u32) {
                                Ok(Some(Self::Int(result)))
                            } else {
                                // Check size before computing to prevent DoS
                                check_pow_size(i64_bits(*base), u64::from(exp_u32), heap)?;
                                let bi = BigInt::from(*base).pow(exp_u32);
                                Ok(Some(LongInt::new(bi).into_value(heap)?))
                            }
                        } else {
                            // Exponent too large - result would be astronomically large
                            // Python handles this, but it would take forever. Use OverflowError
                            Err(SimpleException::new_msg(ExcType::OverflowError, "exponent too large").into())
                        }
                    } else {
                        // Negative LongInt exponent: return float
                        if let (Some(base_f64), Some(exp_f64)) = (Some(*base as f64), li.to_f64()) {
                            Ok(Some(Self::Float(base_f64.powf(exp_f64))))
                        } else {
                            Ok(Some(Self::Float(0.0)))
                        }
                    }
                } else {
                    Ok(None)
                }
            }
            (Self::Float(base), Self::Float(exp)) => {
                if *base == 0.0 && *exp < 0.0 {
                    Err(ExcType::zero_negative_power())
                } else {
                    Ok(Some(Self::Float(base.powf(*exp))))
                }
            }
            (Self::Int(base), Self::Float(exp)) => {
                if *base == 0 && *exp < 0.0 {
                    Err(ExcType::zero_negative_power())
                } else {
                    Ok(Some(Self::Float((*base as f64).powf(*exp))))
                }
            }
            (Self::Float(base), Self::Int(exp)) => {
                if *base == 0.0 && *exp < 0 {
                    Err(ExcType::zero_negative_power())
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
                    Err(ExcType::zero_negative_power())
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
                    Err(ExcType::zero_negative_power())
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
            Self::InternString(string_id) => {
                // Check for slice first
                if let Self::Ref(key_id) = key
                    && let HeapData::Slice(slice_obj) = heap.get(*key_id)
                {
                    let s = interns.get_str(*string_id);
                    let char_count = s.chars().count();
                    let (start, stop, step) = slice_obj
                        .indices(char_count)
                        .map_err(|()| ExcType::value_error_slice_step_zero())?;
                    let result_str = get_str_slice(s, start, stop, step);
                    let heap_id = heap.allocate(HeapData::Str(Str::from(result_str)))?;
                    return Ok(Self::Ref(heap_id));
                }

                // Handle interned string indexing, accepting Int and Bool
                let index = match key {
                    Self::Int(i) => *i,
                    Self::Bool(b) => i64::from(*b),
                    _ => return Err(ExcType::type_error_indices(Type::Str, key.py_type(heap))),
                };

                let s = interns.get_str(*string_id);
                let c = get_char_at_index(s, index).ok_or_else(ExcType::str_index_error)?;
                Ok(allocate_char(c, heap)?)
            }
            Self::InternBytes(bytes_id) => {
                // Check for slice first
                if let Self::Ref(key_id) = key
                    && let HeapData::Slice(slice_obj) = heap.get(*key_id)
                {
                    let bytes = interns.get_bytes(*bytes_id);
                    let (start, stop, step) = slice_obj
                        .indices(bytes.len())
                        .map_err(|()| ExcType::value_error_slice_step_zero())?;
                    let result_bytes = get_bytes_slice(bytes, start, stop, step);
                    let heap_id = heap.allocate(HeapData::Bytes(crate::types::Bytes::new(result_bytes)))?;
                    return Ok(Self::Ref(heap_id));
                }

                // Handle interned bytes indexing - returns integer byte value
                let index = match key {
                    Self::Int(i) => *i,
                    Self::Bool(b) => i64::from(*b),
                    _ => return Err(ExcType::type_error_indices(Type::Bytes, key.py_type(heap))),
                };

                let bytes = interns.get_bytes(*bytes_id);
                let byte = get_byte_at_index(bytes, index).ok_or_else(ExcType::bytes_index_error)?;
                Ok(Self::Int(i64::from(byte)))
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
            _ => Err(ExcType::type_error(format!(
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
            // Interned strings/bytes/bigints use their index directly - the index is the stable identifier
            Self::InternString(string_id) => INTERN_STR_ID_TAG | (string_id.index() & INTERN_STR_ID_MASK),
            Self::InternBytes(bytes_id) => INTERN_BYTES_ID_TAG | (bytes_id.index() & INTERN_BYTES_ID_MASK),
            Self::InternLongInt(long_int_id) => {
                INTERN_LONG_INT_ID_TAG | (long_int_id.index() & INTERN_LONG_INT_ID_MASK)
            }
            // Already heap-allocated (includes Range and Exception), return id within a dedicated tag range
            Self::Ref(id) => heap_tagged_id(*id),
            // Value-based IDs for immediate types (no heap allocation!)
            Self::Int(v) => int_value_id(*v),
            Self::Float(v) => float_value_id(*v),
            Self::Builtin(c) => builtin_value_id(*c),
            Self::ModuleFunction(mf) => module_function_value_id(*mf),
            Self::DefFunction(f_id) => function_value_id(*f_id),
            Self::ExtFunction(f_id) => ext_function_value_id(*f_id),
            // Markers get deterministic IDs based on discriminant
            Self::Marker(m) => marker_value_id(*m),
            // Properties get deterministic IDs based on discriminant
            Self::Property(p) => property_value_id(*p),
            // ExternalFutures get IDs based on their call_id
            Self::ExternalFuture(call_id) => external_future_value_id(*call_id),
            #[cfg(feature = "ref-count-panic")]
            Self::Dereferenced => panic!("Cannot get id of Dereferenced object"),
        }
    }

    /// Returns the Ref ID if this value is a reference, otherwise returns None.
    pub fn ref_id(&self) -> Option<HeapId> {
        match self {
            Self::Ref(id) => Some(*id),
            _ => None,
        }
    }

    /// Returns the module name if this value is a module, otherwise returns "<unknown>".
    ///
    /// Used for error messages in `from module import name` when the name doesn't exist.
    pub fn module_name(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> String {
        match self {
            Self::Ref(id) => match heap.get(*id) {
                HeapData::Module(module) => interns.get_str(module.name()).to_string(),
                _ => "<unknown>".to_string(),
            },
            _ => "<unknown>".to_string(),
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
        // strings bytes bigints and heap allocated values have their own hashing logic
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
            // Hash BigInt consistently with LongInt (using sign and bytes for large values)
            Self::InternLongInt(long_int_id) => {
                let bi = interns.get_long_int(*long_int_id);
                let mut hasher = DefaultHasher::new();
                let (sign, bytes) = bi.to_bytes_le();
                sign.hash(&mut hasher);
                bytes.hash(&mut hasher);
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
            Self::ModuleFunction(mf) => mf.hash(&mut hasher),
            // Hash functions based on function ID
            Self::DefFunction(f_id) => f_id.hash(&mut hasher),
            Self::ExtFunction(f_id) => f_id.hash(&mut hasher),
            // Markers are hashable based on their discriminant (already included above)
            Self::Marker(m) => m.hash(&mut hasher),
            // Properties are hashable based on their OS function discriminant
            Self::Property(p) => p.hash(&mut hasher),
            // ExternalFutures are hashable based on their call ID
            Self::ExternalFuture(call_id) => call_id.raw().hash(&mut hasher),
            Self::InternString(_) | Self::InternBytes(_) | Self::InternLongInt(_) | Self::Ref(_) => {
                unreachable!("covered above")
            }
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
                    HeapData::Range(range) => {
                        // Range containment is O(1) - check bounds and step alignment
                        let n = match item {
                            Self::Int(i) => *i,
                            Self::Bool(b) => i64::from(*b),
                            Self::Float(f) => {
                                // Floats are contained if they equal an integer in the range
                                // e.g., 3.0 in range(5) is True, but 3.5 in range(5) is False
                                if f.fract() != 0.0 {
                                    return Ok(false);
                                }
                                // Check if float is within i64 range and convert safely
                                // f64 can represent integers up to 2^53 exactly
                                let int_val = f.trunc();
                                if int_val < i64::MIN as f64 || int_val > i64::MAX as f64 {
                                    return Ok(false);
                                }
                                // Safe conversion: we've verified it's a whole number in i64 range
                                #[expect(clippy::cast_possible_truncation)]
                                let n = int_val as i64;
                                n
                            }
                            _ => return Ok(false),
                        };
                        Ok(range.contains(n))
                    }
                    other => {
                        let type_name = other.py_type(heap);
                        Err(ExcType::type_error(format!(
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
                Err(ExcType::type_error(format!(
                    "argument of type '{type_name}' is not iterable"
                )))
            }
        }
    }

    /// Gets an attribute from this value.
    ///
    /// Dispatches to `py_getattr` on the underlying types where appropriate.
    ///
    /// Returns `AttributeError` for other types or unknown attributes.
    pub fn py_getattr(
        &self,
        name_id: StringId,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<AttrCallResult> {
        match self {
            Self::Ref(heap_id) => {
                // Use with_entry_mut to get access to both data and heap without borrow conflicts.
                // This allows py_getattr to allocate (for computed attributes) while we hold the data.
                let opt_result = heap.with_entry_mut(*heap_id, |heap, data| data.py_getattr(name_id, heap, interns))?;
                if let Some(call_result) = opt_result {
                    return Ok(call_result);
                }
            }
            Self::Builtin(Builtins::Type(t)) => {
                // Handle type object attributes like __name__
                if name_id == StaticStrings::DunderName {
                    let name_str = t.to_string();
                    let str_id = heap.allocate(HeapData::Str(Str::from(name_str)))?;
                    return Ok(AttrCallResult::Value(Self::Ref(str_id)));
                }
            }
            _ => {}
        }
        let type_name = self.py_type(heap);
        Err(ExcType::attribute_error(type_name, interns.get_str(name_id)))
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

    /// Extracts an integer value from the Value.
    ///
    /// Accepts `Int` and `LongInt` (if it fits in i64). Returns a `TypeError` for other types
    /// and an `OverflowError` if the `LongInt` value is too large.
    ///
    /// Note: The LongInt-to-i64 conversion path is defensive code. In normal execution,
    /// heap-allocated `LongInt` values always exceed i64 range because `LongInt::into_value()`
    /// automatically demotes i64-fitting values to `Value::Int`. However, this path could be
    /// reached via deserialization of crafted snapshot data.
    pub fn as_int(&self, heap: &Heap<impl ResourceTracker>) -> RunResult<i64> {
        match self {
            Self::Int(i) => Ok(*i),
            Self::Ref(heap_id) => {
                if let HeapData::LongInt(li) = heap.get(*heap_id) {
                    li.to_i64().ok_or_else(ExcType::overflow_shift_count)
                } else {
                    let msg = format!("'{}' object cannot be interpreted as an integer", self.py_type(heap));
                    Err(SimpleException::new_msg(ExcType::TypeError, msg).into())
                }
            }
            _ => {
                let msg = format!("'{}' object cannot be interpreted as an integer", self.py_type(heap));
                Err(SimpleException::new_msg(ExcType::TypeError, msg).into())
            }
        }
    }

    /// Extracts an index value for sequence operations.
    ///
    /// Accepts `Int`, `Bool` (True=1, False=0), and `LongInt` (if it fits in i64).
    /// Returns a `TypeError` for other types with the container type name included.
    /// Returns an `IndexError` if the `LongInt` value is too large to use as an index.
    ///
    /// Note: The LongInt-to-i64 conversion path is defensive code. In normal execution,
    /// heap-allocated `LongInt` values always exceed i64 range because `LongInt::into_value()`
    /// automatically demotes i64-fitting values to `Value::Int`. However, this path could be
    /// reached via deserialization of crafted snapshot data.
    pub fn as_index(&self, heap: &Heap<impl ResourceTracker>, container_type: Type) -> RunResult<i64> {
        match self {
            Self::Int(i) => Ok(*i),
            Self::Bool(b) => Ok(i64::from(*b)),
            Self::Ref(heap_id) => {
                if let HeapData::LongInt(li) = heap.get(*heap_id) {
                    li.to_i64().ok_or_else(ExcType::index_error_int_too_large)
                } else {
                    Err(ExcType::type_error_indices(container_type, self.py_type(heap)))
                }
            }
            _ => Err(ExcType::type_error_indices(container_type, self.py_type(heap))),
        }
    }

    /// Performs a binary bitwise operation on two values.
    ///
    /// Python only supports bitwise operations on integers (and bools, which coerce to int).
    /// Returns a `TypeError` if either operand is not an integer, bool, or LongInt.
    ///
    /// For shift operations:
    /// - Negative shift counts raise `ValueError`
    /// - Left shifts may produce LongInt results for large shifts
    /// - Right shifts with large counts return 0 (or -1 for negative numbers)
    pub fn py_bitwise(
        &self,
        other: &Self,
        op: BitwiseOp,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<Self, RunError> {
        // Capture types for error messages
        let lhs_type = self.py_type(heap);
        let rhs_type = other.py_type(heap);

        // Extract BigInt from all numeric types
        let lhs_bigint = extract_bigint(self, heap);
        let rhs_bigint = extract_bigint(other, heap);

        if let (Some(l), Some(r)) = (lhs_bigint, rhs_bigint) {
            let result = match op {
                BitwiseOp::And => l & r,
                BitwiseOp::Or => l | r,
                BitwiseOp::Xor => l ^ r,
                BitwiseOp::LShift => {
                    // Get shift amount as i64 for validation
                    let shift_amount = r.to_i64();
                    if let Some(shift) = shift_amount {
                        if shift < 0 {
                            return Err(ExcType::value_error_negative_shift_count());
                        }
                        // Python allows arbitrarily large left shifts - use BigInt's shift
                        // Safety: shift >= 0 is guaranteed by the check above
                        #[expect(clippy::cast_sign_loss)]
                        let shift_u64 = shift as u64;
                        // Check size before computing to prevent DoS
                        // Skip check if value is 0 - result is always 0 regardless of shift
                        let value_bits = l.bits();
                        if value_bits > 0
                            && let Some(estimated) = LongInt::estimate_lshift_bytes(value_bits, shift_u64)
                            && estimated > LARGE_RESULT_THRESHOLD
                        {
                            heap.tracker().check_large_result(estimated)?;
                        }
                        l << shift_u64
                    } else if r.sign() == num_bigint::Sign::Minus {
                        return Err(ExcType::value_error_negative_shift_count());
                    } else {
                        // Shift amount too large to fit in i64 - this would be astronomically large
                        return Err(ExcType::overflow_shift_count());
                    }
                }
                BitwiseOp::RShift => {
                    // Get shift amount as i64 for validation
                    let shift_amount = r.to_i64();
                    if let Some(shift) = shift_amount {
                        if shift < 0 {
                            return Err(ExcType::value_error_negative_shift_count());
                        }
                        // Safety: shift >= 0 is guaranteed by the check above
                        #[expect(clippy::cast_sign_loss)]
                        let shift_u64 = shift as u64;
                        l >> shift_u64
                    } else if r.sign() == num_bigint::Sign::Minus {
                        return Err(ExcType::value_error_negative_shift_count());
                    } else {
                        // Shift amount too large - result is 0 or -1 depending on sign
                        if l.sign() == num_bigint::Sign::Minus {
                            BigInt::from(-1)
                        } else {
                            BigInt::from(0)
                        }
                    }
                }
            };
            // Convert result back to Value, demoting to i64 if it fits
            LongInt::new(result).into_value(heap).map_err(Into::into)
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
    #[cfg(not(feature = "ref-count-panic"))]
    #[inline]
    pub fn drop_with_heap(self, heap: &mut Heap<impl ResourceTracker>) {
        if let Self::Ref(id) = self {
            heap.dec_ref(id);
        }
    }
    /// With `ref-count-panic` enabled, `Ref` variants are replaced with `Dereferenced` and
    /// the original is forgotten to prevent the Drop impl from panicking. Non-Ref variants
    /// are left unchanged since they don't trigger the Drop panic.
    #[cfg(feature = "ref-count-panic")]
    pub fn drop_with_heap(mut self, heap: &mut Heap<impl ResourceTracker>) {
        let old = std::mem::replace(&mut self, Self::Dereferenced);
        if let Self::Ref(id) = &old {
            heap.dec_ref(*id);
            std::mem::forget(old);
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
            Self::ModuleFunction(mf) => Self::ModuleFunction(*mf),
            Self::DefFunction(f) => Self::DefFunction(*f),
            Self::ExtFunction(f) => Self::ExtFunction(*f),
            Self::InternString(s) => Self::InternString(*s),
            Self::InternBytes(b) => Self::InternBytes(*b),
            Self::InternLongInt(bi) => Self::InternLongInt(*bi),
            Self::Marker(m) => Self::Marker(*m),
            Self::Property(p) => Self::Property(*p),
            Self::ExternalFuture(call_id) => Self::ExternalFuture(*call_id),
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
    pub fn as_either_str(&self, heap: &Heap<impl ResourceTracker>) -> Option<EitherStr> {
        match self {
            Self::InternString(id) => Some(EitherStr::Interned(*id)),
            Self::Ref(heap_id) => match heap.get(*heap_id) {
                HeapData::Str(s) => Some(EitherStr::Heap(s.as_str().to_owned())),
                _ => None,
            },
            _ => None,
        }
    }

    /// check if the value is a string.
    pub fn is_str(&self, heap: &Heap<impl ResourceTracker>) -> bool {
        match self {
            Self::InternString(_) => true,
            Self::Ref(heap_id) => matches!(heap.get(*heap_id), HeapData::Str(_)),
            _ => false,
        }
    }
}

/// Interned or heap-owned string identifier.
///
/// Used when a string value can come from either the intern table (for known
/// static strings and keywords) or from a heap-allocated Python string object.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum EitherStr {
    /// Interned string identifier (cheap comparisons and no allocation).
    Interned(StringId),
    /// Heap-owned string extracted from a `str` object.
    Heap(String),
}

impl From<StringId> for EitherStr {
    fn from(id: StringId) -> Self {
        Self::Interned(id)
    }
}

impl From<StaticStrings> for EitherStr {
    fn from(s: StaticStrings) -> Self {
        Self::Interned(s.into())
    }
}

/// Convert String to EitherStr: use Interned for known static strings,
/// otherwise use Heap for user-defined field names.
impl From<String> for EitherStr {
    fn from(s: String) -> Self {
        match StaticStrings::from_str(&s) {
            Ok(s) => s.into(),
            Err(_) => Self::Heap(s),
        }
    }
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

    /// Returns the `StringId` if this is an interned attribute.
    #[inline]
    pub fn string_id(&self) -> Option<StringId> {
        match self {
            Self::Interned(id) => Some(*id),
            Self::Heap(_) => None,
        }
    }

    /// Returns the `StaticStrings` if this is an interned attribute from `StaticStrings`s.
    #[inline]
    pub fn static_string(&self) -> Option<StaticStrings> {
        match self {
            Self::Interned(id) => StaticStrings::from_string_id(*id),
            Self::Heap(_) => None,
        }
    }

    pub fn py_estimate_size(&self) -> usize {
        match self {
            Self::Interned(_) => 0,
            Self::Heap(s) => s.capacity(),
        }
    }
}

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

/// Marker values for special objects that exist but have minimal functionality.
///
/// These are used for:
/// - System objects like `sys.stdout` and `sys.stderr` that need to exist but don't
///   provide functionality in the sandboxed environment
/// - Typing constructs from the `typing` module that are imported for type hints but
///   don't need runtime functionality
///
/// Wraps a `StaticStrings` variant to leverage its string conversion capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct Marker(pub StaticStrings);

impl Marker {
    /// Returns the Python type of this marker.
    ///
    /// System markers (stdout, stderr) are `TextIOWrapper`.
    /// `typing.Union` has type `type` (matching CPython).
    /// Other typing markers (Any, Optional, etc.) are `_SpecialForm`.
    pub(crate) fn py_type(self) -> Type {
        match self.0 {
            StaticStrings::Stdout | StaticStrings::Stderr => Type::TextIOWrapper,
            StaticStrings::UnionType => Type::Type,
            _ => Type::SpecialForm,
        }
    }

    /// Writes the Python repr for this marker.
    ///
    /// System markers have special repr formats ("<stdout>", "<stderr>").
    /// `typing.Union` uses `<class 'typing.Union'>` format (matching CPython).
    /// Other typing markers are prefixed with "typing." (e.g., "typing.Any").
    fn py_repr_fmt(self, f: &mut impl Write) -> fmt::Result {
        let s: &'static str = self.0.into();
        match self.0 {
            StaticStrings::Stdout => f.write_str("<stdout>")?,
            StaticStrings::Stderr => f.write_str("<stderr>")?,
            StaticStrings::UnionType => f.write_str("<class 'typing.Union'>")?,
            _ => write!(f, "typing.{s}")?,
        }
        Ok(())
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
/// High-bit tag for Marker value-based IDs (stdout, stderr, etc.).
const MARKER_ID_TAG: usize = 1usize << (usize::BITS - 10);
/// High-bit tag for ExternalFuture value-based IDs.
const EXTERNAL_FUTURE_ID_TAG: usize = 1usize << (usize::BITS - 11);
/// High-bit tag for ModuleFunction value-based IDs.
const MODULE_FUNCTION_ID_TAG: usize = 1usize << (usize::BITS - 12);
/// High-bit tag for interned LongInt `id()` values.
const INTERN_LONG_INT_ID_TAG: usize = 1usize << (usize::BITS - 13);
/// High-bit tag for Property value-based IDs.
const PROPERTY_ID_TAG: usize = 1usize << (usize::BITS - 14);

/// Masks for value-based ID tags (keep bits below the tag bit).
const INT_ID_MASK: usize = INT_ID_TAG - 1;
const FLOAT_ID_MASK: usize = FLOAT_ID_TAG - 1;
const BUILTIN_ID_MASK: usize = BUILTIN_ID_TAG - 1;
const FUNCTION_ID_MASK: usize = FUNCTION_ID_TAG - 1;
const EXTFUNCTION_ID_MASK: usize = EXTFUNCTION_ID_TAG - 1;
const MARKER_ID_MASK: usize = MARKER_ID_TAG - 1;
const EXTERNAL_FUTURE_ID_MASK: usize = EXTERNAL_FUTURE_ID_TAG - 1;
const MODULE_FUNCTION_ID_MASK: usize = MODULE_FUNCTION_ID_TAG - 1;
const INTERN_LONG_INT_ID_MASK: usize = INTERN_LONG_INT_ID_TAG - 1;
const PROPERTY_ID_MASK: usize = PROPERTY_ID_TAG - 1;

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
    b.hash(&mut hasher);
    let hash_u64 = hasher.finish();
    // wrapping here is fine
    #[expect(clippy::cast_possible_truncation)]
    let hash_usize = hash_u64 as usize;
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

/// Computes a deterministic ID for a marker value based on its discriminant.
#[inline]
fn marker_value_id(m: Marker) -> usize {
    MARKER_ID_TAG | ((m.0 as usize) & MARKER_ID_MASK)
}

/// Computes a deterministic ID for a property value based on its discriminant.
#[inline]
fn property_value_id(p: Property) -> usize {
    let discriminant = match p {
        Property::Os(os_fn) => os_fn as usize,
    };
    PROPERTY_ID_TAG | (discriminant & PROPERTY_ID_MASK)
}

/// Computes a deterministic ID for an external future based on its call ID.
#[inline]
fn external_future_value_id(call_id: CallId) -> usize {
    EXTERNAL_FUTURE_ID_TAG | ((call_id.raw() as usize) & EXTERNAL_FUTURE_ID_MASK)
}

/// Computes a deterministic ID for a module function based on its discriminant.
#[inline]
fn module_function_value_id(mf: ModuleFunctions) -> usize {
    let mut hasher = DefaultHasher::new();
    mf.hash(&mut hasher);
    let hash_u64 = hasher.finish();
    // wrapping here is fine
    #[expect(clippy::cast_possible_truncation)]
    let hash_usize = hash_u64 as usize;
    MODULE_FUNCTION_ID_TAG | (hash_usize & MODULE_FUNCTION_ID_MASK)
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

/// Converts a LongInt repeat count to usize, handling negative values and overflow.
///
/// Returns 0 for negative values (Python treats negative repeat counts as 0).
/// Returns `OverflowError` if the value exceeds `usize::MAX`.
#[inline]
fn longint_to_repeat_count(li: &LongInt) -> RunResult<usize> {
    if li.is_negative() {
        Ok(0)
    } else if let Some(count) = li.to_usize() {
        Ok(count)
    } else {
        Err(ExcType::overflow_repeat_count().into())
    }
}

/// Extracts a BigInt from a Value for bitwise operations.
///
/// Returns `Some(BigInt)` for Int, Bool, and LongInt values.
/// Returns `None` for other types (Float, Str, etc.).
fn extract_bigint(value: &Value, heap: &Heap<impl ResourceTracker>) -> Option<BigInt> {
    match value {
        Value::Int(i) => Some(BigInt::from(*i)),
        Value::Bool(b) => Some(BigInt::from(i64::from(*b))),
        Value::Ref(id) => {
            if let HeapData::LongInt(li) = heap.get(*id) {
                Some(li.inner().clone())
            } else {
                None
            }
        }
        _ => None,
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

/// Computes the number of significant bits in an i64.
///
/// Returns 0 for 0, otherwise returns ceil(log2(|value|)) + 1 (accounting for sign).
/// For example: 0 -> 0, 1 -> 1, 2 -> 2, 255 -> 8, 256 -> 9.
fn i64_bits(value: i64) -> u64 {
    if value == 0 {
        0
    } else {
        // For negative numbers, use unsigned_abs to get magnitude
        u64::from(64 - value.unsigned_abs().leading_zeros())
    }
}

/// Checks if a pow operation result would exceed the large result threshold.
///
/// If the estimated result is larger than `LARGE_RESULT_THRESHOLD`, calls
/// `heap.tracker().check_large_result()` to allow the tracker to reject the operation.
/// Returns `Ok(())` if the operation should proceed, or an error to reject.
fn check_pow_size(base_bits: u64, exp: u64, heap: &Heap<impl ResourceTracker>) -> Result<(), RunError> {
    // Special case: 0 or 1 bit bases can't produce large results worth checking
    // (0**n = 0, 1**n = 1, -1**n = ±1)
    if base_bits <= 1 {
        return Ok(());
    }

    if let Some(estimated) = LongInt::estimate_pow_bytes(base_bits, exp)
        && estimated > LARGE_RESULT_THRESHOLD
    {
        heap.tracker().check_large_result(estimated)?;
    }
    // If estimate overflows, proceed anyway - into_value will catch it during allocation
    Ok(())
}

/// Computes BigInt exponentiation for exponents larger than u32::MAX.
///
/// Uses repeated squaring for efficiency. This is needed when the exponent
/// doesn't fit in a u32, which is required by the `num-bigint` pow method.
fn bigint_pow(base: BigInt, exp: u64) -> BigInt {
    if exp == 0 {
        return BigInt::from(1);
    }
    if exp == 1 {
        return base;
    }

    // Use repeated squaring
    let mut result = BigInt::from(1);
    let mut b = base;
    let mut e = exp;

    while e > 0 {
        if e & 1 == 1 {
            result *= &b;
        }
        b = &b * &b;
        e >>= 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use num_bigint::BigInt;

    use super::*;
    use crate::resource::NoLimitTracker;

    /// Creates a heap and directly allocates a LongInt with the given BigInt value.
    ///
    /// This bypasses `LongInt::into_value()` which would demote i64-fitting values.
    /// Used to test defensive code paths that handle LongInt-as-index scenarios.
    fn create_heap_with_longint(value: BigInt) -> (Heap<NoLimitTracker>, HeapId) {
        let mut heap = Heap::new(16, NoLimitTracker);
        let long_int = LongInt::new(value);
        let heap_id = heap.allocate(HeapData::LongInt(long_int)).unwrap();
        (heap, heap_id)
    }

    /// Tests that `as_index()` correctly handles a LongInt containing an i64-fitting value.
    ///
    /// This tests a defensive code path that's normally unreachable because
    /// `LongInt::into_value()` demotes i64-fitting values to `Value::Int`.
    /// However, this path could be reached via deserialization of crafted data.
    #[test]
    fn as_index_longint_fits_in_i64() {
        let (mut heap, heap_id) = create_heap_with_longint(BigInt::from(42));
        let value = Value::Ref(heap_id);

        let result = value.as_index(&heap, Type::List);
        assert_eq!(result.unwrap(), 42);
        value.drop_with_heap(&mut heap);
    }

    /// Tests that `as_index()` correctly handles a negative LongInt that fits in i64.
    #[test]
    fn as_index_longint_negative_fits_in_i64() {
        let (mut heap, heap_id) = create_heap_with_longint(BigInt::from(-100));
        let value = Value::Ref(heap_id);

        let result = value.as_index(&heap, Type::List);
        assert_eq!(result.unwrap(), -100);
        value.drop_with_heap(&mut heap);
    }

    /// Tests that `as_index()` returns IndexError for LongInt values too large for i64.
    #[test]
    fn as_index_longint_too_large() {
        // 2^100 is way larger than i64::MAX
        let big_value = BigInt::from(2).pow(100);
        let (mut heap, heap_id) = create_heap_with_longint(big_value);
        let value = Value::Ref(heap_id);

        let result = value.as_index(&heap, Type::List);
        assert!(result.is_err());
        value.drop_with_heap(&mut heap);
    }

    /// Tests that `as_int()` correctly handles a LongInt containing an i64-fitting value.
    ///
    /// Similar to `as_index`, this tests a defensive code path normally unreachable.
    #[test]
    fn as_int_longint_fits_in_i64() {
        let (mut heap, heap_id) = create_heap_with_longint(BigInt::from(12345));
        let value = Value::Ref(heap_id);

        let result = value.as_int(&heap);
        assert_eq!(result.unwrap(), 12345);
        value.drop_with_heap(&mut heap);
    }

    /// Tests that `as_int()` returns an error for LongInt values too large for i64.
    #[test]
    fn as_int_longint_too_large() {
        let big_value = BigInt::from(2).pow(100);
        let (mut heap, heap_id) = create_heap_with_longint(big_value);
        let value = Value::Ref(heap_id);

        let result = value.as_int(&heap);
        assert!(result.is_err());
        value.drop_with_heap(&mut heap);
    }

    /// Tests boundary values: i64::MAX as a LongInt.
    #[test]
    fn as_index_longint_at_i64_max() {
        let (mut heap, heap_id) = create_heap_with_longint(BigInt::from(i64::MAX));
        let value = Value::Ref(heap_id);

        let result = value.as_index(&heap, Type::List);
        assert_eq!(result.unwrap(), i64::MAX);
        value.drop_with_heap(&mut heap);
    }

    /// Tests boundary values: i64::MIN as a LongInt.
    #[test]
    fn as_index_longint_at_i64_min() {
        let (mut heap, heap_id) = create_heap_with_longint(BigInt::from(i64::MIN));
        let value = Value::Ref(heap_id);

        let result = value.as_index(&heap, Type::List);
        assert_eq!(result.unwrap(), i64::MIN);
        value.drop_with_heap(&mut heap);
    }

    /// Tests boundary values: i64::MAX + 1 as a LongInt (should fail).
    #[test]
    fn as_index_longint_just_over_i64_max() {
        let big_value = BigInt::from(i64::MAX) + BigInt::from(1);
        let (mut heap, heap_id) = create_heap_with_longint(big_value);
        let value = Value::Ref(heap_id);

        let result = value.as_index(&heap, Type::List);
        assert!(result.is_err());
        value.drop_with_heap(&mut heap);
    }

    /// Tests boundary values: i64::MIN - 1 as a LongInt (should fail).
    #[test]
    fn as_index_longint_just_under_i64_min() {
        let big_value = BigInt::from(i64::MIN) - BigInt::from(1);
        let (mut heap, heap_id) = create_heap_with_longint(big_value);
        let value = Value::Ref(heap_id);

        let result = value.as_index(&heap, Type::List);
        assert!(result.is_err());
        value.drop_with_heap(&mut heap);
    }
}
