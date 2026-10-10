//! Runtime support for `match` statement patterns: the `MatchShape`,
//! `MatchKeys` and `MatchClass` opcodes, Monty's counterparts of CPython's
//! `MATCH_SEQUENCE` / `MATCH_MAPPING` / `MATCH_KEYS` / `MATCH_CLASS`.
//!
//! The compiler (`bytecode/pattern.rs`) only emits these against values it has
//! already shaped: `MatchKeys` always follows a successful mapping `MatchShape`,
//! and the keys / keyword-name tuples are built by the preceding `BuildTuple`.

use super::{CallResult, VM};
use crate::{
    builtins::{Builtins, isinstance::isinstance_check},
    bytecode::op::{MATCH_SHAPE_MAPPING, MATCH_SHAPE_MIN_LEN},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, DropWithContext, HeapData, HeapReadOutput},
    types::{Dict, PyTrait, Type, instance::class_name, tuple::allocate_tuple},
    value::{EitherStr, Value},
};

/// Builtin types whose class pattern with one positional sub-pattern matches
/// the subject itself (`case int(x)`), CPython's `_Py_TPFLAGS_MATCH_SELF`.
const SELF_MATCHING_TYPES: [Type; 10] = [
    Type::Bool,
    Type::Bytes,
    Type::Dict,
    Type::Float,
    Type::FrozenSet,
    Type::Int,
    Type::List,
    Type::Set,
    Type::Str,
    Type::Tuple,
];

/// What a class's `__match_args__` allows for positional sub-patterns.
enum MatchArgs {
    /// A self-matching builtin: the single positional sub-pattern gets the subject.
    SelfMatch,
    /// Attribute names in `__match_args__` order, truncated to the names actually
    /// used, plus the total count for the arity error.
    Names { allowed: usize, names: Vec<EitherStr> },
}

impl VM<'_> {
    /// `MatchShape`: pushes whether the subject on top of the stack is a
    /// sequence or mapping (per `flags`) of an acceptable length.
    ///
    /// Sequences are `list`, `tuple`, namedtuples, `range` and `deque`, never
    /// `str` or `bytes`; mappings are `dict` and its `collections` subclasses.
    pub(super) fn match_shape(&mut self, length: u16, flags: u8) {
        let subject = self.peek();
        let ty = subject.py_type(self);
        let shape_ok = if flags & MATCH_SHAPE_MAPPING != 0 {
            matches!(ty, Type::Dict | Type::DefaultDict | Type::Counter)
        } else {
            matches!(
                ty,
                Type::List | Type::Tuple | Type::NamedTuple | Type::Range | Type::Deque
            )
        };
        let matched = shape_ok && {
            let actual = subject.py_len(self).unwrap_or(0);
            if flags & MATCH_SHAPE_MIN_LEN != 0 {
                actual >= usize::from(length)
            } else {
                actual == usize::from(length)
            }
        };
        self.push(Value::Bool(matched));
    }

    /// `MatchKeys` (values mode): `[subject, keys] -> [subject, keys, values_or_None, bool]`.
    ///
    /// Every key is looked up without triggering `defaultdict` factories, as
    /// CPython's `dict.get`-based lookup does; one missing key fails the match.
    pub(super) fn match_keys(&mut self) -> RunResult<()> {
        let keys = self.pop();
        let subject = self.pop();
        let mut guard = DropGuard::new((subject, keys), self);
        let ((subject, keys), vm) = guard.as_parts();
        let values = lookup_pattern_keys(subject, keys, vm)?;
        let ((subject, keys), vm) = guard.into_parts();
        vm.push(subject);
        vm.push(keys);
        if let Some(values) = values {
            let tuple = allocate_tuple(values.into_iter().collect(), vm.heap);
            vm.push(tuple);
            vm.push(Value::Bool(true));
        } else {
            vm.push(Value::None);
            vm.push(Value::Bool(false));
        }
        Ok(())
    }

    /// `MatchKeys` (rest mode): `[subject, keys] -> [rest]`, a new plain `dict`
    /// of the subject's entries minus the matched keys, for `**rest`.
    pub(super) fn match_keys_rest(&mut self) -> RunResult<()> {
        let this = self;
        let keys = this.pop();
        let subject = this.pop();
        defer_drop!(keys, this);
        defer_drop!(subject, this);
        let rest = build_rest_dict(subject, keys, this)?;
        this.push(rest);
        Ok(())
    }

    /// `MatchClass`: `[subject, cls, kwd_names] -> [attrs_or_None, bool]`.
    ///
    /// Checks `isinstance(subject, cls)`, then extracts `nargs` attributes named
    /// by `cls.__match_args__` followed by the keyword attributes; a missing
    /// attribute fails the match, every other problem raises `TypeError`.
    pub(super) fn match_class(&mut self, nargs: usize) -> RunResult<()> {
        let this = self;
        let names = this.pop();
        let cls = this.pop();
        let subject = this.pop();
        defer_drop!(names, this);
        defer_drop!(cls, this);
        defer_drop!(subject, this);
        if let Some(attrs) = class_pattern_attrs(subject, cls, names, nargs, this)? {
            let tuple = allocate_tuple(attrs.into_iter().collect(), this.heap);
            this.push(tuple);
            this.push(Value::Bool(true));
        } else {
            this.push(Value::None);
            this.push(Value::Bool(false));
        }
        Ok(())
    }
}

/// Looks every key of the `keys` tuple up in the mapping `subject`: the values
/// in key order, or `None` as soon as one key is absent.
fn lookup_pattern_keys(subject: &Value, keys: &Value, vm: &mut VM<'_>) -> RunResult<Option<Vec<Value>>> {
    let (Value::Ref(subject_id), Value::Ref(keys_id)) = (subject, keys) else {
        return Ok(None);
    };
    let (HeapReadOutput::Dict(dict), HeapReadOutput::Tuple(keys)) = (vm.heap.read(*subject_id), vm.heap.read(*keys_id))
    else {
        return Ok(None);
    };
    let len = keys.get(vm.heap).as_slice().len();
    let mut guard = DropGuard::new(Vec::with_capacity(len), vm);
    let (values, vm) = guard.as_parts_mut();
    for i in 0..len {
        let key = keys.clone_item(i, vm);
        defer_drop!(key, vm);
        match dict.dict_get(key, vm)? {
            Some(value) => values.push(value),
            None => return Ok(None),
        }
    }
    Ok(Some(guard.into_inner()))
}

/// Builds the `**rest` dict: a plain `dict` copy of `subject` with every key
/// in the `keys` tuple removed.
fn build_rest_dict(subject: &Value, keys: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let (Value::Ref(subject_id), Value::Ref(keys_id)) = (subject, keys) else {
        unreachable!("MatchKeys rest mode runs after a successful mapping MatchShape")
    };
    let (HeapReadOutput::Dict(dict), HeapReadOutput::Tuple(keys)) = (vm.heap.read(*subject_id), vm.heap.read(*keys_id))
    else {
        unreachable!("MatchKeys rest mode runs after a successful mapping MatchShape")
    };
    let pairs = dict.clone_all_pairs(vm)?;
    let rest = Dict::from_pairs(pairs, vm)?;
    let rest_id = vm.heap.allocate(HeapData::Dict(rest));
    let mut guard = DropGuard::new(Value::Ref(rest_id), vm);
    let (_, vm) = guard.as_parts();
    let HeapReadOutput::Dict(mut rest) = vm.heap.read(rest_id) else {
        unreachable!("just allocated as a dict")
    };
    let len = keys.get(vm.heap).as_slice().len();
    for i in 0..len {
        let key = keys.clone_item(i, vm);
        defer_drop!(key, vm);
        if let Some(removed) = rest.pop(key, vm)? {
            removed.drop_with(vm);
        }
    }
    Ok(guard.into_inner())
}

/// The attribute values a class pattern sub-matches, or `None` when the
/// subject is not an instance of `cls` or lacks one of the attributes.
fn class_pattern_attrs(
    subject: &Value,
    cls: &Value,
    kwd_names: &Value,
    nargs: usize,
    vm: &mut VM<'_>,
) -> RunResult<Option<Vec<Value>>> {
    if !is_class_object(cls, vm) {
        return Err(ExcType::type_error("called match pattern must be a class"));
    }
    if !isinstance_check(subject, cls, vm)? {
        return Ok(None);
    }
    let mut guard = DropGuard::new(Vec::with_capacity(nargs), vm);
    let (attrs, vm) = guard.as_parts_mut();
    // Attribute names already consumed, so `P(1, x=2)` with `x` first in
    // `__match_args__` is rejected like CPython does.
    let mut seen: Vec<String> = Vec::new();
    if nargs > 0 {
        match class_match_args(cls, nargs, vm)? {
            MatchArgs::SelfMatch => {
                if nargs > 1 {
                    return Err(positional_count_error(cls, 1, nargs, vm));
                }
                attrs.push(subject.clone_with_heap(vm.heap));
            }
            MatchArgs::Names { allowed, names } => {
                if allowed < nargs {
                    return Err(positional_count_error(cls, allowed, nargs, vm));
                }
                for name in &names {
                    match class_pattern_attr(subject, cls, name, &mut seen, vm)? {
                        Some(value) => attrs.push(value),
                        None => return Ok(None),
                    }
                }
            }
        }
    }
    let Value::Ref(names_id) = kwd_names else {
        unreachable!("MatchClass keyword names are a tuple built by the compiler")
    };
    let HeapReadOutput::Tuple(kwd_names) = vm.heap.read(*names_id) else {
        unreachable!("MatchClass keyword names are a tuple built by the compiler")
    };
    let len = kwd_names.get(vm.heap).as_slice().len();
    for i in 0..len {
        let name = kwd_names.clone_item(i, vm);
        defer_drop!(name, vm);
        let name = name
            .as_either_str(vm.heap)
            .expect("MatchClass keyword names are interned strings");
        match class_pattern_attr(subject, cls, &name, &mut seen, vm)? {
            Some(value) => attrs.push(value),
            None => return Ok(None),
        }
    }
    Ok(Some(guard.into_inner()))
}

/// Whether `value` is something `isinstance` accepts as a single class.
fn is_class_object(value: &Value, vm: &VM<'_>) -> bool {
    match value {
        Value::Builtin(Builtins::Type(_) | Builtins::ExcType(_)) => true,
        Value::Ref(id) => matches!(
            vm.heap.get(*id),
            HeapData::Class(_) | HeapData::NamedTupleClass(_) | HeapData::HostClassType(_)
        ),
        _ => false,
    }
}

/// Resolves `cls.__match_args__` for the first `nargs` positional sub-patterns.
///
/// Builtins carry no `__match_args__`: the self-matching ones accept one
/// positional sub-pattern, the rest none. A namedtuple's are its fields; a
/// user or host class's come from its namespace and must be a tuple of strings
/// (checked only as far as `nargs`, like CPython).
fn class_match_args(cls: &Value, nargs: usize, vm: &mut VM<'_>) -> RunResult<MatchArgs> {
    let no_match_args = MatchArgs::Names {
        allowed: 0,
        names: Vec::new(),
    };
    let Value::Ref(cls_id) = cls else {
        return Ok(match cls {
            Value::Builtin(Builtins::Type(t)) if SELF_MATCHING_TYPES.contains(t) => MatchArgs::SelfMatch,
            _ => no_match_args,
        });
    };
    let match_args = match vm.heap.get(*cls_id) {
        HeapData::NamedTupleClass(class) => {
            let fields = class.field_names();
            return Ok(MatchArgs::Names {
                allowed: fields.len(),
                names: fields.iter().take(nargs).cloned().collect(),
            });
        }
        HeapData::Class(class) => class
            .namespace()
            .get_by_str("__match_args__", vm.heap, vm.interns)
            .map(|value| value.clone_with_heap(vm.heap)),
        // A host class only has `__match_args__` if the host sent it as a class attribute.
        HeapData::HostClassType(class) => class
            .attrs()
            .get_by_str("__match_args__", vm.heap, vm.interns)
            .map(|value| value.clone_with_heap(vm.heap)),
        _ => None,
    };
    let Some(match_args) = match_args else {
        return Ok(no_match_args);
    };
    defer_drop!(match_args, vm);
    let Value::Ref(args_id) = match_args else {
        return Err(match_args_type_error(cls, match_args, vm));
    };
    let HeapReadOutput::Tuple(tuple) = vm.heap.read(*args_id) else {
        return Err(match_args_type_error(cls, match_args, vm));
    };
    let allowed = tuple.get(vm.heap).as_slice().len();
    let mut names = Vec::with_capacity(nargs.min(allowed));
    for i in 0..nargs.min(allowed) {
        let item = tuple.clone_item(i, vm);
        defer_drop!(item, vm);
        let Some(name) = item.as_either_str(vm.heap) else {
            let type_name = item.py_type_name(vm);
            return Err(ExcType::type_error(format!(
                "__match_args__ elements must be strings (got {type_name})"
            )));
        };
        names.push(name);
    }
    Ok(MatchArgs::Names { allowed, names })
}

/// `TypeError: Foo.__match_args__ must be a tuple (got list)`.
fn match_args_type_error(cls: &Value, match_args: &Value, vm: &VM<'_>) -> RunError {
    let class = class_pattern_name(cls, vm);
    let type_name = match_args.py_type_name(vm);
    ExcType::type_error(format!("{class}.__match_args__ must be a tuple (got {type_name})"))
}

/// `TypeError: Foo() accepts 2 positional sub-patterns (3 given)`.
fn positional_count_error(cls: &Value, allowed: usize, given: usize, vm: &VM<'_>) -> RunError {
    let class = class_pattern_name(cls, vm);
    let plural = if allowed == 1 { "" } else { "s" };
    ExcType::type_error(format!(
        "{class}() accepts {allowed} positional sub-pattern{plural} ({given} given)"
    ))
}

/// Reads one attribute for a class pattern: `None` when the subject has no
/// such attribute, `TypeError` when the pattern already consumed the name.
fn class_pattern_attr(
    subject: &Value,
    cls: &Value,
    name: &EitherStr,
    seen: &mut Vec<String>,
    vm: &mut VM<'_>,
) -> RunResult<Option<Value>> {
    let name_str = name.as_str(vm.interns);
    if seen.iter().any(|s| s == name_str) {
        let class = class_pattern_name(cls, vm);
        return Err(ExcType::type_error(format!(
            "{class}() got multiple sub-patterns for attribute '{name_str}'"
        )));
    }
    seen.push(name_str.to_owned());
    match subject.py_getattr(name, vm) {
        Ok(CallResult::Value(value)) => Ok(Some(value)),
        // A host-backed object whose attribute lives host-side: the lookup would
        // suspend the VM mid-pattern, which `match` cannot resume.
        Ok(other) => {
            other.drop_with(vm);
            Err(ExcType::not_implemented(format!(
                "class pattern attribute '{name_str}' requires a host lookup, which match statements do not support"
            ))
            .into())
        }
        Err(RunError::Exc(exc)) if exc.exc.exc_type() == ExcType::AttributeError => Ok(None),
        Err(err) => Err(err),
    }
}

/// The class's `__name__`, as CPython's class pattern errors spell it.
fn class_pattern_name(cls: &Value, vm: &VM<'_>) -> String {
    match cls {
        Value::Builtin(Builtins::Type(t)) => t.dunder_name(vm.heap, vm.interns).into_owned(),
        Value::Builtin(Builtins::ExcType(exc_type)) => {
            let qualified = exc_type.to_string();
            qualified.rsplit('.').next().unwrap_or(&qualified).to_owned()
        }
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Class(_) => class_name(*id, vm.heap, vm.interns).into_owned(),
            HeapData::NamedTupleClass(class) => class.name(vm.interns).to_owned(),
            HeapData::HostClassType(class) => class.name(vm.interns).to_owned(),
            _ => cls.py_type_name(vm).to_string(),
        },
        _ => cls.py_type_name(vm).to_string(),
    }
}
