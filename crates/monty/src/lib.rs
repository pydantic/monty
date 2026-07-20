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
mod expressions;
mod fstring;
mod function;
mod hash;
mod heap_data;
mod identity;
mod intern;
mod modules;
mod name_map;
mod namespace;
mod object_bridge;
mod os_dispatch;
mod parse;
mod prepare;
mod repl;
mod resource_checks;
mod run;
mod run_progress;
mod sorting;
mod source_map;
mod string_builder;
mod types;
mod value;

pub use monty_types::{
    AssertMessageAnnotations, CodeLoc, CompileOptions, DEFAULT_MAX_RECURSION_DEPTH, DictPairs, ExcData, ExcType,
    ExtFunctionResult, FileMode, GetenvArgs, InvalidInputError, JsonErrorData, LimitedTracker, MkdirCallArgs,
    MontyDate, MontyDateTime, MontyException, MontyFileHandle, MontyObject, MontyPath, MontyTimeDelta, MontyTimeZone,
    MontyType, NameLookupResult, NoLimitTracker, OpenCallArgs, OsFunctionCall, PathBytesDataArgs, PathStringDataArgs,
    PrintStream, PrintWriter, PrintWriterCallback, RenameCallArgs, ResourceError, ResourceLimits, ResourceTracker,
    StackFrame, StringRepr, UnicodeErrorData, UnicodeErrorObject, dir_stat, file_stat, stat_result, symlink_stat,
    unicode_decode_error_msg, utf8_error_reason,
};

#[cfg(feature = "ref-count-return")]
pub use crate::run::RefCountOutput;
pub use crate::{
    repl::{
        MontyRepl, ReplContinuationMode, ReplFunctionCall, ReplNameLookup, ReplOsCall, ReplProgress,
        ReplResolveFutures, ReplStartError, detect_repl_continuation_mode,
    },
    run::MontyRun,
    run_progress::{FunctionCall, NameLookup, OsCall, ResolveFutures, RunProgress},
};
