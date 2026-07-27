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
mod stringize;
mod types;
mod value;

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
