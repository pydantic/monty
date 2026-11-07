//! Scoped Python values with storage differences hidden from operation dispatch.
//!
//! Protocol dispatch converts heap reads to this view without copying their payloads.

use std::{
    borrow::Cow,
    fmt::{self, Write},
};

use num_bigint::BigInt;

use crate::{
    annotations,
    args::ArgValues,
    builtins::Builtins,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    expressions::CmpOperator,
    fstring::FormatFloat,
    hash::{HashValue, identity_hash},
    heap::{DropWithContext, Heap, HeapData, HeapId, HeapObjectRead, HeapReadOutput, HeapReader},
    heap_data::heap_payloads,
    identity::Identity,
    intern::{FunctionId, StaticStrings},
    modules::{ModuleFunctions, collections::defaultdict::defaultdict_missing},
    types::{
        GenericAlias, LazyHeapSet, LongInt, Property, PyTrait, Type, TypeRef, Union, allocate_string,
        bytes::PyBytes,
        long_int::{PyInt, arithmetic::float_binary},
        numeric::{round_digits, round_float},
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
macro_rules! forward_py_value {
    ($self:expr, |$value:ident| $body:expr, else $fallback:expr) => {
        forward_py_value!($self, |$value| $body, { _ => $fallback, })
    };
    ($self:expr, |$value:ident| $body:expr, {$($custom:tt)*}) => {
        match $self {
            Self::Str($value) => $body,
            Self::Bytes($value) => $body,
            Self::List($value) => $body,
            Self::Deque($value) => $body,
            Self::ListIterator($value) => $body,
            Self::DequeIterator($value) => $body,
            Self::TupleIterator($value) => $body,
            Self::StringIterator($value) => $body,
            Self::BytesIterator($value) => $body,
            Self::RangeIterator($value) => $body,
            Self::DictKeyIterator($value) => $body,
            Self::DictItemIterator($value) => $body,
            Self::DictValueIterator($value) => $body,
            Self::SetIterator($value) => $body,
            Self::CallableIterator($value) => $body,
            Self::Itertools($value) => $body,
            Self::DirScan($value) => $body,
            Self::Csv($value) => $body,
            Self::Partial($value) => $body,
            Self::Random($value) => $body,
            Self::HashObject($value) => $body,
            Self::GenericAlias($value) => $body,
            Self::Union($value) => $body,
            Self::ForwardRef($value) => $body,
            Self::LruCache($value) => $body,
            Self::Super($value) => $body,
            Self::Tuple($value) => $body,
            Self::NamedTuple($value) => $body,
            Self::NamedTupleClass($value) => $body,
            Self::Dict($value) => $body,
            Self::DictKeysView($value) => $body,
            Self::DictItemsView($value) => $body,
            Self::DictValuesView($value) => $body,
            Self::Set($value) => $body,
            Self::FrozenSet($value) => $body,
            Self::Range($value) => $body,
            Self::Slice($value) => $body,
            Self::HostClass($value) => $body,
            Self::HostClassType($value) => $body,
            Self::Class($value) => $body,
            Self::Instance($value) => $body,
            Self::BoundMethod($value) => $body,
            Self::DataclassField($value) => $body,
            Self::DataclassMethod($value) => $body,
            Self::DataclassParams($value) => $body,
            Self::Complex($value) => $body,
            Self::Path($value) => $body,
            Self::OpenFile($value) => $body,
            Self::RePattern($value) => $body,
            Self::ReMatch($value) => $body,
            Self::Date($value) => $body,
            Self::DateTime($value) => $body,
            Self::Time($value) => $body,
            Self::TimeDelta($value) => $body,
            Self::TimeZone($value) => $body,
            Self::Closure($value) => $body,
            Self::FunctionDefaults($value) => $body,
            Self::ExtFunction($value) => $body,
            Self::Decimal($value) => $body,
            Self::DecimalContext($value) => $body,
            Self::DecimalSignalDict($value) => $body,
            Self::DecimalContextManager($value) => $body,
            Self::Int($value) => $body,
            $($custom)*
        }
    };
}

/// Forwards numeric protocols while keeping bool promotion separate from its identity.
macro_rules! numeric_binary {
    ($method:ident, $reflected:literal, |$lhs:ident, $rhs:ident, $vm:ident| $body:expr) => {
        fn $method(&self, other: &Value, $vm: &mut VM<'h>) -> RunResult<Option<Value>> {
            forward_py_value!(self, |value| value.$method(other, $vm), {
                Self::Bool(value) => PyInt::Small(i64::from(*value)).$method(other, $vm),
                Self::Float(value) => float_binary(*value, other, $vm, $reflected, |$lhs, $rhs| $body),
                _ => Ok(None),
            })
        }
    };
    ($method:ident) => {
        fn $method(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
            forward_py_value!(self, |value| value.$method(other, vm), {
                Self::Bool(value) => PyInt::Small(i64::from(*value)).$method(other, vm),
                _ => Ok(None),
            })
        }
    };
}

impl<'h> PyValue<'h> {
    /// Reads a heap object once and opens its normalized Python-type view.
    #[inline]
    pub(crate) fn from_heap(id: HeapId, heap: &HeapReader<'h>) -> Self {
        heap.read(id).into()
    }

    /// Converts an integer or an object implementing `__index__` into a repetition count.
    /// Returns `None` for unsupported operands; out-of-range counts raise `OverflowError`.
    /// Negative counts within range become zero.
    pub(crate) fn repeat_count(self, vm: &mut VM<'h>) -> RunResult<Option<usize>> {
        let count = self.py_index_impl(vm)?;
        // An integer may return itself; release its reader before cleaning up the result.
        drop(self);
        let Some(count) = count else {
            return Ok(None);
        };
        defer_drop!(count, vm);

        // Check overflow before clamping: even an oversized negative count must raise.
        let count = count.as_int_with_overflow(vm, || ExcType::overflow_index_sized_int().into())?;
        // In-range negatives mean no repetitions; check that the result fits the host's usize.
        Ok(Some(
            usize::try_from(count.max(0)).map_err(|_| ExcType::overflow_index_sized_int())?,
        ))
    }

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
        forward_py_value!(self, |value| value.py_repr_fmt(f, vm, heap_ids), {
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
            Self::Module(m) => Ok(f.write_str(&m.get(vm.heap).repr(vm.interns))?),
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
    fn py_matmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_matmul_impl(other, vm), else Ok(None))
    }

    fn py_rmatmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_rmatmul_impl(other, vm), else Ok(None))
    }

    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        forward_py_value!(
            self,
            |value| value.py_call(args, vm),
            else {
                args.drop_with(vm);
                Err(ExcType::type_error_not_callable_object(&self.py_type_name(vm)))
            }
        )
    }

    fn py_is_iterator(&self, vm: &VM<'h>) -> bool {
        match self {
            // A user-defined class is an iterator only if it defines `__next__`.
            Self::Instance(inst) => inst.py_is_iterator(vm),
            // Every built-in iterator is identified by its type, so there is no
            // list to keep in step with new iterator types here.
            other => other.py_type(vm).is_iterator(),
        }
    }

    fn py_is_iterable(&self, vm: &VM<'h>) -> bool {
        forward_py_value!(self, |value| value.py_is_iterable(vm), else false)
    }

    fn py_is_context_manager(&self, vm: &VM<'h>) -> bool {
        // A real __enter__ may raise AttributeError; check protocol support before calling it.
        forward_py_value!(self, |value| value.py_is_context_manager(vm), else false)
    }

    fn py_enter(&mut self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        forward_py_value!(
            self,
            |value| value.py_enter(vm),
            else { Err(ExcType::attribute_error(self.py_type_name(vm), "__enter__")) }
        )
    }

    fn py_exit(&mut self, vm: &mut VM<'h>, exc: Option<HeapId>) -> RunResult<CallResult> {
        forward_py_value!(
            self,
            |value| value.py_exit(vm, exc),
            else { Err(ExcType::attribute_error(self.py_type_name(vm), "__exit__")) }
        )
    }

    fn py_iadd_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        forward_py_value!(self, |value| value.py_iadd_impl(other, vm), else Ok(false))
    }

    fn py_imul_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        forward_py_value!(self, |value| value.py_imul_impl(other, vm), else Ok(false))
    }

    fn py_ixor_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        forward_py_value!(self, |value| value.py_ixor_impl(other, vm), else Ok(false))
    }

    fn py_isub_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        forward_py_value!(self, |value| value.py_isub_impl(other, vm), else Ok(false))
    }

    fn py_iand_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        forward_py_value!(self, |value| value.py_iand_impl(other, vm), else Ok(false))
    }

    fn py_ior_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        forward_py_value!(self, |value| value.py_ior_impl(other, vm), else Ok(false))
    }

    fn py_cmp_op(&self, other: &Value, op: CmpOperator, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        forward_py_value!(self, |value| value.py_cmp_op(other, op, vm), else Ok(None))
    }

    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        forward_py_value!(
            self,
            |item| item.py_setitem(key, value, vm),
            else {
                key.drop_with(vm);
                value.drop_with(vm);
                Err(ExcType::type_error_not_sub_assignment(
                    &self.py_type_name(vm),
                ))
            }
        )
    }

    fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        if let Self::Module(module) = self {
            return module.py_set_attr(name, value, vm);
        }
        forward_py_value!(
            self,
            |item| item.py_set_attr(name, value, vm),
            else {
                value.drop_with(vm);
                let type_name = self.py_type_name(vm);
                Err(ExcType::attribute_error_no_setattr(
                    &type_name,
                    name.as_str(vm.interns),
                ))
            }
        )
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        forward_py_value!(
            self,
            |value| value.py_getattr(attr, vm),
            else {
                match self {
                    Self::Module(m) => Ok(m.py_getattr(attr, vm)),
                    Self::Exception(e) => Ok(e.py_getattr(attr, vm)),
                    _ => Ok(None),
                }
            }
        )
    }

    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        forward_py_value!(self, |value| value.py_hash(vm), {
            Self::Cell(value) => Ok(Some(identity_hash(value.id()))),
            Self::Module(value) => Ok(Some(identity_hash(value.id()))),
            Self::Undefined => Value::Undefined.py_hash(vm),
            Self::Ellipsis => Value::Ellipsis.py_hash(vm),
            Self::NotImplemented => Value::NotImplemented.py_hash(vm),
            Self::None => Value::None.py_hash(vm),
            Self::Bool(value) => Value::Bool(*value).py_hash(vm),
            Self::Float(value) => Value::Float(*value).py_hash(vm),
            Self::Builtin(value) => Value::Builtin(*value).py_hash(vm),
            Self::ModuleFunction(value) => Value::ModuleFunction(*value).py_hash(vm),
            Self::DefFunction(id) => Value::DefFunction(*id).py_hash(vm),
            Self::Marker(value) => Value::Marker(*value).py_hash(vm),
            Self::Property(value) => Value::Property(*value).py_hash(vm),
            _ => Ok(None),
        })
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        forward_py_value!(self, |value| value.py_iter(vm),
            else Err(ExcType::type_error_not_iterable(&self.py_type_name(vm))))
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_next(vm),
            else Err(ExcType::type_error_not_iterator(&self.py_type_name(vm))))
    }

    fn py_type(&self, vm: &VM<'h>) -> TypeRef {
        forward_py_value!(self, |value| value.py_type(vm), {
            Self::Undefined => panic!("Cannot get type of undefined value"),
            Self::Ellipsis => Type::Ellipsis.into(),
            Self::NotImplemented => Type::NotImplementedType.into(),
            Self::None => Type::NoneType.into(),
            Self::Bool(_) => Type::Bool.into(),
            Self::Float(_) => Type::Float.into(),
            Self::Builtin(value) => value.py_type().into(),
            Self::ModuleFunction(_) => Type::BuiltinFunction.into(),
            Self::DefFunction(_) => Type::Function.into(),
            Self::Marker(value) => value.py_type().into(),
            Self::Property(_) => Type::Property.into(),
            Self::Cell(_) => Type::Cell.into(),
            Self::Exception(e) => e.py_type(vm),
            Self::Module(_) => Type::Module.into(),
            Self::Coroutine(_) | Self::GatherFuture(_) | Self::ExternalFuture(_) => Type::Coroutine.into(),
        })
    }

    fn py_type_name(&self, vm: &VM<'h>) -> Cow<'h, str> {
        forward_py_value!(self, |value| value.py_type_name(vm), else self.py_type(vm).name(vm.heap, vm.interns))
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        forward_py_value!(self, |value| value.py_len(vm), else None)
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        forward_py_value!(self, |value| value.py_bool(vm), {
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
        forward_py_value!(self, |value| value.py_str(vm), {
            Self::Exception(value) => Ok(allocate_string(value.get(vm.heap).py_str(), vm.heap)),
            _ => self.py_repr(vm),
        })
    }

    fn py_contains_impl(&self, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        forward_py_value!(self, |value| value.py_contains_impl(item, vm), else Ok(None))
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        forward_py_value!(self, |value| value.py_eq_impl(other, vm), {
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
        forward_py_value!(self, |value| value.py_add_impl(other, vm), {
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
        forward_py_value!(self, |value| value.py_mul_impl(other, vm), {
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
        forward_py_value!(self, |value| value.py_radd_impl(other, vm), else Ok(None))
    }

    fn py_rmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_rmul_impl(other, vm), else Ok(None))
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
        forward_py_value!(self, |value| value.py_index_impl(vm), {
            Self::Bool(value) => Ok(Some(Value::Int(i64::from(*value)))),
            _ => Ok(None),
        })
    }

    fn py_neg_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_neg_impl(vm), {
            Self::Bool(value) => PyInt::Small(i64::from(*value)).py_neg_impl(vm),
            Self::Float(value) => Ok(Some(Value::Float(-value))),
            _ => Ok(None),
        })
    }

    fn py_abs_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_abs_impl(vm), {
            Self::Bool(value) => Ok(Some(Value::Int(i64::from(*value)))),
            Self::Float(value) => Ok(Some(Value::Float(value.abs()))),
            _ => Ok(None),
        })
    }

    fn py_invert_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_invert_impl(vm), {
            Self::Bool(value) => Ok(Some(Value::Int(!i64::from(*value)))),
            _ => Ok(None),
        })
    }

    fn py_round_impl(&self, ndigits: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_round_impl(ndigits, vm), {
            Self::Bool(value) => PyInt::Small(i64::from(*value)).py_round_impl(ndigits, vm),
            Self::Float(value) => {
                let digits = round_digits(ndigits, vm)?;
                round_float(*value, digits, vm.heap).map(Some)
            }
            _ => Ok(None),
        })
    }

    fn py_int_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_int_impl(vm), {
            Self::Bool(value) => Ok(Some(Value::Int(i64::from(*value)))),
            Self::Float(value) => LongInt::value_from_f64(*value, vm.heap).map(Some),
            _ => Ok(None),
        })
    }

    fn py_float_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<f64>> {
        forward_py_value!(self, |value| value.py_float_impl(vm), {
            Self::Bool(value) => Ok(Some(f64::from(*value))),
            Self::Float(value) => Ok(Some(*value)),
            _ => Ok(None),
        })
    }

    fn py_floor_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_floor_impl(vm), {
            Self::Bool(value) => Ok(Some(Value::Int(i64::from(*value)))),
            Self::Float(value) => LongInt::value_from_f64(value.floor(), vm.heap).map(Some),
            _ => Ok(None),
        })
    }

    fn py_ceil_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_ceil_impl(vm), {
            Self::Bool(value) => Ok(Some(Value::Int(i64::from(*value)))),
            Self::Float(value) => LongInt::value_from_f64(value.ceil(), vm.heap).map(Some),
            _ => Ok(None),
        })
    }

    fn py_trunc_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_trunc_impl(vm), {
            Self::Bool(value) => Ok(Some(Value::Int(i64::from(*value)))),
            Self::Float(value) => LongInt::value_from_f64(*value, vm.heap).map(Some),
            _ => Ok(None),
        })
    }

    fn py_pos_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_pos_impl(vm), {
            Self::Bool(value) => Ok(Some(Value::Int(i64::from(*value)))),
            Self::Float(value) => Ok(Some(Value::Float(*value))),
            _ => Ok(None),
        })
    }

    fn py_pow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_pow_impl(other, modulus, vm), {
            Self::Bool(value) => PyInt::Small(i64::from(*value)).py_pow_impl(other, modulus, vm),
            Self::Float(_) if modulus.is_some() => Err(ExcType::type_error_pow_modulo()),
            Self::Float(value) => float_binary(*value, other, vm, false, |base, exp| float_pow_value(base, exp, vm.heap)),
            _ => Ok(None),
        })
    }

    fn py_rpow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_rpow_impl(other, modulus, vm), {
            Self::Float(_) if modulus.is_some() => Err(ExcType::type_error_pow_modulo()),
            _ => Ok(None),
        })
    }

    fn py_and_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_and_impl(other, vm), {
            Self::Bool(lhs) if let Value::Bool(rhs) = other => Ok(Some(Value::Bool(*lhs && *rhs))),
            Self::Bool(value) => PyInt::Small(i64::from(*value)).py_and_impl(other, vm),
            _ => Ok(None),
        })
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_or_impl(other, vm), {
            Self::Bool(lhs) if let Value::Bool(rhs) = other => Ok(Some(Value::Bool(*lhs || *rhs))),
            Self::Bool(value) => PyInt::Small(i64::from(*value)).py_or_impl(other, vm),
            Self::Builtin(value) => Union::try_or(&Value::Builtin(*value), other, vm),
            Self::None => Union::try_or(&Value::None, other, vm),
            Self::Marker(value) => Union::try_or(&Value::Marker(*value), other, vm),
            _ => Ok(None),
        })
    }

    fn py_ror_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_ror_impl(other, vm), {
            Self::Builtin(value) => Union::try_or(other, &Value::Builtin(*value), vm),
            Self::None => Union::try_or(other, &Value::None, vm),
            Self::Marker(value) => Union::try_or(other, &Value::Marker(*value), vm),
            _ => Ok(None),
        })
    }

    fn py_xor_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        forward_py_value!(self, |value| value.py_xor_impl(other, vm), {
            Self::Bool(lhs) if let Value::Bool(rhs) = other => Ok(Some(Value::Bool(*lhs ^ *rhs))),
            Self::Bool(value) => PyInt::Small(i64::from(*value)).py_xor_impl(other, vm),
            _ => Ok(None),
        })
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        forward_py_value!(self, |value| value.py_call_attr(vm, attr, args), {
            Self::Module(value) => value.py_call_attr(vm, attr, args),
            Self::Builtin(Builtins::Type(ty)) => ty.call_class_method(attr, args, vm),
            Self::DefFunction(func_id) => {
                match annotations::function_call_attr(*func_id, None, attr.as_str(vm.interns), args, vm) {
                    Some(result) => result,
                    None => Err(ExcType::attribute_error(self.py_type_name(vm), attr.as_str(vm.interns))),
                }
            }
            _ => {
                args.drop_with(vm);
                Err(ExcType::attribute_error(self.py_type_name(vm), attr.as_str(vm.interns)))
            }
        })
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        forward_py_value!(self, |value| value.py_getitem(key, vm), {
            Self::Builtin(Builtins::Type(t)) if t.has_class_getitem() => Ok(GenericAlias::subscript(*t, key.clone_with_heap(vm), vm)),
            Self::Builtin(Builtins::Type(Type::Union)) => Union::subscript(key.clone_with_heap(vm), vm),
            Self::Marker(Marker(StaticStrings::Optional)) => Union::optional(key.clone_with_heap(vm), vm),
            Self::Marker(form) if form.is_generic_form() => GenericAlias::subscript_special(*form, key.clone_with_heap(vm), vm),
            // CPython's wording per kind of `typing` object: `Any` and `TypeVar`
            // are classes, `Self` and friends special forms, the two bases generic.
            Self::Marker(Marker(StaticStrings::Any)) => Err(ExcType::type_error_type_not_subscriptable("Any")),
            Self::Marker(Marker(StaticStrings::TypeVar)) => Err(ExcType::type_error_type_not_subscriptable("typing.TypeVar")),
            Self::Marker(Marker(form @ (StaticStrings::Generic | StaticStrings::Protocol))) => Err(ExcType::type_error(format!(
                "Parameters to {}[...] must all be type variables or parameter specification variables.",
                <&str>::from(*form)
            ))),
            Self::Marker(form) if form.py_type() == Type::SpecialForm => {
                Err(ExcType::type_error(format!("typing.{} is not subscriptable", <&str>::from(form.0))))
            }
            Self::Builtin(Builtins::Type(t)) => Err(ExcType::type_error_type_not_subscriptable(t.name())),
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

#[cfg(test)]
mod tests {
    use monty_types::{CompileOptions, PrintWriter, ResourceTracker};

    use super::*;
    use crate::{
        builtins::BuiltinsFunctions,
        defer_drop_mut,
        heap::{DropGuard, HeapReader},
        heap_data::CellValue,
        intern::{BytesId, CompileInterns, LongIntId, StringId},
        modules::math::MathFunctions,
        run::Executor,
        types::{Bytes, List, inline_str::InlineStr, property::ZeroArgOsProperty, str::allocate_string_on_heap},
    };

    struct TestIds {
        string: StringId,
        bytes: BytesId,
        integer: LongIntId,
        function: FunctionId,
    }

    fn with_vm(test: impl for<'h> FnOnce(&mut VM<'h>, TestIds)) {
        let mut executor = Executor::new(
            "def example():\n    return 1".to_owned(),
            "test.py",
            Vec::new(),
            CompileOptions::default(),
        )
        .unwrap();
        let ids = {
            let mut interns = CompileInterns::direct(&mut executor.tables.interns);
            TestIds {
                string: interns.intern("aé🐍b"),
                bytes: interns.intern_bytes(b"a\0\xff"),
                integer: interns.intern_long_int(BigInt::from(1_u128 << 100)),
                function: FunctionId::from_index(0),
            }
        };
        let mut heap = Heap::new(16, ResourceTracker::default());
        HeapReader::with(&mut heap, &mut executor, |reader, executor| {
            let vm = &mut VM::new(
                Vec::new(),
                &mut executor.tables,
                &executor.program,
                reader,
                PrintWriter::Disabled,
            );
            test(vm, ids);
        });
    }

    #[test]
    fn normalized_hashes_match_value_hashes_across_representations() {
        with_vm(|vm, ids| {
            let values = vec![
                Value::Undefined,
                Value::Ellipsis,
                Value::NotImplemented,
                Value::None,
                Value::Bool(true),
                Value::Float(1.25),
                Value::Builtin(Builtins::Function(BuiltinsFunctions::Len)),
                Value::Builtin(Builtins::Type(Type::ValueError)),
                Value::Builtin(Builtins::Type(Type::List)),
                Value::Builtin(Builtins::Type(Type::Type)),
                Value::ModuleFunction(ModuleFunctions::Math(MathFunctions::Sqrt)),
                Value::DefFunction(ids.function),
                Value::Marker(Marker(StaticStrings::Optional)),
                Value::Property(Property::Os(ZeroArgOsProperty::GetEnviron)),
                Value::Int(42),
                Value::InternLongInt(ids.integer),
                LongInt::new(BigInt::from(1_u128 << 100)).into_value(vm.heap),
                Value::InternString(ids.string),
                Value::InlineString(InlineStr::new("aé🐍b").unwrap()),
                allocate_string_on_heap("aé🐍b", vm.heap),
                Value::InternBytes(ids.bytes),
                Value::Ref(vm.heap.allocate(HeapData::Bytes(Bytes::new(b"a\0\xff".to_vec())))),
            ];
            defer_drop!(values, vm);
            for value in values {
                let expected = value.py_hash(vm).unwrap();
                assert_eq!(value.py_value(vm).py_hash(vm).unwrap(), expected);
            }
        });
    }

    #[test]
    fn identity_and_unsupported_hash_protocols_survive_conversion() {
        with_vm(|vm, _| {
            let cell_id = vm.heap.allocate(HeapData::Cell(CellValue(Value::None)));
            let cell = Value::Ref(cell_id);
            defer_drop!(cell, vm);
            assert_eq!(cell.py_value(vm).py_hash(vm).unwrap(), Some(identity_hash(cell_id)));

            let exception = Value::Ref(
                vm.heap
                    .allocate(HeapData::Exception(SimpleException::new(ExcType::ValueError, None))),
            );
            defer_drop!(exception, vm);
            assert_eq!(exception.py_value(vm).py_hash(vm).unwrap(), None);
        });
    }

    #[test]
    fn normalized_iterators_retain_their_source_and_preserve_items() {
        with_vm(|vm, ids| {
            let values = vec![
                Value::InternString(ids.string),
                Value::InlineString(InlineStr::new("aé🐍b").unwrap()),
                allocate_string_on_heap("aé🐍b", vm.heap),
                Value::InternBytes(ids.bytes),
                Value::Ref(vm.heap.allocate(HeapData::Bytes(Bytes::new(b"a\0\xff".to_vec())))),
            ];
            defer_drop_mut!(values, vm);
            while let Some(value) = values.pop() {
                let mut owner = DropGuard::new(value, vm);
                let (value, vm) = owner.as_parts_mut();
                let view = value.py_value(vm);
                assert!(view.py_is_iterable(vm));
                assert!(!view.py_is_iterator(vm));
                let (expected, iterator_type, bytes): (Vec<String>, _, _) = match &view {
                    PyValue::Str(string) => (
                        string.as_str(vm).chars().map(|c| c.to_string()).collect(),
                        Type::StrIterator,
                        false,
                    ),
                    PyValue::Bytes(bytes) => (
                        bytes.as_slice(vm).iter().map(u8::to_string).collect(),
                        Type::BytesIterator,
                        true,
                    ),
                    _ => unreachable!(),
                };
                let iterator = view.py_iter(vm).unwrap().require_synchronous(vm).unwrap();
                drop(view);
                let (value, vm) = owner.into_parts();
                value.drop_with(vm);
                defer_drop_mut!(iterator, vm);
                assert_eq!(iterator.py_type(vm), iterator_type);
                assert!(iterator.py_is_iterator(vm));
                let mut actual = Vec::new();
                while let Some(item) = iterator.py_next(vm).unwrap() {
                    defer_drop!(item, vm);
                    actual.push(if bytes {
                        let Value::Int(byte) = item else {
                            panic!("bytes must yield integers")
                        };
                        byte.to_string()
                    } else {
                        item.as_str(vm).unwrap().to_owned()
                    });
                }
                assert_eq!(actual, expected);
            }
        });
    }

    #[test]
    fn unsupported_protocols_report_errors_and_release_owned_arguments() {
        with_vm(|vm, _| {
            let payload_id = vm.heap.allocate(HeapData::List(List::new(Vec::new())));
            {
                let payload = Value::Ref(payload_id);
                defer_drop!(payload, vm);
                let subjects = vec![Value::None, Value::InlineString(InlineStr::new("text").unwrap())];
                defer_drop!(subjects, vm);
                let name = EitherStr::Heap("missing".to_owned());
                for subject in subjects {
                    let mut view = subject.py_value(vm);
                    assert!(!view.py_is_context_manager(vm));
                    assert!(view.py_enter(vm).err().unwrap().is_catchable(ExcType::AttributeError));
                    assert!(
                        view.py_exit(vm, None)
                            .err()
                            .unwrap()
                            .is_catchable(ExcType::AttributeError)
                    );
                    assert!(view.py_next(vm).err().unwrap().is_catchable(ExcType::TypeError));
                    if matches!(subject, Value::None) {
                        assert!(view.py_iter(vm).err().unwrap().is_catchable(ExcType::TypeError));
                    }
                    assert!(view.py_getattr(&name, vm).unwrap().is_none());
                    assert!(
                        view.py_call(ArgValues::One(payload.clone_with_heap(vm)), vm)
                            .err()
                            .unwrap()
                            .is_catchable(ExcType::TypeError)
                    );
                    assert!(
                        view.py_setitem(payload.clone_with_heap(vm), payload.clone_with_heap(vm), vm)
                            .err()
                            .unwrap()
                            .is_catchable(ExcType::TypeError)
                    );
                    assert!(
                        view.py_set_attr(&name, payload.clone_with_heap(vm), vm)
                            .err()
                            .unwrap()
                            .is_catchable(ExcType::AttributeError)
                    );
                }
            }
            // A leaked argument clone would keep this slot alive after its original owner is dropped.
            assert!(vm.heap.read_ptr(payload_id).try_entry(vm.heap).is_none());
        });
    }
}
