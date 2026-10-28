//! The `dataclasses` module functions that read a class the decorator already
//! made: `fields`, `asdict`, `astuple` and `replace`, plus the `_replace`
//! function the decorator stores as every dataclass's `__replace__`.
//!
//! None of them build a class, so they live apart from the decorator. Like
//! CPython's, they read `__dataclass_fields__` afresh on every call and
//! construct through the class itself.

use smallvec::SmallVec;

use super::{FieldSpec, field::FieldKind, field_at_id, field_specs};
use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    builtins::Builtins,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext, HeapData, HeapId},
    intern::StringId,
    modules::copy::{Memo, deep_copy},
    types::{
        Dict, List, TupleVec, Type, allocate_tuple,
        instance::{class_name, instance_attr, namespace_member},
        str::allocate_string,
    },
    value::Value,
};

/// `dataclasses.fields(class_or_instance)`: the real fields as a tuple of
/// `Field`s, `InitVar` pseudo-fields left out.
pub(super) fn fields(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let FieldsArgs { class_or_instance } = FieldsArgs::from_args(args, vm)?;
    defer_drop!(class_or_instance, vm);
    let Some(fields_id) = class_or_instance_fields(class_or_instance, vm) else {
        return Err(ExcType::type_error("must be called with a dataclass type or instance"));
    };
    let items: TupleVec = field_specs(fields_id, vm.heap)
        .into_iter()
        .filter(|spec| spec.kind == FieldKind::Field)
        .map(|spec| {
            vm.heap.inc_ref(spec.field_id);
            Value::Ref(spec.field_id)
        })
        .collect();
    Ok(allocate_tuple(items, vm.heap))
}

/// `fields()` arguments; a `def` in CPython.
#[derive(FromArgs)]
#[from_args(name = "fields", style = def)]
struct FieldsArgs {
    class_or_instance: Value,
}

/// `dataclasses.asdict(obj, *, dict_factory=dict)`: an instance as a dict,
/// recursing into fields, lists, tuples and dicts.
pub(super) fn asdict(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let AsdictArgs { obj, dict_factory } = AsdictArgs::from_args(args, vm)?;
    let values = [obj, dict_factory];
    defer_drop!(values, vm);
    let [obj, factory] = values;
    if instance_fields(obj, vm).is_none() {
        return Err(ExcType::type_error("asdict() should be called on dataclass instances"));
    }
    let shape = Shape::Dict(factory_or_builtin(factory, Type::Dict));
    as_inner(obj, shape, vm)
}

/// `asdict()` arguments, mirroring CPython's `def`.
#[derive(FromArgs)]
#[from_args(name = "asdict", style = def)]
struct AsdictArgs {
    obj: Value,
    #[from_args(kw_only, default = Value::Builtin(Builtins::Type(Type::Dict)))]
    dict_factory: Value,
}

/// `dataclasses.astuple(obj, *, tuple_factory=tuple)`: an instance as a tuple,
/// recursing as [`asdict`] does.
pub(super) fn astuple(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let AstupleArgs { obj, tuple_factory } = AstupleArgs::from_args(args, vm)?;
    let values = [obj, tuple_factory];
    defer_drop!(values, vm);
    let [obj, factory] = values;
    if instance_fields(obj, vm).is_none() {
        return Err(ExcType::type_error("astuple() should be called on dataclass instances"));
    }
    let shape = Shape::Tuple(factory_or_builtin(factory, Type::Tuple));
    as_inner(obj, shape, vm)
}

/// `astuple()` arguments, mirroring CPython's `def`.
#[derive(FromArgs)]
#[from_args(name = "astuple", style = def)]
struct AstupleArgs {
    obj: Value,
    #[from_args(kw_only, default = Value::Builtin(Builtins::Type(Type::Tuple)))]
    tuple_factory: Value,
}

/// What a dataclass instance becomes in one `asdict`/`astuple` walk, with the
/// factory to call: `None` for the builtin `dict`/`tuple`, which CPython
/// special-cases to skip the intermediate list.
#[derive(Clone, Copy)]
enum Shape<'a> {
    Dict(Option<&'a Value>),
    Tuple(Option<&'a Value>),
}

/// `None` when `factory` is the builtin `ty`, else the factory itself.
fn factory_or_builtin(factory: &Value, ty: Type) -> Option<&Value> {
    (!matches!(factory, Value::Builtin(Builtins::Type(t)) if *t == ty)).then_some(factory)
}

/// CPython's `_asdict_inner` / `_astuple_inner`: a dataclass instance becomes
/// the requested shape, a list, tuple or dict is rebuilt around converted
/// members, and anything else is deep-copied.
///
/// Charges a recursion level per step, as every Rust-side walk does, so a
/// self-referential structure raises `RecursionError`. Each heap value also
/// polls the limits: a shared sub-graph is rebuilt once per path, so the output
/// can be exponentially larger than the input, with every container short.
fn as_inner(value: &Value, shape: Shape<'_>, vm: &mut VM<'_>) -> RunResult<Value> {
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    if matches!(value, Value::Ref(_)) {
        vm.heap.tracker.check_memory_time()?;
    }
    if let Some(fields_id) = instance_fields(value, vm) {
        return convert_dataclass(value, fields_id, shape, vm);
    }
    let Value::Ref(id) = value else {
        return deep_copy_leaf(value, vm);
    };
    match vm.heap.get(*id) {
        HeapData::List(_) => {
            let items = convert_items(*id, shape, vm)?;
            Ok(Value::Ref(
                vm.heap.allocate(HeapData::List(List::new(items.into_vec()))),
            ))
        }
        HeapData::Tuple(_) => {
            let items = convert_items(*id, shape, vm)?;
            Ok(allocate_tuple(items, vm.heap))
        }
        HeapData::Dict(_) => convert_dict(*id, shape, vm),
        _ => deep_copy_leaf(value, vm),
    }
}

/// A value `as_inner` does not rebuild, deep-copied with a memo of its own as
/// CPython's bare `copy.deepcopy(obj)` is: two leaves sharing a value do not
/// share their copies.
fn deep_copy_leaf(value: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let memo = Memo::fresh(vm)?;
    defer_drop_mut!(memo, vm);
    deep_copy(value, memo, vm)
}

/// One dataclass instance as a dict or tuple of its converted fields, read as
/// attributes in definition order.
fn convert_dataclass(value: &Value, fields_id: HeapId, shape: Shape<'_>, vm: &mut VM<'_>) -> RunResult<Value> {
    let &Value::Ref(self_id) = value else {
        unreachable!("instance_fields matched an instance")
    };
    let names: Vec<StringId> = real_fields(fields_id, vm).into_iter().map(|spec| spec.name).collect();
    match shape {
        Shape::Dict(None) => {
            let dict = vm.heap.allocate_as(Dict::with_capacity(names.len()));
            let mut guard = DropGuard::new(dict, vm);
            for (index, name) in names.into_iter().enumerate() {
                let (dict, vm) = guard.as_parts_mut();
                vm.heap.tracker.check_time_every(index)?;
                let converted = convert_field(self_id, name, shape, vm)?;
                let key = allocate_string(vm.interns.get_str(name).to_owned(), vm.heap);
                let replaced = dict.read(vm.heap).set(key, converted, vm)?;
                replaced.drop_with(vm);
            }
            Ok(guard.into_inner().into_value())
        }
        Shape::Tuple(None) => {
            let items = convert_fields(self_id, &names, shape, vm, |_, value, _| Ok(value))?;
            Ok(allocate_tuple(items, vm.heap))
        }
        // A custom factory gets a list: of `(name, value)` pairs for `asdict`,
        // of values for `astuple`.
        Shape::Dict(Some(factory)) => {
            let items = convert_fields(self_id, &names, shape, vm, |name, value, vm| {
                let key = allocate_string(vm.interns.get_str(name).to_owned(), vm.heap);
                Ok(allocate_tuple(SmallVec::from_iter([key, value]), vm.heap))
            })?;
            call_factory(factory, items, vm)
        }
        Shape::Tuple(Some(factory)) => {
            let items = convert_fields(self_id, &names, shape, vm, |_, value, _| Ok(value))?;
            call_factory(factory, items, vm)
        }
    }
}

/// Converts each named field of an instance, mapping the result through `wrap`.
fn convert_fields(
    self_id: HeapId,
    names: &[StringId],
    shape: Shape<'_>,
    vm: &mut VM<'_>,
    wrap: impl Fn(StringId, Value, &mut VM<'_>) -> RunResult<Value>,
) -> RunResult<TupleVec> {
    let mut guard = DropGuard::new(TupleVec::new(), vm);
    for (index, &name) in names.iter().enumerate() {
        let (items, vm) = guard.as_parts_mut();
        vm.heap.tracker.check_time_every(index)?;
        let converted = convert_field(self_id, name, shape, vm)?;
        items.push(wrap(name, converted, vm)?);
    }
    Ok(guard.into_inner())
}

/// `self.name`, converted. A field the instance does not have raises
/// `AttributeError`, as CPython's `getattr` does.
fn convert_field(self_id: HeapId, name: StringId, shape: Shape<'_>, vm: &mut VM<'_>) -> RunResult<Value> {
    let interns = vm.interns;
    let field_name = interns.get_str(name);
    let Some(value) = instance_attr(self_id, field_name, vm) else {
        let class_id = instance_class(self_id, vm);
        let class_name = class_name(class_id, vm.heap, vm.interns).into_owned();
        return Err(ExcType::attribute_error(&class_name, field_name));
    };
    defer_drop!(value, vm);
    as_inner(value, shape, vm)
}

/// Calls a custom `dict_factory`/`tuple_factory` with a list of `items`.
fn call_factory(factory: &Value, items: TupleVec, vm: &mut VM<'_>) -> RunResult<Value> {
    let list = Value::Ref(vm.heap.allocate(HeapData::List(List::new(items.into_vec()))));
    vm.evaluate_function(factory, ArgValues::One(list))
}

/// Converts every member of the list or tuple at `id`, reading each one fresh,
/// so a conversion that mutates the source is observed as CPython's loop would.
fn convert_items(id: HeapId, shape: Shape<'_>, vm: &mut VM<'_>) -> RunResult<TupleVec> {
    let mut guard = DropGuard::new(TupleVec::new(), vm);
    for index in 0.. {
        let (items, vm) = guard.as_parts_mut();
        vm.heap.tracker.check_time_every(index)?;
        let Some(item) = sequence_item(id, index, vm) else {
            break;
        };
        defer_drop!(item, vm);
        items.push(as_inner(item, shape, vm)?);
    }
    Ok(guard.into_inner())
}

/// The member at `index` of a list or tuple, as an owned reference, or `None`
/// past its end.
fn sequence_item(id: HeapId, index: usize, vm: &VM<'_>) -> Option<Value> {
    let items = match vm.heap.get(id) {
        HeapData::List(list) => list.as_slice(),
        HeapData::Tuple(tuple) => tuple.as_slice(),
        _ => unreachable!("convert_items runs on a list or tuple"),
    };
    items.get(index).map(|item| item.clone_with_heap(vm.heap))
}

/// A dict rebuilt with its keys and values converted, as CPython rebuilds it.
///
/// The pairs are snapshotted first, where CPython iterates live and raises if a
/// conversion resizes the dict.
fn convert_dict(id: HeapId, shape: Shape<'_>, vm: &mut VM<'_>) -> RunResult<Value> {
    let HeapData::Dict(dict) = vm.heap.get(id) else {
        unreachable!("convert_dict runs on a dict")
    };
    let pairs: Vec<(Value, Value)> = dict
        .iter()
        .map(|(key, item)| (key.clone_with_heap(vm.heap), item.clone_with_heap(vm.heap)))
        .collect();
    defer_drop!(pairs, vm);
    let copy = vm.heap.allocate_as(Dict::with_capacity(pairs.len()));
    let mut guard = DropGuard::new(copy, vm);
    for (index, (key, item)) in pairs.iter().enumerate() {
        let (copy, vm) = guard.as_parts_mut();
        vm.heap.tracker.check_time_every(index)?;
        let key = as_inner(key, shape, vm)?;
        let mut key = DropGuard::new(key, vm);
        let (_, vm) = key.as_parts_mut();
        let item = as_inner(item, shape, vm)?;
        let (key, vm) = key.into_parts();
        let replaced = copy.read(vm.heap).set(key, item, vm)?;
        replaced.drop_with(vm);
    }
    Ok(guard.into_inner().into_value())
}

/// `dataclasses.replace(obj, /, **changes)`: a new instance built by calling
/// the class with `changes` plus the current value of every other `__init__`
/// parameter.
pub(super) fn replace(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let ReplaceArgs { obj, changes } = ReplaceArgs::from_args(args, vm)?;
    let Some(fields_id) = instance_fields(&obj, vm) else {
        obj.drop_with(vm);
        changes.drop_with(vm);
        return Err(ExcType::type_error("replace() should be called on dataclass instances"));
    };
    replace_fields(obj, fields_id, changes, vm)
}

/// `replace()` arguments, mirroring CPython's `def replace(obj, /, **changes)`.
#[derive(FromArgs)]
#[from_args(name = "replace", style = def)]
struct ReplaceArgs {
    #[from_args(pos_only)]
    obj: Value,
    #[from_args(varkwargs)]
    changes: KwargsValues,
}

/// CPython's `dataclasses._replace(self, /, **changes)`, the one function every
/// dataclass binds as `__replace__` and `copy.replace()` calls. Unlike
/// [`replace`], it reads `self.__dataclass_fields__` unchecked, so any other
/// `self` raises `AttributeError`.
pub(super) fn replace_dunder(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let ReplaceDunderArgs { this, changes } = ReplaceDunderArgs::from_args(args, vm)?;
    let Some(fields_id) = instance_fields(&this, vm) else {
        let type_name = this.py_type_name(vm).into_owned();
        this.drop_with(vm);
        changes.drop_with(vm);
        return Err(ExcType::attribute_error(type_name, "__dataclass_fields__"));
    };
    replace_fields(this, fields_id, changes, vm)
}

/// `_replace()` arguments; `self` is a Rust keyword, hence the field name.
#[derive(FromArgs)]
#[from_args(name = "_replace", style = def)]
struct ReplaceDunderArgs {
    #[from_args(pos_only, name = "self")]
    this: Value,
    #[from_args(varkwargs)]
    changes: KwargsValues,
}

/// The body both [`replace`] and [`replace_dunder`] share once `obj` is known
/// to be a dataclass instance: calls the class with `changes` plus the current
/// value of every other `__init__` parameter. Consumes `obj` and `changes`.
/// The class call is a tail call, so a user `__init__` may suspend to the host.
fn replace_fields(obj: Value, fields_id: HeapId, changes: KwargsValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let mut guard = DropGuard::new((obj, changes), vm);
    let ((obj, changes), vm) = guard.as_parts_mut();
    let &mut Value::Ref(self_id) = obj else {
        unreachable!("instance_fields matched an instance")
    };
    let specs = field_specs(fields_id, vm.heap);
    let mut carried = DropGuard::new(Vec::new(), vm);
    for spec in specs {
        let (carried, vm) = carried.as_parts_mut();
        let interns = vm.interns;
        let name = interns.get_str(spec.name);
        let given = changes.get(name, vm.heap, vm.interns).is_some();
        if !spec.flags.init {
            if given {
                return Err(ExcType::type_error(format!(
                    "field {name} is declared with init=False, it cannot be specified with replace()"
                )));
            }
            continue;
        }
        if given {
            continue;
        }
        let has_default = field_at_id(vm.heap, spec.field_id).is_some_and(|field| field.default().is_some());
        if spec.kind == FieldKind::InitVar && !has_default {
            return Err(ExcType::type_error(format!(
                "InitVar '{name}' must be specified with replace()"
            )));
        }
        // An `InitVar` was never stored, so its default comes from the class
        // attribute, exactly as CPython's `getattr` finds it.
        let Some(value) = instance_attr(self_id, name, vm) else {
            let class_name = class_name(instance_class(self_id, vm), vm.heap, vm.interns).into_owned();
            return Err(ExcType::attribute_error(&class_name, name));
        };
        carried.push((Value::InternString(spec.name), value));
    }
    let (carried, _) = carried.into_parts();
    let ((obj, changes), vm) = guard.into_parts();
    let class_id = instance_class(self_id, vm);
    vm.heap.inc_ref(class_id);
    let class = Value::Ref(class_id);
    obj.drop_with(vm);
    defer_drop!(class, vm);
    // CPython adds the carried values to the `changes` dict, after the given ones.
    let mut pairs: Vec<(Value, Value)> = changes.into_iter().collect();
    pairs.extend(carried);
    vm.call_function(class, ArgValues::Kwargs(KwargsValues::Pairs(pairs)))
}

/// The fields `asdict`/`astuple` report: the real ones, in definition order.
fn real_fields(fields_id: HeapId, vm: &VM<'_>) -> Vec<FieldSpec> {
    field_specs(fields_id, vm.heap)
        .into_iter()
        .filter(|spec| spec.kind == FieldKind::Field)
        .collect()
}

/// The `__dataclass_fields__` dict of a dataclass instance's class, or `None`
/// for anything else: CPython's `hasattr(type(obj), '__dataclass_fields__')`.
fn instance_fields(value: &Value, vm: &VM<'_>) -> Option<HeapId> {
    let Value::Ref(id) = value else { return None };
    match vm.heap.get(*id) {
        HeapData::Instance(instance) => class_fields(instance.class(), vm),
        _ => None,
    }
}

/// The `__dataclass_fields__` of a class, or of an instance's class: what
/// `fields()` accepts.
fn class_or_instance_fields(value: &Value, vm: &VM<'_>) -> Option<HeapId> {
    let Value::Ref(id) = value else { return None };
    match vm.heap.get(*id) {
        HeapData::Class(_) => class_fields(*id, vm),
        _ => instance_fields(value, vm),
    }
}

/// The dict a class's `__dataclass_fields__` holds, if it holds one.
fn class_fields(class_id: HeapId, vm: &VM<'_>) -> Option<HeapId> {
    match namespace_member(class_id, "__dataclass_fields__", vm.heap, vm.interns) {
        Some(Value::Ref(id)) if matches!(vm.heap.get(*id), HeapData::Dict(_)) => Some(*id),
        _ => None,
    }
}

/// The class of the instance at `self_id`.
fn instance_class(self_id: HeapId, vm: &VM<'_>) -> HeapId {
    match vm.heap.get(self_id) {
        HeapData::Instance(instance) => instance.class(),
        _ => unreachable!("dataclass helper on a non-instance"),
    }
}
