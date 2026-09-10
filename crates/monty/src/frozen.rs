//! Python implementations of interpreter functions compiled once and shared by all programs.
//!
//! Frozen functions are entered through their existing builtin or module-function
//! wrappers and captured separately from user functions. Their source-level
//! frames remain resumable but are omitted from user tracebacks.

use std::sync::{Mutex, MutexGuard, OnceLock};

use monty_types::CompileOptions;
use serde::{Deserialize, Deserializer, Serialize, de::Error};

use crate::{
    bytecode::{Compiler, Opcode},
    function::Function,
    intern::{InternerBuilder, Interns},
    parse::parse,
    prepare::prepare,
};

const FROZEN_FILENAME: &str = "<monty-frozen>";
const FROZEN_FUNCTION_COUNT: usize = 1;
const FROZEN_ARGUMENT_COUNT: u16 = 3;
const REDUCE_LOOP_ENTRY: usize = 2;
const FROZEN_SOURCE: &str = r"
def __monty_functools_reduce(function, iterator, accumulator):
    for item in iterator:
        accumulator = function(accumulator, item)
    return accumulator
";

/// Stable identities for Python functions implemented by the interpreter.
///
/// Values index the captured frozen-function table and are therefore append-only.
/// Reordering or removing variants requires a dump-format version bump.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum FrozenFunction {
    /// Suspendable fold loop behind `functools.reduce`.
    FunctoolsReduce = 0,
}

impl FrozenFunction {
    /// Returns this frozen function's stable index in captured metadata.
    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

/// Frozen function metadata captured alongside its bytecode in dumps.
///
/// Entry metadata travels with the code because an idle dumped session may call
/// the function after restoration under a newer patch release.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct FrozenFunctionCode {
    /// Ordinary compiled Python function metadata and bytecode.
    function: Function,
    /// Bytecode offset after native setup has prepared the operand stack.
    entry_ip: usize,
    /// Local slot containing the iterator copied to the initial operand stack.
    iterator_slot: u16,
}

impl FrozenFunctionCode {
    /// Constructs validated frozen metadata for compiler-owned bytecode.
    fn new(function: Function, entry_ip: usize, iterator_slot: u16) -> Self {
        let frozen = Self {
            function,
            entry_ip,
            iterator_slot,
        };
        frozen.validate().expect("compiled frozen function metadata is invalid");
        frozen
    }

    /// Returns the compiled Python function.
    pub(crate) fn function(&self) -> &Function {
        &self.function
    }

    /// Returns the post-setup bytecode entry point.
    pub(crate) fn entry_ip(&self) -> usize {
        self.entry_ip
    }

    /// Returns the local iterator slot used to seed the operand stack.
    pub(crate) fn iterator_slot(&self) -> u16 {
        self.iterator_slot
    }

    /// Validates invariants relied upon by binder-free frozen entry.
    fn validate(&self) -> Result<(), &'static str> {
        if self.function.namespace_size < usize::from(FROZEN_ARGUMENT_COUNT) {
            Err("frozen function has too few local slots")
        } else if self.iterator_slot >= FROZEN_ARGUMENT_COUNT {
            Err("frozen function iterator slot is outside its arguments")
        } else if self.function.code.bytecode().get(self.entry_ip).copied() != Some(Opcode::ForIter as u8) {
            Err("frozen function entry point is not a for-loop")
        } else {
            Ok(())
        }
    }
}

/// Serialized fields for [`FrozenFunctionCode`].
#[derive(Deserialize)]
struct FrozenFunctionCodeFields {
    /// Ordinary compiled Python function metadata and bytecode.
    function: Function,
    /// Bytecode offset after native setup has prepared the operand stack.
    entry_ip: usize,
    /// Local slot containing the iterator copied to the initial operand stack.
    iterator_slot: u16,
}

impl<'de> Deserialize<'de> for FrozenFunctionCode {
    /// Rejects metadata that could violate direct-entry stack assumptions.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = FrozenFunctionCodeFields::deserialize(deserializer)?;
        let frozen = Self {
            function: fields.function,
            entry_ip: fields.entry_ip,
            iterator_slot: fields.iterator_slot,
        };
        frozen.validate().map_err(D::Error::custom)?;
        Ok(frozen)
    }
}

/// Creates an interner seeded with the frozen function bundle.
///
/// Frozen string IDs form a stable prefix, allowing compiled bytecode to be
/// cloned into each program while user strings are appended afterwards. Frozen
/// bodies must be self-contained because global slots address the user module.
pub(crate) fn seed_interner(code: &str) -> InternerBuilder {
    InternerBuilder::from_interns(&frozen_interns(), code)
}

/// Returns frozen functions to capture in a newly compiled program.
pub(crate) fn functions() -> Vec<FrozenFunctionCode> {
    frozen_interns().frozen_functions_clone()
}

/// Clones the frozen metadata for a new empty REPL session.
pub(crate) fn interns() -> Interns {
    frozen_interns().clone()
}

/// Returns the process-wide frozen bundle, compiling its trusted source once.
fn frozen_interns() -> MutexGuard<'static, Interns> {
    static INTERNS: OnceLock<Mutex<Interns>> = OnceLock::new();
    INTERNS
        .get_or_init(|| Mutex::new(compile_frozen_interns()))
        .lock()
        .expect("frozen intern lock should not be poisoned")
}

/// Compiles trusted frozen source into process-wide strings, code, and entry metadata.
fn compile_frozen_interns() -> Interns {
    let parsed = parse(FROZEN_SOURCE, FROZEN_FILENAME).expect("frozen Python source should parse");
    let prepared = prepare(parsed, Vec::new()).expect("frozen Python source should prepare");
    let mut interns = Interns::new(prepared.interner, Vec::new());
    let compiled = Compiler::compile_module(&prepared.nodes, &interns, &prepared.globals, CompileOptions::default())
        .expect("frozen Python source should compile");
    assert_eq!(
        compiled.functions.len(),
        FROZEN_FUNCTION_COUNT,
        "frozen function table changed without updating IDs"
    );
    let mut functions = compiled.functions.into_iter();
    let reduce = functions.next().expect("frozen reduce function should exist");
    assert_eq!(
        reduce.code.bytecode().get(..REDUCE_LOOP_ENTRY),
        Some([Opcode::LoadLocal1 as u8, Opcode::GetIter as u8].as_slice()),
        "frozen reduce setup changed without updating its entry point"
    );
    assert!(
        reduce.code.bytecode().contains(&(Opcode::CallLocal2 as u8)),
        "frozen reduce callback should use CallLocal2"
    );
    let frozen_functions = vec![FrozenFunctionCode::new(reduce, REDUCE_LOOP_ENTRY, 1)];
    interns.set_frozen_functions(frozen_functions);
    interns
}
