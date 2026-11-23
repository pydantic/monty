//! Float storage: ordinary values stay immediate, while NaNs retain object identity.

use crate::{
    hash::{HashValue, hash_f64, identity_hash},
    heap::{Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReader},
    identity::Identity,
    types::py_trait::PyObjectIdentity,
    value::Value,
};

/// A heap float, used for NaNs whose identity cannot be recovered from their bits.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct HeapFloat(pub(crate) f64);

impl HeapItem for HeapFloat {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}
}

/// A float's numeric payload and, for a heap float, a guard retaining its identity.
pub(crate) struct PyFloat<'h> {
    value: f64,
    object: Option<HeapObjectRead<'h, HeapFloat>>,
}

impl<'h> PyFloat<'h> {
    /// Opens an immediate float without a heap read.
    pub(crate) fn immediate(value: f64) -> Self {
        Self { value, object: None }
    }

    /// Keeps the heap object alive while exposing its immutable numeric payload.
    pub(crate) fn from_heap(object: HeapObjectRead<'h, HeapFloat>, heap: &HeapReader<'h>) -> Self {
        Self {
            value: object.get(heap).0,
            object: Some(object),
        }
    }

    /// Returns the IEEE-754 value without changing Python identity.
    pub(crate) fn value(&self) -> f64 {
        self.value
    }

    /// Returns an owned reference to the original float rather than allocating another NaN.
    pub(crate) fn to_value(&self, heap: &Heap) -> Value {
        if let Some(object) = &self.object {
            heap.inc_ref(object.id());
            Value::Ref(object.id())
        } else {
            Value::Float(self.value)
        }
    }

    /// NaNs hash by identity; all other floats keep their numeric hash.
    pub(crate) fn hash(&self) -> HashValue {
        self.object
            .as_ref()
            .map_or_else(|| hash_f64(self.value), |object| identity_hash(object.id()))
    }
}

impl PyObjectIdentity for PyFloat<'_> {
    fn py_identity(&self) -> Identity {
        self.object.as_ref().map_or_else(
            || Identity::Float(self.value.to_bits()),
            |object| Identity::from_heap_id(object.id()),
        )
    }
}

impl Value {
    /// Creates a Python float, allocating only when object identity matters for NaN.
    #[inline]
    pub(crate) fn float(value: f64, heap: &Heap) -> Self {
        if value.is_nan() {
            Self::Ref(heap.allocate(HeapData::Float(HeapFloat(value))))
        } else {
            Self::Float(value)
        }
    }

    /// Reads a builtin float without coercion or Python execution.
    #[inline]
    pub(crate) fn as_float(&self, heap: &Heap) -> Option<f64> {
        match self {
            Self::Float(value) => Some(*value),
            Self::Ref(id) if let HeapData::Float(value) = heap.get(*id) => Some(value.0),
            _ => None,
        }
    }
}
