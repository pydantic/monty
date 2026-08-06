use std::{
    borrow::Cow,
    cmp::Ordering,
    fmt::{self, Write},
    mem::{self, discriminant},
    str::FromStr,
};

use num_bigint::{BigInt, Sign};
use num_traits::FromPrimitive;

use crate::{
    builtins::Builtins,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    expressions::CmpOperator,
    fstring::FormatFloat,
    hash::{HashValue, hash_one, hash_python_long_int},
    heap::{ContainsHeap, DropWithContext, Heap, HeapData, HeapId, HeapReadOutput},
    heap_data::heap_subscript,
    identity::Identity,
    intern::{BytesId, FunctionId, Interns, LongIntId, StaticStrings, StringId},
    modules::ModuleFunctions,
    resource_checks::check_pow_size,
    types::{
        Bytes, BytesIterator, CmpOrder, LazyHeapSet, LongInt, Property, PyTrait, StringIterator, Type,
        bytes::{bytes_contains, bytes_repr_fmt, concat_bytes, get_byte_at_index, repeat_bytes},
        instance::{instance_dataclass_eq, instance_getattr, instance_repr_fmt, instance_str, instance_user_eq},
        long_int::{
            bigint_cmp_f64, bigint_cmp_i64, bigint_eq_f64, bigint_eq_i64, check_bits_str_digits_limit, i64_cmp_f64,
            repeat_count, wide_i128_into_value,
        },
        namedtuple::cmp_item_seqs,
        slice::slice_collect_iterator,
        str::{
            allocate_char, allocate_string, concat_allocate_str, get_char_at_index, repeat_str, str_contains,
            string_repr_fmt,
        },
    },
};

/// Primary value type representing Python objects at runtime.
///
/// This enum uses a hybrid design: small immediate values (Int, Bool, None) are stored
/// inline, while heap-allocated values (List, Str, Dict, etc.) are stored in the arena
/// and referenced via `Ref(HeapId)`.
///
/// NOTE: `Clone` is intentionally NOT derived. Use `clone_with_heap()`. Direct cloning via `.clone()` would
/// bypass reference counting and cause memory leaks.
///
/// NOTE: it's important to keep this size small to minimize memory overhead!
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum Value {
    // Immediate values (stored inline, no heap allocation)
    Undefined,
    Ellipsis,
    NotImplemented,
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

/// Scoped view of a value that keeps a referenced heap entry open for repeated operations.
pub(crate) enum ValueRead<'h, 'v> {
    /// Immediate values need no heap access.
    Immediate(&'v Value),
    /// Heap values retain both their owner and the typed heap read handle.
    Heap {
        /// The `Value::Ref` this view was taken from. Keeps the entry alive for
        /// the view's lifetime, and supplies the `HeapId` that `py_next` needs
        /// to drive a user-defined `__next__`.
        owner: &'v Value,
        value: HeapReadOutput<'h>,
    },
}

impl<'h> ValueRead<'h, '_> {
    /// Advances this value without reacquiring its heap entry.
    ///
    /// This is the timeout boundary for Rust-side loops over retained iterators.
    /// Bytecode iteration dispatches directly after the VM's per-opcode check.
    pub(crate) fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        vm.heap.check_time()?;
        match self {
            Self::Immediate(value) => Err(ExcType::type_error_not_iterator(&value.py_type_name(vm))),
            Self::Heap { owner, value } => value.py_next(owner.ref_id(), vm),
        }
    }

    /// Returns the iterator's internal remaining-length hint when available.
    pub(crate) fn iter_size_hint(&self, vm: &VM<'h>) -> usize {
        match self {
            Self::Heap {
                value: HeapReadOutput::ListIterator(iter),
                ..
            } => iter.get(vm.heap).size_hint(vm.heap),
            Self::Heap {
                value: HeapReadOutput::DequeIterator(iter),
                ..
            } => iter.get(vm.heap).size_hint(vm.heap),
            Self::Heap {
                value: HeapReadOutput::TupleIterator(iter),
                ..
            } => iter.get(vm.heap).size_hint(vm.heap),
            Self::Heap {
                value: HeapReadOutput::StringIterator(iter),
                ..
            } => iter.get(vm.heap).size_hint(vm),
            Self::Heap {
                value: HeapReadOutput::BytesIterator(iter),
                ..
            } => iter.get(vm.heap).size_hint(vm),
            Self::Heap {
                value: HeapReadOutput::RangeIterator(iter),
                ..
            } => iter.get(vm.heap).size_hint(),
            Self::Heap {
                value: HeapReadOutput::DictKeyIterator(iter),
                ..
            } => iter.get(vm.heap).size_hint(),
            Self::Heap {
                value: HeapReadOutput::DictItemIterator(iter),
                ..
            } => iter.get(vm.heap).size_hint(),
            Self::Heap {
                value: HeapReadOutput::DictValueIterator(iter),
                ..
            } => iter.get(vm.heap).size_hint(),
            Self::Heap {
                value: HeapReadOutput::SetIterator(iter),
                ..
            } => iter.get(vm.heap).size_hint(),
            Self::Heap {
                value: HeapReadOutput::Itertools(iter),
                ..
            } => iter.get(vm.heap).size_hint(),
            _ => 0,
        }
    }
}

/// Size of a single `Value` slot in bytes.
///
/// Used to preflight operations that allocate many value slots at once.
pub(crate) const VALUE_SIZE: usize = mem::size_of::<Value>();

/// Borrowed integer payload used to format a Python `id()` in callable reprs.
enum PythonIdDisplay<'a> {
    /// Identity that fits Monty's immediate integer representation.
    Int(i64),
    /// Arbitrary-precision identity borrowed from its heap value.
    LongInt(&'a BigInt),
}

impl<'a> PythonIdDisplay<'a> {
    /// Extracts the integer payload returned by the structural identity encoder.
    fn new(value: &'a Value, heap: &'a Heap) -> Self {
        match value {
            Value::Int(value) => Self::Int(*value),
            Value::Ref(id) => match heap.get(*id) {
                HeapData::LongInt(value) => Self::LongInt(value.inner()),
                _ => unreachable!("identity values are integers"),
            },
            _ => unreachable!("identity values are integers"),
        }
    }
}

impl fmt::LowerHex for PythonIdDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(value) => fmt::LowerHex::fmt(value, f),
            Self::LongInt(value) => fmt::LowerHex::fmt(value, f),
        }
    }
}

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
    fn py_type(&self, vm: &VM<'_>) -> Type {
        match self {
            Self::Undefined => panic!("Cannot get type of undefined value"),
            Self::Ellipsis => Type::Ellipsis,
            Self::NotImplemented => Type::NotImplementedType,
            Self::None => Type::NoneType,
            Self::Bool(_) => Type::Bool,
            Self::Int(_) | Self::InternLongInt(_) => Type::Int,
            Self::Float(_) => Type::Float,
            Self::InternString(_) => Type::Str,
            Self::InternBytes(_) => Type::Bytes,
            Self::Builtin(c) => c.py_type(),
            Self::ModuleFunction(_) => Type::BuiltinFunction,
            Self::DefFunction(_) => Type::Function,
            Self::Marker(m) => m.py_type(),
            Self::Property(_) => Type::Property,
            Self::Ref(id) => vm.heap.read(*id).py_type(vm),
            #[cfg(feature = "memory-model-checks")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    fn py_len(&self, vm: &VM<'_>) -> Option<usize> {
        match self {
            // Count Unicode characters, not bytes, to match Python semantics
            Self::InternString(string_id) => Some(vm.interns.get_str(*string_id).chars().count()),
            Self::InternBytes(bytes_id) => Some(vm.interns.get_bytes(*bytes_id).len()),
            Self::Ref(id) => vm.heap.read(*id).py_len(vm),
            _ => None,
        }
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'_>) -> RunResult<Option<bool>> {
        match self {
            // `Undefined` is a sentinel and is never equal to anything.
            Self::Undefined => Ok(Some(false)),

            Self::None => Ok(matches!(other, Self::None).then_some(true)),
            Self::Ellipsis => Ok(matches!(other, Self::Ellipsis).then_some(true)),
            Self::NotImplemented => Ok(matches!(other, Self::NotImplemented).then_some(true)),
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
            Self::Marker(m) => Ok(match other {
                Self::Marker(o) => Some(m == o),
                _ => None,
            }),
            Self::Property(p) => Ok(match other {
                Self::Property(o) => Some(p == o),
                _ => None,
            }),
            Self::Ref(id) => vm.heap.read(*id).py_eq_impl(other, vm),
            #[cfg(feature = "memory-model-checks")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    fn py_cmp(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<CmpOrder> {
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
                // A namedtuple orders like the tuple it subclasses, including
                // against a plain tuple in either direction. Both sides are
                // cloned first because the comparison needs `&mut VM`, which
                // cannot coexist with two live `HeapRead`s.
                (HeapReadOutput::NamedTuple(a), HeapReadOutput::NamedTuple(b)) => {
                    let (a, b) = (a.cloned_items(vm)?, b.cloned_items(vm)?);
                    cmp_item_seqs(a, b, vm)
                }
                (HeapReadOutput::NamedTuple(a), HeapReadOutput::Tuple(b)) => {
                    let (a, b) = (a.cloned_items(vm)?, b.cloned_items(vm)?);
                    cmp_item_seqs(a, b, vm)
                }
                (HeapReadOutput::Tuple(a), HeapReadOutput::NamedTuple(b)) => {
                    let (a, b) = (a.cloned_items(vm)?, b.cloned_items(vm)?);
                    cmp_item_seqs(a, b, vm)
                }
                (HeapReadOutput::List(a), HeapReadOutput::List(b)) => a.py_cmp(&b, vm),
                (HeapReadOutput::Deque(a), HeapReadOutput::Deque(b)) => a.py_cmp(&b, vm),
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

    fn py_bool(&self, vm: &mut VM<'_>) -> RunResult<bool> {
        match self {
            Self::NotImplemented => Err(SimpleException::new_msg(
                ExcType::TypeError,
                "NotImplemented should not be used in a boolean context",
            )
            .into()),
            Self::Ref(id) => vm.heap.read(*id).py_bool(vm),
            Self::Undefined | Self::None => Ok(false),
            Self::Ellipsis => Ok(true),
            Self::Bool(b) => Ok(*b),
            Self::Int(v) => Ok(*v != 0),
            Self::Float(f) => Ok(*f != 0.0),
            // InternLongInt is always truthy (if it were zero, it would fit in i64).
            Self::InternLongInt(_) => Ok(true),
            Self::Builtin(_) | Self::ModuleFunction(_) => Ok(true),
            Self::DefFunction(_) => Ok(true),
            Self::Marker(_) | Self::Property(_) => Ok(true),
            Self::InternString(string_id) => Ok(!vm.interns.get_str(*string_id).is_empty()),
            Self::InternBytes(bytes_id) => Ok(!vm.interns.get_bytes(*bytes_id).is_empty()),
            #[cfg(feature = "memory-model-checks")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'_>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let interns = vm.interns;
        match self {
            Self::Undefined => Ok(f.write_str("Undefined")?),
            Self::Ellipsis => Ok(f.write_str("Ellipsis")?),
            Self::NotImplemented => Ok(f.write_str("NotImplemented")?),
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
            Self::ModuleFunction(mf) => {
                let py_id = self.id().into_value(vm.heap);
                defer_drop!(py_id, vm);
                Ok(mf.py_repr_fmt(f, PythonIdDisplay::new(py_id, vm.heap))?)
            }
            Self::DefFunction(f_id) => {
                let py_id = self.id().into_value(vm.heap);
                defer_drop!(py_id, vm);
                Ok(interns
                    .get_function(*f_id)
                    .py_repr_fmt(f, interns, PythonIdDisplay::new(py_id, vm.heap))?)
            }
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
                        // A deque prints its items inside list-shaped brackets
                        // (`deque([...])`), so the placeholder is a list's.
                        HeapData::Deque(_) => Ok(f.write_str("[...]")?),
                        // Other types don't typically have cycles, but handle gracefully
                        _ => Ok(f.write_str("...")?),
                    }
                } else if matches!(vm.heap.get(*id), HeapData::Instance(_)) {
                    // Handled here, not at the heap level, because dispatch needs
                    // the heap id for `self`. A user `__repr__` recurses on the
                    // *Rust* stack (see limitations/classes.md); the synthesized
                    // dataclass form carries `heap_ids`, so a cycle in one hits
                    // the branch above.
                    instance_repr_fmt(*id, f, vm, heap_ids)
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
    fn py_repr(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        match self {
            Self::None => Ok(Self::InternString(StaticStrings::NoneRepr.into())),
            Self::Bool(true) => Ok(Self::InternString(StaticStrings::TrueRepr.into())),
            Self::Bool(false) => Ok(Self::InternString(StaticStrings::FalseRepr.into())),
            Self::Ellipsis => Ok(Self::InternString(StaticStrings::EllipsisRepr.into())),
            Self::NotImplemented => Ok(Self::InternString(StaticStrings::NotImplementedRepr.into())),
            Self::Int(i) => Ok(allocate_string(itoa::Buffer::new().format(*i), vm.heap)),
            _ => {
                let mut s = String::new();
                let mut heap_ids = LazyHeapSet::default();
                self.py_repr_fmt(&mut s, vm, &mut heap_ids)?;
                Ok(allocate_string(s, vm.heap))
            }
        }
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
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

    fn py_iadd_impl(&mut self, other: &Self, vm: &mut VM<'_>, _self_id: Option<HeapId>) -> RunResult<bool> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_iadd_impl(other, vm, Some(*id))
        } else {
            Ok(false)
        }
    }

    fn py_isub_impl(&mut self, other: &Self, vm: &mut VM<'_>, _self_id: Option<HeapId>) -> RunResult<bool> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_isub_impl(other, vm, Some(*id))
        } else {
            Ok(false)
        }
    }

    fn py_iand_impl(&mut self, other: &Self, vm: &mut VM<'_>, _self_id: Option<HeapId>) -> RunResult<bool> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_iand_impl(other, vm, Some(*id))
        } else {
            Ok(false)
        }
    }

    fn py_ior_impl(&mut self, other: &Self, vm: &mut VM<'_>, _self_id: Option<HeapId>) -> RunResult<bool> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_ior_impl(other, vm, Some(*id))
        } else {
            Ok(false)
        }
    }

    fn py_cmp_op(
        &self,
        other: &Self,
        op: CmpOperator,
        vm: &mut VM<'_>,
        _self_id: Option<HeapId>,
    ) -> RunResult<Option<bool>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_cmp_op(other, op, vm, Some(*id))
        } else {
            Ok(None)
        }
    }

    fn py_neg_impl(&self, vm: &mut VM<'_>, _self_id: Option<HeapId>) -> RunResult<Option<Self>> {
        match self {
            // `checked_neg` catches `i64::MIN`, whose negation only fits a LongInt.
            Self::Int(n) => match n.checked_neg() {
                Some(negated) => Ok(Some(Self::Int(negated))),
                None => Ok(Some((-LongInt::from(*n)).into_value(vm.heap))),
            },
            Self::Float(f) => Ok(Some(Self::Float(-f))),
            Self::Bool(b) => Ok(Some(Self::Int(if *b { -1 } else { 0 }))),
            Self::Ref(id) => vm.heap.read(*id).py_neg_impl(vm, Some(*id)),
            _ => Ok(None),
        }
    }

    fn py_pos_impl(&self, vm: &mut VM<'_>, _self_id: Option<HeapId>) -> RunResult<Option<Self>> {
        match self {
            // `+x` leaves a number as it is; the clone is what the caller pushes
            // in place of the operand it drops.
            Self::Int(_) | Self::Float(_) => Ok(Some(self.clone_with_heap(vm.heap))),
            Self::Bool(b) => Ok(Some(Self::Int(i64::from(*b)))),
            Self::Ref(id) => vm.heap.read(*id).py_pos_impl(vm, Some(*id)),
            _ => Ok(None),
        }
    }

    /// One-sided implementation of Python `+`.
    fn py_add_impl(&self, other: &Self, vm: &mut VM<'_>, _self_id: Option<HeapId>) -> RunResult<Option<Self>> {
        let interns = vm.interns;
        match (self, other) {
            // Int + Int with overflow detection
            (Self::Int(a), Self::Int(b)) => {
                if let Some(result) = a.checked_add(*b) {
                    Ok(Some(Self::Int(result)))
                } else {
                    Ok(Some(wide_i128_into_value(i128::from(*a) + i128::from(*b), vm.heap)))
                }
            }
            (Self::Float(v1), Self::Float(v2)) => Ok(Some(Self::Float(v1 + v2))),
            // Int + Float and Float + Int
            (Self::Int(a), Self::Float(b)) => Ok(Some(Self::Float(*a as f64 + b))),
            (Self::Float(a), Self::Int(b)) => Ok(Some(Self::Float(a + *b as f64))),
            (Self::InternString(s1), Self::InternString(s2)) => Ok(Some(concat_allocate_str(
                interns.get_str(*s1),
                interns.get_str(*s2),
                vm.heap,
            )?)),
            // for strings we need to account for the fact they might be either interned or not
            (Self::InternString(string_id), Self::Ref(id2)) if let HeapData::Str(s2) = vm.heap.get(*id2) => Ok(Some(
                concat_allocate_str(interns.get_str(*string_id), s2.as_str(), vm.heap)?,
            )),
            // same for bytes
            (Self::InternBytes(lhs), Self::InternBytes(rhs)) => Ok(Some(concat_bytes(
                interns.get_bytes(*lhs),
                interns.get_bytes(*rhs),
                vm.heap,
            )?)),
            (Self::InternBytes(lhs), Self::Ref(rhs)) if let HeapData::Bytes(rhs) = vm.heap.get(*rhs) => {
                Ok(Some(concat_bytes(interns.get_bytes(*lhs), rhs.as_slice(), vm.heap)?))
            }
            (Self::Ref(id), _) => vm.heap.read(*id).py_add_impl(other, vm, Some(*id)),
            _ => Ok(None),
        }
    }

    /// Reflected implementation of Python `+`.
    fn py_radd_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_radd_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// One-sided implementation of Python `-`.
    fn py_sub_impl(&self, other: &Self, vm: &mut VM<'_>, _self_id: Option<HeapId>) -> RunResult<Option<Self>> {
        match (self, other) {
            // Int - Int with overflow detection
            (Self::Int(a), Self::Int(b)) => {
                if let Some(result) = a.checked_sub(*b) {
                    Ok(Some(Self::Int(result)))
                } else {
                    Ok(Some(wide_i128_into_value(i128::from(*a) - i128::from(*b), vm.heap)))
                }
            }
            // Float - Float
            (Self::Float(a), Self::Float(b)) => Ok(Some(Self::Float(a - b))),
            // Int - Float and Float - Int
            (Self::Int(a), Self::Float(b)) => Ok(Some(Self::Float(*a as f64 - b))),
            (Self::Float(a), Self::Int(b)) => Ok(Some(Self::Float(a - *b as f64))),
            (Self::Ref(id), _) => vm.heap.read(*id).py_sub_impl(other, vm, Some(*id)),
            _ => Ok(None),
        }
    }

    /// Reflected implementation of Python `-`.
    fn py_rsub_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_rsub_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// One-sided implementation of Python `*`.
    fn py_mul_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => {
                if let Some(result) = a.checked_mul(*b) {
                    Ok(Some(Self::Int(result)))
                } else {
                    Ok(Some(wide_i128_into_value(i128::from(*a) * i128::from(*b), vm.heap)))
                }
            }
            (Self::Float(a), Self::Float(b)) => Ok(Some(Self::Float(a * b))),
            (Self::Int(a), Self::Float(b)) => Ok(Some(Self::Float(*a as f64 * b))),
            (Self::Float(a), Self::Int(b)) => Ok(Some(Self::Float(a * *b as f64))),
            (Self::Bool(a), Self::Int(b)) => Ok(Some(Self::Int(i64::from(*a) * b))),
            (Self::Int(a), Self::Bool(b)) => Ok(Some(Self::Int(a * i64::from(*b)))),
            (Self::Bool(a), Self::Float(b)) => Ok(Some(Self::Float(f64::from(*a) * b))),
            (Self::Float(a), Self::Bool(b)) => Ok(Some(Self::Float(a * f64::from(*b)))),
            (Self::Bool(a), Self::Bool(b)) => Ok(Some(Self::Int(i64::from(*a) * i64::from(*b)))),
            (Self::InternString(id), count) | (count, Self::InternString(id)) => {
                let Some(count) = repeat_count(count, vm)? else {
                    return Ok(None);
                };
                Ok(Some(repeat_str(vm.interns.get_str(*id), count, vm.heap)?))
            }
            (Self::InternBytes(id), count) | (count, Self::InternBytes(id)) => {
                let Some(count) = repeat_count(count, vm)? else {
                    return Ok(None);
                };
                Ok(Some(repeat_bytes(vm.interns.get_bytes(*id), count, vm.heap)?))
            }
            (Self::Ref(id), _) => vm.heap.read(*id).py_mul_impl(other, vm),
            _ => Ok(None),
        }
    }

    /// Reflected implementation of Python `*`.
    fn py_rmul_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_rmul_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// One-sided implementation of Python `/`.
    fn py_truediv_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
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
            (Self::Ref(id), _) => vm.heap.read(*id).py_truediv_impl(other, vm),
            _ => Ok(None),
        }
    }

    /// Reflected implementation of Python `/`.
    fn py_rtruediv_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_rtruediv_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// One-sided implementation of Python `//`.
    fn py_floordiv_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        match (self, other) {
            // Floor division: int // int returns int
            (Self::Int(a), Self::Int(b)) => {
                if *b == 0 {
                    Err(ExcType::zero_division().into())
                } else if let Some((d, _)) = floor_divmod(*a, *b) {
                    Ok(Some(Self::Int(d)))
                } else {
                    Ok(Some(wide_i128_into_value(
                        i128::from(*a).div_euclid(i128::from(*b)),
                        vm.heap,
                    )))
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
            (Self::Ref(id), _) => vm.heap.read(*id).py_floordiv_impl(other, vm),
            _ => Ok(None),
        }
    }

    /// Reflected implementation of Python `//`.
    fn py_rfloordiv_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_rfloordiv_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// One-sided implementation of Python `%`.
    fn py_mod_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
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
            (Self::Ref(id), _) => vm.heap.read(*id).py_mod_impl(other, vm),
            _ => Ok(None),
        }
    }

    /// Reflected implementation of Python `%`.
    fn py_rmod_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_rmod_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// One-sided implementation of Python `** or pow()`.
    fn py_pow_impl(&self, other: &Self, modulus: Option<&Self>, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if modulus.is_some() {
            if let Self::Ref(id) = self {
                vm.heap.read(*id).py_pow_impl(other, modulus, vm)
            } else {
                Ok(None)
            }
        } else {
            match (self, other) {
                (Self::Int(base), Self::Int(exp)) => {
                    if *base == 0 && *exp < 0 {
                        Err(ExcType::zero_negative_power())
                    } else if *exp >= 0 {
                        // Positive exponent: try to return int, promote to LongInt on overflow
                        if let Ok(exp_u32) = u32::try_from(*exp) {
                            if let Some(result) = base.checked_pow(exp_u32) {
                                Ok(Some(Self::Int(result)))
                            } else if let Some(result) = i128::from(*base).checked_pow(exp_u32) {
                                Ok(Some(wide_i128_into_value(result, vm.heap)))
                            } else {
                                check_pow_size(i64_bits(*base), u64::from(exp_u32), vm.heap.tracker())?;
                                let bi = BigInt::from(*base).pow(exp_u32);
                                Ok(Some(LongInt::new(bi).into_value(vm.heap)))
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
                            Ok(Some(LongInt::new(bi).into_value(vm.heap)))
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
                (Self::Ref(id), _) => vm.heap.read(*id).py_pow_impl(other, modulus, vm),
                _ => Ok(None),
            }
        }
    }

    /// Reflected implementation of Python `** or pow()`.
    fn py_rpow_impl(&self, other: &Self, modulus: Option<&Self>, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_rpow_impl(other, modulus, vm)
        } else {
            Ok(None)
        }
    }

    /// One-sided implementation of Python `&`.
    fn py_and_impl(&self, other: &Self, vm: &mut VM<'_>, _self_id: Option<HeapId>) -> RunResult<Option<Self>> {
        if let (Self::Bool(lhs), Self::Bool(rhs)) = (self, other) {
            Ok(Some(Self::Bool(*lhs && *rhs)))
        } else if let (Some(lhs), Some(rhs)) = (immediate_int(self), immediate_int(other)) {
            Ok(Some(Self::Int(lhs & rhs)))
        } else if let Self::Ref(id) = self {
            vm.heap.read(*id).py_and_impl(other, vm, Some(*id))
        } else {
            Ok(None)
        }
    }

    /// Reflected implementation of Python `&`.
    fn py_rand_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_rand_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// One-sided implementation of Python `|`.
    fn py_or_impl(&self, other: &Self, vm: &mut VM<'_>, _self_id: Option<HeapId>) -> RunResult<Option<Self>> {
        if let (Self::Bool(lhs), Self::Bool(rhs)) = (self, other) {
            Ok(Some(Self::Bool(*lhs || *rhs)))
        } else if let (Some(lhs), Some(rhs)) = (immediate_int(self), immediate_int(other)) {
            Ok(Some(Self::Int(lhs | rhs)))
        } else if let Self::Ref(id) = self {
            vm.heap.read(*id).py_or_impl(other, vm, Some(*id))
        } else {
            Ok(None)
        }
    }

    /// Reflected implementation of Python `|`.
    fn py_ror_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_ror_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// One-sided implementation of Python `^`.
    fn py_xor_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let (Self::Bool(lhs), Self::Bool(rhs)) = (self, other) {
            Ok(Some(Self::Bool(*lhs ^ *rhs)))
        } else if let (Some(lhs), Some(rhs)) = (immediate_int(self), immediate_int(other)) {
            Ok(Some(Self::Int(lhs ^ rhs)))
        } else if let Self::Ref(id) = self {
            vm.heap.read(*id).py_xor_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// Reflected implementation of Python `^`.
    fn py_rxor_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_rxor_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// One-sided implementation of Python `<<`.
    fn py_lshift_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let (Some(lhs), Some(rhs)) = (immediate_int(self), immediate_int(other)) {
            if rhs < 0 {
                Err(ExcType::value_error_negative_shift_count())
            } else {
                #[expect(clippy::cast_sign_loss)]
                Ok(Some(LongInt::left_shift_i64(lhs, rhs as u64, vm)?))
            }
        } else if let Self::Ref(id) = self {
            vm.heap.read(*id).py_lshift_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// Reflected implementation of Python `<<`.
    fn py_rlshift_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_rlshift_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// One-sided implementation of Python `>>`.
    fn py_rshift_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let (Some(lhs), Some(rhs)) = (immediate_int(self), immediate_int(other)) {
            if rhs < 0 {
                Err(ExcType::value_error_negative_shift_count())
            } else {
                let value = u32::try_from(rhs)
                    .ok()
                    .filter(|shift| *shift < 64)
                    .map_or_else(|| if lhs < 0 { -1 } else { 0 }, |shift| lhs >> shift);
                Ok(Some(Self::Int(value)))
            }
        } else if let Self::Ref(id) = self {
            vm.heap.read(*id).py_rshift_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// Reflected implementation of Python `>>`.
    fn py_rrshift_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_rrshift_impl(other, vm)
        } else {
            Ok(None)
        }
    }

    /// One-sided implementation of matrix multiplication.
    fn py_matmul_impl(&self, _other: &Self, _vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        Err(ExcType::not_implemented("matrix multiplication (@) is not supported").into())
    }

    /// Reflected implementation of matrix multiplication.
    fn py_rmatmul_impl(&self, _other: &Self, _vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        Ok(None)
    }

    fn py_getitem(&self, key: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        let interns = vm.interns;
        match self {
            // `heap_subscript` owns the one case a heap read cannot: a
            // defaultdict miss, which calls its factory and re-enters the VM.
            Self::Ref(id) => heap_subscript(*id, key, vm),
            Self::InternString(string_id) => {
                // Check for slice first
                if let Self::Ref(key_id) = key
                    && let HeapData::Slice(slice_obj) = vm.heap.get(*key_id)
                {
                    let s = interns.get_str(*string_id);
                    let result_str: Box<str> = slice_collect_iterator(vm, slice_obj, s.chars(), |c| c)?;
                    return Ok(allocate_string(result_str, vm.heap));
                }

                // Handle interned string indexing, accepting Int and Bool
                let index = match key {
                    Self::Int(i) => *i,
                    Self::Bool(b) => i64::from(*b),
                    _ => return Err(ExcType::type_error_indices(Type::Str, &key.py_type_name(vm))),
                };

                let s = interns.get_str(*string_id);
                let c = get_char_at_index(s, index).ok_or_else(ExcType::str_index_error)?;
                Ok(allocate_char(c, vm.heap))
            }
            Self::InternBytes(bytes_id) => {
                // Check for slice first
                if let Self::Ref(key_id) = key
                    && let HeapData::Slice(slice_obj) = vm.heap.get(*key_id)
                {
                    let bytes = interns.get_bytes(*bytes_id);
                    let result_bytes = slice_collect_iterator(vm, slice_obj, bytes.iter(), |b| *b)?;
                    let heap_id = vm.heap.allocate(HeapData::Bytes(Bytes::new(result_bytes)));
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

    fn py_setitem(&mut self, key: Self, value: Self, vm: &mut VM<'_>) -> RunResult<()> {
        match self {
            Self::Ref(id) => vm.heap.read(*id).py_setitem(key, value, vm),
            _ => Err(ExcType::type_error(format!(
                "'{}' object does not support item assignment",
                self.py_type_name(vm)
            ))),
        }
    }

    fn py_is_iterator(&self, vm: &VM<'_>) -> bool {
        // No immediate value is an iterator; interned `str`/`bytes` are iterable
        // but, as in CPython, are not their own iterators.
        match self {
            Self::Ref(id) => vm.heap.read(*id).py_is_iterator(vm),
            _ => false,
        }
    }

    fn py_is_iterable(&self, vm: &VM<'_>) -> bool {
        match self {
            // Interned string and bytes literals iterate without ever reaching
            // the heap, so they answer here rather than in `HeapReadOutput`.
            Self::InternString(_) | Self::InternBytes(_) => true,
            Self::Ref(id) => vm.heap.read(*id).py_is_iterable(vm),
            _ => false,
        }
    }

    fn py_iter(&self, _: Option<HeapId>, vm: &mut VM<'_>) -> RunResult<Self> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_iter(Some(*id), vm)
        } else {
            match self {
                Self::InternString(id) => Ok(StringIterator::from_intern(*id, vm)),
                Self::InternBytes(id) => Ok(BytesIterator::from_intern(*id, vm)),
                _ => Err(ExcType::type_error_not_iterable(&self.py_type_name(vm))),
            }
        }
    }

    fn py_next(&mut self, _: Option<HeapId>, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Self::Ref(id) = self {
            vm.heap.read(*id).py_next(Some(*id), vm)
        } else {
            Err(ExcType::type_error_not_iterator(&self.py_type_name(vm)))
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
    pub(crate) fn py_type_heap(&self, heap: &Heap) -> Type {
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
    pub(crate) fn py_type_name<'h>(&self, vm: &VM<'h>) -> Cow<'h, str> {
        self.namedtuple_class_name(vm.heap, vm.interns)
            .unwrap_or_else(|| self.py_type(vm).name(vm.heap, vm.interns))
    }

    /// [`py_type_name`](Self::py_type_name) for contexts without a `&VM` —
    /// notably the macro-generated `from_args` bodies, which are passed
    /// `heap` + `interns` instead (mirrors [`py_type_heap`](Self::py_type_heap)).
    #[must_use]
    pub(crate) fn py_type_name_heap<'i>(&self, heap: &Heap, interns: &'i Interns) -> Cow<'i, str> {
        self.namedtuple_class_name(heap, interns)
            .unwrap_or_else(|| self.py_type_heap(heap).name(heap, interns))
    }

    /// Class name of a named tuple, if this value is one.
    ///
    /// Named tuples keep their class name in the heap entry rather than in
    /// [`Type`], which carries no identity for them. Error messages therefore
    /// have to reach for it here to name the class (`'P'`) rather than the
    /// generic `'namedtuple'`, matching CPython — including for structseqs,
    /// whose stored name is already the qualified `sys.version_info`.
    #[must_use]
    fn namedtuple_class_name<'i>(&self, heap: &Heap, interns: &'i Interns) -> Option<Cow<'i, str>> {
        if let Self::Ref(heap_id) = self
            && let HeapData::NamedTuple(nt) = heap.get(*heap_id)
        {
            Some(match nt.name_either() {
                EitherStr::Interned(id) => Cow::Borrowed(interns.get_str(*id)),
                EitherStr::Heap(s) => Cow::Owned(s.clone()),
            })
        } else {
            None
        }
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
            Self::NotImplemented => Type::NotImplementedType,
            Self::Bool(_) => Type::Bool,
            Self::Int(_) | Self::InternLongInt(_) => Type::Int,
            Self::Float(_) => Type::Float,
            Self::InternString(_) => Type::Str,
            Self::InternBytes(_) => Type::Bytes,
            Self::Builtin(_) => Type::BuiltinFunction,
            Self::ModuleFunction(_) | Self::DefFunction(_) => Type::Function,
            Self::Marker(_) => Type::SpecialForm,
            Self::Property(_) => Type::Property,
            Self::Ref(_) => Type::NoneType, // callers should resolve Ref via HeapData::py_type()
            #[cfg(feature = "memory-model-checks")]
            Self::Dereferenced => Type::NoneType,
        }
    }

    /// Returns this value's complete structural identity.
    ///
    /// Immediate values retain Monty's value-derived identity, while heap objects
    /// use their arena index.
    pub(crate) fn id(&self) -> Identity {
        Identity::new(self)
    }

    /// Returns the Ref ID if this value is a reference, otherwise returns None.
    pub fn ref_id(&self) -> Option<HeapId> {
        match self {
            Self::Ref(id) => Some(*id),
            _ => None,
        }
    }

    /// Consumes this Value as a reference, returning the contained HeapId otherwise returns None.
    ///
    /// The caller is responsible for ensuring the `HeapId` is properly decref'd.
    pub fn into_ref_id(self) -> Option<HeapId> {
        match self {
            Self::Ref(id) => {
                #[cfg(feature = "memory-model-checks")]
                {
                    // Mark the value as dereferenced to prevent double-free
                    mem::forget(self);
                }
                Some(id)
            }
            _ => None,
        }
    }

    /// Returns the module name if this value is a module, otherwise returns "<unknown>".
    ///
    /// Used for error messages in `from module import name` when the name doesn't exist.
    pub fn module_name(&self, vm: &mut VM<'_>) -> String {
        match self {
            Self::Ref(id) => match vm.heap.get(*id) {
                HeapData::Module(module) => vm.interns.get_str(module.name()).to_string(),
                _ => "<unknown>".to_string(),
            },
            _ => "<unknown>".to_string(),
        }
    }

    /// Python-visible `is` operator using complete structural identity.
    ///
    /// Values compare using their full immediate or arena identity.
    pub fn is(&self, other: &Self) -> bool {
        self.id() == other.id()
    }

    /// Whether this value is Python's `NotImplemented` singleton.
    #[must_use]
    pub(crate) fn is_not_implemented(&self) -> bool {
        matches!(self, Self::NotImplemented)
    }

    /// Equality as containers perform it: identity before user equality.
    pub fn py_eq(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<bool> {
        if self.is(other) {
            Ok(true)
        } else {
            self.py_eq_operator(other, vm)
        }
    }

    /// Python's `==` operator normalized to a boolean.
    pub fn py_eq_operator(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<bool> {
        let result = self.py_rich_eq(other, vm)?;
        defer_drop!(result, vm);
        result.py_bool(vm)
    }

    /// Direct Python `==`, preserving any arbitrary value returned by `__eq__`.
    ///
    /// Tries the left operand, then reflected equality when it returns
    /// `NotImplemented`; identity is only the final fallback after both decline.
    pub(crate) fn py_rich_eq(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        let lhs_result = self.py_rich_eq_impl(other, vm)?;
        if !lhs_result.is_not_implemented() {
            return Ok(lhs_result);
        }

        let rhs_result = other.py_rich_eq_impl(self, vm)?;
        if !rhs_result.is_not_implemented() {
            return Ok(rhs_result);
        }

        Ok(Self::Bool(self.is(other)))
    }

    /// Runs one side of rich equality, using `NotImplemented` to request reflected dispatch.
    fn py_rich_eq_impl(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        if let Self::Ref(id) = self
            && matches!(vm.heap.get(*id), HeapData::Instance(_))
        {
            if let Some(result) = instance_user_eq(*id, other, vm)? {
                Ok(result)
            } else if self.is(other) {
                Ok(Self::Bool(true))
            } else if let Some(result) = instance_dataclass_eq(*id, other, vm)? {
                Ok(Self::Bool(result))
            } else {
                Ok(Self::NotImplemented)
            }
        } else if let Some(result) = self.py_eq_impl(other, vm)? {
            Ok(Self::Bool(result))
        } else {
            Ok(Self::NotImplemented)
        }
    }

    /// Returns an iterator for this value using its type-specific protocol when available.
    pub fn py_iter(&self, vm: &mut VM<'_>) -> RunResult<Self> {
        <Self as PyTrait<'_>>::py_iter(self, None, vm)
    }

    /// Advances this value as an iterator, mirroring the inherent [`Value::py_iter`].
    ///
    /// The trait method's `self_id` is redundant for a `Value`, which already
    /// carries its own `HeapId`, so callers use this and let the impl resolve it.
    pub(crate) fn py_next(&mut self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        <Self as PyTrait<'_>>::py_next(self, None, vm)
    }

    /// Converts an owned value into its Python iterator and releases the original reference.
    ///
    /// This is the ownership-preserving entry point for Rust consumers of the
    /// iteration protocol; the returned iterator retains its source as needed.
    pub(crate) fn into_py_iter(self, vm: &mut VM<'_>) -> RunResult<Self> {
        let iterator = self.py_iter(vm);
        self.drop_with(vm);
        iterator
    }

    /// Creates a scoped view that retains this value's heap reader when needed.
    pub(crate) fn read<'h, 'v>(&'v self, vm: &VM<'h>) -> ValueRead<'h, 'v> {
        match self {
            Self::Ref(id) => ValueRead::Heap {
                owner: self,
                value: vm.heap.read(*id),
            },
            _ => ValueRead::Immediate(self),
        }
    }

    /// Reads the heap entry this value references, or `None` if it is not a
    /// heap reference.
    ///
    /// Used by per-type [`PyTrait::py_eq_impl`] impls to resolve the other operand
    /// to a heap object of their own type, returning `NotImplemented` otherwise.
    pub(crate) fn read_heap<'a>(&self, vm: &VM<'a>) -> Option<HeapReadOutput<'a>> {
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
    pub fn py_hash(&self, vm: &mut VM<'_>) -> RunResult<Option<HashValue>> {
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
            Self::Undefined | Self::Ellipsis | Self::NotImplemented | Self::None => {
                Ok(Some(hash_one(discriminant(self))))
            }
            Self::Builtin(b) => Ok(Some(hash_one(b))),
            Self::ModuleFunction(mf) => Ok(Some(hash_one(mf))),
            // Hash functions based on function ID
            Self::DefFunction(f_id) => Ok(Some(hash_one(f_id))),
            // Markers are hashable based on their discriminant
            Self::Marker(m) => Ok(Some(hash_one(m))),
            // Properties are hashable based on their OS function discriminant
            Self::Property(p) => Ok(Some(hash_one(p))),
            #[cfg(feature = "memory-model-checks")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    /// Checks if `item` is contained in `self` (the container) — Python's `in`.
    ///
    /// Type-specific logic lives on each type's [`PyTrait::py_contains`]; this
    /// resolves the value and applies CPython's protocol order: a type's own
    /// containment first (`sq_contains`), then consuming it by iteration
    /// (`tp_iter`), then `TypeError`.
    pub fn py_contains(&self, item: &Self, vm: &mut VM<'_>) -> RunResult<bool> {
        match self {
            Self::Ref(heap_id) => match vm.heap.read(*heap_id).py_contains_impl(*heap_id, item, vm)? {
                Some(found) => Ok(found),
                None if self.py_is_iterable(vm) => self.contains_by_iteration(item, vm),
                None => Err(ExcType::type_error_not_container(&self.py_type_name(vm))),
            },
            // Interned strings and bytes never reach the heap, so they answer here.
            Self::InternString(string_id) => {
                let container_str = vm.interns.get_str(*string_id);
                str_contains(container_str, item, vm.heap, vm.interns)
            }
            Self::InternBytes(bytes_id) => {
                let container = vm.interns.get_bytes(*bytes_id);
                bytes_contains(container, item, vm)
            }
            _ => Err(ExcType::type_error_not_container(&self.py_type_name(vm))),
        }
    }

    /// Consumes `self` as an iterable, reporting whether `item` compares equal
    /// to any element — the `in` fallback for anything without `__contains__`.
    ///
    /// Only called for values already known to be iterable, so it does not
    /// re-check. An endless iterable runs until a resource limit fires.
    fn contains_by_iteration(&self, item: &Self, vm: &mut VM<'_>) -> RunResult<bool> {
        let iter = self.py_iter(vm)?;
        defer_drop!(iter, vm);
        let mut iter = iter.read(vm);
        loop {
            let Some(el) = iter.py_next(vm)? else {
                break Ok(false);
            };
            let eq = item.py_eq(&el, vm);
            el.drop_with(vm);
            if eq? {
                break Ok(true);
            }
        }
    }

    /// Gets an attribute from this value.
    ///
    /// Dispatches to `py_getattr` on the underlying types where appropriate.
    /// Accepts `EitherStr` to support both interned and heap-allocated attribute names.
    ///
    /// Returns `AttributeError` for other types or unknown attributes.
    pub fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'_>) -> RunResult<CallResult> {
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
                    return Ok(CallResult::Value(allocate_string(t.name(vm.heap, vm.interns), vm.heap)));
                }
                if *t == Type::TimeZone && attr.as_str(vm.interns) == "utc" {
                    return Ok(CallResult::Value(vm.heap.get_timezone_utc()));
                }
            }
            _ => {}
        }
        let type_name = self.py_type_name(vm);
        Err(ExcType::attribute_error(type_name, attr.as_str(vm.interns)))
    }

    /// Sets an attribute, consuming `value` on both success and error.
    pub fn py_set_attr(&self, name: &EitherStr, value: Self, vm: &mut VM<'_>) -> RunResult<()> {
        if let Self::Ref(heap_id) = self {
            vm.heap.read(*heap_id).py_set_attr(name, value, vm)
        } else {
            value.drop_with(vm);
            let type_name = self.py_type_name(vm);
            Err(ExcType::attribute_error_no_setattr(&type_name, name.as_str(vm.interns)))
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
    pub fn as_int(&self, vm: &VM<'_>) -> RunResult<i64> {
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
    pub fn as_index(&self, vm: &VM<'_>, container_type: Type) -> RunResult<i64> {
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

    /// True when this is a `LongInt`-valued int (interned or heap-allocated)
    /// that is negative — lets fixed-width consumers pick the right overflow
    /// direction/message (`round`'s i64 clamp, `os`'s fd converter). False for
    /// every other value, `Int`/`Bool` included: pair it with
    /// [`is_long_int`](crate::args::is_long_int) rather than using it alone to
    /// classify an int.
    pub(crate) fn long_int_is_negative(&self, vm: &VM<'_>) -> bool {
        match self {
            Self::InternLongInt(id) => vm.interns.get_long_int(*id).sign() == Sign::Minus,
            Self::Ref(id) => matches!(vm.heap.get(*id), HeapData::LongInt(li) if li.is_negative()),
            _ => false,
        }
    }

    /// Performs Python `+` with reflected-operation fallback.
    pub(crate) fn py_add(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        if let Some(result) = self.py_add_result(other, vm)? {
            Ok(result)
        } else {
            let lhs_type = self.py_type(vm);
            Err(ExcType::binary_type_error(
                "+",
                lhs_type,
                self.py_type_name(vm),
                other.py_type_name(vm),
            ))
        }
    }

    /// Tries direct and reflected addition without producing the final type error.
    pub(crate) fn py_add_result(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Option<Self>> {
        if let Some(result) = self.py_add_impl(other, vm, self.ref_id())? {
            Ok(Some(result))
        } else {
            other.py_radd_impl(self, vm)
        }
    }

    /// Performs Python `-` with reflected-operation fallback.
    pub(crate) fn py_sub(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        self.binary_op(
            other,
            vm,
            |vm| self.py_sub_impl(other, vm, self.ref_id()),
            |vm| other.py_rsub_impl(self, vm),
            "-",
        )
    }

    /// Performs Python `*` with reflected-operation fallback.
    pub(crate) fn py_mul(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        self.binary_op(
            other,
            vm,
            |vm| self.py_mul_impl(other, vm),
            |vm| other.py_rmul_impl(self, vm),
            "*",
        )
    }

    /// Performs Python `@` with reflected-operation fallback.
    pub(crate) fn py_matmul(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        self.binary_op(
            other,
            vm,
            |vm| self.py_matmul_impl(other, vm),
            |vm| other.py_rmatmul_impl(self, vm),
            "@",
        )
    }

    /// Performs Python `/` with reflected-operation fallback.
    pub(crate) fn py_truediv(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        self.binary_op(
            other,
            vm,
            |vm| self.py_truediv_impl(other, vm),
            |vm| other.py_rtruediv_impl(self, vm),
            "/",
        )
    }

    /// Performs Python `//` with reflected-operation fallback.
    pub(crate) fn py_floordiv(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        self.binary_op(
            other,
            vm,
            |vm| self.py_floordiv_impl(other, vm),
            |vm| other.py_rfloordiv_impl(self, vm),
            "//",
        )
    }

    /// Performs Python `%` with reflected-operation fallback.
    pub(crate) fn py_mod(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        self.binary_op(
            other,
            vm,
            |vm| self.py_mod_impl(other, vm),
            |vm| other.py_rmod_impl(self, vm),
            "%",
        )
    }

    /// Performs Python `** or pow()` with reflected-operation fallback.
    pub(crate) fn py_pow(&self, other: &Self, modulus: Option<&Self>, vm: &mut VM<'_>) -> RunResult<Self> {
        self.binary_op(
            other,
            vm,
            |vm| self.py_pow_impl(other, modulus, vm),
            |vm| other.py_rpow_impl(self, modulus, vm),
            "** or pow()",
        )
    }

    /// Performs Python `&` with reflected-operation fallback.
    pub(crate) fn py_and(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        self.binary_op(
            other,
            vm,
            |vm| self.py_and_impl(other, vm, self.ref_id()),
            |vm| other.py_rand_impl(self, vm),
            "&",
        )
    }

    /// Performs Python `|` with reflected-operation fallback.
    pub(crate) fn py_or(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        self.binary_op(
            other,
            vm,
            |vm| self.py_or_impl(other, vm, self.ref_id()),
            |vm| other.py_ror_impl(self, vm),
            "|",
        )
    }

    /// Performs Python `^` with reflected-operation fallback.
    pub(crate) fn py_xor(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        self.binary_op(
            other,
            vm,
            |vm| self.py_xor_impl(other, vm),
            |vm| other.py_rxor_impl(self, vm),
            "^",
        )
    }

    /// Performs Python `<<` with reflected-operation fallback.
    pub(crate) fn py_lshift(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        self.binary_op(
            other,
            vm,
            |vm| self.py_lshift_impl(other, vm),
            |vm| other.py_rlshift_impl(self, vm),
            "<<",
        )
    }

    /// Performs Python `>>` with reflected-operation fallback.
    pub(crate) fn py_rshift(&self, other: &Self, vm: &mut VM<'_>) -> RunResult<Self> {
        self.binary_op(
            other,
            vm,
            |vm| self.py_rshift_impl(other, vm),
            |vm| other.py_rrshift_impl(self, vm),
            ">>",
        )
    }

    /// Applies the shared direct, reflected, then unsupported binary protocol.
    fn binary_op(
        &self,
        other: &Self,
        vm: &mut VM<'_>,
        direct: impl FnOnce(&mut VM<'_>) -> RunResult<Option<Self>>,
        reflected: impl FnOnce(&mut VM<'_>) -> RunResult<Option<Self>>,
        operator: &'static str,
    ) -> RunResult<Self> {
        if let Some(result) = direct(vm)? {
            Ok(result)
        } else if let Some(result) = reflected(vm)? {
            Ok(result)
        } else {
            let lhs_type = self.py_type(vm);
            let lhs_name = self.py_type_name(vm);
            Err(ExcType::binary_type_error(
                operator,
                lhs_type,
                lhs_name,
                other.py_type_name(vm),
            ))
        }
    }

    /// Clones a value with proper heap reference counting.
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
            Self::Undefined => Self::Undefined,
            Self::Ellipsis => Self::Ellipsis,
            Self::NotImplemented => Self::NotImplemented,
            Self::None => Self::None,
            Self::Bool(b) => Self::Bool(*b),
            Self::Int(v) => Self::Int(*v),
            Self::Float(v) => Self::Float(*v),
            Self::Builtin(b) => Self::Builtin(*b),
            Self::ModuleFunction(mf) => Self::ModuleFunction(*mf),
            Self::DefFunction(f) => Self::DefFunction(*f),
            Self::InternString(s) => Self::InternString(*s),
            Self::InternBytes(b) => Self::InternBytes(*b),
            Self::InternLongInt(bi) => Self::InternLongInt(*bi),
            Self::Marker(m) => Self::Marker(*m),
            Self::Property(p) => Self::Property(*p),
            Self::Ref(id) => {
                heap.heap().inc_ref(*id);
                Self::Ref(*id)
            }
            #[cfg(feature = "memory-model-checks")]
            Self::Dereferenced => panic!("Cannot copy Dereferenced object"),
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
    /// a value to prevent memory leaks. Call it directly only on a simple, linear
    /// cleanup path; use `defer_drop!` or `DropGuard` when any branch or `?` could
    /// bypass cleanup.
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
    pub fn as_either_str(&self, heap: &Heap) -> Option<EitherStr> {
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
    pub(crate) fn to_str<'a>(&'a self, vm: &'a VM<'_>) -> RunResult<&'a str> {
        self.to_str_heap(vm.heap, vm.interns)
    }

    /// [`to_str`](Self::to_str) for contexts without a `&VM` — takes `heap` and
    /// `interns` separately so callers can keep a disjoint `&mut` borrow of
    /// another `VM` field alive (e.g. resolving a `str` `Value` produced by
    /// `py_str` while writing it to `vm.print_writer`).
    pub(crate) fn to_str_heap<'a>(&'a self, heap: &'a Heap, interns: &'a Interns) -> RunResult<&'a str> {
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
    pub fn is_str(&self, heap: &Heap) -> bool {
        match self {
            Self::InternString(_) => true,
            Self::Ref(heap_id) => matches!(heap.get(*heap_id), HeapData::Str(_)),
            _ => false,
        }
    }

    /// Whether calling this value would succeed at dispatch.
    ///
    /// Dispatch can't double as the predicate — it pushes a frame, clones
    /// defaults, constructs an instance — so the two are kept in lockstep by
    /// `debug_assert!`s in dispatch's "not callable" arms.
    pub(crate) fn is_callable(&self, heap: &Heap) -> bool {
        match self {
            Self::Builtin(_) | Self::ModuleFunction(_) | Self::DefFunction(_) => true,
            Self::Ref(id) => heap.get(*id).is_callable(),
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
pub(crate) fn eq_i64(a: i64, other: &Value, vm: &VM<'_>) -> Option<bool> {
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
pub(crate) fn eq_f64(f: f64, other: &Value, vm: &VM<'_>) -> Option<bool> {
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
pub(crate) fn eq_bigint(b: &BigInt, other: &Value, vm: &VM<'_>) -> Option<bool> {
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
pub(crate) fn eq_str(s: &str, other: &Value, vm: &VM<'_>) -> Option<bool> {
    match other {
        Value::InternString(id) => Some(s == vm.interns.get_str(*id)),
        Value::Ref(id) if let HeapData::Str(o) = vm.heap.get(*id) => Some(s == o.as_str()),
        _ => None,
    }
}

/// `b == other`, resolving the other operand from interned or heap bytes.
pub(crate) fn eq_bytes(b: &[u8], other: &Value, vm: &VM<'_>) -> Option<bool> {
    match other {
        Value::InternBytes(id) => Some(b == vm.interns.get_bytes(*id)),
        Value::Ref(id) if let HeapData::Bytes(o) = vm.heap.get(*id) => Some(b == o.as_slice()),
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

/// Extracts an immediate integer without promoting it to `BigInt`.
fn immediate_int(value: &Value) -> Option<i64> {
    match value {
        Value::Int(value) => Some(*value),
        Value::Bool(value) => Some(i64::from(*value)),
        _ => None,
    }
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
    use monty_types::{AssertMessageAnnotations, PrintWriter, ResourceTracker};
    use num_bigint::BigInt;

    use super::*;
    use crate::{heap::HeapReader, intern::InternerBuilder};

    /// Creates a heap and directly allocates a LongInt with the given BigInt value.
    ///
    /// This bypasses `LongInt::into_value()` which would demote i64-fitting values.
    /// Used to test defensive code paths that handle LongInt-as-index scenarios.
    fn create_heap_with_longint(value: BigInt) -> (Heap, HeapId) {
        let heap = Heap::new(16, ResourceTracker::default());
        let long_int = LongInt::new(value);
        let heap_id = heap.allocate(HeapData::LongInt(long_int));
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
