//! The Python-facing `decimal.Context`, its `flags` / `traps` mappings, the
//! `localcontext()` manager and the session's current context.
//!
//! A context is a mutable heap object (`HeapData::DecimalContext`), shared by
//! reference exactly as in CPython: `getcontext()` returns the same object each time,
//! so `getcontext().prec = 10` changes every later operation. The current context and
//! the three module templates (`DefaultContext`, `BasicContext`, `ExtendedContext`)
//! are session state ([`SessionDecimal`]), carried across REPL feeds and dumps like
//! `random`'s generator; each `import decimal` sees the same template objects.

use std::{fmt::Write, iter};

use smallvec::smallvec;

use super::{
    context::{Conditions, DecContext, MAX_EMAX, MAX_PREC, MIN_EMIN, Rounding, Signal},
    py::{self, ContextMethod},
};
use crate::{
    args::{ArgValues, FromArgs, is_long_int},
    builtins::Builtins,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead},
    types::{Dict, LazyHeapSet, List, PyTrait, Type, allocate_string, iter::collect_iterable, tuple::allocate_tuple},
    value::{EitherStr, Value},
};

/// The heap slots holding the session's decimal contexts (see `Heap::decimal_context`).
///
/// The heap owns one reference to each, so the current context and the templates
/// survive across REPL feeds and dumps with the rest of the session's objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecimalSlot {
    /// The context `getcontext()` returns; a copy of `DefaultContext` when created.
    Current,
    /// `decimal.DefaultContext`.
    Default,
    /// `decimal.BasicContext`.
    Basic,
    /// `decimal.ExtendedContext`.
    Extended,
}

impl DecimalSlot {
    /// The number of slots.
    pub(crate) const COUNT: usize = 4;
}

/// The three module-level template contexts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Template {
    Default,
    Basic,
    Extended,
}

impl Template {
    /// The template's initial settings.
    fn settings(self) -> DecContext {
        match self {
            Self::Default => DecContext::DEFAULT,
            Self::Basic => DecContext::BASIC,
            Self::Extended => DecContext::EXTENDED,
        }
    }

    /// The heap slot holding the template.
    fn slot(self) -> DecimalSlot {
        match self {
            Self::Default => DecimalSlot::Default,
            Self::Basic => DecimalSlot::Basic,
            Self::Extended => DecimalSlot::Extended,
        }
    }
}

/// The heap id of a template context, allocating it on first use.
pub(crate) fn template_id(vm: &mut VM<'_>, template: Template) -> HeapId {
    vm.heap.decimal_context(template.slot()).unwrap_or_else(|| {
        let id = vm.heap.allocate(HeapData::DecimalContext(template.settings()));
        let previous = vm.heap.replace_decimal_context(template.slot(), id);
        debug_assert!(previous.is_none(), "template slot was empty");
        id
    })
}

/// The heap id of the current context, creating it from `DefaultContext` on first use.
pub(crate) fn current_context(vm: &mut VM<'_>) -> HeapId {
    vm.heap.decimal_context(DecimalSlot::Current).unwrap_or_else(|| {
        let default = template_id(vm, Template::Default);
        let mut settings = settings_of(vm, default);
        settings.flags = Conditions::EMPTY;
        let id = vm.heap.allocate(HeapData::DecimalContext(settings));
        let previous = vm.heap.replace_decimal_context(DecimalSlot::Current, id);
        debug_assert!(previous.is_none(), "current slot was empty");
        id
    })
}

/// The settings of the context at `id`.
pub(crate) fn settings_of(vm: &VM<'_>, id: HeapId) -> DecContext {
    match vm.heap.get(id) {
        HeapData::DecimalContext(settings) => *settings,
        _ => unreachable!("decimal context id points at a different object"),
    }
}

/// Mutates the settings of the context at `id`.
fn update_settings(vm: &mut VM<'_>, id: HeapId, update: impl FnOnce(&mut DecContext)) {
    let context = vm.heap.read_as::<DecContext>(id).expect("decimal context id");
    update(context.get_mut(vm.heap));
}

/// Applies an operation's conditions to the context it ran under: every condition
/// sets its flag, and a trapped one raises.
pub(crate) fn apply_status(vm: &mut VM<'_>, id: HeapId, status: Conditions) -> RunResult<()> {
    if status.is_empty() {
        return Ok(());
    }
    let mut traps = Conditions::EMPTY;
    update_settings(vm, id, |settings| {
        settings.flags.merge(status);
        traps = settings.traps;
    });
    match status.trapped(traps) {
        Some((exc_type, message)) => Err(ExcType::decimal_signal(exc_type, message)),
        None => Ok(()),
    }
}

/// `setcontext(ctx)`: makes `ctx` current, copying it first if it is a template.
fn set_current(vm: &mut VM<'_>, id: HeapId) {
    let is_template = [DecimalSlot::Default, DecimalSlot::Basic, DecimalSlot::Extended]
        .into_iter()
        .any(|slot| vm.heap.decimal_context(slot) == Some(id));
    let new_current = if is_template {
        let mut settings = settings_of(vm, id);
        settings.flags = Conditions::EMPTY;
        vm.heap.allocate(HeapData::DecimalContext(settings))
    } else {
        vm.heap.inc_ref(id);
        id
    };
    let old = vm.heap.replace_decimal_context(DecimalSlot::Current, new_current);
    old.map(Value::Ref).drop_with(vm);
}

/// The context id a `Value` refers to, if it is a `decimal.Context`.
pub(crate) fn as_context(value: &Value, vm: &VM<'_>) -> Option<HeapId> {
    match value {
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::DecimalContext(_)) => Some(*id),
        _ => None,
    }
}

/// `decimal.getcontext()`.
pub(crate) fn getcontext(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    args.check_zero_args("_decimal.getcontext", vm.heap)?;
    let id = current_context(vm);
    vm.heap.inc_ref(id);
    Ok(Value::Ref(id))
}

/// `decimal.setcontext(context)`.
pub(crate) fn setcontext(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("_decimal.setcontext", vm.heap)?;
    defer_drop!(value, vm);
    let id = as_context(value, vm).ok_or_else(|| ExcType::type_error("argument must be a context"))?;
    set_current(vm, id);
    Ok(Value::None)
}

/// Arguments of `decimal.localcontext(ctx=None, **kwargs)`.
#[derive(FromArgs)]
#[from_args(name = "localcontext", style = c)]
struct LocalContextArgs {
    #[from_args(default = Value::None)]
    ctx: Value,
    #[from_args(kw_only, default = Value::None)]
    prec: Value,
    #[from_args(kw_only, default = Value::None)]
    rounding: Value,
    #[from_args(kw_only, default = Value::None, static_string = "Emin")]
    emin: Value,
    #[from_args(kw_only, default = Value::None, static_string = "Emax")]
    emax: Value,
    #[from_args(kw_only, default = Value::None)]
    capitals: Value,
    #[from_args(kw_only, default = Value::None)]
    clamp: Value,
    #[from_args(kw_only, default = Value::None)]
    flags: Value,
    #[from_args(kw_only, default = Value::None)]
    traps: Value,
}

/// `decimal.localcontext(ctx=None, **kwargs)`: a manager that installs a copy of
/// `ctx` (or of the current context), with the given attributes changed, for the
/// duration of a `with` block.
pub(crate) fn localcontext(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let LocalContextArgs {
        ctx,
        prec,
        rounding,
        emin,
        emax,
        capitals,
        clamp,
        flags,
        traps,
    } = LocalContextArgs::from_args(args, vm)?;
    let fields = [prec, rounding, emin, emax, capitals, clamp, flags, traps];
    defer_drop!(fields, vm);
    defer_drop!(ctx, vm);
    let source = if matches!(ctx, Value::None) {
        current_context(vm)
    } else {
        as_context(ctx, vm).ok_or_else(|| ExcType::type_error("optional argument must be a context"))?
    };
    let mut settings = settings_of(vm, source);
    apply_fields(&mut settings, fields, vm)?;
    let local = vm.heap.allocate(HeapData::DecimalContext(settings));
    let manager = ContextManager { local, saved: None };
    Ok(Value::Ref(vm.heap.allocate(HeapData::DecimalContextManager(manager))))
}

/// Arguments of `decimal.Context(...)`.
#[derive(FromArgs)]
#[from_args(name = "Context", style = c)]
struct ContextArgs {
    #[from_args(default = Value::None)]
    prec: Value,
    #[from_args(default = Value::None)]
    rounding: Value,
    #[from_args(default = Value::None, static_string = "Emin")]
    emin: Value,
    #[from_args(default = Value::None, static_string = "Emax")]
    emax: Value,
    #[from_args(default = Value::None)]
    capitals: Value,
    #[from_args(default = Value::None)]
    clamp: Value,
    #[from_args(default = Value::None)]
    flags: Value,
    #[from_args(default = Value::None)]
    traps: Value,
}

/// `decimal.Context(prec=None, rounding=None, Emin=None, Emax=None, capitals=None,
/// clamp=None, flags=None, traps=None)`: a new context starting from `DefaultContext`.
pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let ContextArgs {
        prec,
        rounding,
        emin,
        emax,
        capitals,
        clamp,
        flags,
        traps,
    } = ContextArgs::from_args(args, vm)?;
    let fields = [prec, rounding, emin, emax, capitals, clamp, flags, traps];
    defer_drop!(fields, vm);
    let default = template_id(vm, Template::Default);
    let mut settings = settings_of(vm, default);
    settings.flags = Conditions::EMPTY;
    apply_fields(&mut settings, fields, vm)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::DecimalContext(settings))))
}

/// `decimal.IEEEContext(bits)`: the context of an IEEE 754 decimal interchange format.
pub(crate) fn ieee_context(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let bits = args.get_one_arg("IEEEContext", vm.heap)?;
    defer_drop!(bits, vm);
    let bits = match bits {
        Value::Int(n) => *n,
        Value::Bool(b) => i64::from(*b),
        _ => return Err(ExcType::type_error_integer_required()),
    };
    if bits <= 0 || bits > IEEE_CONTEXT_MAX_BITS || bits % 32 != 0 {
        return Err(ExcType::value_error(format!(
            "argument must be a multiple of 32, with a maximum of {IEEE_CONTEXT_MAX_BITS}"
        )));
    }
    let emax = 3 * (1i64 << (bits / 16 + 3));
    let settings = DecContext {
        prec: 9 * (bits / 32) - 2,
        rounding: Rounding::HalfEven,
        emin: 1 - emax,
        emax,
        capitals: true,
        clamp: true,
        flags: Conditions::EMPTY,
        traps: Conditions::EMPTY,
    };
    Ok(Value::Ref(vm.heap.allocate(HeapData::DecimalContext(settings))))
}

/// `decimal.IEEE_CONTEXT_MAX_BITS` on 64-bit builds.
pub(crate) const IEEE_CONTEXT_MAX_BITS: i64 = 512;

/// Applies the eight `Context(...)` / `localcontext(...)` arguments, skipping `None`.
fn apply_fields(settings: &mut DecContext, fields: &[Value; 8], vm: &mut VM<'_>) -> RunResult<()> {
    for (field, value) in ContextField::CONSTRUCTOR_ORDER.into_iter().zip(fields) {
        if !matches!(value, Value::None) {
            match field {
                ContextField::Flags => settings.flags = signal_set_arg(value, vm)?,
                ContextField::Traps => settings.traps = signal_set_arg(value, vm)?,
                _ => field.set(settings, value, vm)?,
            }
        }
    }
    Ok(())
}

/// The settable attributes of a context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextField {
    Prec,
    Rounding,
    Emin,
    Emax,
    Capitals,
    Clamp,
    Flags,
    Traps,
}

impl ContextField {
    /// The order of `Context()`'s parameters.
    const CONSTRUCTOR_ORDER: [Self; 8] = [
        Self::Prec,
        Self::Rounding,
        Self::Emin,
        Self::Emax,
        Self::Capitals,
        Self::Clamp,
        Self::Flags,
        Self::Traps,
    ];

    /// Resolves an attribute name.
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "prec" => Self::Prec,
            "rounding" => Self::Rounding,
            "Emin" => Self::Emin,
            "Emax" => Self::Emax,
            "capitals" => Self::Capitals,
            "clamp" => Self::Clamp,
            "flags" => Self::Flags,
            "traps" => Self::Traps,
            _ => return None,
        })
    }

    /// Reads the attribute; `flags` and `traps` are handled by the caller.
    fn get(self, settings: &DecContext, vm: &VM<'_>) -> Value {
        match self {
            Self::Prec => Value::Int(settings.prec),
            Self::Rounding => allocate_string(settings.rounding.name(), vm.heap),
            Self::Emin => Value::Int(settings.emin),
            Self::Emax => Value::Int(settings.emax),
            Self::Capitals => Value::Int(i64::from(settings.capitals)),
            Self::Clamp => Value::Int(i64::from(settings.clamp)),
            Self::Flags | Self::Traps => unreachable!("signal dicts are allocated by the caller"),
        }
    }

    /// Validates and stores a new value for a scalar attribute, with C `_decimal`'s errors.
    fn set(self, settings: &mut DecContext, value: &Value, vm: &mut VM<'_>) -> RunResult<()> {
        if self == Self::Rounding {
            settings.rounding = py::rounding_arg(value, vm)?;
            return Ok(());
        }
        let n = integer_arg(value, vm)?;
        match self {
            Self::Prec => {
                settings.prec = n
                    .filter(|n| (1..=MAX_PREC).contains(n))
                    .ok_or_else(|| ExcType::value_error("valid range for prec is [1, MAX_PREC]"))?;
            }
            Self::Emin => {
                settings.emin = n
                    .filter(|n| (MIN_EMIN..=0).contains(n))
                    .ok_or_else(|| ExcType::value_error("valid range for Emin is [MIN_EMIN, 0]"))?;
            }
            Self::Emax => {
                settings.emax = n
                    .filter(|n| (0..=MAX_EMAX).contains(n))
                    .ok_or_else(|| ExcType::value_error("valid range for Emax is [0, MAX_EMAX]"))?;
            }
            Self::Capitals => {
                settings.capitals =
                    binary_flag(n).ok_or_else(|| ExcType::value_error("valid values for capitals are 0 or 1"))?;
            }
            Self::Clamp => {
                settings.clamp =
                    binary_flag(n).ok_or_else(|| ExcType::value_error("valid values for clamp are 0 or 1"))?;
            }
            Self::Rounding | Self::Flags | Self::Traps => unreachable!("handled above or by the caller"),
        }
        Ok(())
    }
}

/// An integer attribute value: `OverflowError` for an integer outside `i64` and
/// `TypeError` for anything that is not an `int`, as C's `PyLong_AsSsize_t` raises.
fn integer_arg(value: &Value, vm: &VM<'_>) -> RunResult<Option<i64>> {
    match value {
        Value::Int(n) => Ok(Some(*n)),
        Value::Bool(b) => Ok(Some(i64::from(*b))),
        _ if is_long_int(value, vm) => Err(ExcType::overflow_error("Python int too large to convert to C ssize_t")),
        _ => Err(ExcType::type_error("an integer is required")),
    }
}

/// `0` or `1` as a flag; `None` for anything else.
fn binary_flag(n: Option<i64>) -> Option<bool> {
    match n {
        Some(0) => Some(false),
        Some(1) => Some(true),
        _ => None,
    }
}

/// The signal named by a `SignalDict` key, or C `_decimal`'s `KeyError`.
fn signal_key(key: &Value) -> RunResult<Signal> {
    signal_of(key).ok_or_else(ExcType::decimal_invalid_signal)
}

/// The signal an exception class value stands for.
fn signal_of(value: &Value) -> Option<Signal> {
    match value {
        Value::Builtin(Builtins::ExcType(exc_type)) => Signal::from_exc_type(*exc_type),
        _ => None,
    }
}

/// A `flags=` / `traps=` constructor argument: a list of signals, or a dict
/// mapping all nine signals to truth values.
fn signal_set_arg(value: &Value, vm: &mut VM<'_>) -> RunResult<Conditions> {
    let Value::Ref(id) = value else {
        return Err(ExcType::type_error("argument must be a signal dict"));
    };
    let items: Option<Vec<Value>> = match vm.heap.get(*id) {
        HeapData::List(list) => Some(list.as_slice().iter().map(|v| v.clone_with_heap(vm.heap)).collect()),
        _ => None,
    };
    if let Some(items) = items {
        defer_drop!(items, vm);
        let mut set = Conditions::EMPTY;
        for item in items {
            set.set_signal(signal_key(item)?, true);
        }
        Ok(set)
    } else {
        signal_dict_arg(value, vm)
    }
}

/// A dict assigned to `flags` / `traps`: exactly the nine signals as keys.
/// A `SignalDict` copies the other context's set.
fn signal_dict_arg(value: &Value, vm: &mut VM<'_>) -> RunResult<Conditions> {
    let Value::Ref(id) = value else {
        return Err(ExcType::type_error("argument must be a signal dict"));
    };
    match vm.heap.get(*id) {
        HeapData::DecimalSignalDict(dict) => Ok(dict.read(vm)),
        HeapData::Dict(dict) => {
            if dict.len() != Signal::ALL.len() {
                return Err(ExcType::decimal_invalid_signal_dict());
            }
            let pairs: Vec<(Value, Value)> = dict
                .iter()
                .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                .collect();
            defer_drop!(pairs, vm);
            let mut set = Conditions::EMPTY;
            let mut seen = Conditions::EMPTY;
            for (key, flag) in pairs {
                let signal = signal_of(key).ok_or_else(ExcType::decimal_invalid_signal_dict)?;
                seen.set_signal(signal, true);
                set.set_signal(signal, flag.py_bool(vm)?);
            }
            if Signal::ALL.into_iter().all(|signal| seen.has_signal(signal)) {
                Ok(set)
            } else {
                Err(ExcType::decimal_invalid_signal_dict())
            }
        }
        _ => Err(ExcType::type_error("argument must be a signal dict")),
    }
}

impl HeapItem for DecContext {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, DecContext> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::DecimalContext
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    /// Contexts compare by identity, as in CPython.
    fn py_eq_impl(&self, other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(Some(matches!(other, Value::Ref(id) if *id == self.id())))
    }

    fn py_hash(&self, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        Ok(self.get(vm.heap).write_repr(f)?)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let Some(field) = ContextField::from_name(attr.as_str(vm.interns)) else {
            return Ok(None);
        };
        let value = match field {
            ContextField::Flags | ContextField::Traps => {
                vm.heap.inc_ref(self.id());
                let dict = SignalDict {
                    context: self.id(),
                    traps: field == ContextField::Traps,
                };
                Value::Ref(vm.heap.allocate(HeapData::DecimalSignalDict(dict)))
            }
            _ => field.get(self.get(vm.heap), vm),
        };
        Ok(Some(CallResult::Value(value)))
    }

    fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        defer_drop!(value, vm);
        let Some(field) = ContextField::from_name(name.as_str(vm.interns)) else {
            return Err(ExcType::attribute_error(Type::DecimalContext, name.as_str(vm.interns)));
        };
        let mut settings = *self.get(vm.heap);
        match field {
            ContextField::Flags => settings.flags = signal_dict_arg(value, vm)?,
            ContextField::Traps => settings.traps = signal_dict_arg(value, vm)?,
            _ => field.set(&mut settings, value, vm)?,
        }
        *self.get_mut(vm.heap) = settings;
        Ok(())
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let id = self.id();
        let name = attr.as_str(vm.interns);
        match name {
            "clear_flags" => {
                args.check_zero_args("Context.clear_flags", vm.heap)?;
                self.get_mut(vm.heap).flags = Conditions::EMPTY;
                Ok(CallResult::Value(Value::None))
            }
            "clear_traps" => {
                args.check_zero_args("Context.clear_traps", vm.heap)?;
                self.get_mut(vm.heap).traps = Conditions::EMPTY;
                Ok(CallResult::Value(Value::None))
            }
            "copy" | "__copy__" => {
                args.check_zero_args(
                    if name == "copy" {
                        "Context.copy"
                    } else {
                        "Context.__copy__"
                    },
                    vm.heap,
                )?;
                let settings = *self.get(vm.heap);
                Ok(CallResult::Value(Value::Ref(
                    vm.heap.allocate(HeapData::DecimalContext(settings)),
                )))
            }
            "Etiny" => {
                args.check_zero_args("Context.Etiny", vm.heap)?;
                Ok(CallResult::Value(Value::Int(self.get(vm.heap).etiny())))
            }
            "Etop" => {
                args.check_zero_args("Context.Etop", vm.heap)?;
                Ok(CallResult::Value(Value::Int(self.get(vm.heap).etop())))
            }
            _ => match ContextMethod::from_name(name) {
                Some(method) => py::call_context_method(method, id, args, vm).map(CallResult::Value),
                None => Err(ExcType::attribute_error_method(Type::DecimalContext, attr, args, vm)),
            },
        }
    }
}

/// A context's `flags` or `traps`, a live mutable mapping from signal classes to bools.
///
/// Holds an owned reference to its context; reads and writes go straight through.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct SignalDict {
    /// Owned reference to the `DecimalContext` this mapping views.
    context: HeapId,
    /// `true` for `traps`, `false` for `flags`.
    traps: bool,
}

impl SignalDict {
    /// The context this mapping owns a reference to, for the heap's child walkers.
    pub(crate) fn context_id(&self) -> HeapId {
        self.context
    }

    /// The viewed set.
    fn read(&self, vm: &VM<'_>) -> Conditions {
        let settings = settings_of(vm, self.context);
        if self.traps { settings.traps } else { settings.flags }
    }
}

impl HeapItem for SignalDict {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.context);
    }
}

/// The nine signal classes, in `SignalDict` iteration order.
fn signal_keys() -> impl Iterator<Item = (Signal, Value)> {
    Signal::ALL
        .into_iter()
        .map(|signal| (signal, Value::Builtin(Builtins::ExcType(signal.exc_type()))))
}

impl<'h> HeapObjectRead<'h, SignalDict> {
    /// `update(other)`, as `MutableMapping.update`: another signal dict, a dict, or an
    /// iterable of `(signal, value)` pairs, applied in order.
    fn update(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<()> {
        let pairs: Option<Vec<(Value, Value)>> = match other {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::DecimalSignalDict(dict) => {
                    let set = dict.read(vm);
                    for signal in Signal::ALL {
                        self.set(signal, set.has_signal(signal), vm);
                    }
                    return Ok(());
                }
                HeapData::Dict(dict) => Some(
                    dict.iter()
                        .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                        .collect(),
                ),
                _ => None,
            },
            _ => None,
        };
        if let Some(pairs) = pairs {
            defer_drop!(pairs, vm);
            for (key, flag) in pairs {
                self.set(signal_key(key)?, flag.py_bool(vm)?, vm);
            }
        } else {
            let items = collect_iterable(other, vm)?;
            defer_drop!(items, vm);
            for item in items {
                let pair = collect_iterable(item, vm)?;
                defer_drop!(pair, vm);
                let [key, flag] = pair.as_slice() else {
                    return Err(ExcType::unpack_size_error(2, pair.len()));
                };
                self.set(signal_key(key)?, flag.py_bool(vm)?, vm);
            }
        }
        Ok(())
    }

    /// Writes one signal through to the context.
    fn set(&self, signal: Signal, on: bool, vm: &mut VM<'h>) {
        let SignalDict { context, traps } = *self.get(vm.heap);
        update_settings(vm, context, |settings| {
            if traps {
                settings.traps.set_signal(signal, on);
            } else {
                settings.flags.set_signal(signal, on);
            }
        });
    }

    /// A plain `dict` snapshot of the mapping, as `copy()` returns.
    fn to_dict(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let set = self.get(vm.heap).read(vm);
        let mut dict = Dict::new();
        for (signal, key) in signal_keys() {
            dict.set(key, Value::Bool(set.has_signal(signal)), vm)?;
        }
        Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
    }

    /// A `list` of the keys, values or `(key, value)` items.
    fn listing(&self, part: SignalDictPart, vm: &mut VM<'h>) -> Value {
        let set = self.get(vm.heap).read(vm);
        let items = signal_keys()
            .map(|(signal, key)| {
                let flag = Value::Bool(set.has_signal(signal));
                match part {
                    SignalDictPart::Keys => key,
                    SignalDictPart::Values => flag,
                    SignalDictPart::Items => allocate_tuple(smallvec![key, flag], vm.heap),
                }
            })
            .collect();
        Value::Ref(vm.heap.allocate(HeapData::List(List::new(items))))
    }
}

/// Which part of a `SignalDict` a listing method returns.
#[derive(Clone, Copy)]
enum SignalDictPart {
    Keys,
    Values,
    Items,
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, SignalDict> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::DecimalSignalDict
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        Some(Signal::ALL.len())
    }

    /// Equal to another signal dict, or to a dict holding all nine signals, with the
    /// same truth values; any other non-empty dict is unequal, and `{}` raises
    /// `ValueError` as in CPython.
    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Value::Ref(id) = other else { return Ok(None) };
        let mine = self.get(vm.heap).read(vm);
        let theirs = match vm.heap.get(*id) {
            HeapData::DecimalSignalDict(dict) => dict.read(vm),
            HeapData::Dict(dict) if dict.is_empty() => return Err(ExcType::value_error("invalid signal dict")),
            HeapData::Dict(_) => match signal_dict_arg(other, vm) {
                Ok(set) => set,
                Err(_) => return Ok(Some(false)),
            },
            _ => return Ok(None),
        };
        Ok(Some(
            Signal::ALL
                .into_iter()
                .all(|signal| mine.has_signal(signal) == theirs.has_signal(signal)),
        ))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let set = self.get(vm.heap).read(vm);
        f.write_char('{')?;
        for (i, signal) in Signal::ALL.into_iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            let name: &'static str = signal.exc_type().into();
            let flag = if set.has_signal(signal) { "True" } else { "False" };
            write!(f, "<class '{name}'>:{flag}")?;
        }
        Ok(f.write_char('}')?)
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        let signal = signal_key(key)?;
        Ok(Value::Bool(self.get(vm.heap).read(vm).has_signal(signal)))
    }

    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        defer_drop!(key, vm);
        defer_drop!(value, vm);
        let signal = signal_key(key)?;
        let on = value.py_bool(vm)?;
        self.set(signal, on, vm);
        Ok(())
    }

    fn py_contains_impl(&self, item: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(Some(signal_of(item).is_some()))
    }

    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let keys = self.listing(SignalDictPart::Keys, vm);
        defer_drop!(keys, vm);
        keys.py_iter(vm)
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let value = match attr.as_str(vm.interns) {
            "keys" => {
                args.check_zero_args("keys", vm.heap)?;
                self.listing(SignalDictPart::Keys, vm)
            }
            "values" => {
                args.check_zero_args("values", vm.heap)?;
                self.listing(SignalDictPart::Values, vm)
            }
            "items" => {
                args.check_zero_args("items", vm.heap)?;
                self.listing(SignalDictPart::Items, vm)
            }
            "copy" => {
                args.check_zero_args("copy", vm.heap)?;
                self.to_dict(vm)?
            }
            "get" => {
                let (key, default) = args.get_one_two_args("get", vm.heap)?;
                defer_drop!(key, vm);
                match signal_of(key) {
                    Some(signal) => {
                        default.drop_with(vm);
                        Value::Bool(self.get(vm.heap).read(vm).has_signal(signal))
                    }
                    None => default.unwrap_or(Value::None),
                }
            }
            "update" => {
                if let Some(other) = args.get_zero_one_arg("update", vm.heap)? {
                    defer_drop!(other, vm);
                    self.update(other, vm)?;
                }
                Value::None
            }
            "__delitem__" | "pop" | "popitem" | "clear" => {
                args.drop_with(vm);
                return Err(ExcType::value_error("signal keys cannot be deleted"));
            }
            _ => return Err(ExcType::attribute_error_method(Type::DecimalSignalDict, attr, args, vm)),
        };
        Ok(CallResult::Value(value))
    }
}

/// The manager `localcontext()` returns: `__enter__` installs `local` as the
/// current context and returns it, `__exit__` restores the one it replaced.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ContextManager {
    /// Owned reference to the context installed on entry.
    local: HeapId,
    /// Owned reference to the context replaced on entry, restored on exit.
    saved: Option<HeapId>,
}

impl ContextManager {
    /// The contexts this manager owns references to, for the heap's child walkers.
    pub(crate) fn context_ids(&self) -> impl Iterator<Item = HeapId> {
        iter::once(self.local).chain(self.saved)
    }
}

impl HeapItem for ContextManager {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.extend(self.context_ids());
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, ContextManager> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::DecimalContextManager
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(Some(matches!(other, Value::Ref(id) if *id == self.id())))
    }

    fn py_hash(&self, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_is_context_manager(&self, _vm: &VM<'h>) -> bool {
        true
    }

    fn py_enter(&mut self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let saved = current_context(vm);
        vm.heap.inc_ref(saved);
        let local = self.get(vm.heap).local;
        if let Some(previous) = self.get_mut(vm.heap).saved.replace(saved) {
            vm.heap.dec_ref(previous);
        }
        set_current(vm, local);
        vm.heap.inc_ref(local);
        Ok(CallResult::Value(Value::Ref(local)))
    }

    fn py_exit(&mut self, vm: &mut VM<'h>, _exc: Option<HeapId>) -> RunResult<CallResult> {
        if let Some(saved) = self.get_mut(vm.heap).saved.take() {
            set_current(vm, saved);
            vm.heap.dec_ref(saved);
        }
        Ok(CallResult::Value(Value::None))
    }
}
