//! The `pydantic.dataclasses` module: `dataclass`, a stdlib `@dataclass` whose
//! `__init__` validates its arguments, and `is_pydantic_dataclass`.
//!
//! The decorator applies the stdlib decoration, stores the config as
//! `__pydantic_config__` and a `TypeAdapter` of the class as
//! `__pydantic_validator__`, then replaces `__init__` with the
//! `ValidatedInit` method kind. Construction validates the arguments against
//! the adapter's dataclass validator as pydantic-core's `dataclass-args`
//! schema does: positional arguments fill the fields in order and report
//! errors by index, keywords match an alias or name and report by key.

use std::mem;

use monty_types::{LocItem, ValidationInputType};

use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    modules::{
        ModuleFunctions,
        dataclasses::{DataclassOptions, apply_dataclass, assign_fields, install_validated_init},
        pydantic::{
            PydanticFunctions,
            arena::{Arena, DataclassValidator, FieldDefault, Validator, ValidatorId},
            catalogue::ErrorType,
            errors::{LineError, ValError, ValResult, into_run_error},
            input::{Input, State},
            python_input::PyInput,
            validate::{field_position, index_loc, validate},
        },
    },
    types::{Dict, Module, Partial, PyTrait, TypeAdapter, instance::namespace_member},
    value::Value,
};

/// Creates the `pydantic.dataclasses` module and allocates it on the heap.
pub(crate) fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::PydanticDataclasses, vm.interns);
    for (name, function) in [
        (StaticStrings::Dataclass, PydanticFunctions::Dataclass),
        (
            StaticStrings::IsPydanticDataclass,
            PydanticFunctions::IsPydanticDataclass,
        ),
        (StaticStrings::PydanticField, PydanticFunctions::Field),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Pydantic(function)), vm);
    }
    vm.heap.allocate_as(module).into_id()
}

/// pydantic's `dataclass(_cls=None, *, init=False, repr=True, eq=True, order=False,
/// unsafe_hash=False, frozen=None, config=None, validate_on_init=None, kw_only=False, slots=False)`.
///
/// Raw `Value`s because the decorator is a `def`; the body takes each flag's truthiness.
#[derive(FromArgs)]
#[from_args(name = "dataclass", style = def)]
struct DataclassArgs {
    #[from_args(pos_only, default = Value::None)]
    cls: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    init: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    repr: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    eq: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    order: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    unsafe_hash: Value,
    #[from_args(kw_only, default = Value::None)]
    frozen: Value,
    #[from_args(kw_only, default = Value::None)]
    config: Value,
    #[from_args(kw_only, default = Value::None)]
    validate_on_init: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    kw_only: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    slots: Value,
}

/// `@dataclass` / `@dataclass(...)`: decorates `cls`, or returns the configured
/// decorator (a `partial` of this function) when no class is given yet.
pub(super) fn dataclass(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DataclassArgs {
        cls,
        init,
        repr,
        eq,
        order,
        unsafe_hash,
        frozen,
        config,
        validate_on_init,
        kw_only,
        slots,
    } = DataclassArgs::from_args(args, vm)?;
    let mut guard = DropGuard::new(
        [
            cls,
            init,
            repr,
            eq,
            order,
            unsafe_hash,
            frozen,
            config,
            validate_on_init,
            kw_only,
            slots,
        ],
        vm,
    );
    let (values, vm) = guard.as_parts();
    let mut truthy = [false; 7];
    for (slot, flag) in truthy.iter_mut().zip([
        &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[9],
    ]) {
        *slot = flag.py_bool(vm)?;
    }
    let [init, repr, eq, order, unsafe_hash, frozen, kw_only] = truthy;
    let slots_flag = values[10].py_bool(vm)?;
    if init {
        return Err(SimpleException::new_msg(
            ExcType::AssertionError,
            "pydantic.dataclasses.dataclass only supports init=False".to_owned(),
        )
        .into());
    }
    // The stdlib decorator's unimplemented options are refused with its wording.
    let unimplemented = [
        ("repr", repr, true),
        ("order", order, false),
        ("unsafe_hash", unsafe_hash, false),
        ("kw_only", kw_only, false),
        ("slots", slots_flag, false),
    ]
    .into_iter()
    .find(|&(_, given, default)| given != default);
    if let Some((name, _, _)) = unimplemented {
        return Err(ExcType::not_implemented(format!("dataclass() does not yet support the {name} option")).into());
    }
    let config_is_dict = matches!(&values[7], Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Dict(_)));
    if !matches!(values[7], Value::None) && !config_is_dict {
        return Err(ExcType::type_error(format!(
            "dataclass() argument 'config' must be a ConfigDict or None, not {}",
            values[7].py_type_name(vm)
        )));
    }
    let options = DataclassOptions { eq, frozen };
    let (values, vm) = guard.into_parts();
    let [
        cls,
        _init,
        _repr,
        _eq,
        _order,
        _unsafe_hash,
        _frozen,
        config,
        validate_on_init,
        _kw_only,
        _slots,
    ] = values;
    validate_on_init.drop_with(vm);
    if matches!(cls, Value::None) {
        return configured(options, config, vm);
    }
    defer_drop!(config, vm);
    apply(cls, options, config, vm)
}

/// The decorator `@dataclass(...)` returns while it waits for the class: a
/// `partial` of `dataclass` with the options bound, so `d = dataclass(frozen=True)`
/// then `@d` works and the class binds positionally or by keyword.
fn configured(options: DataclassOptions, config: Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let function = Value::ModuleFunction(ModuleFunctions::Pydantic(PydanticFunctions::Dataclass));
    let kwargs = KwargsValues::Inline(vec![
        (vm.interns.intern_static(StaticStrings::Eq), Value::Bool(options.eq)),
        (
            vm.interns.intern_static(StaticStrings::Frozen),
            Value::Bool(options.frozen),
        ),
        (vm.interns.intern_static(StaticStrings::Config), config),
    ]);
    Partial::init(
        vm,
        ArgValues::ArgsKargs {
            args: vec![function],
            kwargs,
        },
    )
}

/// Decorates `cls`: the stdlib decoration, then `__pydantic_config__`,
/// `__pydantic_validator__` and the validating `__init__`.
fn apply(cls: Value, options: DataclassOptions, config: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let cls = apply_dataclass(vm, cls, options)?;
    let mut guard = DropGuard::new(cls, vm);
    let (cls, vm) = guard.as_parts();
    let Value::Ref(class_id) = cls else {
        unreachable!("apply_dataclass returns the class")
    };
    let class_id = *class_id;
    // The adapter reads the config from the class, so it is stored first; pydantic
    // stores an empty dict when none was given.
    let config = match config {
        Value::None => Value::Ref(vm.heap.allocate(HeapData::Dict(Dict::new()))),
        given => given.clone_with_heap(vm.heap),
    };
    set_class_attr(class_id, StaticStrings::DunderPydanticConfig, config, vm)?;
    let validator = TypeAdapter::init(vm, ArgValues::One(cls.clone_with_heap(vm.heap)))?;
    set_class_attr(class_id, StaticStrings::DunderPydanticValidator, validator, vm)?;
    let post_init = namespace_member(class_id, "__post_init__", vm.heap, vm.interns).is_some();
    install_validated_init(class_id, options.frozen, post_init, vm)?;
    Ok(guard.into_inner())
}

/// Stores `value` in the class namespace under `name`, releasing what it replaces.
fn set_class_attr(class_id: HeapId, name: StaticStrings, value: Value, vm: &mut VM<'_>) -> RunResult<()> {
    let HeapReadOutput::Class(class) = vm.heap.read(class_id) else {
        unreachable!("apply_dataclass checked the class")
    };
    let name = Value::InternString(vm.interns.intern_static(name));
    let replaced = class.set_attr(name, value, vm)?;
    replaced.drop_with(vm);
    Ok(())
}

/// `is_pydantic_dataclass(obj)`: a class the decorator gave a `__pydantic_validator__`.
pub(super) fn is_pydantic_dataclass(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("is_pydantic_dataclass", vm.heap)?;
    defer_drop!(value, vm);
    let is_pydantic = matches!(value, Value::Ref(id) if namespace_member(*id, "__pydantic_validator__", vm.heap, vm.interns).is_some());
    Ok(Value::Bool(is_pydantic))
}

/// The body of a validating `__init__`: validates `args` through the instance's
/// class `__pydantic_validator__` and stores the field values on `instance_id`.
pub(crate) fn validated_init(
    instance_id: HeapId,
    frozen: bool,
    post_init: bool,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    let HeapData::Instance(instance) = vm.heap.get(instance_id) else {
        unreachable!("a validating __init__ runs on an instance")
    };
    let class_id = instance.class();
    let validator = namespace_member(class_id, "__pydantic_validator__", vm.heap, vm.interns)
        .map(|validator| validator.clone_with_heap(vm.heap));
    let Some(validator) = validator else {
        args.drop_with(vm);
        return Err(ExcType::attribute_error("type", "__pydantic_validator__"));
    };
    // The adapter stays alive while factories and `__post_init__` run Python.
    defer_drop!(validator, vm);
    // Sandbox code can rebind the attribute, so anything else is a `TypeError`.
    let parts = if let Value::Ref(id) = validator
        && let HeapData::TypeAdapter(adapter) = vm.heap.get(*id)
    {
        Some(adapter.parts())
    } else {
        None
    };
    let Some((arena, root, title)) = parts else {
        args.drop_with(vm);
        return Err(ExcType::type_error("__pydantic_validator__ is not a TypeAdapter"));
    };
    let (positional, kwargs) = args.into_parts();
    let values = validate_init_args(&arena, root, positional.collect(), kwargs.into_iter().collect(), vm)
        .map_err(|err| into_run_error(err, &title, ValidationInputType::Python, vm))?;
    let names: Vec<_> = dataclass_of(&arena, root)
        .1
        .fields
        .iter()
        .map(|field| field.name)
        .collect();
    assign_fields(instance_id, &names, values, frozen, post_init, vm)
}

/// The dataclass validator the adapter's root is, under a config's `Strict` wrapper.
fn dataclass_of(arena: &Arena, root: ValidatorId) -> (bool, &DataclassValidator) {
    match arena.get(root) {
        Validator::Strict {
            inner,
            strict,
            scope: true,
        } => match arena.get(*inner) {
            Validator::Dataclass(dataclass) => (*strict, dataclass),
            _ => unreachable!("a pydantic dataclass's adapter is built from the class"),
        },
        Validator::Dataclass(dataclass) => (false, dataclass),
        _ => unreachable!("a pydantic dataclass's adapter is built from the class"),
    }
}

/// The owned state of one validating call, released together on any failure.
struct InitScratch {
    /// The arguments still to validate: positional in order, keywords as parallel keys and values.
    positional: Vec<Value>,
    kw_keys: Vec<Value>,
    kw_values: Vec<Value>,
    /// Clones of every argument keyed by field name or keyword, the `input` of a `missing` error.
    given_keys: Vec<Value>,
    given_values: Vec<Value>,
    /// One slot and one error list per field.
    slots: Vec<Option<Value>>,
    errors: Vec<Option<Vec<LineError>>>,
    /// Surplus positional, duplicate and forbidden keyword errors, in input order.
    extras: Vec<LineError>,
}

impl<C: ContainsHeap> DropWithContext<C> for InitScratch {
    fn drop_with(self, ctx: &mut C) {
        self.positional.drop_with(ctx);
        self.kw_keys.drop_with(ctx);
        self.kw_values.drop_with(ctx);
        self.given_keys.drop_with(ctx);
        self.given_values.drop_with(ctx);
        self.slots.drop_with(ctx);
        self.errors.drop_with(ctx);
        self.extras.drop_with(ctx);
    }
}

/// pydantic-core's `dataclass-args` validation of a call: one value per field,
/// in field order. Positional arguments fill the fields in order and are located
/// by index; keywords match an alias or name and are located by key; a field
/// given both ways is `multiple_argument_values`, a surplus positional
/// `unexpected_positional_argument`, and an unknown keyword is dropped unless
/// the config forbids extras. Takes ownership of every argument.
fn validate_init_args(
    arena: &Arena,
    root: ValidatorId,
    positional: Vec<Value>,
    kwargs: Vec<(Value, Value)>,
    vm: &mut VM<'_>,
) -> ValResult<Vec<Value>> {
    let (strict, dataclass) = dataclass_of(arena, root);
    let fields = &dataclass.fields;
    let mut state = State::new(strict);
    let positional_count = positional.len();
    let mut scratch = InitScratch {
        given_keys: positional
            .iter()
            .zip(fields)
            .map(|(_, field)| Value::InternString(field.name))
            .chain(kwargs.iter().map(|(key, _)| key.clone_with_heap(vm.heap)))
            .collect(),
        given_values: positional
            .iter()
            .zip(fields)
            .map(|(value, _)| value.clone_with_heap(vm.heap))
            .chain(kwargs.iter().map(|(_, value)| value.clone_with_heap(vm.heap)))
            .collect(),
        positional,
        kw_keys: Vec::with_capacity(kwargs.len()),
        kw_values: Vec::with_capacity(kwargs.len()),
        slots: (0..fields.len()).map(|_| None).collect(),
        errors: (0..fields.len()).map(|_| None).collect(),
        extras: Vec::new(),
    };
    for (key, value) in kwargs {
        scratch.kw_keys.push(key);
        scratch.kw_values.push(value);
    }
    let mut guard = DropGuard::new(scratch, vm);
    for index in 0..positional_count {
        let (scratch, vm) = guard.as_parts_mut();
        vm.heap.tracker.check_memory_time_every(index)?;
        let input = PyInput::new(mem::replace(&mut scratch.positional[index], Value::None));
        defer_drop_mut!(input, vm);
        if let Some(field) = fields.get(index) {
            match validate(arena, field.validator, input, &mut state, vm) {
                Ok(v) => scratch.slots[index] = Some(v),
                Err(ValError::Lines(lines)) => {
                    let lines = lines
                        .into_iter()
                        .map(|line| line.with_outer_loc(index_loc(index)))
                        .collect();
                    scratch.errors[index] = Some(lines);
                }
                Err(err) => return Err(err),
            }
        } else {
            let value = input.to_value(vm)?;
            let error = LineError::owned(ErrorType::UnexpectedPositionalArgument {}, value);
            scratch.extras.push(error.with_outer_loc(index_loc(index)));
        }
    }
    let kw_count = guard.as_parts_mut().0.kw_keys.len();
    for step in 0..kw_count {
        let (scratch, vm) = guard.as_parts_mut();
        vm.heap.tracker.check_memory_time_every(positional_count + step)?;
        let key = PyInput::new(mem::replace(&mut scratch.kw_keys[step], Value::None));
        defer_drop!(key, vm);
        let input = PyInput::new(mem::replace(&mut scratch.kw_values[step], Value::None));
        defer_drop_mut!(input, vm);
        let key_text = key.key_value().and_then(|key| key.as_str(vm)).map(str::to_owned);
        let loc = || LocItem::Key(key_text.clone().unwrap_or_default());
        match field_position(dataclass, key, vm) {
            Some((index, _)) if index < positional_count => {
                let value = input.to_value(vm)?;
                let error = LineError::owned(ErrorType::MultipleArgumentValues {}, value);
                scratch.extras.push(error.with_outer_loc(loc()));
            }
            Some((index, _)) => {
                if let Some(earlier) = scratch.slots[index].take() {
                    earlier.drop_with(vm);
                }
                if let Some(earlier) = scratch.errors[index].take() {
                    earlier.drop_with(vm);
                }
                match validate(arena, fields[index].validator, input, &mut state, vm) {
                    Ok(v) => scratch.slots[index] = Some(v),
                    Err(ValError::Lines(lines)) => {
                        scratch.errors[index] =
                            Some(lines.into_iter().map(|line| line.with_outer_loc(loc())).collect());
                    }
                    Err(err) => return Err(err),
                }
            }
            None if dataclass.extra_forbid => {
                let value = input.to_value(vm)?;
                let error = LineError::owned(ErrorType::UnexpectedKeywordArgument {}, value);
                scratch.extras.push(error.with_outer_loc(loc()));
            }
            None => {}
        }
    }
    // Defaults for the unset fields, in field order; a factory may run Python.
    let (scratch, vm) = guard.as_parts_mut();
    let mut errors_guard = DropGuard::new(Vec::<LineError>::new(), vm);
    for (index, field) in fields.iter().enumerate() {
        let (line_errors, vm) = errors_guard.as_parts_mut();
        if let Some(lines) = scratch.errors[index].take() {
            line_errors.extend(lines);
            continue;
        }
        if scratch.slots[index].is_some() {
            continue;
        }
        match &field.default {
            FieldDefault::Value(value) => scratch.slots[index] = Some(value.clone_with_heap(vm.heap)),
            FieldDefault::Factory(factory) => {
                scratch.slots[index] = Some(vm.evaluate_function(factory, ArgValues::Empty).map_err(ValError::Run)?);
            }
            FieldDefault::Required => {
                let pairs: Vec<(Value, Value)> = scratch
                    .given_keys
                    .iter()
                    .zip(&scratch.given_values)
                    .map(|(key, value)| (key.clone_with_heap(vm.heap), value.clone_with_heap(vm.heap)))
                    .collect();
                let whole = Dict::from_pairs(pairs, vm)?;
                let whole = Value::Ref(vm.heap.allocate(HeapData::Dict(whole)));
                let loc = LocItem::Key(field.lookup_name(vm.interns).to_owned());
                line_errors.push(LineError::owned(ErrorType::Missing {}, whole).with_outer_loc(loc));
            }
        }
    }
    errors_guard.as_parts_mut().0.append(&mut scratch.extras);
    let (line_errors, _) = errors_guard.into_parts();
    if line_errors.is_empty() {
        let (scratch, vm) = guard.into_parts();
        let InitScratch {
            positional,
            kw_keys,
            kw_values,
            given_keys,
            given_values,
            slots,
            errors,
            extras,
        } = scratch;
        for owned in [positional, kw_keys, kw_values, given_keys, given_values] {
            owned.drop_with(vm);
        }
        errors.drop_with(vm);
        extras.drop_with(vm);
        Ok(slots
            .into_iter()
            .map(|slot| slot.expect("every field is set or errored"))
            .collect())
    } else {
        Err(ValError::Lines(line_errors))
    }
}
