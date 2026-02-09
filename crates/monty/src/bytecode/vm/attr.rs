//! Attribute access helpers for the VM.
//!
//! Implements the full Python descriptor protocol for attribute access:
//! - Data descriptors (classes with `__set__` or `__delete__`) take priority over instance attrs
//! - Instance attrs take priority over non-data descriptors
//! - Non-data descriptors (classes with only `__get__`) come last
//! - `@property` (`UserProperty`) is a special case of data descriptor

use super::{PendingGetAttrKind, VM};
use crate::{
    args::ArgValues,
    bytecode::vm::CallResult,
    exception_private::{ExcType, RunError},
    heap::{HeapData, HeapId},
    intern::{StaticStrings, StringId},
    io::PrintWriter,
    resource::ResourceTracker,
    types::{
        AttrCallResult, PyTrait, SlotDescriptorKind, Type,
        class::{has_descriptor_get, is_data_descriptor},
    },
    value::Value,
};

/// Result of looking up a descriptor for setter/deleter/getter operations.
enum DescriptorLookup {
    /// No descriptor found - use normal store/delete/get behavior.
    NotDescriptor,
    /// Descriptor found with a function to call (UserProperty setter/deleter/getter
    /// or custom descriptor `__set__`/`__delete__`/`__get__` method).
    HasFunc(Value),
    /// Descriptor found but missing the required function (read-only property for set).
    ReadOnly,
    /// Custom descriptor found with `__set__` dunder - needs to call as dunder.
    /// The Value is the descriptor instance, and the calling convention requires
    /// calling `descriptor.__set__(instance, value)`.
    CustomDescriptorSet(Value),
    /// Custom descriptor found with `__delete__` dunder - needs to call as dunder.
    /// The Value is the descriptor instance.
    CustomDescriptorDelete(Value),
    /// Slot descriptor found - needs to access instance slot storage directly.
    SlotDescriptor(Value),
}

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
    /// Loads an attribute from an object and pushes it onto the stack.
    ///
    /// Returns an AttributeError if the attribute doesn't exist.
    /// Handles `PropertyCall` results by calling the getter function.
    /// Handles `DescriptorGet` by calling `descriptor.__get__(instance, type)`.
    pub(super) fn load_attr(&mut self, name_id: StringId) -> Result<CallResult, RunError> {
        let obj = self.pop();
        let obj_id = match &obj {
            Value::Ref(id) => Some(*id),
            _ => None,
        };

        // If this is a user instance, honor __getattribute__ / __getattr__ overrides.
        if let Some(obj_id) = obj_id
            && matches!(self.heap.get(obj_id), HeapData::Instance(_))
        {
            let getattribute_id: StringId = StaticStrings::DunderGetattribute.into();
            if let Some(method) = self.lookup_type_dunder(obj_id, getattribute_id) {
                let name_val = Value::InternString(name_id);
                let result = self.call_dunder(obj_id, method, ArgValues::One(name_val));
                obj.drop_with_heap(self.heap);
                match result {
                    Ok(CallResult::Push(v)) => return Ok(CallResult::Push(v)),
                    Ok(CallResult::FramePushed) => {
                        self.push_pending_getattr_fallback(obj_id, name_id, PendingGetAttrKind::Instance);
                        return Ok(CallResult::FramePushed);
                    }
                    Ok(CallResult::External(ext_id, args)) => return Ok(CallResult::External(ext_id, args)),
                    Ok(CallResult::OsCall(func, args)) => return Ok(CallResult::OsCall(func, args)),
                    Err(e) => {
                        if let RunError::Exc(exc) = &e
                            && exc.exc.exc_type() == ExcType::AttributeError
                        {
                            let getattr_id: StringId = StaticStrings::DunderGetattr.into();
                            if let Some(getattr) = self.lookup_type_dunder(obj_id, getattr_id) {
                                let name_val = Value::InternString(name_id);
                                return self.call_dunder(obj_id, getattr, ArgValues::One(name_val));
                            }
                        }
                        return Err(e);
                    }
                }
            }
        }

        // If this is a class object, honor metaclass __getattribute__/__getattr__,
        // then fall back to type.__getattribute__ semantics.
        if let Some(obj_id) = obj_id
            && matches!(self.heap.get(obj_id), HeapData::ClassObject(_))
        {
            let getattribute_id: StringId = StaticStrings::DunderGetattribute.into();
            if let Some(method) = self.lookup_metaclass_dunder(obj_id, getattribute_id) {
                let name_val = Value::InternString(name_id);
                let result = self.call_class_dunder(obj_id, method, ArgValues::One(name_val));
                obj.drop_with_heap(self.heap);
                match result {
                    Ok(CallResult::Push(v)) => return Ok(CallResult::Push(v)),
                    Ok(CallResult::FramePushed) => {
                        self.push_pending_getattr_fallback(obj_id, name_id, PendingGetAttrKind::Class);
                        return Ok(CallResult::FramePushed);
                    }
                    Ok(CallResult::External(ext_id, args)) => return Ok(CallResult::External(ext_id, args)),
                    Ok(CallResult::OsCall(func, args)) => return Ok(CallResult::OsCall(func, args)),
                    Err(e) => {
                        if let RunError::Exc(exc) = &e
                            && exc.exc.exc_type() == ExcType::AttributeError
                        {
                            let getattr_id: StringId = StaticStrings::DunderGetattr.into();
                            if let Some(getattr) = self.lookup_metaclass_dunder(obj_id, getattr_id) {
                                let name_val = Value::InternString(name_id);
                                return self.call_class_dunder(obj_id, getattr, ArgValues::One(name_val));
                            }
                        }
                        return Err(e);
                    }
                }
            }

            return self.load_class_attr_default(obj, obj_id, name_id);
        }

        let result = obj.py_getattr(name_id, self.heap, self.interns);

        match result {
            Ok(AttrCallResult::PropertyCall(getter, instance)) => {
                obj.drop_with_heap(self.heap);
                // Property getter: call getter(instance) and return result
                let args = ArgValues::One(instance);
                self.call_function(getter, args)
            }
            Ok(AttrCallResult::DescriptorGet(descriptor)) => {
                // Custom descriptor __get__: call descriptor.__get__(instance, type)
                let instance_ref = obj; // The object being accessed
                self.call_descriptor_get(descriptor, instance_ref)
            }
            Ok(AttrCallResult::Value(value)) => {
                // Check for class-level access on custom descriptors.
                // When obj is a ClassObject and the value is an Instance with __get__,
                // we need to invoke the descriptor protocol (__get__(None, owner_class)).
                // This can't be done in ClassObject::py_getattr due to heap borrow conflicts
                // (the class entry is borrowed by with_entry_mut).
                let is_class_access = if let Value::Ref(obj_id) = &obj {
                    matches!(self.heap.get(*obj_id), HeapData::ClassObject(_))
                } else {
                    false
                };
                if is_class_access && let Value::Ref(val_id) = &value {
                    let val_id = *val_id;
                    // Two-phase: extract class_id from Instance first
                    let desc_class_id = match self.heap.get(val_id) {
                        HeapData::Instance(inst) => Some(inst.class_id()),
                        _ => None,
                    };
                    if let Some(desc_class_id) = desc_class_id {
                        let has_get = match self.heap.get(desc_class_id) {
                            HeapData::ClassObject(desc_cls) => {
                                desc_cls.mro_has_attr("__get__", desc_class_id, self.heap, self.interns)
                            }
                            _ => false,
                        };
                        if has_get {
                            // Invoke descriptor protocol: __get__(None, owner_class)
                            let instance_ref = obj;
                            return self.call_descriptor_get(value, instance_ref);
                        }
                    }
                }
                obj.drop_with_heap(self.heap);
                Ok(CallResult::Push(value))
            }
            Ok(other) => {
                obj.drop_with_heap(self.heap);
                Ok(other.into())
            }
            Err(e) => {
                if let Some(obj_id) = obj_id
                    && matches!(self.heap.get(obj_id), HeapData::Instance(_))
                    && matches!(e, RunError::Exc(ref exc) if exc.exc.exc_type() == ExcType::AttributeError)
                {
                    let getattr_id: StringId = StaticStrings::DunderGetattr.into();
                    if let Some(getattr) = self.lookup_type_dunder(obj_id, getattr_id) {
                        obj.drop_with_heap(self.heap);
                        let name_val = Value::InternString(name_id);
                        return self.call_dunder(obj_id, getattr, ArgValues::One(name_val));
                    }
                }
                obj.drop_with_heap(self.heap);
                Err(e)
            }
        }
    }

    /// Calls a custom descriptor's `__get__` method.
    ///
    /// Implements `descriptor.__get__(instance, type)` for the descriptor protocol.
    /// For class-level access (`obj is None`), passes `(None, type)`.
    /// For instance-level access, passes `(instance, type(instance))`.
    fn call_descriptor_get(&mut self, descriptor: Value, instance: Value) -> Result<CallResult, RunError> {
        let get_id: StringId = StaticStrings::DunderDescGet.into();

        if let Value::Ref(desc_id) = &descriptor
            && let HeapData::SlotDescriptor(desc) = self.heap.get(*desc_id)
        {
            let name = desc.name().to_string();
            let kind = desc.kind();
            return self.call_slot_descriptor_get(&name, kind, descriptor, instance);
        }

        if let Value::Ref(desc_id) = &descriptor {
            let desc_id = *desc_id;
            if let Some(method) = self.lookup_type_dunder(desc_id, get_id) {
                // Determine obj and type args for __get__(self, obj, type).
                // Instance-level: __get__(descriptor, instance, type(instance))
                // Class-level: __get__(descriptor, None, owner_class)
                let (obj_val, type_val) = if let Value::Ref(inst_id) = &instance {
                    let inst_id = *inst_id;
                    match self.heap.get(inst_id) {
                        HeapData::Instance(inst) => {
                            let cid = inst.class_id();
                            self.heap.inc_ref(cid);
                            // Instance access: obj=instance, type=class
                            (instance, Value::Ref(cid))
                        }
                        HeapData::ClassObject(_) => {
                            // Class access: obj=None, type=class
                            // instance already has a ref from the caller
                            (Value::None, instance)
                        }
                        _ => (instance, Value::None),
                    }
                } else {
                    (instance, Value::None)
                };

                // Call __get__(descriptor_self, obj, type)
                self.heap.inc_ref(desc_id);
                let args = ArgValues::ArgsKargs {
                    args: vec![Value::Ref(desc_id), obj_val, type_val],
                    kwargs: crate::args::KwargsValues::Empty,
                };
                let result = self.call_function(method, args);
                descriptor.drop_with_heap(self.heap);
                return result;
            }
        }

        // No __get__ found - return descriptor itself
        instance.drop_with_heap(self.heap);
        Ok(CallResult::Push(descriptor))
    }

    /// Handles slot descriptor `__get__` behavior for `__slots__` entries.
    ///
    /// For class-level access, returns the descriptor itself.
    /// For instance-level access, reads the slot storage or raises AttributeError.
    fn call_slot_descriptor_get(
        &mut self,
        name: &str,
        kind: SlotDescriptorKind,
        descriptor: Value,
        instance: Value,
    ) -> Result<CallResult, RunError> {
        let instance_id = if let Value::Ref(id) = &instance {
            *id
        } else {
            instance.drop_with_heap(self.heap);
            return Ok(CallResult::Push(descriptor));
        };

        if matches!(self.heap.get(instance_id), HeapData::ClassObject(_)) {
            instance.drop_with_heap(self.heap);
            return Ok(CallResult::Push(descriptor));
        }

        let result = self.heap.with_entry_mut(instance_id, |heap, data| {
            let HeapData::Instance(inst) = data else {
                return Err(ExcType::attribute_error(Type::Instance, name));
            };

            match kind {
                SlotDescriptorKind::Member => {
                    if let Some(value) = inst.slot_value(name, heap) {
                        Ok(value.clone_with_heap(heap))
                    } else {
                        let class_name = match heap.get(inst.class_id()) {
                            HeapData::ClassObject(cls) => cls.name(self.interns).to_string(),
                            _ => "<unknown>".to_string(),
                        };
                        Err(ExcType::attribute_error(class_name, name))
                    }
                }
                SlotDescriptorKind::Dict => {
                    let has_dict = match heap.get(inst.class_id()) {
                        HeapData::ClassObject(cls) => cls.instance_has_dict(),
                        _ => false,
                    };
                    if !has_dict {
                        let class_name = match heap.get(inst.class_id()) {
                            HeapData::ClassObject(cls) => cls.name(self.interns).to_string(),
                            _ => "<unknown>".to_string(),
                        };
                        return Err(ExcType::attribute_error(class_name, "__dict__"));
                    }
                    let Some(attrs_id) = inst.attrs_id() else {
                        return Err(ExcType::attribute_error(Type::Instance, "__dict__"));
                    };
                    heap.inc_ref(attrs_id);
                    Ok(Value::Ref(attrs_id))
                }
                SlotDescriptorKind::Weakref => {
                    let has_weakref = match heap.get(inst.class_id()) {
                        HeapData::ClassObject(cls) => cls.instance_has_weakref(),
                        _ => false,
                    };
                    if !has_weakref {
                        let class_name = match heap.get(inst.class_id()) {
                            HeapData::ClassObject(cls) => cls.name(self.interns).to_string(),
                            _ => "<unknown>".to_string(),
                        };
                        return Err(ExcType::attribute_error(class_name, "__weakref__"));
                    }
                    if let Some(weakref_id) = inst.weakref_id()
                        && heap.get_if_live(weakref_id).is_some()
                    {
                        heap.inc_ref(weakref_id);
                        Ok(Value::Ref(weakref_id))
                    } else {
                        Ok(Value::None)
                    }
                }
            }
        });

        instance.drop_with_heap(self.heap);
        descriptor.drop_with_heap(self.heap);
        result.map(CallResult::Push)
    }

    /// Stores a value using a slot descriptor (for `__slots__`).
    fn set_slot_descriptor(
        &mut self,
        descriptor: Value,
        instance: Value,
        value: Value,
    ) -> Result<CallResult, RunError> {
        let desc_id = if let Value::Ref(id) = &descriptor {
            *id
        } else {
            descriptor.drop_with_heap(self.heap);
            instance.drop_with_heap(self.heap);
            value.drop_with_heap(self.heap);
            return Err(RunError::internal("slot descriptor was not a heap value"));
        };

        let (name, kind) = if let HeapData::SlotDescriptor(desc) = self.heap.get(desc_id) {
            (desc.name().to_string(), desc.kind())
        } else {
            descriptor.drop_with_heap(self.heap);
            instance.drop_with_heap(self.heap);
            value.drop_with_heap(self.heap);
            return Err(RunError::internal("slot descriptor type mismatch"));
        };

        let instance_id = if let Value::Ref(id) = &instance {
            *id
        } else {
            descriptor.drop_with_heap(self.heap);
            instance.drop_with_heap(self.heap);
            value.drop_with_heap(self.heap);
            return Err(RunError::internal("slot descriptor target not instance"));
        };

        let result = self.heap.with_entry_mut(instance_id, |heap, data| {
            let HeapData::Instance(inst) = data else {
                value.drop_with_heap(heap);
                return Err(ExcType::attribute_error(Type::Instance, &name));
            };

            match kind {
                SlotDescriptorKind::Member => {
                    if let Some(old) = inst.set_slot_value(&name, value, heap, self.interns)? {
                        old.drop_with_heap(heap);
                    }
                    Ok(())
                }
                SlotDescriptorKind::Dict => {
                    let has_dict = match heap.get(inst.class_id()) {
                        HeapData::ClassObject(cls) => cls.instance_has_dict(),
                        _ => false,
                    };
                    if !has_dict {
                        value.drop_with_heap(heap);
                        let class_name = match heap.get(inst.class_id()) {
                            HeapData::ClassObject(cls) => cls.name(self.interns).to_string(),
                            _ => "<unknown>".to_string(),
                        };
                        return Err(ExcType::attribute_error_no_dict_for_setting(&class_name, "__dict__"));
                    }

                    let Value::Ref(dict_id) = value else {
                        let type_name = value.py_type(heap);
                        value.drop_with_heap(heap);
                        return Err(ExcType::type_error(format!(
                            "__dict__ must be set to a dictionary, not a '{type_name}'"
                        )));
                    };
                    if !matches!(heap.get(dict_id), HeapData::Dict(_)) {
                        let type_name = heap.get(dict_id).py_type(heap);
                        Value::Ref(dict_id).drop_with_heap(heap);
                        return Err(ExcType::type_error(format!(
                            "__dict__ must be set to a dictionary, not a '{type_name}'"
                        )));
                    }

                    let name_val = Value::InternString(StaticStrings::DunderDictAttr.into());
                    if let Some(old) = inst.set_attr(name_val, Value::Ref(dict_id), heap, self.interns)? {
                        old.drop_with_heap(heap);
                    }
                    Ok(())
                }
                SlotDescriptorKind::Weakref => {
                    value.drop_with_heap(heap);
                    let class_name = match heap.get(inst.class_id()) {
                        HeapData::ClassObject(cls) => cls.name(self.interns).to_string(),
                        _ => "<unknown>".to_string(),
                    };
                    Err(ExcType::attribute_error_weakref_not_writable(&class_name))
                }
            }
        });

        descriptor.drop_with_heap(self.heap);
        instance.drop_with_heap(self.heap);
        result.map(|()| CallResult::Push(Value::None))
    }

    /// Deletes a value using a slot descriptor (for `__slots__`).
    fn delete_slot_descriptor(&mut self, descriptor: Value, instance: Value) -> Result<CallResult, RunError> {
        let desc_id = if let Value::Ref(id) = &descriptor {
            *id
        } else {
            descriptor.drop_with_heap(self.heap);
            instance.drop_with_heap(self.heap);
            return Err(RunError::internal("slot descriptor was not a heap value"));
        };

        let (name, kind) = if let HeapData::SlotDescriptor(desc) = self.heap.get(desc_id) {
            (desc.name().to_string(), desc.kind())
        } else {
            descriptor.drop_with_heap(self.heap);
            instance.drop_with_heap(self.heap);
            return Err(RunError::internal("slot descriptor type mismatch"));
        };

        let instance_id = if let Value::Ref(id) = &instance {
            *id
        } else {
            descriptor.drop_with_heap(self.heap);
            instance.drop_with_heap(self.heap);
            return Err(RunError::internal("slot descriptor target not instance"));
        };

        let result = self.heap.with_entry_mut(instance_id, |heap, data| {
            let HeapData::Instance(inst) = data else {
                return Err(ExcType::attribute_error(Type::Instance, &name));
            };

            match kind {
                SlotDescriptorKind::Member => {
                    let old = inst.delete_slot_value(&name, heap, self.interns)?;
                    if let Some(old) = old {
                        old.drop_with_heap(heap);
                        Ok(())
                    } else {
                        let class_name = match heap.get(inst.class_id()) {
                            HeapData::ClassObject(cls) => cls.name(self.interns).to_string(),
                            _ => "<unknown>".to_string(),
                        };
                        Err(ExcType::attribute_error(class_name, &name))
                    }
                }
                SlotDescriptorKind::Dict => {
                    let has_dict = match heap.get(inst.class_id()) {
                        HeapData::ClassObject(cls) => cls.instance_has_dict(),
                        _ => false,
                    };
                    if !has_dict {
                        let class_name = match heap.get(inst.class_id()) {
                            HeapData::ClassObject(cls) => cls.name(self.interns).to_string(),
                            _ => "<unknown>".to_string(),
                        };
                        return Err(ExcType::attribute_error_no_dict_for_setting(&class_name, "__dict__"));
                    }
                    let new_id = heap.allocate(HeapData::Dict(crate::types::Dict::new()))?;
                    let name_val = Value::InternString(StaticStrings::DunderDictAttr.into());
                    if let Some(old) = inst.set_attr(name_val, Value::Ref(new_id), heap, self.interns)? {
                        old.drop_with_heap(heap);
                    }
                    Ok(())
                }
                SlotDescriptorKind::Weakref => {
                    let class_name = match heap.get(inst.class_id()) {
                        HeapData::ClassObject(cls) => cls.name(self.interns).to_string(),
                        _ => "<unknown>".to_string(),
                    };
                    Err(ExcType::attribute_error_weakref_not_writable(&class_name))
                }
            }
        });

        descriptor.drop_with_heap(self.heap);
        instance.drop_with_heap(self.heap);
        result.map(|()| CallResult::Push(Value::None))
    }

    /// Calls a descriptor's `__get__` with explicit obj and owner arguments.
    ///
    /// Used for metaclass descriptor lookups where `obj` is the class object
    /// and `owner` is the metaclass.
    fn call_descriptor_get_with_owner(
        &mut self,
        descriptor: Value,
        obj: Value,
        owner: Value,
    ) -> Result<CallResult, RunError> {
        let get_id: StringId = StaticStrings::DunderDescGet.into();

        if let Value::Ref(desc_id) = &descriptor {
            let desc_id = *desc_id;
            if let Some(method) = self.lookup_type_dunder(desc_id, get_id) {
                self.heap.inc_ref(desc_id);
                let args = ArgValues::ArgsKargs {
                    args: vec![Value::Ref(desc_id), obj, owner],
                    kwargs: crate::args::KwargsValues::Empty,
                };
                let result = self.call_function(method, args);
                descriptor.drop_with_heap(self.heap);
                return result;
            }
        }

        obj.drop_with_heap(self.heap);
        owner.drop_with_heap(self.heap);
        Ok(CallResult::Push(descriptor))
    }

    /// Default attribute lookup for class objects (type.__getattribute__ semantics).
    ///
    /// Implements:
    /// 1. Metaclass data descriptors
    /// 2. Class namespace + MRO lookup
    /// 3. Metaclass non-data descriptors/attrs
    /// 4. Metaclass __getattr__ fallback
    fn load_class_attr_default(
        &mut self,
        obj: Value,
        obj_id: HeapId,
        name_id: StringId,
    ) -> Result<CallResult, RunError> {
        let attr_name = self.interns.get_str(name_id);

        let mut meta_attr: Option<Value> = None;
        let mut meta_id: Option<HeapId> = None;
        let mut meta_is_data_descriptor = false;

        if let HeapData::ClassObject(cls) = self.heap.get(obj_id)
            && let Value::Ref(mid) = cls.metaclass()
        {
            meta_id = Some(*mid);
            if let HeapData::ClassObject(meta_cls) = self.heap.get(*mid)
                && let Some((value, _)) = meta_cls.mro_lookup_attr(attr_name, *mid, self.heap, self.interns)
            {
                if let Value::Ref(desc_id) = &value {
                    meta_is_data_descriptor = is_data_descriptor(*desc_id, self.heap, self.interns);
                }
                meta_attr = Some(value);
            }
        }

        if meta_is_data_descriptor {
            let meta_id = meta_id.expect("meta id missing for data descriptor");
            self.heap.inc_ref(meta_id);
            let owner_val = Value::Ref(meta_id);
            return self.call_descriptor_get_with_owner(
                meta_attr.expect("meta attr missing for data descriptor"),
                obj,
                owner_val,
            );
        }

        let result = obj.py_getattr(name_id, self.heap, self.interns);
        match result {
            Ok(AttrCallResult::PropertyCall(getter, instance)) => {
                if let Some(meta_val) = meta_attr {
                    meta_val.drop_with_heap(self.heap);
                }
                obj.drop_with_heap(self.heap);
                let args = ArgValues::One(instance);
                self.call_function(getter, args)
            }
            Ok(AttrCallResult::DescriptorGet(descriptor)) => {
                if let Some(meta_val) = meta_attr {
                    meta_val.drop_with_heap(self.heap);
                }
                let instance_ref = obj;
                self.call_descriptor_get(descriptor, instance_ref)
            }
            Ok(AttrCallResult::Value(value)) => {
                if let Some(meta_val) = meta_attr {
                    meta_val.drop_with_heap(self.heap);
                }
                // Class-level descriptor __get__ handling
                if let Value::Ref(val_id) = &value {
                    let desc_class_id = match self.heap.get(*val_id) {
                        HeapData::Instance(inst) => Some(inst.class_id()),
                        _ => None,
                    };
                    if let Some(desc_class_id) = desc_class_id {
                        let has_get = match self.heap.get(desc_class_id) {
                            HeapData::ClassObject(desc_cls) => {
                                desc_cls.mro_has_attr("__get__", desc_class_id, self.heap, self.interns)
                            }
                            _ => false,
                        };
                        if has_get {
                            let instance_ref = obj;
                            return self.call_descriptor_get(value, instance_ref);
                        }
                    }
                }
                obj.drop_with_heap(self.heap);
                Ok(CallResult::Push(value))
            }
            Ok(other) => {
                if let Some(meta_val) = meta_attr {
                    meta_val.drop_with_heap(self.heap);
                }
                obj.drop_with_heap(self.heap);
                Ok(other.into())
            }
            Err(e) => {
                if let Some(meta_val) = meta_attr {
                    if let Some(meta_id) = meta_id
                        && let Value::Ref(desc_id) = &meta_val
                        && has_descriptor_get(*desc_id, self.heap, self.interns)
                    {
                        self.heap.inc_ref(meta_id);
                        let owner_val = Value::Ref(meta_id);
                        return self.call_descriptor_get_with_owner(meta_val, obj, owner_val);
                    }
                    obj.drop_with_heap(self.heap);
                    return Ok(CallResult::Push(meta_val));
                }

                if let RunError::Exc(exc) = &e
                    && exc.exc.exc_type() == ExcType::AttributeError
                {
                    let getattr_id: StringId = StaticStrings::DunderGetattr.into();
                    if let Some(getattr) = self.lookup_metaclass_dunder(obj_id, getattr_id) {
                        obj.drop_with_heap(self.heap);
                        let name_val = Value::InternString(name_id);
                        return self.call_class_dunder(obj_id, getattr, ArgValues::One(name_val));
                    }
                }

                obj.drop_with_heap(self.heap);
                Err(e)
            }
        }
    }

    /// Loads an attribute from a module for `from ... import` and pushes it onto the stack.
    ///
    /// Returns an ImportError (not AttributeError) if the attribute doesn't exist,
    /// matching CPython's behavior for `from module import name`.
    pub(super) fn load_attr_import(&mut self, name_id: StringId) -> Result<CallResult, RunError> {
        let obj = self.pop();
        let result = obj.py_getattr(name_id, self.heap, self.interns);
        match result {
            Ok(result) => {
                obj.drop_with_heap(self.heap);
                Ok(result.into())
            }
            Err(RunError::Exc(exc)) if exc.exc.exc_type() == ExcType::AttributeError => {
                // Only compute module_name when we need it for the error message
                let module_name = obj.module_name(self.heap, self.interns);
                obj.drop_with_heap(self.heap);
                let name_str = self.interns.get_str(name_id);
                Err(ExcType::cannot_import_name(name_str, &module_name))
            }
            Err(e) => {
                obj.drop_with_heap(self.heap);
                Err(e)
            }
        }
    }

    /// Stores a value as an attribute on an object.
    ///
    /// For instances, checks if the class has a data descriptor (UserProperty or
    /// custom descriptor with `__set__`). If so, calls the descriptor's setter
    /// instead of setting directly.
    /// Returns `CallResult::Push(None)` for normal stores, or `CallResult::FramePushed`
    /// if a descriptor setter was called.
    pub(super) fn store_attr(&mut self, name_id: StringId) -> Result<CallResult, RunError> {
        let obj = self.pop();
        let value = self.pop();

        // Check if this is an instance with a descriptor setter on the class
        if let Value::Ref(heap_id) = &obj {
            let heap_id = *heap_id;
            if matches!(self.heap.get(heap_id), HeapData::Instance(_)) {
                // If __setattr__ is overridden, call it and skip default descriptor handling.
                let setattr_id: StringId = StaticStrings::DunderSetattr.into();
                if let Some(method) = self.lookup_type_dunder(heap_id, setattr_id) {
                    let name_val = Value::InternString(name_id);
                    let args = ArgValues::Two(name_val, value);
                    let result = self.call_dunder(heap_id, method, args)?;
                    obj.drop_with_heap(self.heap);
                    match result {
                        CallResult::Push(ret) => {
                            ret.drop_with_heap(self.heap);
                            return Ok(CallResult::Push(Value::None));
                        }
                        CallResult::FramePushed => return Ok(CallResult::FramePushed),
                        other => return Ok(other),
                    }
                }

                match self.find_descriptor_setter(heap_id, name_id) {
                    DescriptorLookup::HasFunc(setter_func) => {
                        // UserProperty setter found - call setter(instance, value)
                        let args = ArgValues::Two(obj, value);
                        let result = self.call_function(setter_func, args)?;
                        match result {
                            CallResult::Push(ret) => {
                                ret.drop_with_heap(self.heap);
                                return Ok(CallResult::Push(Value::None));
                            }
                            CallResult::FramePushed => {
                                self.pending_discard_return = true;
                                return Ok(CallResult::FramePushed);
                            }
                            other => return Ok(other),
                        }
                    }
                    DescriptorLookup::CustomDescriptorSet(desc_instance) => {
                        // Custom descriptor with __set__:
                        // call descriptor.__set__(instance, value)
                        let set_id: StringId = StaticStrings::DunderDescSet.into();
                        if let Value::Ref(desc_id) = &desc_instance {
                            let desc_id = *desc_id;
                            // Look up __set__ on the descriptor's type
                            if let Some(method) = self.lookup_type_dunder(desc_id, set_id) {
                                // call __set__(descriptor_self, instance, value)
                                self.heap.inc_ref(desc_id);
                                let args = ArgValues::ArgsKargs {
                                    args: vec![Value::Ref(desc_id), obj, value],
                                    kwargs: crate::args::KwargsValues::Empty,
                                };
                                let result = self.call_function(method, args)?;
                                desc_instance.drop_with_heap(self.heap);
                                match result {
                                    CallResult::Push(ret) => {
                                        ret.drop_with_heap(self.heap);
                                        return Ok(CallResult::Push(Value::None));
                                    }
                                    CallResult::FramePushed => {
                                        self.pending_discard_return = true;
                                        return Ok(CallResult::FramePushed);
                                    }
                                    other => return Ok(other),
                                }
                            }
                        }
                        desc_instance.drop_with_heap(self.heap);
                        // Fall through to normal store if __set__ lookup fails
                    }
                    DescriptorLookup::SlotDescriptor(desc_instance) => {
                        let result = self.set_slot_descriptor(desc_instance, obj, value)?;
                        return Ok(result);
                    }
                    DescriptorLookup::ReadOnly => {
                        // Descriptor exists but has no setter - read-only
                        obj.drop_with_heap(self.heap);
                        value.drop_with_heap(self.heap);
                        return Err(ExcType::attribute_error("property", "setter"));
                    }
                    DescriptorLookup::NotDescriptor | DescriptorLookup::CustomDescriptorDelete(_) => {
                        // Not a descriptor or delete-only - fall through to normal store
                    }
                }
            } else if matches!(self.heap.get(heap_id), HeapData::ClassObject(_)) {
                let setattr_id: StringId = StaticStrings::DunderSetattr.into();
                if let Some(method) = self.lookup_metaclass_dunder(heap_id, setattr_id) {
                    let name_val = Value::InternString(name_id);
                    let args = ArgValues::Two(name_val, value);
                    let result = self.call_class_dunder(heap_id, method, args)?;
                    obj.drop_with_heap(self.heap);
                    match result {
                        CallResult::Push(ret) => {
                            ret.drop_with_heap(self.heap);
                            return Ok(CallResult::Push(Value::None));
                        }
                        CallResult::FramePushed => return Ok(CallResult::FramePushed),
                        other => return Ok(other),
                    }
                }
            }
        }

        // Normal attribute store (no descriptor setter)
        let result = obj.py_set_attr(name_id, value, self.heap, self.interns);
        obj.drop_with_heap(self.heap);
        result.map(|()| CallResult::Push(Value::None))
    }

    /// Looks up a descriptor setter in an instance's class hierarchy.
    ///
    /// Checks for both `UserProperty` (built-in @property) and custom descriptors
    /// (user-defined classes with `__set__` method).
    fn find_descriptor_setter(&mut self, instance_id: HeapId, name_id: StringId) -> DescriptorLookup {
        let attr_name = self.interns.get_str(name_id);

        // Get the class from the instance
        let class_id = match self.heap.get(instance_id) {
            HeapData::Instance(inst) => inst.class_id(),
            _ => return DescriptorLookup::NotDescriptor,
        };

        // Look up the attribute in the class MRO
        let prop_value = match self.heap.get(class_id) {
            HeapData::ClassObject(cls) => cls.mro_lookup_attr(attr_name, class_id, self.heap, self.interns),
            _ => return DescriptorLookup::NotDescriptor,
        };

        if let Some((value, _found_in)) = prop_value {
            if let Value::Ref(ref_id) = &value {
                match self.heap.get(*ref_id) {
                    HeapData::UserProperty(up) => {
                        if let Some(func) = up.fset() {
                            let func = func.clone_with_heap(self.heap);
                            value.drop_with_heap(self.heap);
                            return DescriptorLookup::HasFunc(func);
                        }
                        value.drop_with_heap(self.heap);
                        return DescriptorLookup::ReadOnly;
                    }
                    HeapData::SlotDescriptor(_) => {
                        return DescriptorLookup::SlotDescriptor(value);
                    }
                    // Check for custom descriptor: an Instance whose class has __set__
                    HeapData::Instance(desc_inst) => {
                        let desc_class_id = desc_inst.class_id();
                        let set_name: StringId = StaticStrings::DunderDescSet.into();
                        let set_name_str = self.interns.get_str(set_name);
                        let has_set = match self.heap.get(desc_class_id) {
                            HeapData::ClassObject(desc_cls) => {
                                desc_cls.mro_has_attr(set_name_str, desc_class_id, self.heap, self.interns)
                            }
                            _ => false,
                        };
                        if has_set {
                            return DescriptorLookup::CustomDescriptorSet(value);
                        }
                    }
                    _ => {}
                }
            }
            value.drop_with_heap(self.heap);
        }
        DescriptorLookup::NotDescriptor
    }

    /// Looks up a descriptor function (setter or deleter) in an instance's class hierarchy.
    ///
    /// Handles both `UserProperty` and custom descriptors.
    fn find_descriptor_func(
        &mut self,
        instance_id: HeapId,
        name_id: StringId,
        get_func: impl Fn(&crate::types::UserProperty) -> Option<&Value>,
        custom_dunder: StaticStrings,
    ) -> DescriptorLookup {
        let attr_name = self.interns.get_str(name_id);

        // Get the class from the instance
        let class_id = match self.heap.get(instance_id) {
            HeapData::Instance(inst) => inst.class_id(),
            _ => return DescriptorLookup::NotDescriptor,
        };

        // Look up the attribute in the class MRO
        let prop_value = match self.heap.get(class_id) {
            HeapData::ClassObject(cls) => cls.mro_lookup_attr(attr_name, class_id, self.heap, self.interns),
            _ => return DescriptorLookup::NotDescriptor,
        };

        if let Some((value, _found_in)) = prop_value {
            if let Value::Ref(ref_id) = &value {
                match self.heap.get(*ref_id) {
                    HeapData::UserProperty(up) => {
                        if let Some(func) = get_func(up) {
                            let func = func.clone_with_heap(self.heap);
                            value.drop_with_heap(self.heap);
                            return DescriptorLookup::HasFunc(func);
                        }
                        value.drop_with_heap(self.heap);
                        return DescriptorLookup::ReadOnly;
                    }
                    HeapData::SlotDescriptor(_) => {
                        return DescriptorLookup::SlotDescriptor(value);
                    }
                    // Check for custom descriptor instance
                    HeapData::Instance(desc_inst) => {
                        let desc_class_id = desc_inst.class_id();
                        let dunder_id: StringId = custom_dunder.into();
                        let dunder_str = self.interns.get_str(dunder_id);
                        let has_dunder = match self.heap.get(desc_class_id) {
                            HeapData::ClassObject(desc_cls) => {
                                desc_cls.mro_has_attr(dunder_str, desc_class_id, self.heap, self.interns)
                            }
                            _ => false,
                        };
                        if has_dunder {
                            if matches!(custom_dunder, StaticStrings::DunderDescDelete) {
                                return DescriptorLookup::CustomDescriptorDelete(value);
                            }
                            return DescriptorLookup::CustomDescriptorSet(value);
                        }
                    }
                    _ => {}
                }
            }
            value.drop_with_heap(self.heap);
        }
        DescriptorLookup::NotDescriptor
    }

    /// Deletes an attribute from an object.
    ///
    /// For instances, checks if the class has a data descriptor with a deleter
    /// (`UserProperty` or custom descriptor with `__delete__`).
    /// If so, calls the deleter function instead of deleting directly.
    pub(super) fn delete_attr(&mut self, name_id: StringId) -> Result<CallResult, RunError> {
        let obj = self.pop();

        // Check if this is an instance with a descriptor deleter on the class
        if let Value::Ref(heap_id) = &obj {
            let heap_id = *heap_id;
            if matches!(self.heap.get(heap_id), HeapData::Instance(_)) {
                // If __delattr__ is overridden, call it and skip default descriptor handling.
                let delattr_id: StringId = StaticStrings::DunderDelattr.into();
                if let Some(method) = self.lookup_type_dunder(heap_id, delattr_id) {
                    let name_val = Value::InternString(name_id);
                    let result = self.call_dunder(heap_id, method, ArgValues::One(name_val))?;
                    obj.drop_with_heap(self.heap);
                    match result {
                        CallResult::Push(ret) => {
                            ret.drop_with_heap(self.heap);
                            return Ok(CallResult::Push(Value::None));
                        }
                        CallResult::FramePushed => return Ok(CallResult::FramePushed),
                        other => return Ok(other),
                    }
                }

                match self.find_descriptor_func(
                    heap_id,
                    name_id,
                    crate::types::class::UserProperty::fdel,
                    StaticStrings::DunderDescDelete,
                ) {
                    DescriptorLookup::HasFunc(deleter_func) => {
                        // UserProperty deleter found - call deleter(instance)
                        let args = ArgValues::One(obj);
                        let result = self.call_function(deleter_func, args)?;
                        match result {
                            CallResult::Push(ret) => {
                                ret.drop_with_heap(self.heap);
                                return Ok(CallResult::Push(Value::None));
                            }
                            CallResult::FramePushed => {
                                self.pending_discard_return = true;
                                return Ok(CallResult::FramePushed);
                            }
                            other => return Ok(other),
                        }
                    }
                    DescriptorLookup::CustomDescriptorDelete(desc_instance) => {
                        // Custom descriptor with __delete__:
                        // call descriptor.__delete__(instance)
                        let delete_id: StringId = StaticStrings::DunderDescDelete.into();
                        if let Value::Ref(desc_id) = &desc_instance {
                            let desc_id = *desc_id;
                            if let Some(method) = self.lookup_type_dunder(desc_id, delete_id) {
                                self.heap.inc_ref(desc_id);
                                let args = ArgValues::Two(Value::Ref(desc_id), obj);
                                let result = self.call_function(method, args)?;
                                desc_instance.drop_with_heap(self.heap);
                                match result {
                                    CallResult::Push(ret) => {
                                        ret.drop_with_heap(self.heap);
                                        return Ok(CallResult::Push(Value::None));
                                    }
                                    CallResult::FramePushed => {
                                        self.pending_discard_return = true;
                                        return Ok(CallResult::FramePushed);
                                    }
                                    other => return Ok(other),
                                }
                            }
                        }
                        desc_instance.drop_with_heap(self.heap);
                        // Fall through to normal delete
                    }
                    DescriptorLookup::SlotDescriptor(desc_instance) => {
                        let result = self.delete_slot_descriptor(desc_instance, obj)?;
                        return Ok(result);
                    }
                    DescriptorLookup::ReadOnly => {
                        // Descriptor exists but has no deleter
                        obj.drop_with_heap(self.heap);
                        return Err(ExcType::attribute_error("property", "deleter"));
                    }
                    DescriptorLookup::NotDescriptor | DescriptorLookup::CustomDescriptorSet(_) => {
                        // Not a descriptor or set-only - fall through to normal delete
                    }
                }
            } else if matches!(self.heap.get(heap_id), HeapData::ClassObject(_)) {
                let delattr_id: StringId = StaticStrings::DunderDelattr.into();
                if let Some(method) = self.lookup_metaclass_dunder(heap_id, delattr_id) {
                    let name_val = Value::InternString(name_id);
                    let result = self.call_class_dunder(heap_id, method, ArgValues::One(name_val))?;
                    obj.drop_with_heap(self.heap);
                    match result {
                        CallResult::Push(ret) => {
                            ret.drop_with_heap(self.heap);
                            return Ok(CallResult::Push(Value::None));
                        }
                        CallResult::FramePushed => return Ok(CallResult::FramePushed),
                        other => return Ok(other),
                    }
                }
            }
        }

        // Normal attribute delete (no descriptor deleter)
        let result = obj.py_del_attr(name_id, self.heap, self.interns);
        obj.drop_with_heap(self.heap);
        result.map(|()| CallResult::Push(Value::None))
    }
}
