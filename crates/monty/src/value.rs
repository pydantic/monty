use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::hash_map::DefaultHasher,
    fmt::{self, Write},
    hash::{Hash, Hasher},
    mem::{self, discriminant},
    str::FromStr,
};

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{FromPrimitive, ToPrimitive, Zero};
use smallvec::SmallVec;

use crate::{
    builtins::Builtins,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    fstring::FormatFloat,
    hash::{HashValue, hash_one, hash_python_long_int, hash_python_str},
    heap::{ContainsHeap, DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapReadOutput},
    intern::{BytesId, FunctionId, Interns, LongIntId, StaticStrings, StringId},
    modules::ModuleFunctions,
    resource::{
        ResourceError, ResourceTracker, check_div_size, check_lshift_size, check_mult_size, check_pow_size,
        check_repeat_size,
    },
    types::{
        Bytes, CmpOrder, LazyHeapSet, List, LongInt, Property, PyTrait, Type, allocate_tuple,
        bytes::{bytes_repr_fmt, get_byte_at_index},
        instance::{instance_getattr, instance_repr, instance_str},
        long_int::{
            bigint_cmp_f64, bigint_cmp_i64, bigint_eq_f64, bigint_eq_i64, check_bits_str_digits_limit, i64_cmp_f64,
        },
        path,
        slice::slice_collect_iterator,
        str::{allocate_char, allocate_string, concat_allocate_str, get_char_at_index, string_repr_fmt},
        timedelta,
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
    /// Reference to an external function defined on the host.
    ///
    /// The `StringId` stores the interned function name. When called, the VM yields
    /// a `FrameExit::ExternalCall` with this `StringId` so the host can look up and
    /// execute the function by name.
    ExtFunction(StringId),
    /// A marker value representing special objects like sys.stdout/stderr.
    /// These exist but have minimal functionality in the sandboxed environment.
    Marker(Marker),
    /// A property descriptor that computes its value when accessed.
    /// When retrieved via `py_getattr`, the property's getter is invoked.
    Property(Property),

    // Heap-allocated values (stored in arena)
    Ref(HeapId),

    /// Sentinel value indicating this Value was properly cleaned up via `drop_with`.
    /// Only exists when `memory-model-checks` feature is enabled. Used to verify reference counting
    /// correctness - if a `Ref` variant is dropped without calling `drop_with`, the
    /// Drop impl will panic.
    #[cfg(feature = "memory-model-checks")]
    Dereferenced,
}

/// Size of a single `Value` slot in bytes.
///
/// Used for memory tracking when containers grow (e.g., `list.append`, `list.extend`).
/// Must match the per-element unit used by `py_estimate_size` implementations.
pub(crate) const VALUE_SIZE: usize = mem::size_of::<Value>();

/// Drop implementation that panics if a `Ref` variant is dropped without calling `drop_with`.
/// This helps catch reference counting bugs during development/testing.
/// Only enabled when the `memory-model-checks` feature is active.
#[cfg(feature = "memory-model-checks")]
impl Drop for Value {
    fn drop(&mut self) {
        if let Self::Ref(id) = self {
            panic!("Value::Ref({id:?}) dropped without calling drop_with() - this is a reference counting bug");
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl<'h> PyTrait<'h> for Value {
    fn py_type(&self, vm: &VM<'_, impl ResourceTracker>) -> Type {
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
            Self::Ref(id) => vm.heap.read(*id).py_type(vm),
            #[cfg(feature = "memory-model-checks")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    fn py_len(&self, vm: &VM<'_, impl ResourceTracker>) -> Option<usize> {
        match self {
            // Count Unicode characters, not bytes, to match Python semantics
            Self::InternString(string_id) => Some(vm.interns.get_str(*string_id).chars().count()),
            Self::InternBytes(bytes_id) => Some(vm.interns.get_bytes(*bytes_id).len()),
            Self::Ref(id) => vm.heap.read(*id).py_len(vm),
            _ => None,
        }
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<bool>> {
        match self {
            // `Undefined` is a sentinel and is never equal to anything.
            Self::Undefined => Ok(Some(false)),

            Self::None => Ok(matches!(other, Self::None).then_some(true)),
            Self::Ellipsis => Ok(matches!(other, Self::Ellipsis).then_some(true)),
            Self::Bool(b) => Ok(eq_i64(i64::from(*b), other, vm)),
            Self::Int(a) => Ok(eq_i64(*a, other, vm)),
            Self::Float(f) => Ok(eq_f64(*f, other, vm)),
            // `InternLongInt` is normally materialised to a heap `LongInt` before
            // it can be compared, but handle it directly so equality never
            // silently diverges if one reaches here.
            Self::InternLongInt(id) => Ok(eq_bigint(vm.interns.get_long_int(*id), other, vm)),
            Self::InternString(id) => Ok(match other {
                // Interned strings are deduplicated, so equal ids ⇔ equal content.
                Self::InternString(o) => Some(id == o),
                _ => eq_str(vm.interns.get_str(*id), other, vm),
            }),
            Self::InternBytes(id) => Ok(match other {
                // Fast path for the same interned bytes; otherwise compare content
                // (interned bytes are not deduplicated, unlike strings).
                Self::InternBytes(o) if id == o => Some(true),
                _ => eq_bytes(vm.interns.get_bytes(*id), other, vm),
            }),
            Self::Builtin(b) => Ok(match other {
                Self::Builtin(o) => Some(b == o),
                _ => None,
            }),
            Self::ModuleFunction(mf) => Ok(match other {
                Self::ModuleFunction(o) => Some(mf == o),
                _ => None,
            }),
            Self::DefFunction(f) => Ok(match other {
                Self::DefFunction(o) => Some(f == o),
                _ => None,
            }),
            // External function equality is name-based across both the inline
            // `Value::ExtFunction(StringId)` and heap `HeapData::ExtFunction(String)`
            // representations. (#347)
            Self::ExtFunction(name_id) => Ok(eq_ext_function(vm.interns.get_str(*name_id), other, vm)),
            Self::Marker(m) => Ok(match other {
                Self::Marker(o) => Some(m == o),
                _ => None,
            }),
            Self::Property(p) => Ok(match other {
                Self::Property(o) => Some(p == o),
                _ => None,
            }),
            Self::Ref(id) => {
                // Identity short-circuit: a heap object always equals itself.
                if let Self::Ref(other_id) = other
                    && id == other_id
                {
                    Ok(Some(true))
                } else {
                    vm.heap.read(*id).py_eq_impl(other, vm)
                }
            }
            #[cfg(feature = "memory-model-checks")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    fn py_cmp(&self, other: &Self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<CmpOrder> {
        let interns = vm.interns;
        // py_cmp handles numbers, strings, bytes, tuples, and lists.
        // Recursion depth tracking for tuples/lists is handled by their iterators.
        //
        // `from_numeric` maps a `None` from a numeric helper to
        // `CmpOrder::Unordered` (only `NaN` yields `None` there), while
        // `from_total` maps `None` from a total-order comparison to
        // `CmpOrder::Incomparable`. Any operand pair with no comparison at all
        // falls through to the `Incomparable` catch-alls below.
        match (self, other) {
            (Self::Int(s), Self::Int(o)) => Ok(CmpOrder::Ordered(s.cmp(o))),
            (Self::Float(s), Self::Float(o)) => Ok(CmpOrder::from_numeric(s.partial_cmp(o))),
            // Int/float ordering is exact (no rounding of either operand).
            (Self::Int(s), Self::Float(o)) => Ok(CmpOrder::from_numeric(i64_cmp_f64(*s, *o))),
            (Self::Float(s), Self::Int(o)) => Ok(CmpOrder::from_numeric(i64_cmp_f64(*o, *s).map(Ordering::reverse))),
            (Self::Int(a), Self::InternLongInt(b)) => Ok(CmpOrder::Ordered(
                bigint_cmp_i64(interns.get_long_int(*b), *a).reverse(),
            )),
            (Self::InternLongInt(a), Self::Int(b)) => {
                Ok(CmpOrder::Ordered(bigint_cmp_i64(interns.get_long_int(*a), *b)))
            }
            (Self::Float(a), Self::InternLongInt(b)) => Ok(CmpOrder::from_numeric(
                bigint_cmp_f64(interns.get_long_int(*b), *a).map(Ordering::reverse),
            )),
            (Self::InternLongInt(a), Self::Float(b)) => {
                Ok(CmpOrder::from_numeric(bigint_cmp_f64(interns.get_long_int(*a), *b)))
            }
            (Self::InternLongInt(a), Self::InternLongInt(b)) => Ok(CmpOrder::Ordered(
                interns.get_long_int(*a).cmp(interns.get_long_int(*b)),
            )),
            // Bool promotion: convert to Int and re-dispatch. Recursion is bounded
            // to at most 2 levels (Bool→Int, then Int matches directly above).
            (Self::Bool(s), _) => Self::Int(i64::from(*s)).py_cmp(other, vm),
            (_, Self::Bool(s)) => self.py_cmp(&Self::Int(i64::from(*s)), vm),
            // Int vs LongInt comparison
            (Self::Int(a), Self::Ref(id)) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
                Ok(CmpOrder::Ordered(bigint_cmp_i64(li.inner(), *a).reverse()))
            }
            (Self::InternLongInt(a), Self::Ref(id)) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
                Ok(CmpOrder::Ordered(interns.get_long_int(*a).cmp(li.inner())))
            }
            // LongInt vs Int comparison
            (Self::Ref(id), Self::Int(b)) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
                Ok(CmpOrder::Ordered(bigint_cmp_i64(li.inner(), *b)))
            }
            (Self::Ref(id), Self::InternLongInt(b)) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
                Ok(CmpOrder::Ordered(li.inner().cmp(interns.get_long_int(*b))))
            }
            // Float vs LongInt comparison (exact, no precision loss)
            (Self::Float(s), Self::Ref(id)) if let HeapData::LongInt(li) = vm.heap.get(*id) => Ok(
                CmpOrder::from_numeric(bigint_cmp_f64(li.inner(), *s).map(Ordering::reverse)),
            ),
            // LongInt vs Float comparison (exact, no precision loss)
            (Self::Ref(id), Self::Float(o)) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
                Ok(CmpOrder::from_numeric(li.partial_cmp_f64(*o)))
            }
            // Ref vs Ref comparison: handles LongInt, Str, Tuple, and List
            (Self::Ref(id1), Self::Ref(id2)) => match (vm.heap.read(*id1), vm.heap.read(*id2)) {
                (HeapReadOutput::LongInt(a), HeapReadOutput::LongInt(b)) => {
                    Ok(CmpOrder::Ordered(a.get(vm.heap).inner().cmp(b.get(vm.heap).inner())))
                }
                (HeapReadOutput::Str(a), HeapReadOutput::Str(b)) => {
                    Ok(CmpOrder::Ordered(a.get(vm.heap).as_str().cmp(b.get(vm.heap).as_str())))
                }
                (HeapReadOutput::Tuple(a), HeapReadOutput::Tuple(b)) => a.py_cmp(&b, vm),
                (HeapReadOutput::List(a), HeapReadOutput::List(b)) => a.py_cmp(&b, vm),
                (HeapReadOutput::Date(a), HeapReadOutput::Date(b)) => {
                    Ok(CmpOrder::from_total(a.get(vm.heap).partial_cmp(b.get(vm.heap))))
                }
                (HeapReadOutput::DateTime(a), HeapReadOutput::DateTime(b)) => a.py_cmp(&b, vm),
                (HeapReadOutput::TimeDelta(a), HeapReadOutput::TimeDelta(b)) => {
                    Ok(CmpOrder::from_total(a.get(vm.heap).partial_cmp(b.get(vm.heap))))
                }
                _ => Ok(CmpOrder::Incomparable),
            },
            // Interned string comparisons
            (Self::InternString(s1), Self::InternString(s2)) => {
                Ok(CmpOrder::Ordered(interns.get_str(*s1).cmp(interns.get_str(*s2))))
            }
            // Cross-type string comparisons: interned vs heap-allocated
            (Self::InternString(s1), Self::Ref(id2)) if let HeapData::Str(s2) = vm.heap.get(*id2) => {
                Ok(CmpOrder::Ordered(interns.get_str(*s1).cmp(s2.as_str())))
            }
            (Self::Ref(id1), Self::InternString(s2)) if let HeapData::Str(s1) = vm.heap.get(*id1) => {
                Ok(CmpOrder::Ordered(s1.as_str().cmp(interns.get_str(*s2))))
            }
            (Self::InternBytes(b1), Self::InternBytes(b2)) => {
                Ok(CmpOrder::Ordered(interns.get_bytes(*b1).cmp(interns.get_bytes(*b2))))
            }
            _ => Ok(CmpOrder::Incomparable),
        }
    }

    fn py_bool(&self, vm: &mut VM<'_, impl ResourceTracker>) -> bool {
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
            Self::InternString(string_id) => !vm.interns.get_str(*string_id).is_empty(),
            Self::InternBytes(bytes_id) => !vm.interns.get_bytes(*bytes_id).is_empty(),
            Self::Ref(id) => vm.heap.read(*id).py_bool(vm),
            #[cfg(feature = "memory-model-checks")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'_, impl ResourceTracker>,
        heap_ids: &mut LazyHeapSet,
    ) -> RunResult<()> {
        let interns = vm.interns;
        match self {
            Self::Undefined => Ok(f.write_str("Undefined")?),
            Self::Ellipsis => Ok(f.write_str("Ellipsis")?),
            Self::None => Ok(f.write_str("None")?),
            Self::Bool(true) => Ok(f.write_str("True")?),
            Self::Bool(false) => Ok(f.write_str("False")?),
            // `itoa` formats into a fixed stack buffer, skipping the generic
            // `fmt`/`pad_integral` path and its repeated `RawVec` reallocation.
            Self::Int(v) => Ok(f.write_str(itoa::Buffer::new().format(*v))?),
            Self::InternLongInt(long_int_id) => {
                let bi = interns.get_long_int(*long_int_id);
                check_bits_str_digits_limit(bi.bits())?;
                Ok(write!(f, "{bi}")?)
            }
            Self::Float(v) => Ok(write!(f, "{}", FormatFloat(*v))?),
            Self::Builtin(b) => Ok(b.py_repr_fmt(f)?),
            Self::ModuleFunction(mf) => Ok(mf.py_repr_fmt(f, self.id(vm))?),
            Self::DefFunction(f_id) => Ok(interns.get_function(*f_id).py_repr_fmt(f, interns, self.id(vm))?),
            Self::ExtFunction(name_id) => Ok(write!(f, "<function '{}' external>", interns.get_str(*name_id))?),
            Self::InternString(string_id) => Ok(string_repr_fmt(interns.get_str(*string_id), f)?),
            Self::InternBytes(bytes_id) => Ok(bytes_repr_fmt(interns.get_bytes(*bytes_id), f)?),
            Self::Marker(m) => Ok(m.py_repr_fmt(f)?),
            Self::Property(p) => Ok(write!(f, "<property {p:?}>")?),
            Self::Ref(id) => {
                if heap_ids.contains(id) {
                    // Cycle detected - write type-specific placeholder following Python semantics
                    match vm.heap.get(*id) {
                        HeapData::List(_) => Ok(f.write_str("[...]")?),
                        HeapData::Tuple(_) => Ok(f.write_str("(...)")?),
                        HeapData::Dict(_) => Ok(f.write_str("{...}")?),
                        // Other types don't typically have cycles, but handle gracefully
                        _ => Ok(f.write_str("...")?),
                    }
                } else if matches!(vm.heap.get(*id), HeapData::Instance(_)) {
                    // Instances dispatch to a user `__repr__` (or the default), which
                    // needs the heap id to pass `self` — handled here, not at the heap
                    // level, so no `heap_ids` insertion happens. Recursion here
                    // re-enters the VM on the *Rust* stack, bounded by
                    // `evaluate_function`'s re-entry guard (see the "Recursive/deep
                    // `__repr__`/`__str__`" divergence in limitations/classes.md).
                    let str_value = instance_repr(*id, vm)?;
                    defer_drop!(str_value, vm);
                    Ok(f.write_str(str_value.to_str(vm)?)?)
                } else {
                    heap_ids.insert(*id);
                    let result = vm.heap.read(*id).py_repr_fmt(f, vm, heap_ids);
                    heap_ids.remove(id);
                    result
                }
            }
            #[cfg(feature = "memory-model-checks")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    /// Overrides the default `py_repr` with allocation-light fast paths for the
    /// values whose repr is cheap to produce:
    /// - singletons (`None`/`True`/`False`/`Ellipsis`) resolve to a pre-interned
    ///   `StringId`, so `repr`/`str`/`print`/f-strings allocate nothing at all;
    /// - `int` formats via `itoa` straight into a right-sized `allocate_string`,
    ///   skipping the grow-then-shrink intermediate `String`.
    ///
    /// Every other variant takes the generic `py_repr_fmt` buffered path. `str`
    /// is also served allocation-free, but by [`py_str`](Self::py_str) — `repr`
    /// of a `str` still needs a buffer for quoting/escaping.
    fn py_repr(&self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        match self {
            Self::None => Ok(Self::InternString(StaticStrings::NoneRepr.into())),
            Self::Bool(true) => Ok(Self::InternString(StaticStrings::TrueRepr.into())),
            Self::Bool(false) => Ok(Self::InternString(StaticStrings::FalseRepr.into())),
            Self::Ellipsis => Ok(Self::InternString(StaticStrings::EllipsisRepr.into())),
            Self::Int(i) => Ok(allocate_string(itoa::Buffer::new().format(*i), vm.heap)?),
            _ => {
                let mut s = String::new();
                let mut heap_ids = LazyHeapSet::default();
                self.py_repr_fmt(&mut s, vm, &mut heap_ids)?;
                Ok(allocate_string(s, vm.heap)?)
            }
        }
    }

    fn py_str(&self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        match self {
            // Interned/heap strings are already what `str()` returns — hand the
            // same value back (inc-ref'd for the heap case) instead of cloning
            // the bytes into a fresh allocation.
            Self::InternString(string_id) => Ok(Self::InternString(*string_id)),
            Self::Ref(id) if matches!(vm.heap.get(*id), HeapData::Str(_)) => Ok(self.clone_with_heap(vm.heap)),
            // Instances dispatch to a user `__str__`/`__repr__` (needs the heap id).
            Self::Ref(id) if matches!(vm.heap.get(*id), HeapData::Instance(_)) => instance_str(*id, vm),
            Self::Ref(id) => vm.heap.read(*id).py_str(vm),
            _ => self.py_repr(vm),
        }
    }

    fn py_add(&self, other: &Self, vm: &mut VM<'_, impl ResourceTracker>) -> Result<Option<Value>, ResourceError> {
        let interns = vm.interns;
        match (self, other) {
            // Int + Int with overflow detection
            (Self::Int(a), Self::Int(b)) => {
                if let Some(result) = a.checked_add(*b) {
                    Ok(Some(Self::Int(result)))
                } else {
                    // Overflow - promote to LongInt
                    let li = LongInt::from(*a) + LongInt::from(*b);
                    li.into_value(vm.heap).map(Some)
                }
            }
            // Int + LongInt
            (Self::Int(i), Self::Ref(id)) | (Self::Ref(id), Self::Int(i))
                if let HeapData::LongInt(li) = vm.heap.get(*id) =>
            {
                let result = LongInt::new(li.inner() + i);
                result.into_value(vm.heap).map(Some)
            }
            (Self::Float(v1), Self::Float(v2)) => Ok(Some(Self::Float(v1 + v2))),
            // Int + Float and Float + Int
            (Self::Int(a), Self::Float(b)) => Ok(Some(Self::Float(*a as f64 + b))),
            (Self::Float(a), Self::Int(b)) => Ok(Some(Self::Float(a + *b as f64))),
            (Self::Ref(id1), Self::Ref(id2)) => {
                let left = vm.heap.read(*id1);
                let right = vm.heap.read(*id2);
                left.py_add(&right, vm)
            }
            (Self::InternString(s1), Self::InternString(s2)) => Ok(Some(concat_allocate_str(
                interns.get_str(*s1),
                interns.get_str(*s2),
                vm.heap,
            )?)),
            // for strings we need to account for the fact they might be either interned or not
            (Self::InternString(string_id), Self::Ref(id2)) if let HeapData::Str(s2) = vm.heap.get(*id2) => Ok(Some(
                concat_allocate_str(interns.get_str(*string_id), s2.as_str(), vm.heap)?,
            )),
            (Self::Ref(id1), Self::InternString(string_id)) if let HeapData::Str(s1) = vm.heap.get(*id1) => Ok(Some(
                concat_allocate_str(s1.as_str(), interns.get_str(*string_id), vm.heap)?,
            )),
            // same for bytes
            (Self::InternBytes(b1), Self::InternBytes(b2)) => {
                let bytes1 = interns.get_bytes(*b1);
                let bytes2 = interns.get_bytes(*b2);
                let mut b = Vec::with_capacity(bytes1.len() + bytes2.len());
                b.extend_from_slice(bytes1);
                b.extend_from_slice(bytes2);
                Ok(Some(Self::Ref(vm.heap.allocate(HeapData::Bytes(b.into()))?)))
            }
            (Self::InternBytes(bytes_id), Self::Ref(id2)) if let HeapData::Bytes(b2) = vm.heap.get(*id2) => {
                let bytes1 = interns.get_bytes(*bytes_id);
                let mut b = Vec::with_capacity(bytes1.len() + b2.len());
                b.extend_from_slice(bytes1);
                b.extend_from_slice(b2);
                Ok(Some(Self::Ref(vm.heap.allocate(HeapData::Bytes(b.into()))?)))
            }
            (Self::Ref(id1), Self::InternBytes(bytes_id)) if let HeapData::Bytes(b1) = vm.heap.get(*id1) => {
                let bytes2 = interns.get_bytes(*bytes_id);
                let mut b = Vec::with_capacity(b1.len() + bytes2.len());
                b.extend_from_slice(b1);
                b.extend_from_slice(bytes2);
                Ok(Some(Self::Ref(vm.heap.allocate(HeapData::Bytes(b.into()))?)))
            }
            _ => Ok(None),
        }
    }

    fn py_sub(&self, other: &Self, vm: &mut VM<'_, impl ResourceTracker>) -> Result<Option<Self>, ResourceError> {
        match (self, other) {
            // Int - Int with overflow detection
            (Self::Int(a), Self::Int(b)) => {
                if let Some(result) = a.checked_sub(*b) {
                    Ok(Some(Self::Int(result)))
                } else {
                    // Overflow - promote to LongInt
                    let li = LongInt::from(*a) - LongInt::from(*b);
                    li.into_value(vm.heap).map(Some)
                }
            }
            // Int - LongInt
            (Self::Int(a), Self::Ref(id)) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
                let result = LongInt::from(*a) - LongInt::new(li.inner().clone());
                result.into_value(vm.heap).map(Some)
            }
            // LongInt - Int
            (Self::Ref(id), Self::Int(b)) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
                let result = LongInt::new(li.inner().clone()) - LongInt::from(*b);
                result.into_value(vm.heap).map(Some)
            }
            // LongInt - LongInt
            (Self::Ref(id1), Self::Ref(id2)) => {
                let left = vm.heap.read(*id1);
                let right = vm.heap.read(*id2);
                left.py_sub(&right, vm)
            }
            // Float - Float
            (Self::Float(a), Self::Float(b)) => Ok(Some(Self::Float(a - b))),
            // Int - Float and Float - Int
            (Self::Int(a), Self::Float(b)) => Ok(Some(Self::Float(*a as f64 - b))),
            (Self::Float(a), Self::Int(b)) => Ok(Some(Self::Float(a - *b as f64))),
            _ => Ok(None),
        }
    }

    fn py_mod(&self, other: &Self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<Self>> {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => {
                if *b == 0 {
                    Err(ExcType::zero_division().into())
                } else if let Some(r) = a.checked_rem(*b) {
                    // Python modulo: result has the same sign as divisor (b)
                    let result = if r != 0 && (*a < 0) != (*b < 0) { r + *b } else { r };
                    Ok(Some(Self::Int(result)))
                } else {
                    // Overflow - i64::MIN % -1 is 0
                    Ok(Some(Self::Int(0)))
                }
            }
            // Int % LongInt
            (Self::Int(a), Self::Ref(id)) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
                if li.is_zero() {
                    return Err(ExcType::zero_division().into());
                }
                let bi = BigInt::from(*a).mod_floor(li.inner());
                Ok(Some(LongInt::new(bi).into_value(vm.heap)?))
            }
            // LongInt % Int
            (Self::Ref(id), Self::Int(b)) if let HeapData::LongInt(li) = vm.heap.get(*id) => {
                if *b == 0 {
                    return Err(ExcType::zero_division().into());
                }
                let bi = li.inner().mod_floor(&BigInt::from(*b));
                Ok(Some(LongInt::new(bi).into_value(vm.heap)?))
            }
            // LongInt % LongInt
            (Self::Ref(id1), Self::Ref(id2)) => {
                let left = vm.heap.read(*id1);
                let right = vm.heap.read(*id2);
                left.py_mod(&right, vm)
            }
            (Self::Float(v1), Self::Float(v2)) => {
                if *v2 == 0.0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float(py_float_mod(*v1, *v2))))
                }
            }
            (Self::Float(v1), Self::Int(v2)) => {
                if *v2 == 0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float(py_float_mod(*v1, *v2 as f64))))
                }
            }
            (Self::Int(v1), Self::Float(v2)) => {
                if *v2 == 0.0 {
                    Err(ExcType::zero_division().into())
                } else {
                    Ok(Some(Self::Float(py_float_mod(*v1 as f64, *v2))))
                }
            }
            _ => Ok(None),
        }
    }

    fn py_iadd(
        &mut self,
        other: &Self,
        vm: &mut VM<'_, impl ResourceTracker>,
        _self_id: Option<HeapId>,
    ) -> Result<bool, ResourceError> {
        let interns = vm.interns;
        match (&self, other) {
            (Self::Int(v1), Self::Int(v2)) => {
                if let Some(result) = v1.checked_add(*v2) {
                    *self = Self::Int(result);
                } else {
                    // Overflow - promote to LongInt
                    let li = LongInt::from(*v1) + LongInt::from(*v2);
                    *self = li.into_value(vm.heap)?;
                }
                Ok(true)
            }
            (Self::Float(v1), Self::Float(v2)) => {
                *self = Self::Float(*v1 + *v2);
                Ok(true)
            }
            (Self::InternString(s1), Self::InternString(s2)) => {
                let concat = format!("{}{}", interns.get_str(*s1), interns.get_str(*s2));
                *self = allocate_string(concat, vm.heap)?;
                Ok(true)
            }
            (Self::InternString(string_id), Self::Ref(id2)) => {
                let result = if let HeapData::Str(s2) = vm.heap.get(*id2) {
                    let concat = format!("{}{}", interns.get_str(*string_id), s2.as_str());
                    *self = allocate_string(concat, vm.heap)?;
                    true
                } else {
                    false
                };
                Ok(result)
            }
            // same for bytes
            (Self::InternBytes(b1), Self::InternBytes(b2)) => {
                let bytes1 = interns.get_bytes(*b1);
                let bytes2 = interns.get_bytes(*b2);
                let mut b = Vec::with_capacity(bytes1.len() + bytes2.len());
                b.extend_from_slice(bytes1);
                b.extend_from_slice(bytes2);
                *self = Self::Ref(vm.heap.allocate(HeapData::Bytes(b.into()))?);
                Ok(true)
            }
            (Self::InternBytes(bytes_id), Self::Ref(id2)) => {
                let result = if let HeapData::Bytes(b2) = vm.heap.get(*id2) {
                    let bytes1 = interns.get_bytes(*bytes_id);
                    let mut b = Vec::with_capacity(bytes1.len() + b2.len());
                    b.extend_from_slice(bytes1);
                    b.extend_from_slice(b2);
                    *self = Self::Ref(vm.heap.allocate(HeapData::Bytes(b.into()))?);
                    true
                } else {
                    false
                };
                Ok(result)
            }
            (Self::Ref(id), Self::Ref(_)) => vm.heap.read(*id).py_iadd(other, vm, Some(*id)),
            _ => Ok(false),
        }
    }

    fn py_mult(&self, other: &Self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<Value>> {
        let interns = vm.interns;
        match (self, other) {
            // Numeric multiplication with overflow promotion to LongInt
            (Self::Int(a), Self::Int(b)) => {
                if let Some(result) = a.checked_mul(*b) {
                    Ok(Some(Self::Int(result)))
                } else {
                    // Overflow - promote to LongInt
                    let li = LongInt::from(*a) * LongInt::from(*b);
                    Ok(Some(li.into_value(vm.heap)?))
                }
            }
            // Int * heap-allocated value (commutative for the supported types).
            // Covers LongInt and TimeDelta numeric multiplication, plus repetition
            // of heap-allocated Str/Bytes/List/Tuple sequences by an integer count.
            (Self::Int(n), Self::Ref(id)) | (Self::Ref(id), Self::Int(n)) => match vm.heap.get(*id) {
                HeapData::LongInt(li) => {
                    check_mult_size(li.bits(), i64_bits(*n), vm.heap.tracker())?;
                    let result = LongInt::new(li.inner().clone()) * LongInt::from(*n);
                    Ok(Some(result.into_value(vm.heap)?))
                }
                HeapData::TimeDelta(td) => {
                    let total = timedelta::total_microseconds(td)
                        .checked_mul(i128::from(*n))
                        .ok_or_else(|| {
                            SimpleException::new_msg(ExcType::OverflowError, "timedelta multiplication overflow")
                        })?;
                    let delta = timedelta::from_total_microseconds(total)?;
                    Ok(Some(Self::Ref(vm.heap.allocate(HeapData::TimeDelta(delta))?)))
                }
                HeapData::Str(s) => {
                    let count = i64_to_repeat_count(*n)?;
                    check_repeat_size(s.len(), count, vm.heap.tracker())?;
                    let repeated = s.as_str().repeat(count);
                    Ok(Some(allocate_string(repeated, vm.heap)?))
                }
                HeapData::Bytes(b) => {
                    let count = i64_to_repeat_count(*n)?;
                    check_repeat_size(b.len(), count, vm.heap.tracker())?;
                    Ok(Some(Self::Ref(
                        vm.heap.allocate(HeapData::Bytes(b.as_slice().repeat(count).into()))?,
                    )))
                }
                HeapData::List(list) => {
                    let count = i64_to_repeat_count(*n)?;
                    check_repeat_size(
                        list.len().saturating_mul(mem::size_of::<Self>()),
                        count,
                        vm.heap.tracker(),
                    )?;
                    let mut result = Vec::with_capacity(list.as_slice().len() * count);
                    for _ in 0..count {
                        result.extend(list.as_slice().iter().map(|v| v.clone_with_heap(vm.heap)));
                        vm.heap.check_time()?;
                    }
                    Ok(Some(Self::Ref(vm.heap.allocate(HeapData::List(List::new(result)))?)))
                }
                HeapData::Tuple(tuple) => {
                    let count = i64_to_repeat_count(*n)?;
                    if count == 0 {
                        Ok(Some(vm.heap.get_empty_tuple()))
                    } else {
                        check_repeat_size(
                            tuple.as_slice().len().saturating_mul(mem::size_of::<Self>()),
                            count,
                            vm.heap.tracker(),
                        )?;
                        let mut result = SmallVec::with_capacity(tuple.as_slice().len() * count);
                        for _ in 0..count {
                            result.extend(tuple.as_slice().iter().map(|v| v.clone_with_heap(vm.heap)));
                            vm.heap.check_time()?;
                        }
                        Ok(Some(allocate_tuple(result, vm.heap)?))
                    }
                }
                _ => Ok(None),
            },
            // Ref * Ref: LongInt * LongInt is numeric multiplication; LongInt * sequence
            // (or vice versa) is repetition of a heap-allocated Str/Bytes/List/Tuple.
            (Self::Ref(id1), Self::Ref(id2)) => {
                let (seq_id, count) = match (vm.heap.get(*id1), vm.heap.get(*id2)) {
                    (HeapData::LongInt(a), HeapData::LongInt(b)) => {
                        check_mult_size(a.bits(), b.bits(), vm.heap.tracker())?;
                        let result = LongInt::new(a.inner() * b.inner());
                        return Ok(Some(result.into_value(vm.heap)?));
                    }
                    (HeapData::LongInt(li), _) => (*id2, longint_to_repeat_count(li)?),
                    (_, HeapData::LongInt(li)) => (*id1, longint_to_repeat_count(li)?),
                    _ => return Ok(None),
                };
                match vm.heap.get(seq_id) {
                    HeapData::Str(s) => {
                        check_repeat_size(s.len(), count, vm.heap.tracker())?;
                        let repeated = s.as_str().repeat(count);
                        Ok(Some(allocate_string(repeated, vm.heap)?))
                    }
                    HeapData::Bytes(b) => {
                        check_repeat_size(b.len(), count, vm.heap.tracker())?;
                        Ok(Some(Self::Ref(
                            vm.heap.allocate(HeapData::Bytes(b.as_slice().repeat(count).into()))?,
                        )))
                    }
                    HeapData::List(list) => {
                        check_repeat_size(
                            list.len().saturating_mul(mem::size_of::<Self>()),
                            count,
                            vm.heap.tracker(),
                        )?;
                        let mut result = Vec::with_capacity(list.as_slice().len() * count);
                        for _ in 0..count {
                            result.extend(list.as_slice().iter().map(|v| v.clone_with_heap(vm.heap)));
                            vm.heap.check_time()?;
                        }
                        Ok(Some(Self::Ref(vm.heap.allocate(HeapData::List(List::new(result)))?)))
                    }
                    HeapData::Tuple(tuple) => {
                        if count == 0 {
                            Ok(Some(vm.heap.get_empty_tuple()))
                        } else {
                            check_repeat_size(
                                tuple.as_slice().len().saturating_mul(mem::size_of::<Self>()),
                                count,
                                vm.heap.tracker(),
                            )?;
                            let mut result = SmallVec::with_capacity(tuple.as_slice().len() * count);
                            for _ in 0..count {
                                result.extend(tuple.as_slice().iter().map(|v| v.clone_with_heap(vm.heap)));
                                vm.heap.check_time()?;
                            }
                            Ok(Some(allocate_tuple(result, vm.heap)?))
                        }
                    }
                    _ => Ok(None),
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
                let str_ref = interns.get_str(*s);
                check_repeat_size(str_ref.len(), count, vm.heap.tracker())?;
                let result = str_ref.repeat(count);
                Ok(Some(allocate_string(result, vm.heap)?))
            }

            // Bytes repetition: b"ab" * 3 or 3 * b"ab"
            (Self::InternBytes(b), Self::Int(n)) | (Self::Int(n), Self::InternBytes(b)) => {
                let count = i64_to_repeat_count(*n)?;
                let bytes_ref = interns.get_bytes(*b);
                check_repeat_size(bytes_ref.len(), count, vm.heap.tracker())?;
                let result: Vec<u8> = bytes_ref.repeat(count);
                Ok(Some(Self::Ref(vm.heap.allocate(HeapData::Bytes(result.into()))?)))
            }

            // String repetition with LongInt: "ab" * bigint or bigint * "ab"
            (Self::InternString(s), Self::Ref(id)) | (Self::Ref(id), Self::InternString(s))
                if let HeapData::LongInt(li) = vm.heap.get(*id) =>
            {
                let count = longint_to_repeat_count(li)?;
                let str_ref = interns.get_str(*s);
                check_repeat_size(str_ref.len(), count, vm.heap.tracker())?;
                let result = str_ref.repeat(count);
                Ok(Some(allocate_string(result, vm.heap)?))
            }

            // Bytes repetition with LongInt: b"ab" * bigint or bigint * b"ab"
            (Self::InternBytes(b), Self::Ref(id)) | (Self::Ref(id), Self::InternBytes(b))
                if let HeapData::LongInt(li) = vm.heap.get(*id) =>
            {
                let count = longint_to_repeat_count(li)?;
                let bytes_ref = interns.get_bytes(*b);
                check_repeat_size(bytes_ref.len(), count, vm.heap.tracker())?;
                let result: Vec<u8> = bytes_ref.repeat(count);
                Ok(Some(Self::Ref(vm.heap.allocate(HeapData::Bytes(result.into()))?)))
            }

            _ => Ok(None),
        }
    }

    fn py_div(&self, other: &Self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<Value>> {
        let interns = vm.interns;
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
                if let HeapData::LongInt(li) = vm.heap.get(*id) {
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
            // LongInt / Int or TimeDelta / Int
            (Self::Ref(id), Self::Int(b)) => match vm.heap.get(*id) {
                HeapData::LongInt(li) => {
                    if *b == 0 {
                        Err(ExcType::zero_division().into())
                    } else {
                        // Convert both to f64 for division
                        let a_f64 = li.to_f64().unwrap_or(f64::INFINITY);
                        let b_f64 = *b as f64;
                        Ok(Some(Self::Float(a_f64 / b_f64)))
                    }
                }
                HeapData::TimeDelta(td) => {
                    if *b == 0 {
                        Err(ExcType::zero_division().into())
                    } else {
                        let total = timedelta::total_microseconds(td);
                        let result = timedelta::div_microseconds_round_ties_even(total, i128::from(*b));
                        let delta = timedelta::from_total_microseconds(result)?;
                        Ok(Some(Self::Ref(vm.heap.allocate(HeapData::TimeDelta(delta))?)))
                    }
                }
                _ => Ok(None),
            },
            // LongInt / LongInt
            (Self::Ref(id1), Self::Ref(id2)) => match (vm.heap.get(*id1), vm.heap.get(*id2)) {
                (HeapData::LongInt(li1), HeapData::LongInt(li2)) => {
                    if li2.is_zero() {
                        Err(ExcType::zero_division().into())
                    } else {
                        let a_f64 = li1.to_f64().unwrap_or(f64::INFINITY);
                        let b_f64 = li2.to_f64().unwrap_or(f64::INFINITY);
                        Ok(Some(Self::Float(a_f64 / b_f64)))
                    }
                }
                _ => Ok(None),
            },
            // LongInt / Float
            (Self::Ref(id), Self::Float(b)) => {
                if let HeapData::LongInt(li) = vm.heap.get(*id) {
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
                if let HeapData::LongInt(li) = vm.heap.get(*id) {
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
                    && matches!(vm.heap.get(*id), HeapData::Path(_))
                {
                    return path::path_div(*id, other, vm.heap, interns);
                }
                Ok(None)
            }
        }
    }

    fn py_floordiv(&self, other: &Self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<Value>> {
        match (self, other) {
            // Floor division: int // int returns int
            (Self::Int(a), Self::Int(b)) => {
                if *b == 0 {
                    Err(ExcType::zero_division().into())
                } else if let Some((d, _)) = floor_divmod(*a, *b) {
                    Ok(Some(Self::Int(d)))
                } else {
                    // Overflow - promote to LongInt
                    check_div_size(i64_bits(*a), vm.heap.tracker())?;
                    let bi = BigInt::from(*a).div_floor(&BigInt::from(*b));
                    Ok(Some(LongInt::new(bi).into_value(vm.heap)?))
                }
            }
            // Int // LongInt
            (Self::Int(a), Self::Ref(id)) => {
                if let HeapData::LongInt(li) = vm.heap.get(*id) {
                    if li.is_zero() {
                        Err(ExcType::zero_division().into())
                    } else {
                        let bi = BigInt::from(*a).div_floor(li.inner());
                        Ok(Some(LongInt::new(bi).into_value(vm.heap)?))
                    }
                } else {
                    Ok(None)
                }
            }
            // LongInt // Int or TimeDelta // Int
            (Self::Ref(id), Self::Int(b)) => match vm.heap.get(*id) {
                HeapData::LongInt(li) => {
                    if *b == 0 {
                        Err(ExcType::zero_division().into())
                    } else {
                        let bi = li.inner().div_floor(&BigInt::from(*b));
                        Ok(Some(LongInt::new(bi).into_value(vm.heap)?))
                    }
                }
                HeapData::TimeDelta(td) => {
                    if *b == 0 {
                        Err(ExcType::zero_division().into())
                    } else {
                        let total = timedelta::total_microseconds(td);
                        let result = total.div_euclid(i128::from(*b));
                        let delta = timedelta::from_total_microseconds(result)?;
                        Ok(Some(Self::Ref(vm.heap.allocate(HeapData::TimeDelta(delta))?)))
                    }
                }
                _ => Ok(None),
            },
            // LongInt // LongInt
            (Self::Ref(id1), Self::Ref(id2)) => match (vm.heap.get(*id1), vm.heap.get(*id2)) {
                (HeapData::LongInt(li1), HeapData::LongInt(li2)) => {
                    if li2.is_zero() {
                        Err(ExcType::zero_division().into())
                    } else {
                        let bi = li1.inner().div_floor(li2.inner());
                        Ok(Some(LongInt::new(bi).into_value(vm.heap)?))
                    }
                }
                _ => Ok(None),
            },
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

    fn py_pow(&self, other: &Self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<Value>> {
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
                            check_pow_size(i64_bits(*base), u64::from(exp_u32), vm.heap.tracker())?;
                            let bi = BigInt::from(*base).pow(exp_u32);
                            Ok(Some(LongInt::new(bi).into_value(vm.heap)?))
                        }
                    } else {
                        // exp > u32::MAX - use BigInt with modpow-style exponentiation
                        // For very large exponents, we still need LongInt
                        // Safety: exp >= 0 is guaranteed by the outer if condition
                        #[expect(clippy::cast_sign_loss)]
                        let exp_u64 = *exp as u64;
                        // Check size before computing to prevent DoS
                        check_pow_size(i64_bits(*base), exp_u64, vm.heap.tracker())?;
                        let bi = bigint_pow(BigInt::from(*base), exp_u64);
                        Ok(Some(LongInt::new(bi).into_value(vm.heap)?))
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
                if let HeapData::LongInt(li) = vm.heap.get(*id) {
                    if li.is_zero() && *exp < 0 {
                        Err(ExcType::zero_negative_power())
                    } else if *exp >= 0 {
                        // Use BigInt pow for positive exponents
                        if let Ok(exp_u32) = u32::try_from(*exp) {
                            // Check size before computing to prevent DoS
                            check_pow_size(li.bits(), u64::from(exp_u32), vm.heap.tracker())?;
                            let bi = li.inner().pow(exp_u32);
                            Ok(Some(LongInt::new(bi).into_value(vm.heap)?))
                        } else {
                            // Safety: exp >= 0 is guaranteed by the outer if condition
                            #[expect(clippy::cast_sign_loss)]
                            let exp_u64 = *exp as u64;
                            // Check size before computing to prevent DoS
                            check_pow_size(li.bits(), exp_u64, vm.heap.tracker())?;
                            let bi = bigint_pow(li.inner().clone(), exp_u64);
                            Ok(Some(LongInt::new(bi).into_value(vm.heap)?))
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
                if let HeapData::LongInt(li) = vm.heap.get(*id) {
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
                                check_pow_size(i64_bits(*base), u64::from(exp_u32), vm.heap.tracker())?;
                                let bi = BigInt::from(*base).pow(exp_u32);
                                Ok(Some(LongInt::new(bi).into_value(vm.heap)?))
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

    fn py_getitem(&self, key: &Self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        let interns = vm.interns;
        match self {
            Self::Ref(id) => vm.heap.read(*id).py_getitem(key, vm),
            Self::InternString(string_id) => {
                // Check for slice first
                if let Self::Ref(key_id) = key
                    && let HeapData::Slice(slice_obj) = vm.heap.get(*key_id)
                {
                    let s = interns.get_str(*string_id);
                    let result_str: Box<str> = slice_collect_iterator(vm, slice_obj, s.chars(), |c| c)?;
                    return Ok(allocate_string(result_str, vm.heap)?);
                }

                // Handle interned string indexing, accepting Int and Bool
                let index = match key {
                    Self::Int(i) => *i,
                    Self::Bool(b) => i64::from(*b),
                    _ => return Err(ExcType::type_error_indices(Type::Str, &key.py_type_name(vm))),
                };

                let s = interns.get_str(*string_id);
                let c = get_char_at_index(s, index).ok_or_else(ExcType::str_index_error)?;
                Ok(allocate_char(c, vm.heap)?)
            }
            Self::InternBytes(bytes_id) => {
                // Check for slice first
                if let Self::Ref(key_id) = key
                    && let HeapData::Slice(slice_obj) = vm.heap.get(*key_id)
                {
                    let bytes = interns.get_bytes(*bytes_id);
                    let result_bytes = slice_collect_iterator(vm, slice_obj, bytes.iter(), |b| *b)?;
                    let heap_id = vm.heap.allocate(HeapData::Bytes(Bytes::new(result_bytes)))?;
                    return Ok(Self::Ref(heap_id));
                }

                // Handle interned bytes indexing - returns integer byte value
                let index = match key {
                    Self::Int(i) => *i,
                    Self::Bool(b) => i64::from(*b),
                    _ => return Err(ExcType::type_error_indices(Type::Bytes, &key.py_type_name(vm))),
                };

                let bytes = interns.get_bytes(*bytes_id);
                let byte = get_byte_at_index(bytes, index).ok_or_else(ExcType::bytes_index_error)?;
                Ok(Self::Int(i64::from(byte)))
            }
            _ => Err(ExcType::type_error_not_sub(&self.py_type_name(vm))),
        }
    }

    fn py_setitem(&mut self, key: Self, value: Self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<()> {
        match self {
            Self::Ref(id) => vm.heap.read(*id).py_setitem(key, value, vm),
            _ => Err(ExcType::type_error(format!(
                "'{}' object does not support item assignment",
                self.py_type_name(vm)
            ))),
        }
    }
}

/// `Value` releases its (possible) heap reference through any [`ContainsHeap`]
/// context — `Heap`, `HeapReader`, `VM`, or the json `Encoder`. Forwards to the
/// inherent [`Value::drop_with`], which also serves direct callers.
impl<C: ContainsHeap> DropWithContext<C> for Value {
    #[inline]
    fn drop_with(self, ctx: &mut C) {
        // Resolves to the inherent `Value::drop_with` (inherent methods take
        // priority), not this trait method — so no recursion.
        Self::drop_with(self, ctx);
    }
}

impl Value {
    /// Returns the Python `Type` for this value using only `&Heap` (no full VM borrow).
    ///
    /// Wraps [`py_type_shallow`](Self::py_type_shallow) for immediate values and
    /// delegates to [`HeapData::py_type`] for `Value::Ref`. Useful in code paths
    /// that need a type label (e.g. CPython-style "argument N must be X, not Y"
    /// errors) but don't have a `&VM` handy — notably the macro-generated
    /// `from_args` bodies, which are passed `heap` + `interns` rather than a VM.
    #[must_use]
    pub(crate) fn py_type_heap(&self, heap: &Heap<impl ResourceTracker>) -> Type {
        match self {
            Self::Ref(id) => heap.get(*id).py_type(),
            _ => self.py_type_shallow(),
        }
    }

    /// Resolved display name of this value's type for error messages and
    /// reprs — user-class instances render as their real class name rather
    /// than the generic `"object"`.
    ///
    /// The result borrows only `vm.interns` (never the heap), so it can be
    /// captured before `drop_with` cleanup and formatted after.
    #[must_use]
    pub(crate) fn py_type_name<'h>(&self, vm: &VM<'h, impl ResourceTracker>) -> Cow<'h, str> {
        self.py_type(vm).name(vm.heap, vm.interns)
    }

    /// [`py_type_name`](Self::py_type_name) for contexts without a `&VM` —
    /// notably the macro-generated `from_args` bodies, which are passed
    /// `heap` + `interns` instead (mirrors [`py_type_heap`](Self::py_type_heap)).
    #[must_use]
    pub(crate) fn py_type_name_heap<'i>(
        &self,
        heap: &Heap<impl ResourceTracker>,
        interns: &'i Interns,
    ) -> Cow<'i, str> {
        self.py_type_heap(heap).name(heap, interns)
    }

    /// Returns the Python `Type` for immediate (non-heap) values without VM access.
    ///
    /// For `Value::Ref` variants this cannot determine the concrete type (that requires
    /// reading from the heap), so it falls back to `Type::NoneType` as a sentinel.
    /// Callers handling `Ref` should use `HeapData::py_type()` on the resolved data instead.
    #[must_use]
    pub(crate) fn py_type_shallow(&self) -> Type {
        match self {
            Self::Undefined | Self::None => Type::NoneType,
            Self::Ellipsis => Type::Ellipsis,
            Self::Bool(_) => Type::Bool,
            Self::Int(_) | Self::InternLongInt(_) => Type::Int,
            Self::Float(_) => Type::Float,
            Self::InternString(_) => Type::Str,
            Self::InternBytes(_) => Type::Bytes,
            Self::Builtin(_) => Type::BuiltinFunction,
            Self::ModuleFunction(_) | Self::DefFunction(_) | Self::ExtFunction(_) => Type::Function,
            Self::Marker(_) => Type::SpecialForm,
            Self::Property(_) => Type::Property,
            Self::Ref(_) => Type::NoneType, // callers should resolve Ref via HeapData::py_type()
            #[cfg(feature = "memory-model-checks")]
            Self::Dereferenced => Type::NoneType,
        }
    }

    /// Returns the Python-visible `id()` for this value.
    ///
    /// `ExtFunction` values (inline `Value::ExtFunction` or heap
    /// `HeapData::ExtFunction`) get a name-derived id so that two external
    /// function values with the same name always satisfy CPython's invariant
    /// `a is b ⇒ id(a) == id(b)` — needed because `MontyObject::Function`
    /// conversion has discarded host object identity. All other variants use
    /// a representation-based id.
    ///
    /// For immediate values (Int, Float, Builtins), this computes a deterministic ID
    /// based on the value's hash, avoiding heap allocation. This means `id(5) == id(5)` will
    /// return True (unlike CPython for large integers outside the interning range).
    ///
    /// Singletons (None, True, False, etc.) return IDs from a dedicated tagged range.
    /// Interned strings/bytes use their interner index for stable identity.
    /// Heap-allocated values (Ref) reuse their `HeapId` inside the heap-tagged range,
    /// except for heap `ExtFunction` which uses the name-derived id described above.
    pub fn id(&self, vm: &VM<'_, impl ResourceTracker>) -> usize {
        match self {
            // ExtFunction id is name-derived so the inline and heap representations
            // agree; this also keeps `is(a, b) ⇒ id(a) == id(b)`. The guarded `Ref`
            // arm must precede the bare `Ref` arm below — match evaluation is
            // top-to-bottom.
            Self::ExtFunction(name_id) => ext_function_value_id(vm.interns.get_str(*name_id)),
            Self::Ref(id) if let HeapData::ExtFunction(name) = vm.heap.get(*id) => ext_function_value_id(name.as_str()),
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
            // Markers get deterministic IDs based on discriminant
            Self::Marker(m) => marker_value_id(*m),
            // Properties get deterministic IDs based on discriminant
            Self::Property(p) => property_value_id(*p),
            #[cfg(feature = "memory-model-checks")]
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
    pub fn module_name(&self, vm: &mut VM<'_, impl ResourceTracker>) -> String {
        match self {
            Self::Ref(id) => match vm.heap.get(*id) {
                HeapData::Module(module) => vm.interns.get_str(module.name()).to_string(),
                _ => "<unknown>".to_string(),
            },
            _ => "<unknown>".to_string(),
        }
    }

    /// Python-visible `is` operator. Identity is name-based for `ExtFunction`
    /// values via [`Value::id`], so two callables with the same `__name__`
    /// compare identical regardless of representation.
    pub fn is(&self, other: &Self, vm: &VM<'_, impl ResourceTracker>) -> bool {
        self.id(vm) == other.id(vm)
    }

    /// Python `==`, resolved to a definite boolean.
    ///
    /// Implements CPython's reflected comparison protocol on top of the
    /// one-sided [`PyTrait::py_eq_impl`]: tries `self == other`, and if that is
    /// `NotImplemented` (`None`) tries the reflected `other == self`. If neither
    /// operand's type recognises the other, the values are unequal. This is the
    /// entry point the VM `==`/`!=`/`in` operators and all container element
    /// comparisons use; per-type `py_eq_impl` impls never drive reflection themselves.
    pub fn py_eq(&self, other: &Self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<bool> {
        if let Some(result) = self.py_eq_impl(other, vm)? {
            Ok(result)
        } else if let Some(result) = other.py_eq_impl(self, vm)? {
            Ok(result)
        } else {
            Ok(false)
        }
    }

    /// Reads the heap entry this value references, or `None` if it is not a
    /// heap reference.
    ///
    /// Used by per-type [`PyTrait::py_eq_impl`] impls to resolve the other operand
    /// to a heap object of their own type, returning `NotImplemented` otherwise.
    pub(crate) fn read_heap<'a>(&self, vm: &VM<'a, impl ResourceTracker>) -> Option<HeapReadOutput<'a>> {
        match self {
            Self::Ref(id) => Some(vm.heap.read(*id)),
            _ => None,
        }
    }

    /// Computes the hash value for this value, used for dict keys.
    ///
    /// Returns `Ok(Some(hash))` for hashable types (immediate values and immutable heap types).
    /// Returns `Ok(None)` for unhashable types (list, dict).
    /// Returns `Err(ResourceError::Recursion)` if the recursion limit is exceeded
    /// while hashing deeply nested containers (e.g., tuples of tuples).
    ///
    /// For heap-allocated values (Ref variant), this computes the hash lazily
    /// on first use and caches it for subsequent calls.
    pub fn py_hash(&self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Option<HashValue>> {
        // The hot arms (int/str/ref) return precomputed or cached hashes; only the
        // cold arms construct a hasher, via `hash_one`.
        match self {
            Self::InternString(string_id) => Ok(Some(vm.interns.str_hash(*string_id))),
            Self::InternBytes(bytes_id) => Ok(Some(vm.interns.bytes_hash(*bytes_id))),
            Self::InternLongInt(long_int_id) => Ok(Some(vm.interns.long_int_hash(*long_int_id))),
            // Bool and int hash directly as their value, and are equivalent
            Self::Bool(b) => Ok(Some(HashValue::new((*b).into()))),
            Self::Int(i) => Ok(Some(HashValue::new(i.cast_unsigned()))),
            Self::Float(f) => {
                // 2^63, the first power of two past i64::MAX (exactly representable).
                const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;
                if f.fract() != 0.0 || !f.is_finite() {
                    // Non-integral or non-finite: hash the bit representation.
                    Ok(Some(HashValue::new(f.to_bits())))
                } else if *f >= -TWO_POW_63 && *f < TWO_POW_63 {
                    // Integral float in i64 range hashes as the equivalent int
                    // (e.g. `1.0` hashes the same as `1`).
                    #[expect(clippy::cast_possible_truncation)]
                    Ok(Some(HashValue::new((*f as i64).cast_unsigned())))
                } else {
                    // Integral float outside i64 range hashes as the equivalent
                    // big int, so an exactly-equal `float`/`int` pair (e.g.
                    // `2.0**100 == 2**100`) preserves `hash(a) == hash(b)`.
                    Ok(Some(hash_python_long_int(
                        &BigInt::from_f64(*f).expect("finite f64 converts to BigInt"),
                    )))
                }
            }
            // For heap-allocated values, dispatch to the per-type `py_hash`
            // impl. Types that benefit from caching (Str/Bytes/Tuple/
            // NamedTuple/FrozenSet/Path) carry an inline `cached_hash`;
            // cheap-to-hash types recompute each call.
            Self::Ref(id) => vm.heap.read(*id).py_hash(*id, vm),
            // Singleton values hash by discriminant
            Self::Undefined | Self::Ellipsis | Self::None => Ok(Some(hash_one(discriminant(self)))),
            Self::Builtin(b) => Ok(Some(hash_one(b))),
            Self::ModuleFunction(mf) => Ok(Some(hash_one(mf))),
            // Hash functions based on function ID
            Self::DefFunction(f_id) => Ok(Some(hash_one(f_id))),
            // Hash the function name's string contents so the inline path
            // agrees with the heap `HeapData::ExtFunction` arm in `heap_data.rs`.
            // Required so cross-representation equality (added in the same fix
            // series) preserves the dict invariant `a == b ⇒ hash(a) == hash(b)`.
            Self::ExtFunction(name_id) => Ok(Some(hash_python_str(vm.interns.get_str(*name_id)))),
            // Markers are hashable based on their discriminant
            Self::Marker(m) => Ok(Some(hash_one(m))),
            // Properties are hashable based on their OS function discriminant
            Self::Property(p) => Ok(Some(hash_one(p))),
            #[cfg(feature = "memory-model-checks")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    /// TODO this doesn't have many tests!!! also doesn't cover bytes
    /// Checks if `item` is contained in `self` (the container).
    ///
    /// Implements Python's `in` operator for various container types:
    /// - List/Tuple: linear search with equality
    /// - Dict: key lookup
    /// - Set/FrozenSet: element lookup
    /// - Str: substring search
    pub fn py_contains(&self, item: &Self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<bool> {
        match self {
            Self::Ref(heap_id) => {
                let output = vm.heap.read(*heap_id);
                match output {
                    HeapReadOutput::List(list) => {
                        let len = list.get(vm.heap).len();
                        for i in 0..len {
                            let el = list.clone_item(i, vm);
                            let eq = item.py_eq(&el, vm);
                            el.drop_with(vm);
                            if eq? {
                                return Ok(true);
                            }
                        }
                        Ok(false)
                    }
                    HeapReadOutput::Tuple(tuple) => {
                        let len = tuple.get(vm.heap).as_slice().len();
                        for i in 0..len {
                            let el = tuple.clone_item(i, vm);
                            let eq = item.py_eq(&el, vm);
                            el.drop_with(vm);
                            if eq? {
                                return Ok(true);
                            }
                        }
                        Ok(false)
                    }
                    HeapReadOutput::Dict(dict) => dict.contains_key(item, vm),
                    HeapReadOutput::DictKeysView(view) => {
                        let dict_id = view.get(vm.heap).dict_id();
                        let HeapReadOutput::Dict(dict) = vm.heap.read(dict_id) else {
                            panic!("dict_keys view must reference a dict");
                        };
                        dict.contains_key(item, vm)
                    }
                    HeapReadOutput::DictItemsView(view) => {
                        let dict_id = view.get(vm.heap).dict_id();
                        let Some((key, value)) = cloned_items_view_candidate(item, vm) else {
                            return Ok(false);
                        };
                        let mut key_guard = DropGuard::new(key, vm);
                        let (key, vm) = key_guard.as_parts_mut();
                        let mut value_guard = DropGuard::new(value, vm);
                        let (value, vm) = value_guard.as_parts_mut();
                        let HeapReadOutput::Dict(dict) = vm.heap.read(dict_id) else {
                            panic!("dict_items view must reference a dict");
                        };
                        match dict.dict_get(key, vm) {
                            Ok(Some(existing_value)) => {
                                let result = value.py_eq(&existing_value, vm);
                                existing_value.drop_with(vm);
                                result
                            }
                            Ok(None) => Ok(false),
                            Err(e) => Err(e),
                        }
                    }
                    HeapReadOutput::DictValuesView(view) => {
                        let dict_id = view.get(vm.heap).dict_id();
                        let HeapReadOutput::Dict(dict) = vm.heap.read(dict_id) else {
                            panic!("dict_values view must reference a dict");
                        };
                        // Iterate by index, cloning each value for py_eq comparison
                        let len = dict.get(vm.heap).len();
                        for i in 0..len {
                            // Two-phase clone: read ref discriminant, then inc_ref
                            let ref_id = match dict.get(vm.heap).value_at(i) {
                                Some(Self::Ref(id)) => Some(*id),
                                _ => None,
                            };
                            let el = if let Some(id) = ref_id {
                                vm.heap.inc_ref(id);
                                Self::Ref(id)
                            } else {
                                dict.get(vm.heap).value_at(i).expect("index valid").clone_immediate()
                            };
                            let eq = item.py_eq(&el, vm);
                            el.drop_with(vm);
                            if eq? {
                                return Ok(true);
                            }
                        }
                        Ok(false)
                    }
                    HeapReadOutput::Set(set) => set.contains(item, vm),
                    HeapReadOutput::FrozenSet(fset) => fset.contains(item, vm),
                    HeapReadOutput::Str(s) => {
                        let s_str = s.get(vm.heap).as_str();
                        str_contains(s_str, item, vm.heap, vm.interns)
                    }
                    HeapReadOutput::Range(range) => {
                        // Range containment is O(1) - check bounds and step alignment
                        let range = range.get(vm.heap);
                        let n = match item {
                            Self::Int(i) => *i,
                            Self::Bool(b) => i64::from(*b),
                            Self::Float(f) => {
                                if f.fract() != 0.0 {
                                    return Ok(false);
                                }
                                let int_val = f.trunc();
                                if int_val < i64::MIN as f64 || int_val > i64::MAX as f64 {
                                    return Ok(false);
                                }
                                #[expect(clippy::cast_possible_truncation)]
                                let n = int_val as i64;
                                n
                            }
                            _ => return Ok(false),
                        };
                        Ok(range.contains(n))
                    }
                    _ => {
                        let type_name = self.py_type_name(vm);
                        Err(ExcType::type_error(format!(
                            "argument of type '{type_name}' is not a container or iterable"
                        )))
                    }
                }
            }
            Self::InternString(string_id) => {
                let container_str = vm.interns.get_str(*string_id);
                str_contains(container_str, item, vm.heap, vm.interns)
            }
            _ => {
                let type_name = self.py_type_name(vm);
                Err(ExcType::type_error(format!(
                    "argument of type '{type_name}' is not a container or iterable"
                )))
            }
        }
    }

    /// Gets an attribute from this value.
    ///
    /// Dispatches to `py_getattr` on the underlying types where appropriate.
    /// Accepts `EitherStr` to support both interned and heap-allocated attribute names.
    ///
    /// Returns `AttributeError` for other types or unknown attributes.
    pub fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<CallResult> {
        match self {
            // Instances resolve attributes (instance dict → class methods/vars,
            // binding methods) in a dedicated path that has the heap id needed to
            // build bound methods.
            Self::Ref(heap_id) if matches!(vm.heap.get(*heap_id), HeapData::Instance(_)) => {
                return instance_getattr(*heap_id, attr, vm);
            }
            Self::Ref(heap_id) => {
                if let Some(call_result) = vm.heap.read(*heap_id).py_getattr(attr, vm)? {
                    return Ok(call_result);
                }
            }
            Self::Builtin(Builtins::Type(t)) => {
                // Handle type object attributes like __name__
                let is_dunder_name = attr.static_string().map_or_else(
                    || attr.as_str(vm.interns) == "__name__",
                    |ss| ss == StaticStrings::DunderName,
                );
                if is_dunder_name {
                    return Ok(CallResult::Value(allocate_string(
                        t.name(vm.heap, vm.interns),
                        vm.heap,
                    )?));
                }
                if *t == Type::TimeZone && attr.as_str(vm.interns) == "utc" {
                    return Ok(CallResult::Value(vm.heap.get_timezone_utc()?));
                }
            }
            _ => {}
        }
        let type_name = self.py_type_name(vm);
        Err(ExcType::attribute_error(type_name, attr.as_str(vm.interns)))
    }

    /// Sets an attribute on this value.
    ///
    /// Only Dataclass objects, user-defined class instances, and class objects
    /// support attribute setting. Returns AttributeError for other types.
    ///
    /// Takes ownership of `value` and drops it on error.
    /// On success, drops the old attribute value if one existed.
    pub fn py_set_attr(&self, name: &EitherStr, value: Self, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<()> {
        if let Self::Ref(heap_id) = self {
            let old_value = match vm.heap.read(*heap_id) {
                HeapReadOutput::Dataclass(mut dc) => dc.set_attr(Self::attr_name_value(name, vm)?, value, vm)?,
                HeapReadOutput::Instance(mut instance) => {
                    instance.set_attr(Self::attr_name_value(name, vm)?, value, vm)?
                }
                HeapReadOutput::Class(mut class) => class.set_attr(Self::attr_name_value(name, vm)?, value, vm)?,
                other => {
                    let type_name = other.py_type(vm).name(vm.heap, vm.interns);
                    value.drop_with(vm);
                    return Err(ExcType::attribute_error_no_setattr(&type_name, name.as_str(vm.interns)));
                }
            };
            old_value.drop_with(vm);
            Ok(())
        } else {
            let type_name = self.py_type_name(vm);
            value.drop_with(vm);
            Err(ExcType::attribute_error_no_setattr(&type_name, name.as_str(vm.interns)))
        }
    }

    /// Converts an attribute `name` into a dict-key `Value` for `set_attr` calls,
    /// reusing the interned id when available.
    fn attr_name_value(name: &EitherStr, vm: &VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        Ok(match name {
            EitherStr::Interned(string_id) => Self::InternString(*string_id),
            // TODO: should avoid needing to clone String via `EitherStr` - maybe
            // `EitherStr` should store a `HeapRead<Str>`?
            EitherStr::Heap(s) => allocate_string(s.as_str(), vm.heap)?,
        })
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
    pub fn as_int(&self, vm: &VM<'_, impl ResourceTracker>) -> RunResult<i64> {
        match self {
            Self::Int(i) => Ok(*i),
            Self::Ref(heap_id) => {
                if let HeapData::LongInt(li) = vm.heap.get(*heap_id) {
                    li.to_i64().ok_or_else(ExcType::overflow_c_ssize_t)
                } else {
                    let msg = format!("'{}' object cannot be interpreted as an integer", self.py_type_name(vm));
                    Err(SimpleException::new_msg(ExcType::TypeError, msg).into())
                }
            }
            _ => {
                let msg = format!("'{}' object cannot be interpreted as an integer", self.py_type_name(vm));
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
    pub fn as_index(&self, vm: &VM<'_, impl ResourceTracker>, container_type: Type) -> RunResult<i64> {
        match self {
            Self::Int(i) => Ok(*i),
            Self::Bool(b) => Ok(i64::from(*b)),
            Self::Ref(heap_id) => {
                if let HeapData::LongInt(li) = vm.heap.get(*heap_id) {
                    li.to_i64().ok_or_else(ExcType::index_error_int_too_large)
                } else {
                    Err(ExcType::type_error_indices(container_type, &self.py_type_name(vm)))
                }
            }
            _ => Err(ExcType::type_error_indices(container_type, &self.py_type_name(vm))),
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
        vm: &mut VM<'_, impl ResourceTracker>,
    ) -> Result<Self, RunError> {
        // Capture types for error messages
        let lhs_type = self.py_type(vm);
        let lhs_name = self.py_type_name(vm);
        let rhs_name = other.py_type_name(vm);

        // Extract BigInt from all numeric types
        let lhs_bigint = extract_bigint(self, vm.heap);
        let rhs_bigint = extract_bigint(other, vm.heap);

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
                        check_lshift_size(l.bits(), shift_u64, vm.heap.tracker())?;
                        l << shift_u64
                    } else if r.sign() == num_bigint::Sign::Minus {
                        return Err(ExcType::value_error_negative_shift_count());
                    } else {
                        // Shift amount too large to fit in i64 - this would be astronomically large
                        return Err(ExcType::overflow_c_ssize_t());
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
            LongInt::new(result).into_value(vm.heap).map_err(Into::into)
        } else {
            Err(ExcType::binary_type_error(op.as_str(), lhs_type, lhs_name, rhs_name))
        }
    }

    /// Clones an value with proper heap reference counting.
    ///
    /// For immediate values (Int, Bool, None, etc.), this performs a simple copy.
    /// For heap-allocated values (Ref variant), this increments the reference count
    /// and returns a new reference to the same heap value.
    ///
    /// Takes `ContainsHeap` to allow directly passing the `VM` in many contexts. Where
    /// borrow checking creates conflicts, it may be preferred to pass `&Heap` directly
    /// (e.g. as `vm.heap` / `self.heap` etc.).
    ///
    /// # Important
    /// This method MUST be used instead of the derived `Clone` implementation to ensure
    /// proper reference counting. Using `.clone()` directly will bypass reference counting
    /// and cause memory leaks or double-frees.
    #[must_use]
    pub fn clone_with_heap(&self, heap: &impl ContainsHeap) -> Self {
        match self {
            Self::Ref(id) => {
                heap.heap().inc_ref(*id);
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
    /// Takes `ContainsHeap` to allow directly passing the `VM` in many contexts. Where
    /// borrow checking creates conflicts, it may be preferred to pass `&mut Heap` directly
    /// (e.g. as `vm.heap` / `self.heap` etc.).
    ///
    /// # Important
    /// This method MUST be called before overwriting a namespace slot or discarding
    /// a value to prevent memory leaks.
    #[cfg(not(feature = "memory-model-checks"))]
    #[inline]
    pub fn drop_with(self, heap: &mut impl ContainsHeap) {
        if let Self::Ref(id) = self {
            heap.heap_mut().dec_ref(id);
        }
    }
    /// With `memory-model-checks` enabled, `Ref` variants are replaced with `Dereferenced` and
    /// the original is forgotten to prevent the Drop impl from panicking. Non-Ref variants
    /// are left unchanged since they don't trigger the Drop panic.
    #[cfg(feature = "memory-model-checks")]
    pub fn drop_with(mut self, heap: &mut impl ContainsHeap) {
        let old = mem::replace(&mut self, Self::Dereferenced);
        if let Self::Ref(id) = &old {
            heap.heap_mut().dec_ref(*id);
            mem::forget(old);
        }
    }

    /// Internal helper for copying immediate values without heap interaction.
    ///
    /// This method should only be called by `clone_with_heap()` for immediate values.
    /// Attempting to clone a Ref variant will panic.
    pub fn clone_immediate(&self) -> Self {
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
            Self::Ref(_) => panic!("Ref clones must go through clone_with_heap to maintain refcounts"),
            #[cfg(feature = "memory-model-checks")]
            Self::Dereferenced => panic!("Cannot copy Dereferenced object"),
        }
    }

    /// Mark as Dereferenced to prevent Drop panic
    ///
    /// This should be called from `py_dec_ref_ids` methods only
    #[cfg(feature = "memory-model-checks")]
    pub fn dec_ref_forget(&mut self) {
        let old = mem::replace(self, Self::Dereferenced);
        mem::forget(old);
    }

    /// Pushes any contained `HeapId` onto the stack for reference counting.
    ///
    /// For `Value::Ref` variants, pushes the heap ID so the referenced object's
    /// refcount can be decremented. When `memory-model-checks` is enabled, also marks
    /// this value as `Dereferenced` to prevent Drop panics.
    pub fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Self::Ref(id) = self {
            stack.push(*id);
            #[cfg(feature = "memory-model-checks")]
            self.dec_ref_forget();
        }
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

    /// Borrows the value as a `&str` if it is a string (interned or heap `str`).
    ///
    /// Unlike [`as_either_str`](Self::as_either_str) this never allocates — both
    /// interned and heap strings are returned by borrow — and it errors rather
    /// than returning `None` for non-strings, with CPython's generic `expected
    /// string, not <type>` message. Use it wherever a function reads a `str`
    /// argument it does not need to own; the borrow keeps `self` and the heap
    /// pinned, so drop/allocate only once it ends.
    pub(crate) fn to_str<'a>(&'a self, vm: &'a VM<'_, impl ResourceTracker>) -> RunResult<&'a str> {
        self.to_str_heap(vm.heap, vm.interns)
    }

    /// [`to_str`](Self::to_str) for contexts without a `&VM` — takes `heap` and
    /// `interns` separately so callers can keep a disjoint `&mut` borrow of
    /// another `VM` field alive (e.g. resolving a `str` `Value` produced by
    /// `py_str` while writing it to `vm.print_writer`).
    pub(crate) fn to_str_heap<'a>(
        &'a self,
        heap: &'a Heap<impl ResourceTracker>,
        interns: &'a Interns,
    ) -> RunResult<&'a str> {
        match self {
            Self::InternString(string_id) => return Ok(interns.get_str(*string_id)),
            Self::Ref(heap_id) => {
                if let HeapData::Str(s) = heap.get(*heap_id) {
                    return Ok(s.as_str());
                }
            }
            _ => {}
        }
        Err(ExcType::type_error(format!(
            "expected string, not {}",
            self.py_type_name_heap(heap, interns)
        )))
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

// ---------------------------------------------------------------------------
// Shared one-sided equality helpers
//
// Each compares a primitive operand (extracted from either an inline `Value`
// or a heap object) against an arbitrary `other: &Value`, resolving `other`'s
// representation as needed. They return `None` (NotImplemented) when `other`
// is not a compatible Python type, so the reflected comparison can run. These
// are shared by `Value::py_eq_impl` (inline operands) and
// `HeapReadOutput::py_eq_impl` (heap operands) so the interned-vs-heap and
// numeric-tower logic lives once.
// ---------------------------------------------------------------------------

/// `a == other` over Python's numeric tower (`int`/`bool`/`float`/big `int`).
pub(crate) fn eq_i64(a: i64, other: &Value, vm: &VM<'_, impl ResourceTracker>) -> Option<bool> {
    match other {
        Value::Int(b) => Some(a == *b),
        Value::Bool(b) => Some(a == i64::from(*b)),
        Value::Float(f) => Some(i64_cmp_f64(a, *f) == Some(Ordering::Equal)),
        Value::InternLongInt(id) => Some(bigint_eq_i64(vm.interns.get_long_int(*id), a)),
        Value::Ref(id) if let HeapData::LongInt(li) = vm.heap.get(*id) => Some(bigint_eq_i64(li.inner(), a)),
        _ => None,
    }
}

/// `f == other`, comparing against ints/bools/big ints *exactly* (no rounding).
pub(crate) fn eq_f64(f: f64, other: &Value, vm: &VM<'_, impl ResourceTracker>) -> Option<bool> {
    match other {
        Value::Float(o) => Some(f == *o),
        Value::Int(o) => Some(i64_cmp_f64(*o, f) == Some(Ordering::Equal)),
        Value::Bool(o) => Some(i64_cmp_f64(i64::from(*o), f) == Some(Ordering::Equal)),
        Value::InternLongInt(id) => Some(bigint_eq_f64(vm.interns.get_long_int(*id), f)),
        Value::Ref(id) if let HeapData::LongInt(li) = vm.heap.get(*id) => Some(bigint_eq_f64(li.inner(), f)),
        _ => None,
    }
}

/// `b == other` over the numeric tower, for heap `LongInt` / interned long-int
/// operands. A heap `LongInt` is always outside i64 range, so it never equals
/// an `Int`/`Bool` — but comparing exactly keeps the logic uniform.
pub(crate) fn eq_bigint(b: &BigInt, other: &Value, vm: &VM<'_, impl ResourceTracker>) -> Option<bool> {
    match other {
        Value::Int(o) => Some(bigint_eq_i64(b, *o)),
        Value::Bool(o) => Some(bigint_eq_i64(b, i64::from(*o))),
        Value::Float(f) => Some(bigint_eq_f64(b, *f)),
        Value::InternLongInt(id) => Some(b == vm.interns.get_long_int(*id)),
        Value::Ref(id) if let HeapData::LongInt(li) = vm.heap.get(*id) => Some(b == li.inner()),
        _ => None,
    }
}

/// `s == other`, resolving the other operand from an interned or heap string.
pub(crate) fn eq_str(s: &str, other: &Value, vm: &VM<'_, impl ResourceTracker>) -> Option<bool> {
    match other {
        Value::InternString(id) => Some(s == vm.interns.get_str(*id)),
        Value::Ref(id) if let HeapData::Str(o) = vm.heap.get(*id) => Some(s == o.as_str()),
        _ => None,
    }
}

/// `b == other`, resolving the other operand from interned or heap bytes.
pub(crate) fn eq_bytes(b: &[u8], other: &Value, vm: &VM<'_, impl ResourceTracker>) -> Option<bool> {
    match other {
        Value::InternBytes(id) => Some(b == vm.interns.get_bytes(*id)),
        Value::Ref(id) if let HeapData::Bytes(o) = vm.heap.get(*id) => Some(b == o.as_slice()),
        _ => None,
    }
}

/// External functions compare equal iff their names match — used by both the
/// inline `Value::ExtFunction` and heap `HeapData::ExtFunction` representations. (#347)
pub(crate) fn eq_ext_function(name: &str, other: &Value, vm: &VM<'_, impl ResourceTracker>) -> Option<bool> {
    match other {
        Value::ExtFunction(id) => Some(name == vm.interns.get_str(*id)),
        Value::Ref(id) if let HeapData::ExtFunction(o) = vm.heap.get(*id) => Some(name == o.as_str()),
        _ => None,
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

    /// Converts this `EitherStr` into an owned `String`.
    ///
    /// For interned strings, looks up and clones the string content.
    /// For heap strings, returns the owned string directly.
    pub fn into_string(self, interns: &Interns) -> String {
        match self {
            Self::Interned(id) => interns.get_str(id).to_owned(),
            Self::Heap(s) => s,
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
    pub(crate) fn py_repr_fmt(self, f: &mut impl Write) -> fmt::Result {
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

/// Computes Python-style floor division and modulo.
///
/// Python's division rounds toward negative infinity (floor division),
/// and the remainder has the same sign as the divisor.
/// This differs from Rust's truncating division.
///
/// Returns `None` on overflow (i64::MIN / -1 doesn't fit in i64).
pub(crate) fn floor_divmod(a: i64, b: i64) -> Option<(i64, i64)> {
    let quot = a.checked_div(b)?;
    let rem = a.checked_rem(b)?;

    if rem != 0 && (rem < 0) != (b < 0) {
        Some((quot - 1, rem + b))
    } else {
        Some((quot, rem))
    }
}

/// Computes Python-style float modulo (CPython's `float_rem`).
///
/// Unlike Rust's `%` (which follows the dividend's sign), the result takes the
/// divisor's sign — `-7.0 % 3.0 == 2.0` — and a zero result gets the divisor's
/// sign too (`6.0 % -3.0 == -0.0`). Callers must reject a zero divisor first
/// (`ZeroDivisionError`); this helper assumes `b != 0`.
fn py_float_mod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r == 0.0 {
        0.0f64.copysign(b)
    } else if (b < 0.0) != (r < 0.0) {
        r + b
    } else {
        r
    }
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

/// Computes a deterministic ID for an external function from its name string.
///
/// Used by [`Value::id`] so that inline `Value::ExtFunction` and heap
/// `HeapData::ExtFunction` values referring to the same function name share
/// the same Python-visible `id()` — required by CPython's
/// `a is b ⇒ id(a) == id(b)` invariant. Collisions across distinct names are
/// possible (the masked hash space is finite) but acceptable: Python's `id()`
/// is allowed to collide across distinct objects.
#[inline]
fn ext_function_value_id(name: &str) -> usize {
    let hash_u64 = hash_python_str(name).raw();
    // Mask to usize range before conversion to handle 32-bit platforms.
    let masked = hash_u64 & (usize::MAX as u64);
    let hash_usize = usize::try_from(masked).expect("masked value fits in usize");
    EXTFUNCTION_ID_TAG | (hash_usize & EXTFUNCTION_ID_MASK)
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

/// Extracts and clones the `(key, value)` probe accepted by `dict_items.__contains__`.
///
/// CPython treats only 2-tuples as valid probes for items-view membership. Monty
/// also accepts namedtuples of length two so tuple-like runtime values behave
/// sensibly even though namedtuples are not modeled as a true tuple subclass.
fn cloned_items_view_candidate(item: &Value, heap: &impl ContainsHeap) -> Option<(Value, Value)> {
    let Value::Ref(heap_id) = item else {
        return None;
    };

    match heap.heap().get(*heap_id) {
        HeapData::Tuple(tuple) => {
            let items = tuple.as_slice();
            if items.len() == 2 {
                Some((items[0].clone_with_heap(heap), items[1].clone_with_heap(heap)))
            } else {
                None
            }
        }
        HeapData::NamedTuple(namedtuple) => {
            let items = namedtuple.as_vec();
            if items.len() == 2 {
                Some((items[0].clone_with_heap(heap), items[1].clone_with_heap(heap)))
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
    heap: &Heap<impl ResourceTracker>,
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
    use crate::{
        PrintWriter, heap::HeapReader, intern::InternerBuilder, resource::NoLimitTracker, run::AssertMessageAnnotations,
    };

    /// Creates a heap and directly allocates a LongInt with the given BigInt value.
    ///
    /// This bypasses `LongInt::into_value()` which would demote i64-fitting values.
    /// Used to test defensive code paths that handle LongInt-as-index scenarios.
    fn create_heap_with_longint(value: BigInt) -> (Heap<NoLimitTracker>, HeapId) {
        let heap = Heap::new(16, NoLimitTracker);
        let long_int = LongInt::new(value);
        let heap_id = heap.allocate(HeapData::LongInt(long_int)).unwrap();
        (heap, heap_id)
    }

    /// Creates a minimal Interns for testing.
    fn create_test_interns() -> Interns {
        let interner = InternerBuilder::new("");
        Interns::new(interner, vec![])
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

        let mut interns = create_test_interns();
        let result = HeapReader::with(&mut heap, &mut interns, |reader, interns| {
            let vm = VM::new(
                Vec::new(),
                reader,
                interns,
                PrintWriter::Disabled,
                AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            );
            value.as_index(&vm, Type::List)
        });
        assert_eq!(result.unwrap(), 42);
        value.drop_with(&mut heap);
    }

    /// Tests that `as_index()` correctly handles a negative LongInt that fits in i64.
    #[test]
    fn as_index_longint_negative_fits_in_i64() {
        let (mut heap, heap_id) = create_heap_with_longint(BigInt::from(-100));
        let value = Value::Ref(heap_id);

        let mut interns = create_test_interns();
        let result = HeapReader::with(&mut heap, &mut interns, |reader, interns| {
            let vm = VM::new(
                Vec::new(),
                reader,
                interns,
                PrintWriter::Disabled,
                AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            );
            value.as_index(&vm, Type::List)
        });
        assert_eq!(result.unwrap(), -100);
        value.drop_with(&mut heap);
    }

    /// Tests that `as_index()` returns IndexError for LongInt values too large for i64.
    #[test]
    fn as_index_longint_too_large() {
        // 2^100 is way larger than i64::MAX
        let big_value = BigInt::from(2).pow(100);
        let (mut heap, heap_id) = create_heap_with_longint(big_value);
        let value = Value::Ref(heap_id);

        let mut interns = create_test_interns();
        let result = HeapReader::with(&mut heap, &mut interns, |reader, interns| {
            let vm = VM::new(
                Vec::new(),
                reader,
                interns,
                PrintWriter::Disabled,
                AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            );
            value.as_index(&vm, Type::List)
        });
        assert!(result.is_err());
        value.drop_with(&mut heap);
    }

    /// Tests that `as_int()` correctly handles a LongInt containing an i64-fitting value.
    ///
    /// Similar to `as_index`, this tests a defensive code path normally unreachable.
    #[test]
    fn as_int_longint_fits_in_i64() {
        let (mut heap, heap_id) = create_heap_with_longint(BigInt::from(12345));
        let value = Value::Ref(heap_id);

        let mut interns = create_test_interns();
        let result = HeapReader::with(&mut heap, &mut interns, |reader, interns| {
            let vm = VM::new(
                Vec::new(),
                reader,
                interns,
                PrintWriter::Disabled,
                AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            );
            value.as_int(&vm)
        });
        assert_eq!(result.unwrap(), 12345);
        value.drop_with(&mut heap);
    }

    /// Tests that `as_int()` returns an error for LongInt values too large for i64.
    #[test]
    fn as_int_longint_too_large() {
        let big_value = BigInt::from(2).pow(100);
        let (mut heap, heap_id) = create_heap_with_longint(big_value);
        let value = Value::Ref(heap_id);

        let mut interns = create_test_interns();
        let result = HeapReader::with(&mut heap, &mut interns, |reader, interns| {
            let vm = VM::new(
                Vec::new(),
                reader,
                interns,
                PrintWriter::Disabled,
                AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            );
            value.as_int(&vm)
        });
        assert!(result.is_err());
        value.drop_with(&mut heap);
    }

    /// Tests boundary values: i64::MAX as a LongInt.
    #[test]
    fn as_index_longint_at_i64_max() {
        let (mut heap, heap_id) = create_heap_with_longint(BigInt::from(i64::MAX));
        let value = Value::Ref(heap_id);

        let mut interns = create_test_interns();
        let result = HeapReader::with(&mut heap, &mut interns, |reader, interns| {
            let vm = VM::new(
                Vec::new(),
                reader,
                interns,
                PrintWriter::Disabled,
                AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            );
            value.as_index(&vm, Type::List)
        });
        assert_eq!(result.unwrap(), i64::MAX);
        value.drop_with(&mut heap);
    }

    /// Tests boundary values: i64::MIN as a LongInt.
    #[test]
    fn as_index_longint_at_i64_min() {
        let (mut heap, heap_id) = create_heap_with_longint(BigInt::from(i64::MIN));
        let value = Value::Ref(heap_id);

        let mut interns = create_test_interns();
        let result = HeapReader::with(&mut heap, &mut interns, |reader, interns| {
            let vm = VM::new(
                Vec::new(),
                reader,
                interns,
                PrintWriter::Disabled,
                AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            );
            value.as_index(&vm, Type::List)
        });
        assert_eq!(result.unwrap(), i64::MIN);
        value.drop_with(&mut heap);
    }

    /// Tests boundary values: i64::MAX + 1 as a LongInt (should fail).
    #[test]
    fn as_index_longint_just_over_i64_max() {
        let big_value = BigInt::from(i64::MAX) + BigInt::from(1);
        let (mut heap, heap_id) = create_heap_with_longint(big_value);
        let value = Value::Ref(heap_id);

        let mut interns = create_test_interns();
        let result = HeapReader::with(&mut heap, &mut interns, |reader, interns| {
            let vm = VM::new(
                Vec::new(),
                reader,
                interns,
                PrintWriter::Disabled,
                AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            );
            value.as_index(&vm, Type::List)
        });
        assert!(result.is_err());
        value.drop_with(&mut heap);
    }

    /// Tests boundary values: i64::MIN - 1 as a LongInt (should fail).
    #[test]
    fn as_index_longint_just_under_i64_min() {
        let big_value = BigInt::from(i64::MIN) - BigInt::from(1);
        let (mut heap, heap_id) = create_heap_with_longint(big_value);
        let value = Value::Ref(heap_id);

        let mut interns = create_test_interns();
        let result = HeapReader::with(&mut heap, &mut interns, |reader, interns| {
            let vm = VM::new(
                Vec::new(),
                reader,
                interns,
                PrintWriter::Disabled,
                AssertMessageAnnotations::DEFAULT_MAX_BYTES.get(),
            );
            value.as_index(&vm, Type::List)
        });
        assert!(result.is_err());
        value.drop_with(&mut heap);
    }
}
