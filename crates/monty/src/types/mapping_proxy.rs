//! Read-only dictionaries returned by class `__dict__` lookup.

use std::fmt::Write;

use monty_macros::pyattrs;

use super::{Dict, LazyHeapSet, PyTrait, Type, TypeRef};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput, HeapReader},
    intern::StaticStrings,
    py_attrs::PyAttrs,
    value::Value,
};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct MappingProxy {
    dict_id: HeapId,
}

impl MappingProxy {
    /// Native namespaces are fixed for the lifetime of the interpreter.
    pub(crate) fn class_namespace(owner: Type, vm: &mut VM<'_>) -> RunResult<Value> {
        let mut namespace = DropGuard::new(Dict::new(), vm);
        let (dict, vm) = namespace.as_parts_mut();
        if let Some(table) = owner.def().attrs {
            for &(key, def) in table.entries {
                let name = Value::InternString(vm.interns.intern_static(key));
                let member = def.namespace_value(owner, key, vm);
                dict.set(name, member, vm)?.drop_with(vm);
            }
        }
        let (dict, vm) = namespace.into_parts();
        let dict_id = vm.heap.allocate(HeapData::Dict(dict));
        Ok(Value::allocate(Self { dict_id }, vm.heap))
    }

    pub(crate) fn live(dict_id: HeapId, heap: &Heap) -> Value {
        heap.inc_ref(dict_id);
        Value::allocate(Self { dict_id }, heap)
    }

    pub(crate) fn dict_id(&self) -> HeapId {
        self.dict_id
    }

    /// Accepts plain dictionaries and their read-only proxies as sources for mapping operations.
    pub(crate) fn source<'h>(value: &Value, heap: &HeapReader<'h>) -> Option<HeapObjectRead<'h, Dict>> {
        match heap.read(value.ref_id()?) {
            HeapReadOutput::Dict(dict) => Some(dict),
            HeapReadOutput::MappingProxy(proxy) => Some(proxy.dict(heap)),
            _ => None,
        }
    }
}

impl<'h> HeapObjectRead<'h, MappingProxy> {
    fn dict(&self, heap: &HeapReader<'h>) -> HeapObjectRead<'h, Dict> {
        heap.read_as(self.get(heap).dict_id).expect("mappingproxy holds a dict")
    }

    fn call_read_method(&self, name: StaticStrings, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let dict = self.dict(vm.heap).clone_value(vm.heap);
        vm.call_attr(dict, vm.interns.intern_static(name), args)
    }
}

impl HeapItem for MappingProxy {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.dict_id);
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, MappingProxy> {
    fn py_type(&self, _: &VM<'h>) -> TypeRef {
        Type::MappingProxy.into()
    }
    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        self.dict(vm.heap).py_len(vm)
    }
    fn py_is_iterable(&self, _: &VM<'h>) -> bool {
        true
    }
    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        self.dict(vm.heap).py_iter(vm)
    }
    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        self.dict(vm.heap).py_getitem(key, vm)
    }
    fn py_contains_impl(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        self.dict(vm.heap).py_contains_impl(key, vm)
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(other) = MappingProxy::source(other, vm.heap) else {
            return Ok(None);
        };
        let other = other.clone_value(vm.heap);
        defer_drop!(other, vm);
        self.dict(vm.heap).py_eq_impl(other, vm)
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(other) = MappingProxy::source(other, vm.heap) else {
            return Ok(None);
        };
        let other = other.clone_value(vm.heap);
        defer_drop!(other, vm);
        self.dict(vm.heap).py_or_impl(other, vm)
    }

    fn py_ror_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(other) = MappingProxy::source(other, vm.heap) else {
            return Ok(None);
        };
        let dict = self.dict(vm.heap).clone_value(vm.heap);
        defer_drop!(dict, vm);
        other.py_or_impl(dict, vm)
    }

    fn py_ior_impl(&mut self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<bool> {
        Err(ExcType::type_error(
            "'|=' is not supported by mappingproxy; use '|' instead",
        ))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, ids: &mut LazyHeapSet) -> RunResult<()> {
        f.write_str("mappingproxy(")?;
        self.dict(vm.heap).py_repr_fmt(f, vm, ids)?;
        Ok(f.write_char(')')?)
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        self.dict(vm.heap).py_str(vm)
    }
}

#[pyattrs]
impl<'h> PyAttrs<'h> for MappingProxy {
    type Receiver = HeapObjectRead<'h, MappingProxy>;
    #[pymethod(Get)]
    fn get<'h>(
        proxy: &mut HeapObjectRead<'h, MappingProxy>,
        args: ArgValues,
        vm: &mut VM<'h>,
    ) -> RunResult<CallResult> {
        proxy.call_read_method(StaticStrings::Get, args, vm)
    }
    #[pymethod(Keys)]
    fn keys<'h>(
        proxy: &mut HeapObjectRead<'h, MappingProxy>,
        args: ArgValues,
        vm: &mut VM<'h>,
    ) -> RunResult<CallResult> {
        proxy.call_read_method(StaticStrings::Keys, args, vm)
    }
    #[pymethod(Items)]
    fn items<'h>(
        proxy: &mut HeapObjectRead<'h, MappingProxy>,
        args: ArgValues,
        vm: &mut VM<'h>,
    ) -> RunResult<CallResult> {
        proxy.call_read_method(StaticStrings::Items, args, vm)
    }
    #[pymethod(Values)]
    fn values<'h>(
        proxy: &mut HeapObjectRead<'h, MappingProxy>,
        args: ArgValues,
        vm: &mut VM<'h>,
    ) -> RunResult<CallResult> {
        proxy.call_read_method(StaticStrings::Values, args, vm)
    }
    #[pymethod(Copy)]
    fn copy<'h>(
        proxy: &mut HeapObjectRead<'h, MappingProxy>,
        args: ArgValues,
        vm: &mut VM<'h>,
    ) -> RunResult<CallResult> {
        proxy.call_read_method(StaticStrings::Copy, args, vm)
    }
}
