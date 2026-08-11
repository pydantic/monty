//! Implementation of the `collections` module.
//!
//! The module surface (the factories and their argument handling) lives here;
//! the runtime behaviour of the two dict-backed types lives in the [`counter`]
//! and [`defaultdict`] submodules, keeping it out of `types/dict.rs`.
//!
//! Monty exposes only the subset of CPython's `collections` that it actually
//! implements — currently `deque` (a type) and the factory functions
//! `namedtuple`, `defaultdict`, and `Counter`. `OrderedDict`, `ChainMap` and
//! the `User*` classes are not (see `limitations/collections.md`).
//!
//! `deque` lives entirely in its runtime type (`crate::types::Deque`).
//! `namedtuple` is a factory: [`namedtuple`] validates its arguments and
//! allocates a [`NamedTupleClass`], the callable class object whose instances
//! are ordinary [`NamedTuple`](crate::types::NamedTuple)s. `defaultdict` and
//! `Counter` reuse the `dict` runtime type: [`defaultdict`] and [`counter`]
//! build a `dict` and tag it via `Dict::make_defaultdict` / `make_counter`
//! rather than introducing a new heap type.

pub(crate) mod counter;
pub(crate) mod defaultdict;

use std::iter::once;

use ruff_python_stdlib::identifiers::is_identifier;

use self::counter::counter_update;
use crate::{
    args::{ArgValues, FromArgs},
    builtins::Builtins,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    types::{
        Dict, Module, NamedTupleClass, PyTrait, Type,
        iter::collect_owned_iterable,
        str::{StringRepr, str_isidentifier},
    },
    value::{EitherStr, Value},
};

/// Creates the `collections` module and allocates it on the heap.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Collections);

    module.set_attr(StaticStrings::Deque, Value::Builtin(Builtins::Type(Type::Deque)), vm);
    module.set_attr(
        StaticStrings::Namedtuple,
        Value::ModuleFunction(super::ModuleFunctions::Collections(CollectionsFunctions::Namedtuple)),
        vm,
    );
    // Exposed as the type objects themselves (like `deque`), not as factory
    // functions, so `type(d) is defaultdict` and `isinstance(c, Counter)` hold.
    module.set_attr(
        StaticStrings::Defaultdict,
        Value::Builtin(Builtins::Type(Type::DefaultDict)),
        vm,
    );
    module.set_attr(
        StaticStrings::Counter,
        Value::Builtin(Builtins::Type(Type::Counter)),
        vm,
    );

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// The callable functions exposed by the `collections` module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum CollectionsFunctions {
    Namedtuple,
}

/// Dispatches a `collections` module function call.
pub(super) fn call(vm: &mut VM<'_>, function: CollectionsFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        CollectionsFunctions::Namedtuple => namedtuple(vm, args),
    }
}

/// The `collections.Counter(iterable_or_mapping=None, /, **kwargs)` constructor.
/// Construction is `Counter().update(...)`: a mapping adds its counts, any
/// other iterable counts occurrences, and keyword arguments add counts.
///
/// Reached via [`Type::call`](crate::types::Type::call) — `collections.Counter`
/// is exposed as the type object, so `type(c) is Counter` holds.
pub(crate) fn counter_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (mut pos, kwargs) = args.into_parts();
    let total = pos.len();
    let source = (total > 0).then(|| pos.next().expect("len checked"));

    // Counter accepts at most one positional (the iterable/mapping).
    if total > 1 {
        source.drop_with(vm);
        pos.drop_with(vm);
        kwargs.drop_with(vm);
        return Err(ExcType::type_error_too_many_positional_range(
            "Counter.__init__",
            1,
            2,
            total + 1,
            0,
        ));
    }

    let mut dict = Dict::new();
    dict.make_counter();
    let value = Value::Ref(vm.heap.allocate(HeapData::Dict(dict)));
    let Value::Ref(dict_id) = value else {
        unreachable!("just allocated a ref");
    };
    if let Err(e) = counter_update(dict_id, source, kwargs, false, vm) {
        value.drop_with(vm);
        return Err(e);
    }
    Ok(value)
}

/// The `collections.defaultdict(default_factory=None, /, *args, **kwargs)`
/// constructor. The first positional argument is the `default_factory` (a
/// callable or `None`); any remaining arguments initialise the dict exactly like
/// `dict(...)`.
///
/// Reached via [`Type::call`](crate::types::Type::call) — `collections.defaultdict`
/// is exposed as the type object, so `type(d) is defaultdict` holds.
pub(crate) fn defaultdict_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (mut pos, kwargs) = args.into_parts();
    let factory_arg = (pos.len() > 0).then(|| pos.next().expect("len checked"));

    let factory = match factory_arg {
        None | Some(Value::None) => None,
        Some(value) if value.is_callable(vm.heap) => Some(value),
        Some(value) => {
            value.drop_with(vm);
            pos.drop_with(vm);
            kwargs.drop_with(vm);
            return Err(ExcType::type_error("first argument must be callable or None"));
        }
    };

    // Everything after the factory initialises the dict via the normal `dict`
    // constructor, so `defaultdict`'s init errors match `dict`'s.
    let rest: Vec<Value> = pos.collect();
    let rest_args = if rest.is_empty() && kwargs.is_empty() {
        ArgValues::Empty
    } else {
        ArgValues::ArgsKargs { args: rest, kwargs }
    };
    let dict_value = match Dict::init(vm, rest_args) {
        Ok(value) => value,
        Err(e) => {
            if let Some(factory) = factory {
                factory.drop_with(vm);
            }
            return Err(e);
        }
    };

    let Value::Ref(dict_id) = dict_value else {
        unreachable!("Dict::init returns a dict ref");
    };
    let HeapReadOutput::Dict(mut dict) = vm.heap.read(dict_id) else {
        unreachable!("Dict::init returns a dict");
    };
    dict.get_mut(vm.heap).make_defaultdict(factory);
    Ok(dict_value)
}

/// Arguments for `collections.namedtuple`.
///
/// CPython's `namedtuple` is a pure-Python `def`, so fields stay raw `Value`
/// (`def` binding never type-checks) and coercion happens in the body.
#[derive(FromArgs)]
#[from_args(name = "namedtuple", style = def)]
struct NamedtupleArgs {
    #[from_args(static_string = "Typename")]
    typename: Value,
    #[from_args(static_string = "FieldNames")]
    field_names: Value,
    #[from_args(kw_only, default = Value::Bool(false), static_string = "Rename")]
    rename: Value,
    #[from_args(kw_only, default = Value::None, static_string = "Defaults")]
    defaults: Value,
    #[from_args(kw_only, default = Value::None, static_string = "ModuleKwarg")]
    module: Value,
}

/// The `collections.namedtuple(typename, field_names, *, rename, defaults,
/// module)` factory. Validates the type/field names against CPython's rules
/// and allocates a [`NamedTupleClass`].
///
/// Divergence from CPython: non-string `typename`/`field_names` elements are not
/// `str()`-coerced beyond what the checks below require.
fn namedtuple(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let NamedtupleArgs {
        typename,
        field_names,
        rename,
        defaults,
        module,
    } = NamedtupleArgs::from_args(args, vm)?;
    defer_drop!(typename, vm);
    defer_drop!(field_names, vm);
    defer_drop!(rename, vm);
    defer_drop!(defaults, vm);
    defer_drop!(module, vm);

    // CPython applies `str(typename)` before validating, so any object whose
    // `__str__` yields a valid identifier is accepted. `validate_names` below
    // then rejects the non-identifier cases with CPython's own message.
    let type_name = if let Some(name) = typename.as_either_str(vm.heap) {
        name
    } else {
        let as_str = typename.py_str(vm)?;
        let owned = as_str.to_str(vm).map(str::to_owned);
        as_str.drop_with(vm);
        EitherStr::from(owned?)
    };

    // CPython tests `if rename:`, so any truthy value enables it — not just `True`.
    let rename = rename.py_bool(vm)?;
    let mut names = parse_field_names(field_names, vm)?;
    if rename {
        apply_rename(&mut names);
    }
    validate_names(type_name.as_str(vm.interns), &names, rename)?;

    // Collect defaults (rightmost fields), then validate the count.
    let default_values = collect_defaults(defaults, vm)?;
    if default_values.len() > names.len() {
        default_values.drop_with(vm);
        return Err(ExcType::type_error("Got more default values than field names"));
    }

    // CPython substitutes the calling module's `__name__` for a `None` module, and
    // otherwise stores the argument unvalidated (it need not be a string).
    let module = match module {
        Value::None => Value::InternString(StaticStrings::DunderMain.into()),
        other => other.clone_with_heap(vm.heap),
    };

    let field_names: Vec<EitherStr> = names.into_iter().map(EitherStr::Heap).collect();
    let class = NamedTupleClass::new(type_name, field_names, default_values, module);
    Ok(Value::Ref(vm.heap.allocate(HeapData::NamedTupleClass(Box::new(class)))))
}

/// Parses the `field_names` argument into owned strings.
///
/// A single string is split on commas and whitespace (`'x y'`, `'x,y'`,
/// `'x, y'` all yield `['x', 'y']`); any other value is iterated and each
/// element is `str()`-converted, matching CPython's `list(map(str, ...))`.
fn parse_field_names(field_names: &Value, vm: &mut VM<'_>) -> RunResult<Vec<String>> {
    if let Some(text) = field_names.as_either_str(vm.heap) {
        Ok(text
            .as_str(vm.interns)
            .replace(',', " ")
            .split_whitespace()
            .map(str::to_owned)
            .collect())
    } else {
        let items = collect_owned_iterable::<Vec<Value>>(field_names.clone_with_heap(vm.heap), vm)?;
        let mut names = Vec::with_capacity(items.len());
        // A failing `str()` (a field's `__str__` raising) must release the
        // current item *and* the un-iterated remainder; the guards cover both.
        let iter = items.into_iter();
        defer_drop_mut!(iter, vm);
        for item in iter.by_ref() {
            defer_drop!(item, vm);
            let as_str = item.py_str(vm)?;
            defer_drop!(as_str, vm);
            names.push(as_str.to_str(vm)?.to_owned());
        }
        Ok(names)
    }
}

/// Collects the `defaults` argument (an iterable, or `None`) into values.
fn collect_defaults(defaults: &Value, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
    if matches!(defaults, Value::None) {
        Ok(Vec::new())
    } else {
        collect_owned_iterable::<Vec<Value>>(defaults.clone_with_heap(vm.heap), vm)
    }
}

/// Rewrites invalid field names to positional placeholders (`_0`, `_1`, ...),
/// matching CPython's `rename=True` behaviour.
///
/// A name is replaced when it is not a valid identifier, is a keyword, starts
/// with an underscore, or duplicates an earlier name. Duplicate detection uses
/// the original names, so `['x', 'x']` becomes `['x', '_1']`.
fn apply_rename(names: &mut [String]) {
    let mut seen: Vec<String> = Vec::with_capacity(names.len());
    for (index, name) in names.iter_mut().enumerate() {
        let invalid =
            !str_isidentifier(name) || !is_identifier(name) || name.starts_with('_') || seen.iter().any(|s| s == name);
        seen.push(name.clone());
        if invalid {
            *name = format!("_{index}");
        }
    }
}

/// Validates the type name and field names against CPython's namedtuple rules,
/// in CPython's order: identifier/keyword checks over `[typename] + fields`
/// first, then the underscore/duplicate checks over the fields.
///
/// When `rename` is true the underscore check is skipped, matching CPython —
/// `rename=True` deliberately produces `_0`, `_1`, ... placeholders.
fn validate_names(type_name: &str, field_names: &[String], rename: bool) -> RunResult<()> {
    for name in once(type_name).chain(field_names.iter().map(String::as_str)) {
        if !str_isidentifier(name) {
            return Err(ExcType::value_error(format!(
                "Type names and field names must be valid identifiers: {}",
                repr_name(name)
            )));
        }
        // `str.isidentifier()` accepts keywords, but ruff's `is_identifier`
        // rejects them — so an identifier ruff rejects is exactly a keyword.
        if !is_identifier(name) {
            return Err(ExcType::value_error(format!(
                "Type names and field names cannot be a keyword: {}",
                repr_name(name)
            )));
        }
    }

    let mut seen: Vec<&str> = Vec::with_capacity(field_names.len());
    for name in field_names {
        if name.starts_with('_') && !rename {
            return Err(ExcType::value_error(format!(
                "Field names cannot start with an underscore: {}",
                repr_name(name)
            )));
        }
        if seen.contains(&name.as_str()) {
            return Err(ExcType::value_error(format!(
                "Encountered duplicate field name: {}",
                repr_name(name)
            )));
        }
        seen.push(name);
    }
    Ok(())
}

/// Renders a name as a Python string repr for the `%r` in CPython's namedtuple
/// error messages. Uses the runtime's [`StringRepr`], so a name containing a
/// quote or escape is quoted and escaped like CPython (`a'b` → `"a'b"`) rather
/// than wrapped in bare single quotes (`'a'b'`).
fn repr_name(name: &str) -> StringRepr<'_> {
    StringRepr(name)
}
