//! Implementation of the `sys` module.
//!
//! `sys` is attribute-only in production builds: every name below is a
//! constant fixed at module creation, since the sandbox has no interpreter
//! state a program is allowed to reach. What Monty exposes is limited to
//! values that are true *of Monty* — the Python version it targets, the
//! properties of the `f64` it stores floats in, the Unicode range, and the
//! fact that it has no install tree, no `__pycache__` and no command line.
//! Structseqs describing CPython's C implementation (`hash_info`, `int_info`,
//! `thread_info`) are deliberately absent rather than fabricated; see
//! `limitations/sys.md`.
//!
//! Under the `test-hooks` feature one callable is also exposed:
//! - `setrecursionlimit(n)`: tighten the active recursion ceiling so fixtures
//!   can simulate Monty's lower default depth on CPython too. Only allows
//!   *lowering* the host-configured ceiling — see [`SysFunctions`].

use smallvec::SmallVec;

#[cfg(feature = "test-hooks")]
use crate::{
    args::ArgValues,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    modules::ModuleFunctions,
};
use crate::{
    bytecode::VM,
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    types::{List, Module, NamedTuple, allocate_string, allocate_tuple, long_int::INT_MAX_STR_DIGITS},
    value::{Marker, Value},
};

/// `sys.hexversion` for the version Monty reports: `3.14.0` final, encoded as
/// CPython encodes it — `major << 24 | minor << 16 | micro << 8 | level << 4 | serial`.
const MONTY_HEXVERSION: i64 = 0x030E_00F0;

/// `sys.api_version` — the CPython 3.14 C API version.
///
/// Monty has no C API; the number is reported so version-gated code reads the
/// value it expects from the Python version Monty targets.
const CPYTHON_API_VERSION: i64 = 1013;

/// `sys.maxsize`, pinned to the 64-bit value rather than the host's `isize::MAX`.
///
/// Monty behaves identically on every target, including 32-bit wasm where the
/// real container ceiling is lower — and resource limits bind long before either.
const MAXSIZE: i64 = i64::MAX;

/// `sys.maxunicode` — the largest code point, `U+10FFFF`.
const MAXUNICODE: i64 = 0x0010_FFFF;

/// The modules Monty can import, in the sorted order CPython uses for its own
/// `sys.builtin_module_names`. Every Monty module is compiled into the
/// interpreter, so this is the whole importable set rather than a C-extension
/// subset — keep it in step with [`StandardLib`](super::StandardLib).
const BUILTIN_MODULE_NAMES: &[StaticStrings] = &[
    StaticStrings::Asyncio,
    StaticStrings::Base64,
    StaticStrings::Binascii,
    StaticStrings::Collections,
    StaticStrings::Dataclasses,
    StaticStrings::Datetime,
    StaticStrings::Functools,
    #[cfg(feature = "test-hooks")]
    StaticStrings::Gc,
    StaticStrings::Itertools,
    StaticStrings::Json,
    StaticStrings::Math,
    StaticStrings::Os,
    StaticStrings::Pathlib,
    StaticStrings::Random,
    StaticStrings::Re,
    StaticStrings::Sys,
    StaticStrings::Typing,
    StaticStrings::Unicodedata,
];

/// Functions exposed by the `sys` module under the `test-hooks` feature.
///
/// Production builds keep `sys` attribute-only; this enum exists so fixtures
/// (and only fixtures) can call back into Monty internals that production
/// sandbox code must never reach.
#[cfg(feature = "test-hooks")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum SysFunctions {
    /// `sys.setrecursionlimit(n)` — tightens the live recursion ceiling to
    /// `n`. Only allows lowering; attempting to raise raises `ValueError`.
    Setrecursionlimit,
}

/// Creates the `sys` module and allocates it on the heap.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Sys);

    // Interpreter identity. `platform` is "monty" rather than the host OS, which
    // the sandbox never reveals.
    module.set_attr(StaticStrings::Platform, StaticStrings::Monty.into(), vm);
    module.set_attr(StaticStrings::Version, StaticStrings::MontyVersionString.into(), vm);
    module.set_attr(StaticStrings::VersionInfo, version_info(vm), vm);
    module.set_attr(StaticStrings::Hexversion, Value::Int(MONTY_HEXVERSION), vm);
    module.set_attr(StaticStrings::ApiVersion, Value::Int(CPYTHON_API_VERSION), vm);
    module.set_attr(StaticStrings::Copyright, StaticStrings::MontyCopyright.into(), vm);
    module.set_attr(StaticStrings::BuiltinModuleNames, builtin_module_names(vm), vm);
    module.set_attr(StaticStrings::Argv, argv(vm), vm);

    // Numeric and text limits of the value representations Monty actually uses.
    module.set_attr(StaticStrings::Maxsize, Value::Int(MAXSIZE), vm);
    module.set_attr(StaticStrings::Maxunicode, Value::Int(MAXUNICODE), vm);
    module.set_attr(StaticStrings::Byteorder, StaticStrings::Little.into(), vm);
    module.set_attr(StaticStrings::FloatInfo, float_info(vm), vm);
    module.set_attr(StaticStrings::FloatReprStyle, StaticStrings::Short.into(), vm);

    // The sandbox has no install tree, no bytecode cache and no ABI. CPython
    // documents the empty string for a path it cannot determine, so these report
    // "unknown" instead of raising; `prefix == base_prefix` also answers the
    // usual "am I in a virtualenv?" test correctly.
    module.set_attr(StaticStrings::Executable, StaticStrings::EmptyString.into(), vm);
    module.set_attr(StaticStrings::Prefix, StaticStrings::EmptyString.into(), vm);
    module.set_attr(StaticStrings::ExecPrefix, StaticStrings::EmptyString.into(), vm);
    module.set_attr(StaticStrings::BasePrefix, StaticStrings::EmptyString.into(), vm);
    module.set_attr(StaticStrings::BaseExecPrefix, StaticStrings::EmptyString.into(), vm);
    module.set_attr(StaticStrings::Platlibdir, StaticStrings::Lib.into(), vm);
    module.set_attr(StaticStrings::Abiflags, StaticStrings::EmptyString.into(), vm);
    module.set_attr(StaticStrings::DontWriteBytecode, Value::Bool(true), vm);
    module.set_attr(StaticStrings::PycachePrefix, Value::None, vm);
    module.set_attr(StaticStrings::Flags, flags(vm), vm);

    // sys.stdout / sys.stderr - markers for standard output/error
    module.set_attr(StaticStrings::Stdout, Value::Marker(Marker(StaticStrings::Stdout)), vm);
    module.set_attr(StaticStrings::Stderr, Value::Marker(Marker(StaticStrings::Stderr)), vm);

    // Test-only callables — see the module-level docs and the
    // [`test-hooks`] feature gate.
    #[cfg(feature = "test-hooks")]
    module.set_attr(
        StaticStrings::Setrecursionlimit,
        Value::ModuleFunction(ModuleFunctions::Sys(SysFunctions::Setrecursionlimit)),
        vm,
    );

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Builds `sys.version_info`: `(major=3, minor=14, micro=0, releaselevel='final', serial=0)`.
fn version_info(vm: &VM<'_>) -> Value {
    let named_tuple = NamedTuple::new(
        StaticStrings::SysVersionInfo,
        vec![
            StaticStrings::Major.into(),
            StaticStrings::Minor.into(),
            StaticStrings::Micro.into(),
            StaticStrings::Releaselevel.into(),
            StaticStrings::Serial.into(),
        ],
        vec![
            Value::Int(3),
            Value::Int(14),
            Value::Int(0),
            Value::InternString(StaticStrings::Final.into()),
            Value::Int(0),
        ],
    );
    Value::Ref(vm.heap.allocate(HeapData::NamedTuple(Box::new(named_tuple))))
}

/// Builds `sys.argv`, holding the script name and nothing else.
///
/// Monty runs no command line, so there are no arguments after `argv[0]`;
/// host-supplied ones are not wired up yet. The name is the script's final
/// path component, the same basis as `__file__`, so a host path never reaches
/// the sandbox. Like CPython's, the list is mutable — but Monty builds a fresh
/// module per `import`, so edits do not survive one (see
/// `limitations/modules.md`).
fn argv(vm: &VM<'_>) -> Value {
    let script = allocate_string(vm.env.script_basename(), vm.heap);
    Value::Ref(vm.heap.allocate(HeapData::List(List::new(vec![script]))))
}

/// Builds `sys.float_info` from Rust's `f64` constants.
///
/// Monty stores every float as an `f64`, so each field is the IEEE 754
/// binary64 property CPython reports for its own C `double`. `rounds` is `1`,
/// the `FLT_ROUNDS` code for round-to-nearest, which is the only mode Monty
/// can be in — nothing in the sandbox can change the rounding direction.
fn float_info(vm: &VM<'_>) -> Value {
    let named_tuple = NamedTuple::new(
        StaticStrings::SysFloatInfo,
        vec![
            StaticStrings::Max.into(),
            StaticStrings::MaxExp.into(),
            StaticStrings::Max10Exp.into(),
            StaticStrings::Min.into(),
            StaticStrings::MinExp.into(),
            StaticStrings::Min10Exp.into(),
            StaticStrings::Dig.into(),
            StaticStrings::MantDig.into(),
            StaticStrings::Epsilon.into(),
            StaticStrings::Radix.into(),
            StaticStrings::Rounds.into(),
        ],
        vec![
            Value::Float(f64::MAX),
            Value::Int(i64::from(f64::MAX_EXP)),
            Value::Int(i64::from(f64::MAX_10_EXP)),
            Value::Float(f64::MIN_POSITIVE),
            Value::Int(i64::from(f64::MIN_EXP)),
            Value::Int(i64::from(f64::MIN_10_EXP)),
            Value::Int(i64::from(f64::DIGITS)),
            Value::Int(i64::from(f64::MANTISSA_DIGITS)),
            Value::Float(f64::EPSILON),
            Value::Int(i64::from(f64::RADIX)),
            Value::Int(1),
        ],
    );
    Value::Ref(vm.heap.allocate(HeapData::NamedTuple(Box::new(named_tuple))))
}

/// Builds `sys.flags` — the switches the interpreter was started with.
///
/// Monty is started with none, so every switch reads `0`/`False`. Two fields
/// are not merely "unset" but describe the sandbox: `dont_write_bytecode` is
/// `1` to agree with `sys.dont_write_bytecode`, and `hash_randomization` is
/// `0` because Monty seeds no hashes. `int_max_str_digits` reports the limit
/// Monty actually enforces, though it has no `sys.set_int_max_str_digits` to
/// change it.
fn flags(vm: &VM<'_>) -> Value {
    let named_tuple = NamedTuple::new(
        StaticStrings::SysFlags,
        vec![
            StaticStrings::Debug.into(),
            StaticStrings::Inspect.into(),
            StaticStrings::Interactive.into(),
            StaticStrings::Optimize.into(),
            StaticStrings::DontWriteBytecode.into(),
            StaticStrings::NoUserSite.into(),
            StaticStrings::NoSite.into(),
            StaticStrings::IgnoreEnvironment.into(),
            StaticStrings::Verbose.into(),
            StaticStrings::BytesWarning.into(),
            StaticStrings::Quiet.into(),
            StaticStrings::HashRandomization.into(),
            StaticStrings::Isolated.into(),
            StaticStrings::DevMode.into(),
            StaticStrings::Utf8Mode.into(),
            StaticStrings::WarnDefaultEncoding.into(),
            StaticStrings::SafePath.into(),
            StaticStrings::IntMaxStrDigits.into(),
        ],
        vec![
            Value::Int(0),      // debug
            Value::Int(0),      // inspect
            Value::Int(0),      // interactive
            Value::Int(0),      // optimize
            Value::Int(1),      // dont_write_bytecode
            Value::Int(0),      // no_user_site
            Value::Int(0),      // no_site
            Value::Int(0),      // ignore_environment
            Value::Int(0),      // verbose
            Value::Int(0),      // bytes_warning
            Value::Int(0),      // quiet
            Value::Int(0),      // hash_randomization
            Value::Int(0),      // isolated
            Value::Bool(false), // dev_mode
            Value::Int(0),      // utf8_mode
            Value::Int(0),      // warn_default_encoding
            Value::Bool(false), // safe_path
            int_max_str_digits(),
        ],
    );
    Value::Ref(vm.heap.allocate(HeapData::NamedTuple(Box::new(named_tuple))))
}

/// `sys.flags.int_max_str_digits`, widened from the limit the runtime enforces.
fn int_max_str_digits() -> Value {
    i64::try_from(INT_MAX_STR_DIGITS).map_or(Value::Int(i64::MAX), Value::Int)
}

/// Builds the `sys.builtin_module_names` tuple from [`BUILTIN_MODULE_NAMES`].
fn builtin_module_names(vm: &VM<'_>) -> Value {
    let names: SmallVec<_> = BUILTIN_MODULE_NAMES
        .iter()
        .map(|name| Value::InternString((*name).into()))
        .collect();
    allocate_tuple(names, vm.heap)
}

/// Dispatches a `sys` module function call.
///
/// Only present under the `test-hooks` feature — production builds register
/// no callables on the `sys` module, so this dispatcher would have nothing
/// to do.
#[cfg(feature = "test-hooks")]
pub(super) fn call(vm: &mut VM<'_>, function: SysFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        SysFunctions::Setrecursionlimit => setrecursionlimit(vm, args),
    }
}

/// `sys.setrecursionlimit(n)` — tightens the live recursion ceiling.
///
/// Differs from CPython in one safety-critical way: the limit may only be
/// *lowered*, never raised. Sandboxed code raising the ceiling would let it
/// escape the host-configured depth bound that protects the Rust call stack
/// from overflow inside recursive type machinery (repr, eq, hash, json
/// dump, etc.). Attempts to raise raise `ValueError` with a message
/// pointing at the current cap.
#[cfg(feature = "test-hooks")]
fn setrecursionlimit(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("sys.setrecursionlimit", vm.heap)?;
    let Value::Int(limit) = arg else {
        arg.drop_with(vm);
        return Err(ExcType::type_error("sys.setrecursionlimit() argument must be int"));
    };
    let Ok(new_limit) = usize::try_from(limit) else {
        return Err(ExcType::value_error("recursion limit must be greater or equal than 1"));
    };
    if new_limit == 0 {
        return Err(ExcType::value_error("recursion limit must be greater or equal than 1"));
    }
    match vm.heap.tracker.lower_recursion_limit(new_limit) {
        Ok(()) => Ok(Value::None),
        Err(current) => Err(ExcType::value_error(format!(
            "sys.setrecursionlimit: cannot raise above current limit {current} (sandbox only allows lowering)"
        ))),
    }
}
