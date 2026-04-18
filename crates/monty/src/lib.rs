#![doc = include_str!("../../../README.md")]
// these files first because they include macros for the rest of the crate to use
mod heap;
mod heap_traits;

mod args;
mod asyncio;
mod builtins;
mod bytecode;
mod exception_private;
mod exception_public;
mod expressions;
pub mod fs;
mod fstring;
mod function;
mod heap_data;
mod intern;
mod io;
mod modules;
mod namespace;
mod object;
mod object_json;
mod os;
mod parse;
mod prepare;
mod repl;
mod resource;
mod run;
mod run_progress;
mod signature;
mod sorting;
mod types;
mod value;

#[cfg(feature = "ref-count-return")]
pub use crate::run::RefCountOutput;
pub use crate::{
    exception_private::ExcType,
    exception_public::{CodeLoc, MontyException, StackFrame},
    io::{PrintStream, PrintWriter, PrintWriterCallback},
    object::{DictPairs, InvalidInputError, MontyDate, MontyDateTime, MontyObject, MontyTimeDelta, MontyTimeZone},
    object_json::{JsonMontyArray, JsonMontyObject, JsonMontyPairs},
    os::{OsFunction, dir_stat, file_stat, stat_result, symlink_stat},
    repl::{
        MontyRepl, ReplContinuationMode, ReplFunctionCall, ReplNameLookup, ReplOsCall, ReplProgress,
        ReplResolveFutures, ReplStartError, detect_repl_continuation_mode,
    },
    resource::{
        DEFAULT_MAX_RECURSION_DEPTH, LimitedTracker, NoLimitTracker, ResourceError, ResourceLimits, ResourceTracker,
    },
    run::MontyRun,
    run_progress::{
        ExtFunctionResult, FunctionCall, NameLookup, NameLookupResult, OsCall, ResolveFutures, RunProgress,
    },
};
