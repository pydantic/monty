//! Read-only mapping proxy for class namespaces.
//!
//! Python exposes `type.__dict__` as a `mappingproxy` object that provides a live,
//! read-only view of the class namespace. This wrapper keeps a reference to the
//! owning class and forwards reads to the underlying namespace dict.

use std::fmt::Write;

use ahash::AHashSet;

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    resource::ResourceTracker,
    types::{Dict, List, PyTrait, Type, allocate_tuple},
    value::{EitherStr, Value},
};

/// Read-only view of a class namespace (`type.__dict__`).
///
/// This mirrors CPython's `mappingproxy` by exposing a live mapping while
/// preventing item assignment and mutation methods.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct MappingProxy {
    class_id: HeapId,
}

impl MappingProxy {
    /// Creates a new mapping proxy for a class HeapId.
    #[must_use]
    pub fn new(class_id: HeapId) -> Self {
        Self { class_id }
    }

    /// Returns the HeapId of the class this proxy wraps.
    #[must_use]
    pub fn class_id(&self) -> HeapId {
        self.class_id
    }

    /// Returns whether this proxy holds heap references.
    #[inline]
    #[must_use]
    #[expect(clippy::unused_self)]
    pub fn has_refs(&self) -> bool {
        true
    }

    /// Returns the class namespace dict if the underlying class is still alive.
    fn namespace<'a>(&self, heap: &'a Heap<impl ResourceTracker>) -> Option<&'a Dict> {
        match heap.get(self.class_id) {
            HeapData::ClassObject(cls) => Some(cls.namespace()),
            _ => None,
        }
    }
}

impl PyTrait for MappingProxy {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::MappingProxy
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn py_len(&self, heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        self.namespace(heap).map(Dict::len)
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        heap.with_two(self.class_id, other.class_id, |heap, left, right| match (left, right) {
            (HeapData::ClassObject(a), HeapData::ClassObject(b)) => a.namespace().py_eq(b.namespace(), heap, interns),
            _ => false,
        })
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.class_id);
    }

    fn py_bool(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        self.py_len(heap, interns) != Some(0)
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result {
        f.write_str("mappingproxy(")?;
        if let Some(dict) = self.namespace(heap) {
            dict.py_repr_fmt(f, heap, heap_ids, interns)?;
        } else {
            f.write_str("{}")?;
        }
        f.write_char(')')
    }

    fn py_getitem(&self, key: &Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
        heap.with_entry_mut(self.class_id, |heap, data| {
            let HeapData::ClassObject(cls) = data else {
                return Err(ExcType::type_error("mappingproxy references invalid class".to_string()));
            };
            cls.namespace().py_getitem(key, heap, interns)
        })
    }

    fn py_setitem(
        &mut self,
        key: Value,
        value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> RunResult<()> {
        key.drop_with_heap(heap);
        value.drop_with_heap(heap);
        Err(ExcType::type_error(
            "'mappingproxy' object does not support item assignment".to_string(),
        ))
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        let Some(method) = attr.static_string() else {
            return Err(ExcType::attribute_error(Type::MappingProxy, attr.as_str(interns)));
        };

        heap.with_entry_mut(self.class_id, |heap, data| {
            let HeapData::ClassObject(cls) = data else {
                return Err(ExcType::type_error("mappingproxy references invalid class".to_string()));
            };
            let dict = cls.namespace();

            match method {
                StaticStrings::Get => {
                    let (key, default) = args.get_one_two_args("mappingproxy.get", heap)?;
                    let default = default.unwrap_or(Value::None);
                    let result = match dict.get(&key, heap, interns) {
                        Ok(r) => r,
                        Err(e) => {
                            key.drop_with_heap(heap);
                            default.drop_with_heap(heap);
                            return Err(e);
                        }
                    };
                    let value = match result {
                        Some(v) => v.clone_with_heap(heap),
                        None => default.clone_with_heap(heap),
                    };
                    key.drop_with_heap(heap);
                    default.drop_with_heap(heap);
                    Ok(value)
                }
                StaticStrings::Keys => {
                    args.check_zero_args("mappingproxy.keys", heap)?;
                    let keys = dict.keys(heap);
                    let list_id = heap.allocate(HeapData::List(List::new(keys)))?;
                    Ok(Value::Ref(list_id))
                }
                StaticStrings::Values => {
                    args.check_zero_args("mappingproxy.values", heap)?;
                    let values = dict.values(heap);
                    let list_id = heap.allocate(HeapData::List(List::new(values)))?;
                    Ok(Value::Ref(list_id))
                }
                StaticStrings::Items => {
                    args.check_zero_args("mappingproxy.items", heap)?;
                    let items = dict.items(heap);
                    let mut tuples: Vec<Value> = Vec::with_capacity(items.len());
                    for (k, v) in items {
                        let tuple_val = allocate_tuple(smallvec::smallvec![k, v], heap)?;
                        tuples.push(tuple_val);
                    }
                    let list_id = heap.allocate(HeapData::List(List::new(tuples)))?;
                    Ok(Value::Ref(list_id))
                }
                StaticStrings::Copy => {
                    args.check_zero_args("mappingproxy.copy", heap)?;
                    let dict_copy = dict.clone_with_heap(heap, interns)?;
                    let dict_id = heap.allocate(HeapData::Dict(dict_copy))?;
                    Ok(Value::Ref(dict_id))
                }
                _ => Err(ExcType::attribute_error(Type::MappingProxy, attr.as_str(interns))),
            }
        })
    }
}
