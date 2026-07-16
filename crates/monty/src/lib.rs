#![doc = include_str!("../README.md")]
// these files first because they include macros for the rest of the crate to use
mod heap;
mod heap_traits;

mod args;
mod asyncio;
mod builtins;
mod bytecode;
mod codecs;
mod exception_private;
mod exception_public;
mod expressions;
pub mod fs;
mod fstring;
mod function;
mod hash;
mod heap_data;
mod intern;
mod io;
mod modules;
mod name_map;
mod namespace;
mod object;
mod os;
mod parse;
mod prepare;
mod repl;
mod resource;
mod run;
mod run_progress;
mod sorting;
mod string_builder;
mod types;
mod value;

#[cfg(feature = "ref-count-return")]
pub use crate::run::RefCountOutput;
pub use crate::{
    exception_private::ExcType,
    exception_public::{
        CodeLoc, ExcData, JsonErrorData, MontyException, StackFrame, UnicodeErrorData, UnicodeErrorObject,
    },
    io::{PrintStream, PrintWriter, PrintWriterCallback},
    object::{
        DictPairs, InvalidInputError, MontyDate, MontyDateTime, MontyFileHandle, MontyObject, MontyTimeDelta,
        MontyTimeZone, MontyType,
    },
    os::{
        GetenvArgs, MkdirCallArgs, MontyPath, OpenCallArgs, OsFunctionCall, PathBytesDataArgs, PathStringDataArgs,
        RenameCallArgs, dir_stat, file_stat, stat_result, symlink_stat,
    },
    repl::{
        MontyRepl, ReplContinuationMode, ReplFunctionCall, ReplNameLookup, ReplOsCall, ReplProgress,
        ReplResolveFutures, ReplStartError, detect_repl_continuation_mode,
    },
    resource::{
        DEFAULT_MAX_RECURSION_DEPTH, LimitedTracker, NoLimitTracker, ResourceError, ResourceLimits, ResourceTracker,
    },
    run::{AssertMessageAnnotations, CompileOptions, MontyRun},
    run_progress::{
        ExtFunctionResult, FunctionCall, NameLookup, NameLookupResult, OsCall, ResolveFutures, RunProgress,
    },
    types::{file::FileMode, str::StringRepr},
};
