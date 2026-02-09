//! Runtime generic alias support for `__class_getitem__`.
//!
//! `GenericAlias` mirrors CPython's `types.GenericAlias` objects, capturing
//! the origin class and the provided type arguments.

use std::fmt::Write;

use ahash::AHashSet;
use smallvec::SmallVec;

use crate::{
    builtins::Builtins,
    exception_private::RunResult,
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings, StringId},
    resource::ResourceTracker,
    types::{AttrCallResult, PyTrait, Type, allocate_tuple},
    value::Value,
};

/// Builds a runtime `GenericAlias` for `origin[item]`.
///
/// Accepts either a single item or a tuple of items, matching CPython's
/// `__class_getitem__` calling convention.
pub(crate) fn make_generic_alias(
    origin: Value,
    item: Value,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Value> {
    let args = generic_alias_args_from_item(item, heap);
    let parameters = generic_alias_parameters(&origin, heap, interns);
    let origin = match origin {
        Value::Ref(id) => {
            if let Some(t) = heap.builtin_type_for_class_id(id) {
                Value::Ref(id).drop_with_heap(heap);
                Value::Builtin(Builtins::Type(t))
            } else {
                Value::Ref(id)
            }
        }
        other => other,
    };
    let alias = GenericAlias::new(origin, args, parameters);
    let alias_id = heap.allocate(HeapData::GenericAlias(alias))?;
    Ok(Value::Ref(alias_id))
}

/// Converts a `__class_getitem__` argument into a list of generic arguments.
fn generic_alias_args_from_item(item: Value, heap: &mut Heap<impl ResourceTracker>) -> Vec<Value> {
    match item {
        Value::Ref(id) if matches!(heap.get(id), HeapData::Tuple(_)) => {
            let items = match heap.get(id) {
                HeapData::Tuple(tuple) => tuple.as_vec().iter().map(|value| value.clone_with_heap(heap)).collect(),
                _ => unreachable!(),
            };
            Value::Ref(id).drop_with_heap(heap);
            items
        }
        other => vec![other],
    }
}

/// Extracts `__type_params__` from the origin class, if present.
fn generic_alias_parameters(origin: &Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Vec<Value> {
    let Value::Ref(class_id) = origin else {
        return Vec::new();
    };
    let HeapData::ClassObject(cls) = heap.get(*class_id) else {
        return Vec::new();
    };
    let params = cls
        .namespace()
        .get_by_str("__type_params__", heap, interns)
        .and_then(|value| match value {
            Value::Ref(id) if matches!(heap.get(*id), HeapData::Tuple(_)) => Some(*id),
            _ => None,
        });
    let Some(tuple_id) = params else {
        return Vec::new();
    };
    match heap.get(tuple_id) {
        HeapData::Tuple(tuple) => tuple.as_vec().iter().map(|value| value.clone_with_heap(heap)).collect(),
        _ => Vec::new(),
    }
}

/// A runtime alias capturing `Origin[Args...]` for PEP 695 generics.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct GenericAlias {
    /// The origin class (or callable) being parameterized.
    origin: Value,
    /// The concrete type arguments.
    args: Vec<Value>,
    /// The type parameters declared on the origin, if any.
    parameters: Vec<Value>,
}

impl GenericAlias {
    /// Creates a new `GenericAlias` from origin, args, and parameters.
    #[must_use]
    pub fn new(origin: Value, args: Vec<Value>, parameters: Vec<Value>) -> Self {
        Self {
            origin,
            args,
            parameters,
        }
    }

    /// Returns the origin value.
    #[must_use]
    pub fn origin(&self) -> &Value {
        &self.origin
    }

    /// Returns the argument list.
    #[must_use]
    pub fn args(&self) -> &[Value] {
        &self.args
    }

    /// Returns the type parameters.
    #[must_use]
    pub fn parameters(&self) -> &[Value] {
        &self.parameters
    }
}

impl PyTrait for GenericAlias {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::GenericAlias
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.args.len() * std::mem::size_of::<Value>()
            + self.parameters.len() * std::mem::size_of::<Value>()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        None
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        if self.args.len() != other.args.len() || self.parameters.len() != other.parameters.len() {
            return false;
        }
        if !self.origin.py_eq(&other.origin, heap, interns) {
            return false;
        }
        for (a, b) in self.args.iter().zip(&other.args) {
            if !a.py_eq(b, heap, interns) {
                return false;
            }
        }
        for (a, b) in self.parameters.iter().zip(&other.parameters) {
            if !a.py_eq(b, heap, interns) {
                return false;
            }
        }
        true
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.origin.py_dec_ref_ids(stack);
        for value in &mut self.args {
            value.py_dec_ref_ids(stack);
        }
        for value in &mut self.parameters {
            value.py_dec_ref_ids(stack);
        }
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result {
        let origin_name = match &self.origin {
            Value::Ref(id) => {
                if let HeapData::ClassObject(cls) = heap.get(*id) {
                    cls.name(interns).to_string()
                } else {
                    let mut buf = String::new();
                    self.origin.py_repr_fmt(&mut buf, heap, heap_ids, interns)?;
                    buf
                }
            }
            Value::Builtin(Builtins::Type(t)) => t.to_string(),
            Value::Builtin(b) => {
                let mut buf = String::new();
                b.py_repr_fmt(&mut buf)?;
                buf
            }
            _ => {
                let mut buf = String::new();
                self.origin.py_repr_fmt(&mut buf, heap, heap_ids, interns)?;
                buf
            }
        };

        f.write_str(&origin_name)?;
        f.write_char('[')?;
        let mut first = true;
        for arg in &self.args {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            arg.py_repr_fmt(f, heap, heap_ids, interns)?;
        }
        f.write_char(']')
    }

    fn py_getattr(
        &self,
        attr_id: StringId,
        heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> RunResult<Option<AttrCallResult>> {
        let Some(static_attr) = StaticStrings::from_string_id(attr_id) else {
            return Ok(None);
        };

        match static_attr {
            StaticStrings::DunderOrigin => Ok(Some(AttrCallResult::Value(self.origin.clone_with_heap(heap)))),
            StaticStrings::DunderArgs => {
                let mut items: SmallVec<[Value; 3]> = SmallVec::new();
                for arg in &self.args {
                    items.push(arg.clone_with_heap(heap));
                }
                let tuple_val = allocate_tuple(items, heap)?;
                Ok(Some(AttrCallResult::Value(tuple_val)))
            }
            StaticStrings::DunderParameters => {
                let mut items: SmallVec<[Value; 3]> = SmallVec::new();
                for param in &self.parameters {
                    items.push(param.clone_with_heap(heap));
                }
                let tuple_val = allocate_tuple(items, heap)?;
                Ok(Some(AttrCallResult::Value(tuple_val)))
            }
            _ => Ok(None),
        }
    }
}
