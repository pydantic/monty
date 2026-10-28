//! `dataclasses.make_dataclass`: builds a class from a list of fields, then
//! hands it to the decorator, as CPython's does.

use std::mem;

use ruff_python_stdlib::keyword::is_keyword;

use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapReadOutput},
    intern::{StaticStrings, StringId},
    modules::{ModuleFunctions, dataclasses::DataclassesFunctions},
    types::{Class, Dict, PyTrait, Type, iter::collect_owned_iterable, str::str_isidentifier},
    value::{Marker, Value},
};

/// `make_dataclass(cls_name, fields, *, bases=(), namespace=None, init=True,
/// ..., module=None, decorator=dataclass)`.
///
/// Builds the class `type(cls_name, (), namespace)` would, plus each field's
/// annotation and default, then tail-calls `decorator(cls, init=..., ...)`
/// with the ten options exactly as given, leaving their truthiness to the
/// decorator. A Python `decorator` therefore runs as an ordinary frame.
pub(super) fn make_dataclass(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let MakeDataclassArgs {
        cls_name,
        fields,
        bases,
        namespace,
        init,
        repr,
        eq,
        order,
        unsafe_hash,
        frozen,
        match_args,
        kw_only,
        slots,
        weakref_slot,
        module,
        decorator,
    } = MakeDataclassArgs::from_args(args, vm)?;
    let mut guard = DropGuard::new(
        (
            ([cls_name, fields, namespace, module, decorator], bases),
            [
                init,
                repr,
                eq,
                order,
                unsafe_hash,
                frozen,
                match_args,
                kw_only,
                slots,
                weakref_slot,
            ],
        ),
        vm,
    );
    let ((([cls_name, fields, namespace, module, _], bases), _), vm) = guard.as_parts();
    let class = build_class(cls_name, fields, bases.as_ref(), namespace, module, vm)?;
    let (((values, bases), options), vm) = guard.into_parts();
    let [cls_name, fields, namespace, module, decorator] = values;
    [cls_name, fields, namespace, module].drop_with(vm);
    bases.drop_with(vm);
    defer_drop!(decorator, vm);
    let kwargs = OPTION_NAMES
        .into_iter()
        .zip(options)
        .map(|(name, value)| (Value::InternString(vm.interns.intern_static(name)), value))
        .collect();
    vm.call_function(
        decorator,
        ArgValues::ArgsKargs {
            args: vec![class],
            kwargs: KwargsValues::Pairs(kwargs),
        },
    )
}

/// `make_dataclass()` arguments, mirroring CPython's `def`.
///
/// Raw `Value`s throughout: CPython type-checks nothing up front, and the
/// options go to `decorator` untouched.
#[derive(FromArgs)]
#[from_args(name = "make_dataclass", style = def)]
struct MakeDataclassArgs {
    cls_name: Value,
    fields: Value,
    /// `None` stands in for CPython's `()` default, which would need a heap tuple.
    #[from_args(kw_only, default = None)]
    bases: Option<Value>,
    #[from_args(kw_only, default = Value::None)]
    namespace: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    init: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    repr: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    eq: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    order: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    unsafe_hash: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    frozen: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    match_args: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    kw_only: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    slots: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    weakref_slot: Value,
    #[from_args(kw_only, default = Value::None, static_string = "ModuleKwarg")]
    module: Value,
    #[from_args(kw_only, default = Value::ModuleFunction(ModuleFunctions::Dataclasses(DataclassesFunctions::Dataclass)))]
    decorator: Value,
}

/// The keywords `decorator` receives, in [`MakeDataclassArgs`]' order.
const OPTION_NAMES: [StaticStrings; 10] = [
    StaticStrings::Init,
    StaticStrings::Repr,
    StaticStrings::Eq,
    StaticStrings::Order,
    StaticStrings::UnsafeHash,
    StaticStrings::Frozen,
    StaticStrings::MatchArgs,
    StaticStrings::KwOnly,
    StaticStrings::Slots,
    StaticStrings::WeakrefSlot,
];

/// Allocates the undecorated class, validating in CPython's order: the
/// fields, then `bases`, then `namespace`, then the class name.
///
/// The namespace is a copy of `namespace`, then each field's default, then
/// `__annotations__`, so a field's entries win over a namespace key of the same
/// name, as CPython's `exec_body_callback` and `__annotate__` arrange.
fn build_class(
    cls_name: &Value,
    fields: &Value,
    bases: Option<&Value>,
    namespace: &Value,
    module: &Value,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    let made = collect_fields(fields, vm)?;
    let mut guard = DropGuard::new((made, Vec::new()), vm);
    let ((made, pairs), vm) = guard.as_parts_mut();
    check_bases(bases, vm)?;
    copy_namespace(namespace, cls_name, pairs, vm)?;
    let Some(class_name) = cls_name.as_either_str(vm.heap) else {
        let got = cls_name.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
        return Err(ExcType::type_error_bad_arg_pos("type.__new__", 1, "str", got));
    };
    let mut annotations = Vec::with_capacity(made.len());
    for field in mem::take(made) {
        let name = Value::InternString(field.name);
        if let Some(default) = field.default {
            pairs.push((name.clone_with_heap(vm.heap), default));
        }
        annotations.push((name, field.annotation));
    }
    let annotations = Dict::from_pairs(annotations, vm)?;
    let annotations = vm.heap.allocate(HeapData::Dict(annotations));
    pairs.push((
        Value::InternString(vm.interns.intern_static(StaticStrings::DunderAnnotations)),
        Value::Ref(annotations),
    ));
    // CPython names the calling module when `module` is None; Monty classes
    // have no `__module__` to fall back to, so it is set only when given.
    if !matches!(module, Value::None) {
        pairs.push((
            Value::InternString(vm.interns.intern_static(StaticStrings::DunderModule)),
            module.clone_with_heap(vm.heap),
        ));
    }
    if !pairs.iter().any(|(key, _)| {
        key.as_either_str(vm.heap)
            .is_some_and(|k| k.as_str(vm.interns) == "__doc__")
    }) {
        pairs.push((
            Value::InternString(vm.interns.intern_static(StaticStrings::DunderDoc)),
            Value::None,
        ));
    }
    let ((_, pairs), vm) = guard.into_parts();
    let namespace = Dict::from_pairs(pairs, vm)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Class(Box::new(Class::new(
        class_name, namespace,
    ))))))
}

/// One entry of `make_dataclass`'s `fields`, validated: the name, its
/// annotation (`typing.Any` when only a name was given), and the third item if
/// there was one, which becomes the class attribute the decorator reads.
struct MadeField {
    name: StringId,
    annotation: Value,
    default: Option<Value>,
}

impl<C: ContainsHeap> DropWithContext<C> for MadeField {
    fn drop_with(self, ctx: &mut C) {
        self.annotation.drop_with(ctx);
        self.default.drop_with(ctx);
    }
}

/// Iterates `fields`, validating each entry as CPython does: a `str`, or a
/// 2- or 3-item sequence, whose name is an identifier, not a keyword, and not
/// a repeat.
fn collect_fields(fields: &Value, vm: &mut VM<'_>) -> RunResult<Vec<MadeField>> {
    let items = collect_owned_iterable::<Vec<Value>>(fields.clone_with_heap(vm.heap), vm)?;
    let items = items.into_iter();
    defer_drop_mut!(items, vm);
    let mut made = DropGuard::new(Vec::new(), vm);
    for item in items.by_ref() {
        let (made, vm) = made.as_parts_mut();
        defer_drop!(item, vm);
        let field = parse_field(item, made, vm)?;
        made.push(field);
    }
    Ok(made.into_inner())
}

/// Parses one `fields` entry; `seen` holds the entries before it.
fn parse_field(item: &Value, seen: &[MadeField], vm: &mut VM<'_>) -> RunResult<MadeField> {
    let parts = if item.as_either_str(vm.heap).is_some() {
        // CPython's `_ANY_MARKER`, which `__annotate__` turns into `typing.Any`.
        vec![item.clone_with_heap(vm.heap), Value::Marker(Marker(StaticStrings::Any))]
    } else {
        let Some(len) = item.py_len(vm) else {
            let type_name = item.py_type_name(vm);
            return Err(ExcType::type_error(format!(
                "object of type '{type_name}' has no len()"
            )));
        };
        if !(2..=3).contains(&len) {
            return Err(invalid_field(item, vm));
        }
        let parts = collect_owned_iterable::<Vec<Value>>(item.clone_with_heap(vm.heap), vm)?;
        if parts.len() != len {
            parts.drop_with(vm);
            return Err(invalid_field(item, vm));
        }
        parts
    };
    let mut parts = parts.into_iter();
    let name = parts.next().expect("two or three parts");
    let mut guard = DropGuard::new((name, parts), vm);
    let ((name, _), vm) = guard.as_parts();
    let name_id = field_name_id(name, seen, vm)?;
    let ((name, mut parts), vm) = guard.into_parts();
    name.drop_with(vm);
    Ok(MadeField {
        name: name_id,
        annotation: parts.next().expect("two or three parts"),
        default: parts.next(),
    })
}

/// The interned id of a field name, or CPython's `TypeError` for one it
/// rejects.
///
/// The decorator keys fields by interned name, and Monty cannot intern new
/// text while running, so a name built at runtime that the source never
/// spells out is refused with `NotImplementedError`.
fn field_name_id(name: &Value, seen: &[MadeField], vm: &mut VM<'_>) -> RunResult<StringId> {
    let text = name
        .as_either_str(vm.heap)
        .map(|s| s.as_str(vm.interns).to_owned())
        .filter(|text| str_isidentifier(text));
    let problem = match &text {
        None => Some("must be valid identifiers"),
        Some(text) if is_keyword(text) => Some("must not be keywords"),
        Some(_) => None,
    };
    if let Some(problem) = problem {
        return Err(ExcType::type_error(format!(
            "Field names {problem}: {}",
            repr_of(name, vm)?
        )));
    }
    let text = text.expect("validated above");
    let id = match name {
        Value::InternString(id) => Some(*id),
        _ => vm.interns.get_string_id_by_name(&text),
    };
    let Some(id) = id else {
        return Err(ExcType::not_implemented(format!(
            "make_dataclass() does not yet support the field name '{text}', which appears nowhere in the source code"
        ))
        .into());
    };
    if seen.iter().any(|field| field.name == id) {
        return Err(ExcType::type_error(format!(
            "Field name duplicated: {}",
            repr_of(name, vm)?
        )));
    }
    Ok(id)
}

/// `TypeError: Invalid field: <repr>`, for an entry of the wrong length.
fn invalid_field(item: &Value, vm: &mut VM<'_>) -> RunError {
    match repr_of(item, vm) {
        Ok(repr) => ExcType::type_error(format!("Invalid field: {repr}")),
        Err(error) => error,
    }
}

/// `repr(value)` as text, for an error message.
fn repr_of(value: &Value, vm: &mut VM<'_>) -> RunResult<String> {
    let repr = value.py_repr(vm)?;
    defer_drop!(repr, vm);
    Ok(repr.to_str(vm)?.to_owned())
}

/// Refuses any base class, as `type()` does: Monty classes cannot inherit.
///
/// `None` is the omitted argument. A non-tuple is iterated first, as
/// `types.new_class` does, then refused as `type.__new__` refuses it.
fn check_bases(bases: Option<&Value>, vm: &mut VM<'_>) -> RunResult<()> {
    match bases {
        None => Ok(()),
        Some(bases) if bases.py_type(vm) == Type::Tuple => {
            if bases.py_len(vm) == Some(0) {
                Ok(())
            } else {
                Err(ExcType::type_error("make_dataclass() bases are not supported"))
            }
        }
        Some(bases) => {
            let items = collect_owned_iterable::<Vec<Value>>(bases.clone_with_heap(vm.heap), vm)?;
            items.drop_with(vm);
            let got = bases.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
            Err(ExcType::type_error_bad_arg_pos("type.__new__", 2, "tuple", got))
        }
    }
}

/// Appends the entries `dict.update(namespace)` would add to `pairs`, as
/// CPython's `exec_body_callback` does; `None` adds nothing. Non-string keys are
/// refused as `type()` refuses them.
fn copy_namespace(
    namespace: &Value,
    cls_name: &Value,
    pairs: &mut Vec<(Value, Value)>,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    if matches!(namespace, Value::None) {
        return Ok(());
    }
    let copy = vm.heap.allocate_as(Dict::new()).into_value();
    defer_drop!(copy, vm);
    let &Value::Ref(copy_id) = copy else {
        unreachable!("a dict was just allocated")
    };
    if let HeapReadOutput::Dict(copy) = vm.heap.read(copy_id) {
        copy.merge_from_value(namespace.clone_with_heap(vm.heap), vm)?;
    }
    let HeapData::Dict(copy) = vm.heap.get(copy_id) else {
        unreachable!("a dict was just allocated")
    };
    if let Some((key, _)) = copy.iter().find(|(key, _)| !key.is_str(vm.heap)) {
        let key_type = key.py_type_heap(vm.heap).name(vm.heap, vm.interns);
        let class_name = cls_name
            .as_either_str(vm.heap)
            .map_or_else(String::new, |name| name.as_str(vm.interns).to_owned());
        return Err(ExcType::type_error(format!(
            "non-string key ({key_type}) in the namespace of class '{class_name}'"
        )));
    }
    pairs.extend(
        copy.iter()
            .map(|(key, value)| (key.clone_with_heap(vm.heap), value.clone_with_heap(vm.heap))),
    );
    Ok(())
}
