//! Python implementations of interpreter functions compiled on first use.
//!
//! Internal functions use ordinary VM frames and suspension but are hidden from
//! tracebacks and namespace introspection. Private native helpers arrive as
//! explicit arguments, never through user globals or importable modules.

use monty_types::CompileOptions;
use serde::{Deserialize, Serialize};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{Compiler, VM},
    defer_drop,
    exception_private::RunResult,
    intern::{CompileInterns, FunctionId, Interns},
    name_map::NameMap,
    parse::parse_with_interner,
    prepare::prepare_with_existing_names,
    types::lru_cache,
    value::Value,
};

/// Selects a trusted implementation whose ordinary function ID is cached per session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) enum FrozenFunction {
    /// Suspendable fold loop behind `functools.reduce`.
    FunctoolsReduce,
    /// Store a cached call's result only after it returns successfully.
    CacheMiss,
}

impl FrozenFunction {
    /// Prepares an unpublished implementation using this session's intern IDs.
    /// The caller must admit the frame before committing the overlay and caching its ID.
    pub(crate) fn compile(self, interns: &Interns) -> RunResult<(FunctionId, CompileInterns<'_>)> {
        let source = self.source();
        let mut overlay = CompileInterns::for_frozen(interns);
        let index = overlay.functions_len();
        let mut globals = NameMap::new();
        let nodes =
            parse_with_interner(source, "<monty-frozen>", &mut overlay).map_err(|e| e.into_run_error(source))?;
        let nodes = prepare_with_existing_names(nodes, &overlay, &mut globals).map_err(|e| e.into_run_error(source))?;
        Compiler::compile_module(&nodes, &mut overlay, &globals, CompileOptions::default())
            .map_err(|e| e.into_run_error(source))?;
        assert_eq!(
            overlay.functions_len(),
            index + 1,
            "frozen source must define one function"
        );
        // Successful compilation checked the bytecode's function-ID limit.
        let id = FunctionId::from_index(u16::try_from(index).expect("compiled function ID fits u16"));
        Ok((id, overlay))
    }

    /// Self-contained bodies: any global slot would address the user's module.
    fn source(self) -> &'static str {
        match self {
            Self::FunctoolsReduce => {
                r"
def __monty_functools_reduce(function, iterator, accumulator, next_, stop_iteration):
    while True:
        try:
            item = next_(iterator)
        except stop_iteration:
            return accumulator
        accumulator = function(accumulator, item)
"
            }
            Self::CacheMiss => {
                r"
def __monty_cache_miss(store, cache, key, function, args, kwargs):
    result = function(*args, **kwargs)
    store(cache, key, result)
    return result
"
            }
        }
    }
}

/// Native primitives passed directly to internal Python functions.
/// These identities and their signatures are part of the captured bytecode's ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum FrozenFunctions {
    /// Insert a successful result into a cache wrapper.
    Store,
}

/// Dispatches private helpers using the same argument ownership rules as public natives.
pub(crate) fn call_helper(function: FrozenFunctions, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    match function {
        FrozenFunctions::Store => {
            let StoreArgs { cache, key, value } = StoreArgs::from_args(args, vm)?;
            defer_drop!(cache, vm);
            defer_drop!(value, vm);
            lru_cache::store_result(cache, key, value, vm)?;
            Ok(Value::None)
        }
    }
}

/// Arguments to the private cache insertion primitive.
#[derive(FromArgs)]
#[from_args(name = "store")]
struct StoreArgs {
    #[from_args(pos_only)]
    cache: Value,
    #[from_args(pos_only)]
    key: Value,
    #[from_args(pos_only)]
    value: Value,
}
