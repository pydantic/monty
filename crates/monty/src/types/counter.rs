use std::fmt::Write;

use ahash::AHashSet;

use crate::{
    args::ArgValues,
    exception_private::RunResult,
    heap::{DropWithHeap, Heap, HeapId},
    intern::Interns,
    resource::{ResourceError, ResourceTracker},
    types::{Dict, PyTrait, Type},
    value::{EitherStr, Value},
};

/// Python Counter type (collections.Counter).
///
/// Under the hood, this wraps the Dict type but overrides `py_getitem`
/// to return `0` instead of a KeyError when a key is missing.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Counter {
    pub(crate) dict: Dict,
}

impl DropWithHeap for Counter {
    fn drop_with_heap<T: ResourceTracker>(self, heap: &mut Heap<T>) {
        self.dict.drop_with_heap(heap);
    }
}

impl PyTrait for Counter {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Counter
    }

    fn py_estimate_size(&self) -> usize {
        self.dict.py_estimate_size()
    }

    fn py_len(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Option<usize> {
        self.dict.py_len(heap, interns)
    }

    fn py_eq(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> Result<bool, ResourceError> {
        self.dict.py_eq(&other.dict, heap, interns)
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.dict.py_dec_ref_ids(stack);
    }

    fn py_bool(&self, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        self.dict.py_bool(heap, interns)
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result {
        write!(f, "Counter(")?;
        self.dict.py_repr_fmt(f, heap, heap_ids, interns)?;
        write!(f, ")")
    }

    fn py_getitem(&self, key: &Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
        match self.dict.get(key, heap, interns)? {
            Some(value) => Ok(value.clone_with_heap(heap)),
            None => Ok(Value::Int(0)),
        }
    }

    fn py_setitem(
        &mut self,
        key: Value,
        value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<()> {
        self.dict.py_setitem(key, value, heap, interns)
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        self.dict.py_call_attr(heap, attr, args, interns)
    }
}
