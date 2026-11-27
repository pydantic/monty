use std::fmt::Write;

use serde::{Deserialize, Serialize};

use crate::{
    args::{ArgValues, FromArgs},
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapRead},
    intern::StaticStrings,
    modules::copy::{Memo, PyDeepCopy, deep_copy},
    py_attrs::{self, AttrDef, ClassMethodCall, MethodCall},
    types::{BoundMethod, LazyHeapSet, PyTrait, Type, TypeRef, descriptor, instance::is_method_value},
    value::{EitherStr, Value},
};

/// A proxy that starts lookup after `start` in the receiver type's MRO.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Super {
    start: Value,
    /// `None` for an unbound proxy.
    receiver: Value,
    /// Captured at construction: assigning the receiver's `__class__` does not change this MRO.
    receiver_type: Value,
}

/// Positional arguments to `super()`, `super(type)` and `super(type, obj)`.
#[derive(FromArgs)]
#[from_args(name = "super()", style = unpack, kwarg_error_name = "super")]
struct SuperArgs {
    #[from_args(pos_only, default)]
    start: Option<Value>,
    #[from_args(pos_only, default)]
    receiver: Option<Value>,
}

/// Constructs a bound or unbound `super` proxy.
pub(crate) fn super_new(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let SuperArgs { start, receiver } = SuperArgs::from_args(args, vm)?;
    defer_drop!(start, vm);
    defer_drop!(receiver, vm);
    let unbound = Value::None;
    let (start, receiver) = if let Some(start) = start {
        let start_type = TypeRef::from_value(start, vm.heap).ok_or_else(|| {
            let got = start.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
            ExcType::type_error_bad_arg_pos("super", 1, "a type", got)
        })?;
        (start_type, receiver.as_ref().unwrap_or(&unbound))
    } else {
        implicit_super_args(vm)?
    };
    allocate_super(start, receiver, vm)
}

/// Binds the proxy to the receiver's current type, or leaves it unbound for `None`.
fn allocate_super(start: TypeRef, receiver: &Value, vm: &VM<'_>) -> RunResult<Value> {
    let receiver_type = if matches!(receiver, Value::None) {
        Value::None
    } else {
        supercheck(start, receiver, vm)?.to_value(vm.heap)
    };
    Ok(Value::allocate(
        Super {
            start: start.to_value(vm.heap),
            receiver: receiver.clone_with_heap(vm.heap),
            receiver_type,
        },
        vm.heap,
    ))
}

/// Resolves the implicit arguments of `super()` from its caller's frame.
fn implicit_super_args<'a>(vm: &'a VM<'_>) -> RunResult<(TypeRef, &'a Value)> {
    let receiver = vm
        .frame_first_positional()
        .ok_or_else(|| ExcType::runtime_error_super("no arguments"))?;
    if matches!(receiver, Value::Undefined) {
        return Err(ExcType::runtime_error_super("arg[0] deleted"));
    }
    let class = vm
        .frame_free_var("__class__")
        .ok_or_else(|| ExcType::runtime_error_super("__class__ cell not found"))?;
    if matches!(class, Value::Undefined) {
        Err(ExcType::runtime_error_super("empty __class__ cell"))
    } else if let Some(class) = TypeRef::from_value(class, vm.heap) {
        Ok((class, receiver))
    } else {
        Err(ExcType::runtime_error_super(format_args!(
            "__class__ is not a type ({})",
            class.py_type_name(vm)
        )))
    }
}

/// Resolves the type whose MRO to search: a subclass receiver itself, or an instance's type.
fn supercheck(start: TypeRef, receiver: &Value, vm: &VM<'_>) -> RunResult<TypeRef> {
    let class_receiver = TypeRef::from_value(receiver, vm.heap);
    if let Some(ty) = class_receiver
        && ty.is_subclass_of(start, vm.heap)
    {
        return Ok(ty);
    }
    let actual = receiver.py_class(vm);
    if actual.is_subclass_of(start, vm.heap) {
        Ok(actual)
    } else {
        let start_name = start.name(vm.heap, vm.interns);
        let (kind, ty) = class_receiver.map_or(("instance of", actual), |ty| ("type", ty));
        let name = ty.name(vm.heap, vm.interns);
        Err(ExcType::type_error(format!(
            "super(type, obj): obj ({kind} {name}) is not an instance or subtype of type ({start_name})."
        )))
    }
}

impl Super {
    /// The owned references are traced even when two fields hold the same object.
    pub(crate) fn child_ids(&self) -> impl Iterator<Item = HeapId> + '_ {
        [&self.start, &self.receiver, &self.receiver_type]
            .into_iter()
            .filter_map(Value::ref_id)
    }
}

/// A Python value or a native descriptor available for direct calls.
enum Member {
    Value(Value),
    Descriptor(Value, Option<HeapId>, HeapId),
    Method(Type, MethodCall, TypeRef),
    ClassMethod(ClassMethodCall, TypeRef),
}

impl<'h> HeapRead<'h, Super> {
    /// Looks up a member directly on each class after `start`, bypassing instance attributes.
    fn lookup_member(&self, attr: &EitherStr, vm: &mut VM<'h>) -> Option<Member> {
        let name = attr.as_str(vm.interns);
        // CPython always reads the proxy's own `__class__`, not the receiver's.
        if name == "__class__" {
            return Some(Member::Value(TypeRef::Native(Type::Super).to_value(vm.heap)));
        }
        let proxy = self.get(vm.heap);
        if let Some(receiver_type) = TypeRef::from_value(&proxy.receiver_type, vm.heap) {
            let start = TypeRef::from_value(&proxy.start, vm.heap).expect("super start is a type");
            // Only native MROs can have multiple native branches. For class-like
            // objects the native tail is `object` or `tuple, object`.
            let member = if let Some(native) = receiver_type.native() {
                native
                    .mro()
                    .skip_while(|&ty| TypeRef::Native(ty) != start)
                    .skip(1)
                    .find_map(|ty| native_member(ty, receiver_type, attr, vm))
            } else {
                lookup_class_member(receiver_type, start, attr, vm)
            };
            if let Some(member) = member {
                return Some(match member {
                    Member::Value(value) => self.bind_value(value, name, receiver_type, vm),
                    method => method,
                });
            }
        }
        let proxy = self.get(vm.heap);
        let member = match name {
            "__thisclass__" => &proxy.start,
            "__self__" => &proxy.receiver,
            "__self_class__" => &proxy.receiver_type,
            _ => return None,
        };
        Some(Member::Value(member.clone_with_heap(vm.heap)))
    }

    /// Binds Python methods or defers a descriptor to the read or call path.
    fn bind_value(&self, member: Value, name: &str, receiver_type: TypeRef, vm: &VM<'h>) -> Member {
        let proxy = self.get(vm.heap);
        let class_bound = TypeRef::from_value(&proxy.receiver, vm.heap) == Some(receiver_type);
        if let TypeRef::Class(owner) = receiver_type
            && descriptor::has_get(&member, vm)
        {
            let instance = if class_bound { None } else { proxy.receiver.ref_id() };
            return Member::Descriptor(member, instance, owner);
        }
        let object_method = matches!(
            member,
            Value::Builtin(Builtins::Function(
                BuiltinsFunctions::ObjectInit
                    | BuiltinsFunctions::ObjectSetattr
                    | BuiltinsFunctions::ObjectInitSubclass
            ))
        );
        let bind = object_method || is_method_value(&member, vm);
        let receiver = match name {
            "__new__" => None,
            "__init_subclass__" if bind => Some(&proxy.receiver_type),
            _ if bind && !class_bound => Some(&proxy.receiver),
            _ => None,
        };
        Member::Value(if let Some(receiver) = receiver {
            BoundMethod::allocate(receiver.clone_with_heap(vm.heap), member, vm.heap)
        } else {
            member
        })
    }

    /// Reads a selected value, running descriptor getters when required.
    fn getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        match self.lookup_member(attr, vm) {
            Some(Member::Value(value)) => Ok(Some(CallResult::Value(value))),
            Some(Member::Descriptor(member, instance, owner)) => {
                defer_drop!(member, vm);
                descriptor::get(member, instance, owner, vm).map(Some)
            }
            _ => Ok(None),
        }
    }
}

impl<'h> PyDeepCopy<'h> for HeapRead<'h, Super> {
    #[inline(never)]
    fn py_deep_copy(&self, _source: &Value, memo: &mut Memo, vm: &mut VM<'h>) -> RunResult<Value> {
        let attr = EitherStr::Interned(vm.interns.intern_static(StaticStrings::DunderDeepcopy));
        let hook = match self
            .getattr(&attr, vm)
            .and_then(|result| result.map(|result| result.require_synchronous(vm)).transpose())
        {
            Ok(hook) => hook,
            Err(error) if error.is_catchable(ExcType::AttributeError) => None,
            Err(error) => return Err(error),
        };
        if let Some(hook) = hook
            && !matches!(hook, Value::None)
        {
            defer_drop!(hook, vm);
            vm.evaluate_function(hook, ArgValues::One(memo.dict_value(vm)))
        } else {
            let proxy = self.get(vm.heap);
            let start = TypeRef::from_value(&proxy.start, vm.heap).expect("super start is a type");
            let receiver = proxy.receiver.clone_with_heap(vm.heap);
            defer_drop!(receiver, vm);
            let copied = deep_copy(receiver, memo, vm)?;
            defer_drop!(copied, vm);
            allocate_super(start, copied, vm)
        }
    }
}

impl HeapItem for Super {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.start.py_dec_ref_ids(stack);
        self.receiver.py_dec_ref_ids(stack);
        self.receiver_type.py_dec_ref_ids(stack);
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
        let start = TypeRef::from_value(&proxy.start, vm.heap).expect("super start is a type");
        let name = start.name(vm.heap, vm.interns);
        if let Some(receiver_type) = TypeRef::from_value(&proxy.receiver_type, vm.heap) {
            let receiver_name = receiver_type.name(vm.heap, vm.interns);
            write!(f, "<super: <class '{name}'>, <{receiver_name} object>>")?;
        } else {
            write!(f, "<super: <class '{name}'>, NULL>")?;
        }
        Ok(())
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        self.getattr(attr, vm)
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        match self.lookup_member(attr, vm) {
            Some(Member::Value(value)) => {
                defer_drop!(value, vm);
                vm.call_function(value, args)
            }
            Some(Member::Descriptor(member, instance, owner)) => {
                defer_drop!(member, vm);
                descriptor::bind_and_call(member, instance, owner, args, vm)
            }
            Some(Member::Method(owner, call, receiver_type)) => {
                let receiver = &self.get(vm.heap).receiver;
                if TypeRef::from_value(receiver, vm.heap) == Some(receiver_type) {
                    py_attrs::call_unbound(owner, attr, call, args, vm)
                } else {
                    py_attrs::call_method(owner, &mut receiver.py_value(vm), attr, call, args, vm)
                }
            }
            Some(Member::ClassMethod(call, receiver_type)) => call(receiver_type, args, vm),
            None => {
                args.drop_with(vm);
                Err(ExcType::attribute_error("super", attr.as_str(vm.interns)))
            }
        }
    }
}

/// Searches the user classes, then the native tail, without invoking any host lookup.
fn lookup_class_member(receiver: TypeRef, start: TypeRef, attr: &EitherStr, vm: &mut VM<'_>) -> Option<Member> {
    let name = attr.as_str(vm.interns);
    for ty in receiver.mro(vm.heap).skip_while(|&ty| ty != start).skip(1) {
        match ty {
            TypeRef::Class(id) => {
                if let HeapData::Class(class) = vm.heap.get(id)
                    && let Some(member) = class.namespace().get_by_str(name, vm.heap, vm.interns)
                {
                    return Some(Member::Value(member.clone_with_heap(vm.heap)));
                }
            }
            TypeRef::Native(ty) => return ty.mro().find_map(|ty| native_member(ty, receiver, attr, vm)),
        }
    }
    None
}

/// Looks in a native type's own namespace, not its ancestors'.
fn native_member(ty: Type, receiver_type: TypeRef, attr: &EitherStr, vm: &mut VM<'_>) -> Option<Member> {
    let name = attr.static_string(vm.interns)?;
    Some(match (ty.def().attrs?.lookup)(name)? {
        AttrDef::Value(get) => Member::Value(get(vm)),
        AttrDef::Method(call) => Member::Method(ty, call, receiver_type),
        AttrDef::ClassMethod(call) => Member::ClassMethod(call, receiver_type),
    })
}
