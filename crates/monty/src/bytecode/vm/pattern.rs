//! Runtime support for `match` statement patterns: the `MatchShape`,
//! `MatchKeys`, `MatchClass` and `MatchAttr` opcodes, Monty's counterparts of
//! CPython's `MATCH_SEQUENCE` / `MATCH_MAPPING` / `MATCH_KEYS` / `MATCH_CLASS`.
//!
//! The compiler (`bytecode/pattern.rs`) only emits these against values it has
//! already shaped: `MatchKeys` always follows a successful mapping `MatchShape`,
//! `MatchAttr` a successful `MatchClass`, and the keys / keyword-name tuples
//! are built by the preceding `BuildTuple`. Class patterns read one attribute
//! per `MatchAttr` instruction so a read that has to go to the host can suspend
//! between them like any other attribute load.

use super::{CallResult, FrameResult, VM, attr::PendingLookupEffect};
use crate::{
    args::ArgValues,
    builtins::{Builtins, isinstance::isinstance_check},
    bytecode::op::{MATCH_SHAPE_MAPPING, MATCH_SHAPE_MIN_LEN},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    frozen::{FrozenFunction, FrozenFunctions},
    heap::{DropGuard, DropWithContext, Heap, HeapData, HeapReadOutput},
    modules::ModuleFunctions,
    types::{
        Dict, PyTrait, Type, descriptor,
        instance::{FastAttributeLookupResult, class_dunder, class_name, lookup_attribute_fast},
        set::set_element_hash,
        str::allocate_string,
        tuple::allocate_tuple,
    },
    value::{EitherStr, Value},
};

/// Builtin types whose class pattern with one positional sub-pattern matches
/// the subject itself (`case int(x)`), CPython's `_Py_TPFLAGS_MATCH_SELF`.
/// Subclasses inherit the flag, hence `defaultdict` and `Counter`.
const SELF_MATCHING_TYPES: [Type; 12] = [
    Type::Bool,
    Type::Bytes,
    Type::Counter,
    Type::DefaultDict,
    Type::Dict,
    Type::Float,
    Type::FrozenSet,
    Type::Int,
    Type::List,
    Type::Set,
    Type::Str,
    Type::Tuple,
];

/// How a class pattern's positional sub-patterns read the subject, once their
/// count has been checked against `__match_args__`.
enum MatchArgs {
    /// A self-matching builtin: the single positional sub-pattern gets the subject.
    SelfMatch,
    /// The attribute name for each positional sub-pattern, in order.
    Names(Vec<EitherStr>),
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

    /// `MatchClass`: `[subject, cls, kwd_names] -> [subject, cls, names_or_None, bool]`.
    ///
    /// Checks `isinstance(subject, cls)` and resolves the attribute names the
    /// sub-patterns read: `nargs` from `cls.__match_args__` (a `None` entry
    /// meaning the subject itself, for self-matching builtins), then the
    /// keywords. Every other problem raises `TypeError`; the reads themselves
    /// are the following `MatchAttr` instructions.
    pub(super) fn match_class(&mut self, nargs: usize) -> RunResult<()> {
        let this = self;
        let kwd_names = this.pop();
        let len = this.stack.len();
        let cls = this.stack[len - 1].clone_with_heap(this.heap);
        let subject = this.stack[len - 2].clone_with_heap(this.heap);
        defer_drop!(kwd_names, this);
        defer_drop!(cls, this);
        defer_drop!(subject, this);
        if !is_class_object(cls, this) {
            return Err(ExcType::type_error("called match pattern must be a class"));
        }
        if !isinstance_check(subject, cls, this)? {
            this.push(Value::None);
            this.push(Value::Bool(false));
            return Ok(());
        }
        if nargs > 0
            && matches!(cls, Value::Ref(id)
            if !descriptor::class_descriptor_free(*id, this)
                && class_dunder(*id, "__match_args__", this)
                .is_some_and(|member| descriptor::has_get(member, this)))
        {
            return this.match_descriptor_class(cls, kwd_names, nargs);
        }
        let names = class_pattern_names(cls, kwd_names, nargs, None, this)?;
        let tuple = allocate_tuple(names.into_iter().collect(), this.heap);
        this.push(tuple);
        this.push(Value::Bool(true));
        Ok(())
    }

    #[cold]
    fn match_descriptor_attr(&mut self, subject: &Value, name: &Value) -> RunResult<CallResult> {
        let this = self;
        let result = this.call_frozen_exact(
            FrozenFunction::DescriptorMatch,
            [
                descriptor::attribute_helper(),
                subject.clone_with_heap(this.heap),
                name.clone_with_heap(this.heap),
                Value::Builtin(Builtins::ExcType(ExcType::AttributeError)),
            ],
        )?;
        this.current_frame_mut().result = FrameResult::MatchAttr;
        Ok(result)
    }

    #[cold]
    fn match_descriptor_class(&mut self, cls: &Value, kwd_names: &Value, nargs: usize) -> RunResult<()> {
        let this = self;
        let CallResult::FramePushed = this.call_frozen_exact(
            FrozenFunction::DescriptorMatchClass,
            [
                descriptor::attribute_helper(),
                Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::MatchClassNames)),
                cls.clone_with_heap(this.heap),
                kwd_names.clone_with_heap(this.heap),
                Value::Int(i64::try_from(nargs).expect("pattern count is a byte")),
                Value::Builtin(Builtins::ExcType(ExcType::AttributeError)),
            ],
        )?
        else {
            unreachable!("frozen class pattern lookup installs a frame")
        };
        // Both pattern continuations return the opcode's two stack values as a pair.
        this.current_frame_mut().result = FrameResult::MatchAttr;
        Ok(())
    }

    /// Validate resolved metadata after its descriptor has returned, possibly from the host.
    pub(crate) fn match_class_names(&mut self, args: ArgValues) -> RunResult<CallResult> {
        let this = self;
        let args = args.into_pos_only("class pattern names", this.heap)?;
        defer_drop!(args, this);
        let [cls, keywords, Value::Int(nargs), match_args] = args.as_slice() else {
            unreachable!("private class pattern signature")
        };
        let nargs = usize::try_from(*nargs).expect("pattern count is a byte");
        let names = class_pattern_names(cls, keywords, nargs, Some(match_args), this)?;
        Ok(CallResult::Value(allocate_tuple(
            names.into_iter().collect(),
            this.heap,
        )))
    }

    /// `MatchAttr`: `[subject, cls, names] -> [subject, cls, names, value, bool]`.
    ///
    /// Reads attribute `index` of the names tuple off the subject. A missing
    /// attribute fails the match; a host-side attribute suspends with a
    /// [`PendingLookupEffect::MatchAttr`] that lands the same pair on resume.
    /// A name already read by an earlier index is CPython's
    /// `TypeError: Cls() got multiple sub-patterns for attribute 'x'`, raised
    /// here rather than up front so a missing earlier attribute still wins.
    pub(super) fn match_attr(&mut self, index: usize) -> Result<CallResult, RunError> {
        let this = self;
        let len = this.stack.len();
        let subject = this.stack[len - 3].clone_with_heap(this.heap);
        let cls = this.stack[len - 2].clone_with_heap(this.heap);
        defer_drop!(subject, this);
        defer_drop!(cls, this);
        let Value::Ref(names_id) = &this.stack[len - 1] else {
            unreachable!("MatchAttr follows a successful MatchClass, which pushed the names tuple")
        };
        let HeapReadOutput::Tuple(names) = this.heap.read(*names_id) else {
            unreachable!("MatchAttr follows a successful MatchClass, which pushed the names tuple")
        };
        let name = names.clone_item(index, this);
        defer_drop!(name, this);
        let Some(attr) = name.as_either_str(this.heap) else {
            // A self-matching builtin: the sub-pattern gets the subject itself.
            this.push(subject.clone_with_heap(this.heap));
            return Ok(CallResult::Value(Value::Bool(true)));
        };
        for earlier in 0..index {
            let earlier = names.clone_item(earlier, this);
            defer_drop!(earlier, this);
            let repeated = earlier
                .as_either_str(this.heap)
                .is_some_and(|e| e.as_str(this.interns) == attr.as_str(this.interns));
            if repeated {
                let class = class_pattern_name(cls, this);
                return Err(ExcType::type_error(format!(
                    "{class}() got multiple sub-patterns for attribute '{}'",
                    attr.as_str(this.interns)
                )));
            }
        }
        match lookup_attribute_fast(subject, &attr, this) {
            FastAttributeLookupResult::Found(value) => {
                this.push(value);
                return Ok(CallResult::Value(Value::Bool(true)));
            }
            FastAttributeLookupResult::Missing => {
                this.push(Value::None);
                return Ok(CallResult::Value(Value::Bool(false)));
            }
            FastAttributeLookupResult::Inconclusive => {}
        }
        if descriptor::attribute_uses_descriptor(subject, &attr, this) {
            return this.match_descriptor_attr(subject, name);
        }
        match subject.py_getattr(&attr, this) {
            Ok(CallResult::Value(value)) => {
                this.push(value);
                Ok(CallResult::Value(Value::Bool(true)))
            }
            Ok(CallResult::AttrLookup {
                name,
                class_name,
                object_id,
                type_object,
                effect,
            }) => {
                effect.drop_with(this);
                Ok(CallResult::AttrLookup {
                    name,
                    class_name,
                    object_id,
                    type_object,
                    effect: Some(PendingLookupEffect::MatchAttr),
                })
            }
            Ok(other) => {
                other.drop_with(this);
                Err(ExcType::type_error(format!(
                    "class pattern attribute '{}' is not a plain attribute",
                    attr.as_str(this.interns)
                )))
            }
            Err(RunError::Exc(exc)) if exc.exc.exc_type() == ExcType::AttributeError => {
                this.push(Value::None);
                Ok(CallResult::Value(Value::Bool(false)))
            }
            Err(err) => Err(err),
        }
    }
}

/// Looks every key of the `keys` tuple up in the mapping `subject`: the values
/// in key order, or `None` as soon as one key is absent.
///
/// Keys are checked in order against the ones before them, like CPython's
/// `seen` set: a key equal to an earlier one is a `ValueError`, unless an
/// earlier key was already found missing. Only keys with equal hashes are
/// compared, so large distinct keys never cost a full `==` each.
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
    let mut hashes = Vec::with_capacity(len);
    for i in 0..len {
        let key = keys.clone_item(i, vm);
        defer_drop!(key, vm);
        let hash = set_element_hash(key, vm)?;
        for earlier in (0..i).filter(|&earlier| hashes[earlier] == hash) {
            let earlier = keys.clone_item(earlier, vm);
            defer_drop!(earlier, vm);
            if earlier.py_eq(key, vm)? {
                return Err(ExcType::value_error_duplicate_mapping_key(key, vm));
            }
        }
        hashes.push(hash);
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
    let HeapReadOutput::Dict(rest) = vm.heap.read(rest_id) else {
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

/// The attribute names a class pattern reads, as `str` values (`None` for the
/// subject itself). The caller has already checked the subject's class.
fn class_pattern_names(
    cls: &Value,
    kwd_names: &Value,
    nargs: usize,
    resolved: Option<&Value>,
    vm: &mut VM<'_>,
) -> RunResult<Vec<Value>> {
    let mut guard = DropGuard::new(Vec::with_capacity(nargs), vm);
    let (names, vm) = guard.as_parts_mut();
    if nargs > 0 {
        match if let Some(value) = resolved {
            validate_match_args(cls, nargs, value, vm)?
        } else {
            class_match_args(cls, nargs, vm)?
        } {
            MatchArgs::SelfMatch => names.push(Value::None),
            MatchArgs::Names(positional) => {
                names.extend(positional.iter().map(|name| either_str_value(name, vm.heap)));
            }
        }
    }
    let Value::Ref(kwd_id) = kwd_names else {
        unreachable!("MatchClass keyword names are a tuple built by the compiler")
    };
    let HeapReadOutput::Tuple(kwd_names) = vm.heap.read(*kwd_id) else {
        unreachable!("MatchClass keyword names are a tuple built by the compiler")
    };
    let len = kwd_names.get(vm.heap).as_slice().len();
    for i in 0..len {
        names.push(kwd_names.clone_item(i, vm));
    }
    Ok(guard.into_inner())
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
/// host class's come from its namespace and must be a tuple of strings
/// (checked only as far as `nargs`, like CPython). Too many positional
/// sub-patterns is reported before a bad element, as CPython does.
fn class_match_args(cls: &Value, nargs: usize, vm: &mut VM<'_>) -> RunResult<MatchArgs> {
    let check_count = |allowed: usize, vm: &VM<'_>| {
        if allowed < nargs {
            Err(positional_count_error(cls, allowed, nargs, vm))
        } else {
            Ok(())
        }
    };
    let Value::Ref(cls_id) = cls else {
        return match cls {
            Value::Builtin(Builtins::Type(t)) if SELF_MATCHING_TYPES.contains(t) => {
                check_count(1, vm).map(|()| MatchArgs::SelfMatch)
            }
            _ => check_count(0, vm).map(|()| MatchArgs::Names(Vec::new())),
        };
    };
    let match_args = match vm.heap.get(*cls_id) {
        HeapData::NamedTupleClass(class) => {
            let fields = class.field_names();
            check_count(fields.len(), vm)?;
            return Ok(MatchArgs::Names(fields[..nargs].to_vec()));
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
        check_count(0, vm)?;
        return Ok(MatchArgs::Names(Vec::new()));
    };
    defer_drop!(match_args, vm);
    validate_match_args(cls, nargs, match_args, vm)
}

fn validate_match_args(cls: &Value, nargs: usize, match_args: &Value, vm: &mut VM<'_>) -> RunResult<MatchArgs> {
    let Value::Ref(args_id) = match_args else {
        return Err(match_args_type_error(cls, match_args, vm));
    };
    let HeapReadOutput::Tuple(tuple) = vm.heap.read(*args_id) else {
        return Err(match_args_type_error(cls, match_args, vm));
    };
    let allowed = tuple.get(vm.heap).as_slice().len();
    if allowed < nargs {
        return Err(positional_count_error(cls, allowed, nargs, vm));
    }
    let mut names = Vec::with_capacity(nargs);
    for i in 0..nargs {
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
    Ok(MatchArgs::Names(names))
}

/// An attribute name as a `str` value for the `MatchClass` names tuple.
fn either_str_value(name: &EitherStr, heap: &Heap) -> Value {
    match name {
        EitherStr::Interned(id) => Value::InternString(*id),
        EitherStr::Heap(text) => allocate_string(text.clone(), heap),
    }
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
