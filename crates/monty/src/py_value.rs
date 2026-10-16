//! Scoped Python values with storage differences hidden from operation dispatch.
//!
//! Arithmetic, method calls, subscription, containment, length, truth and repr use
//! this view. Unmigrated protocols still enter through `Value`; heap payloads are never copied.

use std::{
    borrow::Cow,
    fmt::{self, Write},
};

use num_bigint::BigInt;

use crate::{
    args::ArgValues,
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    fstring::FormatFloat,
    heap::{DropWithContext, Heap, HeapData, HeapId, HeapObjectRead, HeapReadOutput},
    heap_data::{heap_payloads, heap_read_output_py_trait_forward},
    identity::Identity,
    intern::{FunctionId, StaticStrings},
    modules::{ModuleFunctions, collections::defaultdict::defaultdict_missing},
    types::{
        GenericAlias, LazyHeapSet, Property, PyTrait, Type, Union, allocate_string,
        bytes::PyBytes,
        long_int::{PyInt, arithmetic::float_binary},
        py_trait::PyObjectIdentity,
        str::PyStr,
    },
    value::{EitherStr, Marker, Value, float_divmod_tuple, float_pow_value, py_float_divmod, py_float_mod},
};

/// Normalizes the three multi-representation types and keeps every other heap type flat.
macro_rules! define_py_value {
    (@collect [$($variant:ident($payload:ty),)*]) => {
        /// A scoped Python-type view retaining the original object's identity.
        /// Drop it before releasing the owning `Value`, so no heap reader survives cleanup.
        pub(crate) enum PyValue<'h> {
            Str(PyStr<'h>),
            Bytes(PyBytes<'h>),
            Int(PyInt<'h>),
            Undefined,
            Ellipsis,
            NotImplemented,
            None,
            Bool(bool),
            Float(f64),
            Builtin(Builtins),
            ModuleFunction(ModuleFunctions),
            DefFunction(FunctionId),
            Marker(Marker),
            Property(Property),
            $($variant(HeapObjectRead<'h, $payload>),)*
        }

        impl<'h> From<HeapReadOutput<'h>> for PyValue<'h> {
            fn from(value: HeapReadOutput<'h>) -> Self {
                match value {
                    HeapReadOutput::Str(value) => Self::Str(PyStr::Heap(value)),
                    HeapReadOutput::Bytes(value) => Self::Bytes(PyBytes::Heap(value)),
                    HeapReadOutput::LongInt(value) => Self::Int(PyInt::Heap(value)),
                    $(HeapReadOutput::$variant(value) => Self::$variant(value),)*
                }
            }
        }

        impl PyObjectIdentity for PyValue<'_> {
            fn py_identity(&self) -> Identity {
                match self {
                    Self::Str(value) => value.py_identity(),
                    Self::Bytes(value) => value.py_identity(),
                    Self::Int(value) => value.py_identity(),
                    Self::Undefined => Identity::Undefined,
                    Self::Ellipsis => Identity::Ellipsis,
                    Self::NotImplemented => Identity::NotImplemented,
                    Self::None => Identity::None,
                    Self::Bool(value) => Identity::Bool(*value),
                    Self::Float(value) => Identity::Float(value.to_bits()),
                    Self::Builtin(value) => Identity::Builtin(*value),
                    Self::ModuleFunction(value) => Identity::ModuleFunction(*value),
                    Self::DefFunction(id) => Identity::DefFunction(id.index()),
                    Self::Marker(value) => Identity::Marker(*value),
                    Self::Property(value) => Identity::Property(*value),
                    $(Self::$variant(value) => value.py_identity(),)*
                }
            }
        }

        impl PyValue<'_> {
            /// Identifies heap reprs needing cycle tracking; scalar views cannot contain cycles.
            fn repr_heap_id(&self) -> Option<HeapId> {
                match self {
                    Self::Str(_) | Self::Bytes(_) | Self::Int(_)
                    | Self::Undefined | Self::Ellipsis | Self::NotImplemented | Self::None
                    | Self::Bool(_) | Self::Float(_) | Self::Builtin(_) | Self::ModuleFunction(_)
                    | Self::DefFunction(_) | Self::Marker(_) | Self::Property(_) => None,
                    $(Self::$variant(value) => Some(value.id()),)*
                }
            }
        }
    };
    (@collect [$($out:tt)*] $(#[$meta:meta])* Str($storage:ident $payload:ty), $($rest:tt)*) => {
        define_py_value!(@collect [$($out)*] $($rest)*);
    };
    (@collect [$($out:tt)*] $(#[$meta:meta])* Bytes($storage:ident $payload:ty), $($rest:tt)*) => {
        define_py_value!(@collect [$($out)*] $($rest)*);
    };
    (@collect [$($out:tt)*] $(#[$meta:meta])* LongInt($storage:ident $payload:ty), $($rest:tt)*) => {
        define_py_value!(@collect [$($out)*] $($rest)*);
    };
    (@collect [$($out:tt)*] $(#[$meta:meta])* $variant:ident($storage:ident $payload:ty), $($rest:tt)*) => {
        define_py_value!(@collect [$($out)* $variant($payload),] $($rest)*);
    };
    ($($registry:tt)*) => { define_py_value!(@collect [] $($registry)*); };
}
heap_payloads!(define_py_value);

/// Combines concrete dispatch and custom arms in one match, allowing exhaustive protocols.
macro_rules! logical_forward {
    ($self:expr, |$value:ident| $body:expr, else $fallback:expr) => {
        logical_forward!($self, |$value| $body, { _ => $fallback, })
    };
    ($self:expr, |$value:ident| $body:expr, {$($custom:tt)*}) => {
        heap_read_output_py_trait_forward!(
            $self, |$value| $body, {
                Self::Int($value) => $body,
                $($custom)*
            }
        )
    };
}

/// Forwards numeric protocols while keeping bool promotion separate from its identity.
macro_rules! numeric_binary {
    ($method:ident, $reflected:literal, |$lhs:ident, $rhs:ident, $vm:ident| $body:expr) => {
        fn $method(&self, other: &Value, $vm: &mut VM<'h>) -> RunResult<Option<Value>> {
            logical_forward!(self, |value| value.$method(other, $vm), {
                Self::Bool(value) => PyInt::Small(i64::from(*value)).$method(other, $vm),
                Self::Float(value) => float_binary(*value, other, $vm, $reflected, |$lhs, $rhs| $body),
                _ => Ok(None),
            })
        }
    };
    ($method:ident) => {
        fn $method(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
            logical_forward!(self, |value| value.$method(other, vm), {
                Self::Bool(value) => PyInt::Small(i64::from(*value)).$method(other, vm),
                _ => Ok(None),
            })
        }
    };
}

impl<'h> PyValue<'h> {
    /// Includes the mutating `defaultdict.__missing__` path outside `PyTrait`.
    pub(crate) fn subscript(self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        match self {
            Self::Dict(mut dict) if dict.get(vm.heap).is_defaultdict() => match dict.dict_get(key, vm)? {
                Some(value) => Ok(value),
                None => defaultdict_missing(&mut dict, key, vm),
            },
            value => value.py_getitem(key, vm),
        }
    }

    /// Formats the payload after the caller has handled repr recursion.
    fn repr_inner(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        logical_forward!(self, |value| value.py_repr_fmt(f, vm, heap_ids), {
            Self::Undefined => Ok(f.write_str("Undefined")?),
            Self::Ellipsis => Ok(f.write_str("Ellipsis")?),
            Self::NotImplemented => Ok(f.write_str("NotImplemented")?),
            Self::None => Ok(f.write_str("None")?),
            Self::Bool(true) => Ok(f.write_str("True")?),
            Self::Bool(false) => Ok(f.write_str("False")?),
            Self::Float(value) => Ok(write!(f, "{}", FormatFloat(*value))?),
            Self::Builtin(value) => Ok(value.py_repr_fmt(f)?),
            Self::ModuleFunction(function) => {
                let py_id = self.py_identity().into_value(vm.heap);
                defer_drop!(py_id, vm);
                Ok(function.py_repr_fmt(f, PythonIdDisplay::new(py_id, vm.heap))?)
            }
            Self::DefFunction(id) => {
                let py_id = self.py_identity().into_value(vm.heap);
                defer_drop!(py_id, vm);
                Ok(vm.interns
                    .get_function(*id)
                    .py_repr_fmt(f, vm.interns, PythonIdDisplay::new(py_id, vm.heap))?)
            }
            Self::Marker(value) => Ok(value.py_repr_fmt(f)?),
            Self::Property(value) => Ok(write!(f, "<property {value:?}>")?),
            Self::Cell(cell) => Ok(write!(f, "<cell: {} object>", cell.get(vm.heap).0.py_type_name(vm))?),
            Self::Exception(e) => Ok(e.get(vm.heap).py_repr_fmt(f)?),
            Self::Module(m) => Ok(write!(f, "<module '{}'>", m.get(vm.heap).name_str(vm.interns))?),
            Self::Coroutine(coro) => {
                let func = vm.interns.get_function(coro.get(vm.heap).func_id);
                Ok(write!(f, "<coroutine object {}>", vm.interns.get_str(func.name.name_id))?)
            }
            Self::GatherFuture(gather) => Ok(write!(f, "<gather({})>", gather.get(vm.heap).item_count())?),
            Self::ExternalFuture(fut) => Ok(write!(f, "<coroutine external_future({})>", fut.get(vm.heap).call_id.raw())?),
        })
    }
}

impl<'h> PyTrait<'h> for PyValue<'h> {
    fn py_type(&self, vm: &VM<'h>) -> Type {
        logical_forward!(self, |value| value.py_type(vm), {
            Self::Undefined => panic!("Cannot get type of undefined value"),
            Self::Ellipsis => Type::Ellipsis,
            Self::NotImplemented => Type::NotImplementedType,
            Self::None => Type::NoneType,
            Self::Bool(_) => Type::Bool,
            Self::Float(_) => Type::Float,
            Self::Builtin(value) => value.py_type(),
            Self::ModuleFunction(_) => Type::BuiltinFunction,
            Self::DefFunction(_) => Type::Function,
            Self::Marker(value) => value.py_type(),
            Self::Property(_) => Type::Property,
            Self::Cell(_) => Type::Cell,
            Self::Exception(e) => e.py_type(vm),
            Self::Module(_) => Type::Module,
            Self::Coroutine(_) | Self::GatherFuture(_) | Self::ExternalFuture(_) => Type::Coroutine,
        })
    }

    fn py_type_name(&self, vm: &VM<'h>) -> Cow<'h, str> {
        logical_forward!(self, |value| value.py_type_name(vm), else self.py_type(vm).name(vm.heap, vm.interns))
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        logical_forward!(self, |value| value.py_len(vm), else None)
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        logical_forward!(self, |value| value.py_bool(vm), {
            Self::NotImplemented => Err(SimpleException::new_msg(
                ExcType::TypeError, "NotImplemented should not be used in a boolean context",
            ).into()),
            Self::Undefined | Self::None => Ok(false),
            Self::Bool(value) => Ok(*value),
            Self::Float(value) => Ok(*value != 0.0),
            _ => Ok(true),
        })
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        if let Some(id) = self.repr_heap_id() {
            if heap_ids.contains(&id) {
                Ok(f.write_str(match self {
                    Self::List(_) | Self::Deque(_) => "[...]",
                    Self::Tuple(_) => "(...)",
                    Self::Dict(_) => "{...}",
                    _ => "...",
                })?)
            } else if matches!(self, Self::Instance(_)) {
                // Dataclass reprs track cycles; user __repr__ recurses until the VM's guard raises.
                self.repr_inner(f, vm, heap_ids)
            } else {
                heap_ids.insert(id);
                let result = self.repr_inner(f, vm, heap_ids);
                heap_ids.remove(&id);
                result
            }
        } else {
            self.repr_inner(f, vm, heap_ids)
        }
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        logical_forward!(self, |value| value.py_str(vm), {
            Self::Exception(value) => Ok(allocate_string(value.get(vm.heap).py_str(), vm.heap)),
            _ => self.py_repr(vm),
        })
    }

    fn py_contains_impl(&self, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        logical_forward!(self, |value| value.py_contains_impl(item, vm), else Ok(None))
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        logical_forward!(self, |value| value.py_eq_impl(other, vm), {
            Self::Undefined => Value::Undefined.py_eq_impl(other, vm),
            Self::Ellipsis => Value::Ellipsis.py_eq_impl(other, vm),
            Self::NotImplemented => Value::NotImplemented.py_eq_impl(other, vm),
            Self::None => Value::None.py_eq_impl(other, vm),
            Self::Bool(value) => Value::Bool(*value).py_eq_impl(other, vm),
            Self::Float(value) => Value::Float(*value).py_eq_impl(other, vm),
            Self::Builtin(value) => Value::Builtin(*value).py_eq_impl(other, vm),
            Self::ModuleFunction(value) => Value::ModuleFunction(*value).py_eq_impl(other, vm),
            Self::DefFunction(id) => Value::DefFunction(*id).py_eq_impl(other, vm),
            Self::Marker(value) => Value::Marker(*value).py_eq_impl(other, vm),
            Self::Property(value) => Value::Property(*value).py_eq_impl(other, vm),
            _ => Ok(None),
        })
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        logical_forward!(self, |value| value.py_add_impl(other, vm), {
            Self::Bool(value) => PyInt::Small(i64::from(*value)).py_add_impl(other, vm),
            Self::Float(lhs) => Ok(match other {
                Value::Int(rhs) => Some(Value::Float(lhs + *rhs as f64)),
                Value::Bool(rhs) => Some(Value::Float(lhs + f64::from(*rhs))),
                Value::Float(rhs) => Some(Value::Float(lhs + rhs)),
                _ => None,
            }),
            _ => Ok(None),
        })
    }

    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        logical_forward!(self, |value| value.py_mul_impl(other, vm), {
            Self::Bool(value) => PyInt::Small(i64::from(*value)).py_mul_impl(other, vm),
            Self::Float(lhs) => Ok(match other {
                Value::Int(rhs) => Some(Value::Float(lhs * *rhs as f64)),
                Value::Bool(rhs) => Some(Value::Float(lhs * f64::from(*rhs))),
                Value::Float(rhs) => Some(Value::Float(lhs * rhs)),
                _ => None,
            }),
            _ => Ok(None),
        })
    }

    fn py_radd_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        logical_forward!(self, |value| value.py_radd_impl(other, vm), else Ok(None))
    }

    fn py_rmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        logical_forward!(self, |value| value.py_rmul_impl(other, vm), else Ok(None))
    }

    numeric_binary!(py_sub_impl, false, |lhs, rhs, vm| Ok(Value::Float(lhs - rhs)));
    numeric_binary!(py_rsub_impl, true, |lhs, rhs, vm| Ok(Value::Float(lhs - rhs)));
    numeric_binary!(py_truediv_impl, false, |lhs, rhs, vm| {
        if rhs == 0.0 {
            Err(ExcType::zero_division().into())
        } else {
            Ok(Value::Float(lhs / rhs))
        }
    });
    numeric_binary!(py_rtruediv_impl, true, |lhs, rhs, vm| {
        if rhs == 0.0 {
            Err(ExcType::zero_division().into())
        } else {
            Ok(Value::Float(lhs / rhs))
        }
    });
    numeric_binary!(py_floordiv_impl, false, |lhs, rhs, vm| {
        if rhs == 0.0 {
            Err(ExcType::zero_division().into())
        } else {
            Ok(Value::Float(py_float_divmod(lhs, rhs).0))
        }
    });
    numeric_binary!(py_rfloordiv_impl, true, |lhs, rhs, vm| {
        if rhs == 0.0 {
            Err(ExcType::zero_division().into())
        } else {
            Ok(Value::Float(py_float_divmod(lhs, rhs).0))
        }
    });
    numeric_binary!(py_mod_impl, false, |lhs, rhs, vm| {
        if rhs == 0.0 {
            Err(ExcType::zero_division().into())
        } else {
            Ok(Value::Float(py_float_mod(lhs, rhs)))
        }
    });
    numeric_binary!(py_rmod_impl, true, |lhs, rhs, vm| {
        if rhs == 0.0 {
            Err(ExcType::zero_division().into())
        } else {
            Ok(Value::Float(py_float_mod(lhs, rhs)))
        }
    });
    numeric_binary!(py_divmod_impl, false, |lhs, rhs, vm| float_divmod_tuple(
        lhs, rhs, vm.heap
    ));
    numeric_binary!(py_rdivmod_impl, true, |lhs, rhs, vm| float_divmod_tuple(
        lhs, rhs, vm.heap
    ));
    numeric_binary!(py_rand_impl);
    numeric_binary!(py_rxor_impl);
    numeric_binary!(py_lshift_impl);
    numeric_binary!(py_rlshift_impl);
    numeric_binary!(py_rshift_impl);
    numeric_binary!(py_rrshift_impl);

    fn py_index_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        logical_forward!(self, |value| value.py_index_impl(vm), {
            Self::Bool(value) => Ok(Some(Value::Bool(*value))),
            _ => Ok(None),
        })
    }

    fn py_neg_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        logical_forward!(self, |value| value.py_neg_impl(vm), {
            Self::Bool(value) => PyInt::Small(i64::from(*value)).py_neg_impl(vm),
            Self::Float(value) => Ok(Some(Value::Float(-value))),
            _ => Ok(None),
        })
    }

    fn py_pos_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        logical_forward!(self, |value| value.py_pos_impl(vm), {
            Self::Bool(value) => Ok(Some(Value::Int(i64::from(*value)))),
            Self::Float(value) => Ok(Some(Value::Float(*value))),
            _ => Ok(None),
        })
    }

    fn py_pow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        logical_forward!(self, |value| value.py_pow_impl(other, modulus, vm), {
            Self::Bool(value) => PyInt::Small(i64::from(*value)).py_pow_impl(other, modulus, vm),
            Self::Float(value) if modulus.is_none() => float_binary(*value, other, vm, false, |base, exp| float_pow_value(base, exp, vm.heap)),
            _ => Ok(None),
        })
    }

    fn py_rpow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        logical_forward!(self, |value| value.py_rpow_impl(other, modulus, vm), else Ok(None))
    }

    fn py_and_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        logical_forward!(self, |value| value.py_and_impl(other, vm), {
            Self::Bool(lhs) if let Value::Bool(rhs) = other => Ok(Some(Value::Bool(*lhs && *rhs))),
            Self::Bool(value) => PyInt::Small(i64::from(*value)).py_and_impl(other, vm),
            _ => Ok(None),
        })
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        logical_forward!(self, |value| value.py_or_impl(other, vm), {
            Self::Bool(lhs) if let Value::Bool(rhs) = other => Ok(Some(Value::Bool(*lhs || *rhs))),
            Self::Bool(value) => PyInt::Small(i64::from(*value)).py_or_impl(other, vm),
            Self::Builtin(value) => Union::try_or(&Value::Builtin(*value), other, vm),
            Self::None => Union::try_or(&Value::None, other, vm),
            Self::Marker(value) => Union::try_or(&Value::Marker(*value), other, vm),
            _ => Ok(None),
        })
    }

    fn py_ror_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        logical_forward!(self, |value| value.py_ror_impl(other, vm), {
            Self::Builtin(value) => Union::try_or(other, &Value::Builtin(*value), vm),
            Self::None => Union::try_or(other, &Value::None, vm),
            Self::Marker(value) => Union::try_or(other, &Value::Marker(*value), vm),
            _ => Ok(None),
        })
    }

    fn py_xor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        logical_forward!(self, |value| value.py_xor_impl(other, vm), {
            Self::Bool(lhs) if let Value::Bool(rhs) = other => Ok(Some(Value::Bool(*lhs ^ *rhs))),
            Self::Bool(value) => PyInt::Small(i64::from(*value)).py_xor_impl(other, vm),
            _ => Ok(None),
        })
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        logical_forward!(self, |value| value.py_call_attr(vm, attr, args), {
            Self::Module(value) => value.py_call_attr(vm, attr, args),
            Self::Builtin(Builtins::Type(ty)) if let EitherStr::Interned(id) = attr => ty.call_class_method(*id, args, vm),
            _ => {
                args.drop_with(vm);
                Err(ExcType::attribute_error(self.py_type_name(vm), attr.as_str(vm.interns)))
            }
        })
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        logical_forward!(self, |value| value.py_getitem(key, vm), {
            Self::Builtin(Builtins::Type(t)) if t.has_class_getitem() => Ok(GenericAlias::subscript(*t, key.clone_with_heap(vm), vm)),
            Self::Builtin(Builtins::Function(BuiltinsFunctions::Type)) => Ok(GenericAlias::subscript(Type::Type, key.clone_with_heap(vm), vm)),
            Self::Builtin(Builtins::Type(Type::Union)) => Union::subscript(key.clone_with_heap(vm), vm),
            Self::Marker(Marker(StaticStrings::Optional)) => Union::optional(key.clone_with_heap(vm), vm),
            Self::Builtin(Builtins::Type(t)) => Err(ExcType::type_error_type_not_subscriptable(&t.name(vm.heap, vm.interns))),
            Self::Builtin(Builtins::ExcType(exc_type)) => Err(ExcType::type_error_type_not_subscriptable((*exc_type).into())),
            _ => Err(ExcType::type_error_not_sub(&self.py_type_name(vm))),
        })
    }
}

/// Borrowed integer payload used to format a Python `id()` in callable reprs.
enum PythonIdDisplay<'a> {
    Int(i64),
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
