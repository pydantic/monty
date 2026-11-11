//! [`TypeRef`]: any Python type, native or a class object on the heap.
//!
//! `py_type()` returns one, and inheritance, `isinstance`, `issubclass` and
//! `except` matching all work on it, so they have one implementation whether
//! the types involved are native, user-defined or a mix.

use std::{borrow::Cow, iter::FusedIterator, slice};

use monty_types::ResourceTracker;

use crate::{
    builtins::Builtins,
    exception_private::RunResult,
    heap::{Heap, HeapData, HeapId},
    intern::Interns,
    types::{Type, native::NativeMro},
    value::Value,
};

/// A borrowed reference to a type: a native [`Type`] or a class object.
///
/// `Copy` and never owns a refcount, so a `TypeRef::Class` is only valid while
/// something else keeps the class alive (the instance it came from, or the
/// class value being inspected). Store an owned [`OwnedTypeRef`] instead, and
/// use [`to_value`](Self::to_value) to hand a type to Python code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum TypeRef {
    /// A Rust-defined type.
    Native(Type),
    /// A class object on the heap: a `Class`, `NamedTupleClass` or `HostClassType`.
    Class(HeapId),
}

impl From<Type> for TypeRef {
    #[inline]
    fn from(ty: Type) -> Self {
        Self::Native(ty)
    }
}

/// Lets `obj.py_type(vm) == Type::Int` read as it did when `py_type` returned a `Type`.
impl PartialEq<Type> for TypeRef {
    #[inline]
    fn eq(&self, other: &Type) -> bool {
        *self == Self::Native(*other)
    }
}

impl TypeRef {
    /// The native type, if this is one.
    #[inline]
    #[must_use]
    pub(crate) fn native(self) -> Option<Type> {
        match self {
            Self::Native(ty) => Some(ty),
            Self::Class(_) => None,
        }
    }

    /// The type a value denotes, if it is a type object: a native type or a class.
    #[must_use]
    pub(crate) fn from_value(value: &Value, heap: &Heap) -> Option<Self> {
        match value {
            Value::Builtin(Builtins::Type(ty)) => Some(Self::Native(*ty)),
            Value::Ref(id) if is_class_like(heap.get(*id)) => Some(Self::Class(*id)),
            _ => None,
        }
    }

    /// The type as a Python value, taking a reference for a class.
    #[must_use]
    pub(crate) fn to_value(self, heap: &Heap) -> Value {
        match self {
            Self::Native(ty) => Value::Builtin(Builtins::Type(ty)),
            Self::Class(id) => {
                heap.inc_ref(id);
                Value::Ref(id)
            }
        }
    }

    /// The Python-visible name: CPython's `tp_name` for a native type, the class
    /// name otherwise. Borrows only `interns`, so it outlives heap mutation.
    #[must_use]
    pub(crate) fn name<'i>(self, heap: &Heap, interns: &'i Interns) -> Cow<'i, str> {
        match self {
            Self::Native(ty) => Cow::Borrowed(ty.name()),
            Self::Class(id) => class_like_name(id, heap, interns),
        }
    }

    /// The name `__name__` reports: native names lose their module qualifier.
    #[must_use]
    pub(crate) fn dunder_name<'i>(self, heap: &Heap, interns: &'i Interns) -> Cow<'i, str> {
        match self {
            Self::Native(ty) => Cow::Borrowed(ty.dunder_name()),
            Self::Class(id) => class_like_name(id, heap, interns),
        }
    }

    /// [`name`](Self::name) as CPython's "argument N must be X, not Y" errors
    /// render it, where `NoneType` reads `None`.
    #[must_use]
    pub(crate) fn cpython_arg_name<'i>(self, heap: &Heap, interns: &'i Interns) -> Cow<'i, str> {
        match self {
            Self::Native(ty) => Cow::Borrowed(ty.cpython_arg_name()),
            Self::Class(id) => class_like_name(id, heap, interns),
        }
    }

    /// Whether multiplication can fall back to sequence repetition; never for a class.
    #[must_use]
    pub(crate) fn has_sequence_repeat(self) -> bool {
        self.native().is_some_and(Type::has_sequence_repeat)
    }

    /// Whether this is one of CPython's concrete iterator types; never for a class.
    #[must_use]
    pub(crate) fn is_iterator(self) -> bool {
        self.native().is_some_and(Type::is_iterator)
    }

    /// This type then its ancestors, in method resolution order, ending at `object`.
    pub(crate) fn mro(self, heap: &Heap) -> TypeMro<'_> {
        match self {
            Self::Native(ty) => TypeMro::Native(ty.mro()),
            Self::Class(id) => TypeMro::Class {
                this: Some(id),
                rest: class_like_mro(heap.get(id)).iter(),
            },
        }
    }

    /// Whether `self` is `other` or inherits from it: `issubclass(self, other)`.
    #[must_use]
    pub(crate) fn is_subclass_of(self, other: Self, heap: &Heap) -> bool {
        match (self, other) {
            _ if self == other => true,
            (_, Self::Native(Type::Object)) => true,
            (Self::Native(ty), Self::Native(base)) => ty.is_subclass_of(base),
            // A native type never inherits from a class.
            (Self::Native(_), Self::Class(_)) => false,
            (Self::Class(_), _) => self.mro(heap).skip(1).any(|ancestor| ancestor == other),
        }
    }
}

/// Iterator over a type's MRO, from [`TypeRef::mro`].
pub(crate) enum TypeMro<'h> {
    /// A native type's own MRO.
    Native(NativeMro),
    /// A class: itself, then its stored ancestors (which end at `object`).
    Class {
        this: Option<HeapId>,
        rest: slice::Iter<'h, TypeRef>,
    },
}

impl Iterator for TypeMro<'_> {
    type Item = TypeRef;

    fn next(&mut self) -> Option<TypeRef> {
        match self {
            Self::Native(mro) => mro.next().map(TypeRef::Native),
            Self::Class { this, rest } => match this.take() {
                Some(id) => Some(TypeRef::Class(id)),
                None => rest.next().copied(),
            },
        }
    }
}

impl FusedIterator for TypeMro<'_> {}

/// A type reference that owns its class reference, for storing in heap objects.
///
/// `Class.bases` holds these; the owner pushes the id from `py_dec_ref_ids`
/// and the cycle collector reaches it through `for_each_child_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum OwnedTypeRef {
    /// A Rust-defined type, which needs no reference.
    Native(Type),
    /// An owned reference to a class object.
    Class(HeapId),
}

impl OwnedTypeRef {
    /// Borrows this as a [`TypeRef`].
    #[inline]
    #[must_use]
    pub(crate) fn borrow(self) -> TypeRef {
        match self {
            Self::Native(ty) => TypeRef::Native(ty),
            Self::Class(id) => TypeRef::Class(id),
        }
    }

    /// The owned class id, if any, for refcount release and GC tracing.
    #[inline]
    #[must_use]
    pub(crate) fn class_id(self) -> Option<HeapId> {
        match self {
            Self::Native(_) => None,
            Self::Class(id) => Some(id),
        }
    }
}

/// Whether heap data is a class object that a [`TypeRef::Class`] may name.
#[must_use]
pub(crate) fn is_class_like(data: &HeapData) -> bool {
    matches!(
        data,
        HeapData::Class(_) | HeapData::NamedTupleClass(_) | HeapData::HostClassType(_)
    )
}

/// The ancestors stored for a class-like object, excluding itself.
fn class_like_mro(data: &HeapData) -> &[TypeRef] {
    /// A named tuple class derives from `tuple`.
    const NAMED_TUPLE_MRO: &[TypeRef] = &[TypeRef::Native(Type::Tuple), TypeRef::Native(Type::Object)];
    /// A host class is opaque here: the host sends no bases.
    const OBJECT_MRO: &[TypeRef] = &[TypeRef::Native(Type::Object)];
    match data {
        HeapData::Class(class) => class.ancestors(),
        HeapData::NamedTupleClass(_) => NAMED_TUPLE_MRO,
        HeapData::HostClassType(_) => OBJECT_MRO,
        _ => unreachable!("TypeRef::Class names a class object"),
    }
}

/// The name of a class-like heap object.
fn class_like_name<'i>(id: HeapId, heap: &Heap, interns: &'i Interns) -> Cow<'i, str> {
    match heap.get(id) {
        HeapData::Class(class) => class.name().to_cow(interns),
        HeapData::NamedTupleClass(class) => class.name_either().to_cow(interns),
        HeapData::HostClassType(class) => class.name_either().to_cow(interns),
        _ => unreachable!("TypeRef::Class names a class object"),
    }
}

/// CPython's C3 merge: linearises `sequences` (each base's MRO, then the list
/// of bases) into one order, or returns `Ok(Err)` with the heads still pending
/// when no consistent order exists.
///
/// Cubic in the number of bases at worst, so it polls `tracker`'s time limit.
pub(crate) fn c3_merge(
    mut sequences: Vec<Vec<TypeRef>>,
    tracker: &ResourceTracker,
) -> RunResult<Result<Vec<TypeRef>, Vec<TypeRef>>> {
    let mut merged = Vec::new();
    let mut heads_tested = 0;
    loop {
        sequences.retain(|sequence| !sequence.is_empty());
        if sequences.is_empty() {
            return Ok(Ok(merged));
        }
        // The first head that appears in no sequence's tail comes next.
        let mut next = None;
        for sequence in &sequences {
            tracker.check_time_every(heads_tested)?;
            heads_tested += 1;
            let head = sequence[0];
            if !sequences.iter().any(|sequence| sequence[1..].contains(&head)) {
                next = Some(head);
                break;
            }
        }
        let Some(next) = next else {
            let mut pending: Vec<TypeRef> = Vec::new();
            for sequence in &sequences {
                if !pending.contains(&sequence[0]) {
                    pending.push(sequence[0]);
                }
            }
            return Ok(Err(pending));
        };
        merged.push(next);
        for sequence in &mut sequences {
            if sequence[0] == next {
                sequence.remove(0);
            }
        }
    }
}
