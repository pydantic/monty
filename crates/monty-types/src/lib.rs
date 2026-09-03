#![doc = include_str!("../README.md")]

/// The monty version this build was compiled as.
pub const MONTY_VERSION: &str = env!("CARGO_PKG_VERSION");

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
mod type_checking;
mod uuid;

pub use crate::{
    builtins::BuiltinsFunctions,
    exceptions::{
        CodeLoc, ExcData, ExcType, JsonErrorData, MontyException, StackFrame, UnicodeErrorData, UnicodeErrorObject,
        unicode_decode_error_msg,
    },
    file_mode::FileMode,
    format::{FormatFloat, StringRepr, bytes_repr, bytes_repr_fmt, string_repr_fmt, utf8_error_reason},
    io::{DEFAULT_MAX_PRINT_COLLECT_BYTES, PrintStream, PrintWriter, PrintWriterCallback, check_print_collect_limit},
    object::{
        ConversionError, DictPairs, InvalidInputError, MAX_TIMEZONE_OFFSET_SECONDS, MIN_TIMEZONE_OFFSET_SECONDS,
        MontyClassInstance, MontyClassType, MontyDate, MontyDateTime, MontyFileHandle, MontyObject, MontyTime,
        MontyTimeDelta, MontyTimeZone, MontyType,
    },
    os::{
        GetenvArgs, MkdirCallArgs, MontyPath, OpenCallArgs, OsFunctionCall, PathBytesDataArgs, PathStringDataArgs,
        RenameCallArgs, dir_stat, file_stat, stat_result, symlink_stat,
    },
    resource::{
        BASELINE_MEMORY, DEFAULT_MAX_RECURSION_DEPTH, DEFAULT_MAX_SUSPENSIONS, LARGE_RESULT_THRESHOLD, LIVE_MEMORY,
        OOM_EXIT_CODE, ResourceError, ResourceLimits, ResourceTracker,
    },
    results::{ExtFunctionResult, NameLookupResult},
    run_options::{AssertMessageAnnotations, CompileOptions},
    type_checking::{TypeCheckState, TypeCheckingConfig, TypeCheckingFormat},
    uuid::MontyUuid,
};
