#![doc = include_str!("../../../README.md")]
// first to include defer_drop macro
mod heap;

mod args;
mod asyncio;
mod builtins;
mod bytecode;
mod exception_private;
mod exception_public;
mod expressions;
mod fstring;
mod function;
mod heap_data;
mod intern;
mod io;
mod modules;
mod namespace;
mod object;
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
    io::{PrintWriter, PrintWriterCallback},
    object::{DictPairs, InvalidInputError, MontyObject},
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
