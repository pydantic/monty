//! Public data types shared between the `monty` interpreter and its host-side
//! crates (`monty-proto`, `monty-pool`, the Python/JS bindings).
//!
//! This crate contains only owned, heap-free boundary types — values
//! ([`MontyObject`]), exceptions ([`MontyException`]), OS-call payloads
//! ([`OsFunctionCall`]), resource limits ([`ResourceLimits`]) and print
//! plumbing — with **no interpreter implementation**. Hosts that only spawn
//! `monty` worker subprocesses depend on this crate instead of `monty`, so
//! their binaries never link the interpreter itself.

pub mod args;
mod builtins;
mod exceptions;
mod file_mode;
pub mod format;
mod io;
mod object;
mod os;
mod resource;
mod results;
mod run_options;

pub use crate::{
    builtins::BuiltinsFunctions,
    exceptions::{
        CodeLoc, ExcData, ExcType, JsonErrorData, MontyException, StackFrame, UnicodeErrorData, UnicodeErrorObject,
        unicode_decode_error_msg,
    },
    file_mode::FileMode,
    format::{FormatFloat, StringRepr, bytes_repr, bytes_repr_fmt, string_repr_fmt, utf8_error_reason},
    io::{PrintStream, PrintWriter, PrintWriterCallback},
    object::{
        ConversionError, DictPairs, InvalidInputError, MontyDate, MontyDateTime, MontyFileHandle, MontyObject,
        MontyTimeDelta, MontyTimeZone, MontyType,
    },
    os::{
        GetenvArgs, MkdirCallArgs, MontyPath, OpenCallArgs, OsFunctionCall, PathBytesDataArgs, PathStringDataArgs,
        RenameCallArgs, dir_stat, file_stat, stat_result, symlink_stat,
    },
    resource::{
        DEFAULT_MAX_RECURSION_DEPTH, LARGE_RESULT_THRESHOLD, LimitedTracker, NoLimitTracker, ResourceError,
        ResourceLimits, ResourceTracker,
    },
    results::{ExtFunctionResult, NameLookupResult},
    run_options::{AssertMessageAnnotations, CompileOptions},
};
