//! `pydantic.validate_call`: a wrapper that validates a function's arguments
//! against its annotations before calling it, and optionally its result after.
//!
//! The wrapper binds the call itself, as pydantic-core's `arguments` schema
//! does: positional arguments fill the positional parameters in order and
//! report errors by index, keywords match a parameter name and report by it,
//! surplus ones go to `*args` / `**kwargs` when the function has them, and the
//! validated arguments are passed to the function from a frozen Python frame
//! so a suspending body still works. A `Field(...)` as a parameter's default
//! supplies its constraints and default, as it does for a dataclass field.

use std::{mem, rc::Rc};

use monty_types::{LocItem, ValidationInputType};
use smallvec::SmallVec;

use crate::{
    args::{ArgValues, FromArgs, KwargsValues, ParameterKind},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    frozen::{FrozenFunction, FrozenFunctions},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId},
    intern::StaticStrings,
    modules::{
        ModuleFunctions,
        pydantic::{
            PydanticFunctions,
            arena::{Arena, FieldDefault},
            build::{build, build_field, dict_of, type_hints},
            catalogue::ErrorType,
            config::Config,
            errors::{ERROR_URL_PREFIX, LineError, ValError, ValResult, into_run_error},
            input::{Input, State},
            python_input::PyInput,
            snapshot::repr_string,
            validate::{index_loc, validate},
        },
    },
    types::{Dict, Partial, PyTrait, ValidateCall, allocate_tuple, validate_call::ParamValidator},
    value::{Marker, Value},
};

/// The frozen frame a wrapped call runs in: the function itself, then the
/// return check, which is a no-op without `validate_return`.
pub(crate) const VALIDATE_CALL_SOURCE: &str = r"
def __monty_validate_call(check_return, wrapper, function, args, kwargs):
    return check_return(wrapper, function(*args, **kwargs))
";

/// `validate_call(func=None, /, *, config=None, validate_return=False)`.
#[derive(FromArgs)]
#[from_args(name = "validate_call", style = def)]
struct ValidateCallArgs {
    #[from_args(pos_only, default = Value::None)]
    func: Value,
    #[from_args(kw_only, default = Value::None)]
    config: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    validate_return: Value,
}

/// `@validate_call` / `@validate_call(...)`: wraps `func`, or returns the
/// configured decorator (a `partial` of this function) when no function is given yet.
pub(super) fn validate_call(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let ValidateCallArgs {
        func,
        config,
        validate_return,
    } = ValidateCallArgs::from_args(args, vm)?;
    defer_drop!(func, vm);
    defer_drop!(validate_return, vm);
    let validate_return = validate_return.py_bool(vm);
    let validate_return = match validate_return {
        Ok(flag) => flag,
        Err(err) => {
            config.drop_with(vm);
            return Err(err);
        }
    };
    if matches!(func, Value::None) {
        let function = Value::ModuleFunction(ModuleFunctions::Pydantic(PydanticFunctions::ValidateCall));
        let kwargs = KwargsValues::Inline(vec![
            (vm.interns.intern_static(StaticStrings::Config), config),
            (
                vm.interns.intern_static(StaticStrings::ValidateReturn),
                Value::Bool(validate_return),
            ),
        ]);
        return Partial::init(
            vm,
            ArgValues::ArgsKargs {
                args: vec![function],
                kwargs,
            },
        );
    }
    defer_drop!(config, vm);
    let config = Config::parse(config, vm)?;
    wrap(func, &config, validate_return, vm)
}

/// Builds the wrapper for `function`.
fn wrap(function: &Value, config: &Config, validate_return: bool, vm: &mut VM<'_>) -> RunResult<Value> {
    let (func_id, defaults) = match function {
        Value::DefFunction(id) => (*id, Vec::new()),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::FunctionDefaults(function) => (function.func_id, clone_all(&function.defaults, vm)),
            HeapData::Closure(closure) => (closure.func_id, clone_all(&closure.defaults, vm)),
            _ => return Err(not_a_function(function, vm)),
        },
        _ => return Err(not_a_function(function, vm)),
    };
    defer_drop!(defaults, vm);
    let (name, parameters, is_async) = {
        let function = vm.interns.get_function(func_id);
        (
            vm.interns.get_str(function.name.name_id).to_owned(),
            function.signature.parameters(),
            function.is_async,
        )
    };
    if is_async && validate_return {
        return Err(ExcType::not_implemented(
            "validate_call(validate_return=True) does not yet support an async function",
        )
        .into());
    }
    let hints = type_hints(function, vm)?;
    defer_drop!(hints, vm);
    let any = Value::Marker(Marker(StaticStrings::Any));
    // The arena and the parameters built so far, released together if a later one fails.
    let mut built = DropGuard::new((Arena::default(), Vec::<ParamValidator>::new()), vm);
    for parameter in &parameters {
        let ((arena, params), vm) = built.as_parts_mut();
        let hint = dict_of(hints, vm)
            .get_by_str(vm.interns.get_str(parameter.name), vm.heap, vm.interns)
            .map_or_else(|| any.clone_with_heap(vm.heap), |hint| hint.clone_with_heap(vm.heap));
        // A `Field(...)` default carries the constraints and the real default.
        let info = parameter
            .default_index
            .and_then(|index| defaults.get(index))
            .and_then(|default| match default {
                Value::Ref(id) => match vm.heap.get(*id) {
                    HeapData::FieldInfo(info) => Some(info.clone_with_heap(vm.heap)),
                    _ => None,
                },
                _ => None,
            });
        let validator = build_field(&hint, info.as_ref(), config, arena, vm);
        hint.drop_with(vm);
        let default = match (&info, parameter.default_index, parameter.kind) {
            (_, _, ParameterKind::VarArgs | ParameterKind::VarKwargs) => None,
            (Some(info), _, _) => Some(match (&info.default, &info.default_factory) {
                (Some(value), _) => FieldDefault::Value(value.clone_with_heap(vm.heap)),
                (None, Some(factory)) => FieldDefault::Factory(factory.clone_with_heap(vm.heap)),
                (None, None) => FieldDefault::Required,
            }),
            (None, None, _) => Some(FieldDefault::Required),
            (None, Some(_), _) => None,
        };
        info.drop_with(vm);
        let validator = match validator {
            Ok(validator) => validator,
            Err(err) => {
                default.drop_with(vm);
                return Err(err);
            }
        };
        params.push(ParamValidator {
            name: parameter.name,
            kind: parameter.kind,
            validator,
            default,
        });
    }
    let return_validator = if validate_return {
        let ((arena, _), vm) = built.as_parts_mut();
        let hint = dict_of(hints, vm)
            .get_by_str("return", vm.heap, vm.interns)
            .map_or_else(|| any.clone_with_heap(vm.heap), |hint| hint.clone_with_heap(vm.heap));
        let validator = build(&hint, config, arena, vm);
        hint.drop_with(vm);
        Some(validator?)
    } else {
        None
    };
    let ((arena, params), vm) = built.into_parts();
    let wrapper = ValidateCall {
        function: function.clone_with_heap(vm.heap),
        arena: Rc::new(arena),
        params,
        return_validator,
        strict: config.strict,
        name,
    };
    Ok(Value::Ref(vm.heap.allocate(HeapData::ValidateCall(Box::new(wrapper)))))
}

/// New references to every value.
fn clone_all(values: &[Value], vm: &VM<'_>) -> Vec<Value> {
    values.iter().map(|value| value.clone_with_heap(vm.heap)).collect()
}

/// pydantic's `PydanticUserError` for decorating something that is not a function.
fn not_a_function(value: &Value, vm: &mut VM<'_>) -> RunError {
    let repr = repr_string(value, vm);
    SimpleException::new_msg(
        ExcType::PydanticUserError,
        format!(
            "Unable to validate {repr}: `validate_call` should be applied to one of the following: function, method, partial, or lambda\n\nFor further information visit {}u/validate-call-type",
            ERROR_URL_PREFIX.trim_end_matches("v/")
        ),
    )
    .into()
}

/// The parameters of the wrapper at `wrapper_id`.
fn params<'a>(wrapper_id: HeapId, vm: &'a VM<'_>) -> &'a [ParamValidator] {
    &wrapper(wrapper_id, vm).params
}

/// The wrapper at `wrapper_id`, which its caller keeps alive.
fn wrapper<'a>(wrapper_id: HeapId, vm: &'a VM<'_>) -> &'a ValidateCall {
    match vm.heap.get(wrapper_id) {
        HeapData::ValidateCall(wrapper) => wrapper,
        _ => unreachable!("call_wrapper runs on a ValidateCall"),
    }
}

/// A call of the wrapper: validates `args`, then calls the function with the
/// validated arguments from the frozen frame.
pub(crate) fn call_wrapper(wrapper_id: HeapId, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    vm.heap.inc_ref(wrapper_id);
    let keep_alive = Value::Ref(wrapper_id);
    defer_drop!(keep_alive, vm);
    let (positional, kwargs) = args.into_parts();
    let name = wrapper(wrapper_id, vm).name.clone();
    let (positional, kwargs) = validate_args(wrapper_id, positional.collect(), kwargs.into_iter().collect(), vm)
        .map_err(|err| into_run_error(err, &name, ValidationInputType::Python, vm))?;
    let args = allocate_tuple(SmallVec::from_vec(positional), vm.heap);
    let kwargs = match Dict::from_pairs(kwargs, vm) {
        Ok(kwargs) => Value::Ref(vm.heap.allocate(HeapData::Dict(kwargs))),
        Err(err) => {
            args.drop_with(vm);
            return Err(err);
        }
    };
    let check_return = Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::ValidateReturn));
    let function = wrapper(wrapper_id, vm).function.clone_with_heap(vm.heap);
    vm.heap.inc_ref(wrapper_id);
    vm.call_frozen_exact(
        FrozenFunction::PydanticValidateCall,
        [check_return, Value::Ref(wrapper_id), function, args, kwargs],
    )
}

/// The frozen frame's `check_return(wrapper, result)`: the result validated
/// against the return annotation when `validate_return` was set, else as is.
pub(crate) fn call_validate_return(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let (wrapper_value, result) = args.get_two_args("check_return", vm.heap)?;
    defer_drop!(wrapper_value, vm);
    let Value::Ref(wrapper_id) = wrapper_value else {
        unreachable!("the frozen frame passes the wrapper")
    };
    let Some(return_validator) = wrapper(*wrapper_id, vm).return_validator else {
        return Ok(result);
    };
    let (arena, strict, name) = {
        let wrapper = wrapper(*wrapper_id, vm);
        (Rc::clone(&wrapper.arena), wrapper.strict, wrapper.name.clone())
    };
    let input = PyInput::new(result);
    defer_drop_mut!(input, vm);
    let mut state = State::new(strict);
    validate(&arena, return_validator, input, &mut state, vm)
        .map_err(|err| into_run_error(err, &name, ValidationInputType::Python, vm))
}

/// The positional arguments and keyword pairs a validated call passes on.
type BoundCall = (Vec<Value>, Vec<(Value, Value)>);

/// The owned state of one call under validation, released together on any failure.
struct CallScratch {
    /// The arguments still to validate, with keywords as parallel keys and values.
    positional: Vec<Value>,
    kw_keys: Vec<Value>,
    kw_values: Vec<Value>,
    /// Clones of every argument keyed by parameter name or keyword, the `input` of a `missing` error.
    given_keys: Vec<Value>,
    given_values: Vec<Value>,
    /// One slot per parameter, and the validated surplus for `*args` and `**kwargs`.
    slots: Vec<Option<Value>>,
    /// Which parameters an argument reached, even one that failed, so they are not also `missing`.
    attempted: Vec<bool>,
    extra_positional: Vec<Value>,
    extra_kw_keys: Vec<Value>,
    extra_kw_values: Vec<Value>,
    errors: Vec<LineError>,
}

impl<C: ContainsHeap> DropWithContext<C> for CallScratch {
    fn drop_with(self, ctx: &mut C) {
        self.positional.drop_with(ctx);
        self.kw_keys.drop_with(ctx);
        self.kw_values.drop_with(ctx);
        self.given_keys.drop_with(ctx);
        self.given_values.drop_with(ctx);
        self.slots.drop_with(ctx);
        self.extra_positional.drop_with(ctx);
        self.extra_kw_keys.drop_with(ctx);
        self.extra_kw_values.drop_with(ctx);
        self.errors.drop_with(ctx);
    }
}

/// pydantic-core's `arguments` validation: the positional arguments and
/// keyword pairs to call the function with. Takes ownership of every argument.
fn validate_args(
    wrapper_id: HeapId,
    positional: Vec<Value>,
    kwargs: Vec<(Value, Value)>,
    vm: &mut VM<'_>,
) -> ValResult<BoundCall> {
    let arena = Rc::clone(&wrapper(wrapper_id, vm).arena);
    let param_count = params(wrapper_id, vm).len();
    let positional_slots = params(wrapper_id, vm)
        .iter()
        .take_while(|param| {
            matches!(
                param.kind,
                ParameterKind::PositionalOnly | ParameterKind::PositionalOrKeyword
            )
        })
        .count();
    let var_args = params(wrapper_id, vm)
        .iter()
        .find(|param| param.kind == ParameterKind::VarArgs)
        .map(|param| param.validator);
    let var_kwargs = params(wrapper_id, vm)
        .iter()
        .find(|param| param.kind == ParameterKind::VarKwargs)
        .map(|param| param.validator);
    let mut state = State::new(wrapper(wrapper_id, vm).strict);
    let positional_count = positional.len();
    let mut scratch = CallScratch {
        given_keys: positional
            .iter()
            .enumerate()
            .map(|(index, _)| match params(wrapper_id, vm).get(index) {
                Some(param) if index < positional_slots => Value::InternString(param.name),
                _ => Value::Int(i64::try_from(index).expect("argument counts fit i64")),
            })
            .chain(kwargs.iter().map(|(key, _)| key.clone_with_heap(vm.heap)))
            .collect(),
        given_values: clone_all(&positional, vm)
            .into_iter()
            .chain(kwargs.iter().map(|(_, value)| value.clone_with_heap(vm.heap)))
            .collect(),
        positional,
        kw_keys: Vec::with_capacity(kwargs.len()),
        kw_values: Vec::with_capacity(kwargs.len()),
        slots: (0..param_count).map(|_| None).collect(),
        attempted: vec![false; param_count],
        extra_positional: Vec::new(),
        extra_kw_keys: Vec::new(),
        extra_kw_values: Vec::new(),
        errors: Vec::new(),
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
        let validator = if index < positional_slots {
            Some(params(wrapper_id, vm)[index].validator)
        } else {
            var_args
        };
        let Some(validator) = validator else {
            let value = input.to_value(vm)?;
            let error = LineError::owned(ErrorType::UnexpectedPositionalArgument {}, value);
            scratch.errors.push(error.with_outer_loc(index_loc(index)));
            continue;
        };
        if index < positional_slots {
            scratch.attempted[index] = true;
        }
        match validate(&arena, validator, input, &mut state, vm) {
            Ok(v) if index < positional_slots => scratch.slots[index] = Some(v),
            Ok(v) => scratch.extra_positional.push(v),
            Err(ValError::Lines(lines)) => {
                scratch
                    .errors
                    .extend(lines.into_iter().map(|line| line.with_outer_loc(index_loc(index))));
            }
            Err(err) => return Err(err),
        }
    }
    let kw_count = guard.as_parts_mut().0.kw_keys.len();
    for step in 0..kw_count {
        let (scratch, vm) = guard.as_parts_mut();
        vm.heap.tracker.check_memory_time_every(positional_count + step)?;
        let key = mem::replace(&mut scratch.kw_keys[step], Value::None);
        defer_drop!(key, vm);
        let input = PyInput::new(mem::replace(&mut scratch.kw_values[step], Value::None));
        defer_drop_mut!(input, vm);
        let name = key.as_str(vm).map(str::to_owned).unwrap_or_default();
        let loc = || LocItem::Key(name.clone());
        // Positional-only parameters cannot be named; their names fall through to `**kwargs`.
        let found = params(wrapper_id, vm).iter().position(|param| {
            matches!(
                param.kind,
                ParameterKind::PositionalOrKeyword | ParameterKind::KeywordOnly
            ) && vm.interns.get_str(param.name) == name
        });
        match found {
            Some(index) if index < positional_count => {
                let value = input.to_value(vm)?;
                let error = LineError::owned(ErrorType::MultipleArgumentValues {}, value);
                scratch.errors.push(error.with_outer_loc(loc()));
            }
            Some(index) => match validate(&arena, params(wrapper_id, vm)[index].validator, input, &mut state, vm) {
                Ok(v) => {
                    scratch.attempted[index] = true;
                    scratch.slots[index] = Some(v);
                }
                Err(ValError::Lines(lines)) => {
                    scratch.attempted[index] = true;
                    scratch
                        .errors
                        .extend(lines.into_iter().map(|line| line.with_outer_loc(loc())));
                }
                Err(err) => return Err(err),
            },
            None => {
                let Some(validator) = var_kwargs else {
                    let value = input.to_value(vm)?;
                    let error = LineError::owned(ErrorType::UnexpectedKeywordArgument {}, value);
                    scratch.errors.push(error.with_outer_loc(loc()));
                    continue;
                };
                match validate(&arena, validator, input, &mut state, vm) {
                    Ok(v) => {
                        scratch.extra_kw_keys.push(key.clone_with_heap(vm.heap));
                        scratch.extra_kw_values.push(v);
                    }
                    Err(ValError::Lines(lines)) => {
                        scratch
                            .errors
                            .extend(lines.into_iter().map(|line| line.with_outer_loc(loc())));
                    }
                    Err(err) => return Err(err),
                }
            }
        }
    }
    // Missing parameters, and the `Field()` defaults the wrapper supplies itself.
    let (scratch, vm) = guard.as_parts_mut();
    let mut missing: Vec<LineError> = Vec::new();
    for index in 0..param_count {
        if scratch.attempted[index] {
            continue;
        }
        let (kind, default) = {
            let param = &params(wrapper_id, vm)[index];
            (
                param.kind,
                param.default.as_ref().map(|default| default.clone_with_heap(vm.heap)),
            )
        };
        match default {
            None => {}
            Some(FieldDefault::Value(value)) => scratch.slots[index] = Some(value),
            Some(FieldDefault::Factory(factory)) => {
                defer_drop!(factory, vm);
                scratch.slots[index] = Some(vm.evaluate_function(factory, ArgValues::Empty)?);
            }
            Some(FieldDefault::Required) => {
                let pairs: Vec<(Value, Value)> = scratch
                    .given_keys
                    .iter()
                    .zip(&scratch.given_values)
                    .map(|(key, value)| (key.clone_with_heap(vm.heap), value.clone_with_heap(vm.heap)))
                    .collect();
                let whole = Dict::from_pairs(pairs, vm)?;
                let whole = Value::Ref(vm.heap.allocate(HeapData::Dict(whole)));
                let name = vm.interns.get_str(params(wrapper_id, vm)[index].name).to_owned();
                let (error, loc) = match kind {
                    ParameterKind::PositionalOnly => (ErrorType::MissingPositionalOnlyArgument {}, index_loc(index)),
                    ParameterKind::KeywordOnly => (ErrorType::MissingKeywordOnlyArgument {}, LocItem::Key(name)),
                    _ => (ErrorType::MissingArgument {}, LocItem::Key(name)),
                };
                missing.push(LineError::owned(error, whole).with_outer_loc(loc));
            }
        }
    }
    // pydantic reports the missing parameters before the surplus and duplicate arguments.
    missing.append(&mut scratch.errors);
    if !missing.is_empty() {
        return Err(ValError::Lines(missing));
    }
    let (scratch, vm) = guard.into_parts();
    let CallScratch {
        positional,
        kw_keys,
        kw_values,
        given_keys,
        given_values,
        mut slots,
        attempted: _,
        extra_positional,
        extra_kw_keys,
        extra_kw_values,
        errors,
    } = scratch;
    for owned in [positional, kw_keys, kw_values, given_keys, given_values] {
        owned.drop_with(vm);
    }
    errors.drop_with(vm);
    // Positional parameters are passed positionally up to the first one left to
    // its own default; the rest, and keyword-only ones, go by name.
    let mut call_positional = Vec::with_capacity(positional_slots + extra_positional.len());
    let mut call_kwargs: Vec<(Value, Value)> = Vec::new();
    let mut positional_run = true;
    for (index, slot) in slots.iter_mut().enumerate() {
        let Some(value) = slot.take() else {
            positional_run = false;
            continue;
        };
        if index < positional_slots && positional_run {
            call_positional.push(value);
        } else {
            call_kwargs.push((Value::InternString(params(wrapper_id, vm)[index].name), value));
        }
    }
    call_positional.extend(extra_positional);
    call_kwargs.extend(extra_kw_keys.into_iter().zip(extra_kw_values));
    Ok((call_positional, call_kwargs))
}
