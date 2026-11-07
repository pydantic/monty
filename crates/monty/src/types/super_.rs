//! `super`: the proxy that looks attributes up past a class in another class's MRO.

use std::fmt::Write;

use super::{LazyHeapSet, OwnedTypeRef, PyTrait, Type, TypeRef, instance::BoundMethod};
use crate::{
    args::{ArgValues, FromArgs},
    builtins::Builtins,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    hash::{HashValue, identity_hash},
    heap::{DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapRead},
    modules::copy::{Memo, PyDeepCopy, deep_copy},
    types::instance::is_method_value,
    value::{EitherStr, Value},
};

/// A `super` object: `super(this_class, obj)`.
///
/// Attribute lookups walk the MRO of `obj`'s class (or of `obj` itself when it
/// is a class) starting *after* `this_class`, binding methods to `obj`. With no
/// `obj` it is unbound and supports only its own attributes.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Super {
    /// The class to search past, owning its class reference.
    this_class: OwnedTypeRef,
    /// The instance or subclass methods bind to; `None` for an unbound `super`.
    obj: Option<Value>,
}

/// Arguments of `super(type=..., object_or_type=..., /)`.
#[derive(FromArgs)]
#[from_args(name = "super", style = unpack)]
struct SuperArgs {
    #[from_args(pos_only, default)]
    this_class: Option<Value>,
    #[from_args(pos_only, default)]
    obj: Option<Value>,
}

/// `super()`, `super(type)` and `super(type, obj)`.
///
/// With no arguments the class is the method's `__class__` cell and the
/// object its first argument, as in CPython.
pub(crate) fn builtin_super(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    // `super_init_impl` checks keywords, then arity, with its own wording.
    let args = args.reject_kwargs("super", vm.heap)?;
    let given = args.count();
    if given > 2 {
        args.drop_with(vm);
        return Err(ExcType::type_error(format!(
            "super() expected at most 2 arguments, got {given}"
        )));
    }
    let SuperArgs { this_class, obj } = SuperArgs::from_args(args, vm)?;
    let implicit = this_class.is_none();
    let (this_class, obj) = if let Some(this_class) = this_class {
        (this_class, obj)
    } else {
        let (class, first) = vm.zero_arg_super()?;
        (class, Some(first))
    };
    defer_drop!(this_class, vm);
    let Some(this_type) = TypeRef::from_value(this_class, vm.heap) else {
        obj.drop_with(vm);
        return Err(if implicit {
            let got = this_class.py_type_name(vm);
            ExcType::runtime_error(format!("super(): __class__ is not a type ({got})"))
        } else {
            let got = this_class.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
            ExcType::type_error(format!("super() argument 1 must be a type, not {got}"))
        });
    };
    let obj = match obj {
        None | Some(Value::None) => None,
        Some(obj) => {
            if let Err(err) = check_super_obj(this_type, &obj, vm) {
                obj.drop_with(vm);
                return Err(err);
            }
            Some(obj)
        }
    };
    let this_class = match this_type {
        TypeRef::Native(ty) => OwnedTypeRef::Native(ty),
        TypeRef::Class(id) => {
            vm.heap.inc_ref(id);
            OwnedTypeRef::Class(id)
        }
    };
    let proxy = Super { this_class, obj };
    Ok(Value::Ref(vm.heap.allocate(HeapData::Super(proxy))))
}

/// Checks that `obj` is an instance or a subclass of `this_class`.
fn check_super_obj(this_class: TypeRef, obj: &Value, vm: &VM<'_>) -> RunResult<()> {
    if obj.py_class(vm).is_subclass_of(this_class, vm.heap) {
        return Ok(());
    }
    let this_name = this_class.name(vm.heap, vm.interns);
    let message = match TypeRef::from_value(obj, vm.heap) {
        Some(obj_type) if obj_type.is_subclass_of(this_class, vm.heap) => return Ok(()),
        Some(obj_type) => format!(
            "super(type, obj): obj (type {}) is not an instance or subtype of type ({this_name}).",
            obj_type.name(vm.heap, vm.interns)
        ),
        None => format!(
            "super(type, obj): obj (instance of {}) is not an instance or subtype of type ({this_name}).",
            obj.py_type_name(vm)
        ),
    };
    Err(ExcType::type_error(message))
}

impl<'h> HeapObjectRead<'h, Super> {
    /// The class whose MRO lookups walk, and whether `obj` is that class itself
    /// (`super()` in `__init_subclass__` or `__new__`) rather than an instance.
    fn self_class(&self, obj: &Value, vm: &VM<'h>) -> (TypeRef, bool) {
        let this_class = self.get(vm.heap).this_class.borrow();
        match TypeRef::from_value(obj, vm.heap) {
            Some(class) if class.is_subclass_of(this_class, vm.heap) => (class, true),
            _ => (obj.py_class(vm), false),
        }
    }

    /// Looks `attr` up past `this_class`, binding what it finds as CPython's
    /// descriptors would: `__new__` stays a plain function, `__init_subclass__`
    /// binds the class, and other methods bind the instance (not a class).
    fn lookup(&self, attr: &EitherStr, vm: &mut VM<'h>) -> Option<Value> {
        let obj = self.get(vm.heap).obj.as_ref()?.clone_with_heap(vm.heap);
        defer_drop!(obj, vm);
        let this_class = self.get(vm.heap).this_class.borrow();
        let (self_class, class_level) = self.self_class(obj, vm);
        let attr_str = attr.as_str(vm.interns);
        let after: Vec<TypeRef> = self_class
            .mro(vm.heap)
            .skip_while(|ty| *ty != this_class)
            .skip(1)
            .collect();
        let member = after.into_iter().find_map(|ancestor| match ancestor {
            TypeRef::Class(id) => match vm.heap.get(id) {
                HeapData::Class(class) => class
                    .namespace()
                    .get_by_str(attr_str, vm.heap, vm.interns)
                    .map(|member| member.clone_with_heap(vm.heap)),
                _ => None,
            },
            TypeRef::Native(ty) => ty.class_constant(attr, vm),
        })?;
        let callable = is_method_value(&member, vm) || matches!(member, Value::Builtin(Builtins::Function(_)));
        Some(if !callable || attr_str == "__new__" {
            member
        } else if attr_str == "__init_subclass__" {
            BoundMethod::allocate(self_class.to_value(vm.heap), member, vm.heap)
        } else if class_level {
            member
        } else {
            BoundMethod::allocate(obj.clone_with_heap(vm.heap), member, vm.heap)
        })
    }

    /// `super`'s own attributes, which shadow anything in the MRO.
    fn own_attr(&self, attr: &str, vm: &VM<'h>) -> Option<Value> {
        let proxy = self.get(vm.heap);
        match attr {
            "__thisclass__" => Some(proxy.this_class.borrow().to_value(vm.heap)),
            "__self__" => Some(
                proxy
                    .obj
                    .as_ref()
                    .map_or(Value::None, |obj| obj.clone_with_heap(vm.heap)),
            ),
            "__self_class__" => Some(match &proxy.obj {
                Some(obj) => self.self_class(obj, vm).0.to_value(vm.heap),
                None => Value::None,
            }),
            _ => None,
        }
    }

    /// `'super' object has no attribute 'name'`.
    fn attribute_error(attr: &str) -> RunError {
        ExcType::attribute_error("super", attr)
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Super> {
    fn py_type(&self, _vm: &VM<'h>) -> TypeRef {
        Type::Super.into()
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_hash(&self, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let proxy = self.get(vm.heap);
        let this_name = proxy.this_class.borrow().name(vm.heap, vm.interns);
        match &proxy.obj {
            Some(obj) => {
                let obj_name = self.self_class(obj, vm).0.name(vm.heap, vm.interns);
                write!(f, "<super: <class '{this_name}'>, <{obj_name} object>>")?;
            }
            None => write!(f, "<super: <class '{this_name}'>, NULL>")?,
        }
        Ok(())
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr_str = attr.as_str(vm.interns);
        match self.own_attr(attr_str, vm).or_else(|| self.lookup(attr, vm)) {
            Some(value) => Ok(Some(CallResult::Value(value))),
            None => Err(Self::attribute_error(attr_str)),
        }
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let attr_str = attr.as_str(vm.interns);
        let Some(member) = self.own_attr(attr_str, vm).or_else(|| self.lookup(attr, vm)) else {
            args.drop_with(vm);
            return Err(Self::attribute_error(attr_str));
        };
        defer_drop!(member, vm);
        vm.call_function(member, args)
    }
}

impl HeapItem for Super {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.extend(self.this_class.class_id());
        if let Some(obj) = &mut self.obj {
            obj.py_dec_ref_ids(stack);
        }
    }
}

impl Super {
    /// Heap references this proxy owns, for the cycle collector.
    pub(crate) fn child_ids(&self) -> impl Iterator<Item = HeapId> {
        self.this_class
            .class_id()
            .into_iter()
            .chain(self.obj.as_ref().and_then(Value::ref_id))
    }
}

impl<'h> PyDeepCopy<'h> for HeapRead<'h, Super> {
    /// A new proxy over the same class and a deep copy of the receiver, as
    /// CPython's reduce-based copy builds.
    fn py_deep_copy(&self, _source: &Value, memo: &mut Memo, vm: &mut VM<'h>) -> RunResult<Value> {
        let proxy = self.get(vm.heap);
        let this_class = proxy.this_class;
        let obj = proxy.obj.as_ref().map(|obj| obj.clone_with_heap(vm.heap));
        let obj = match obj {
            Some(obj) => {
                let copied = deep_copy(&obj, memo, vm);
                obj.drop_with(vm);
                Some(copied?)
            }
            None => None,
        };
        if let Some(id) = this_class.class_id() {
            vm.heap.inc_ref(id);
        }
        let copy = Super { this_class, obj };
        Ok(Value::Ref(vm.heap.allocate(HeapData::Super(copy))))
    }
}
