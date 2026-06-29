//! Bytecode compiler for transforming AST to bytecode.
//!
//! The compiler traverses the prepared AST (`PreparedNode` and `Expr` types from `expressions.rs`)
//! and emits bytecode instructions using `CodeBuilder`. It handles variable scoping,
//! control flow, and expression evaluation order following Python semantics.
//!
//! Functions are compiled recursively: when a `PreparedFunctionDef` is encountered,
//! its body is compiled to bytecode and a `Function` struct is created. All compiled
//! functions are collected and returned along with the module code.

use std::{borrow::Cow, mem};

use ahash::AHashSet;

use super::{
    RESERVED_MODULE_DUNDERS,
    builder::{CodeBuilder, JumpLabel, JumpTarget},
    code::Code,
    op::{FORMAT_VALUE_HAS_SPEC, FORMAT_VALUE_STATIC_SPEC, Opcode},
};
use crate::{
    args::{ArgExprs, CallArg, CallKwarg, Kwarg},
    builtins::Builtins,
    exception_private::ExcType,
    exception_public::{MontyException, SourceMap, StackFrame},
    expressions::{
        AssignTarget, Callable, CmpOperator, Comprehension, DictItem, Expr, ExprLoc, Identifier, Literal, NameScope,
        Node, Operator, PreparedFunctionDef, PreparedNode, SequenceItem, UnpackTarget,
    },
    fstring::{ConversionFlag, FStringPart, FormatSpec},
    function::Function,
    intern::{Interns, StringId},
    modules::StandardLib,
    name_map::NameMap,
    parse::{CodeRange, ExceptHandler, Try},
    value::{EitherStr, Value},
};

/// Maximum number of arguments allowed in a function call.
///
/// This limit comes from the bytecode format: `CallFunction` and `CallAttr`
/// use a u8 operand for the argument count, so max 255. Python itself has no
/// such limit but we need one for our bytecode encoding.
const MAX_CALL_ARGS: usize = 255;

/// Maximum number of distinct names in a single namespace (module or function).
///
/// `LoadLocal`/`LoadGlobal`/`StoreLocal`/etc. encode the namespace slot in 16
/// bits, so the slot index must fit in `u16`. CPython has no equivalent limit
/// but this is intrinsic to our compact bytecode encoding — exceeding it
/// surfaces to the user as a `SyntaxError`.
const MAX_NAMESPACE_SIZE: usize = u16::MAX as usize;

/// Maximum number of targets in a single tuple-unpacking pattern (e.g.
/// `a, b, c = it` or the nested form `(a, b), c = it`).
///
/// `UnpackSequence` / `UnpackEx` encode the per-level target count in `u8`,
/// so any individual unpacking level is capped at 255 targets (with
/// `UnpackEx` splitting that count into before-star and after-star halves).
const MAX_UNPACK_TARGETS: usize = 255;

/// Maximum number of `for ... in ...` clauses in a single comprehension.
///
/// `compile_comprehension_generators` recurses once per generator clause, so
/// without an up-front guard a syntactically valid source file with tens of
/// thousands of clauses can exhaust the Rust call stack during compilation —
/// well before runtime resource limits become active. The cap also matches
/// the `u8` operand consumed by `ListAppend` / `SetAdd` / `DictSetItem`:
/// each additional generator adds one iterator layer (plus its target
/// leaves) to the operand stack, and the bytecode format can only encode a
/// `u8` depth. CPython has no equivalent limit but real Python comprehension
/// usage is far below this cap.
const MAX_COMP_GENERATORS: usize = 255;

/// Converts a `usize` namespace size into the `u16` slot count expected by
/// the bytecode, surfacing a `CompileError` if the limit is exceeded.
///
/// `kind` ("module", "function", or "lambda") is interpolated into the error
/// message so the user can distinguish which scope hit the cap. The position
/// is left as the default `CodeRange` because the relevant location is the
/// whole compile unit — there is no single offending statement to highlight.
fn check_namespace_size_u16(size: usize, kind: &'static str) -> Result<u16, CompileError> {
    u16::try_from(size).map_err(|_| namespace_too_large(size, kind))
}

#[cold]
#[inline(never)]
fn namespace_too_large(size: usize, kind: &'static str) -> CompileError {
    CompileError::new(
        format!(
            "{kind} uses too many distinct names ({size}); the bytecode format supports up to {MAX_NAMESPACE_SIZE}"
        ),
        CodeRange::default(),
    )
}

/// Converts a tuple-unpacking target count into the `u8` operand for
/// `UnpackSequence` (or the before/after halves of `UnpackEx`).
fn check_unpack_targets(count: usize, position: CodeRange) -> Result<u8, CompileError> {
    u8::try_from(count).map_err(|_| too_many_unpack_targets(count, position))
}

/// Rejects comprehensions with more than [`MAX_COMP_GENERATORS`] for-clauses
/// before recursive compilation, so attacker-controlled source cannot
/// trigger a Rust stack overflow during `Compiler::compile_module`.
///
/// Anchored to the body expression's position because that's the
/// comprehension's most stable location to point at in a traceback caret.
fn check_comp_generators(count: usize, position: CodeRange) -> Result<(), CompileError> {
    if count > MAX_COMP_GENERATORS {
        Err(CompileError::new(
            format!("comprehension has too many nested clauses ({count}); maximum is {MAX_COMP_GENERATORS}"),
            position,
        ))
    } else {
        Ok(())
    }
}

/// Returns a position that locates `target` in source for error reporting.
///
/// `Name` / `Starred` carry the identifier's position; `Tuple` carries its
/// own. Used by comp-target unpacking when the per-leaf position isn't
/// available at the error point.
fn target_position(target: &UnpackTarget) -> CodeRange {
    match target {
        UnpackTarget::Name(ident) | UnpackTarget::Starred(ident) => ident.position,
        UnpackTarget::Tuple { position, .. } => *position,
    }
}

#[cold]
#[inline(never)]
fn too_many_unpack_targets(count: usize, position: CodeRange) -> CompileError {
    CompileError::new(
        format!("too many targets in tuple unpacking ({count}); maximum is {MAX_UNPACK_TARGETS}"),
        position,
    )
}

/// Converts an in-memory collection length (list/tuple/dict/set literal element
/// count, dict pair count) into the `u16` operand of `BuildList`/`BuildTuple`/
/// `BuildDict`/`BuildSet`.
fn check_collection_size_u16(count: usize, position: CodeRange) -> Result<u16, CompileError> {
    u16::try_from(count).map_err(|_| collection_too_large(count, position))
}

#[cold]
#[inline(never)]
fn collection_too_large(count: usize, position: CodeRange) -> CompileError {
    CompileError::new(
        format!(
            "collection literal has too many elements ({count}); maximum is {}",
            u16::MAX
        ),
        position,
    )
}

/// Converts the index of a newly-defined function into the `u16` operand used
/// by `MakeFunction`/`MakeClosure`. The cap is the total number of
/// `def`/`lambda`/comprehension function objects in the *whole module*, since
/// `FunctionId`s are allocated linearly across nested scopes.
fn check_function_count_u16(func_id: usize, position: CodeRange) -> Result<u16, CompileError> {
    u16::try_from(func_id).map_err(|_| too_many_functions(func_id, position))
}

#[cold]
#[inline(never)]
fn too_many_functions(func_id: usize, position: CodeRange) -> CompileError {
    CompileError::new(
        format!(
            "module defines too many functions/lambdas ({}); maximum is {}",
            func_id + 1,
            u16::MAX
        ),
        position,
    )
}

/// Converts a `StringId` (intern pool index) into the `u16` operand used by
/// every name-bearing opcode (`LoadAttr`, `StoreAttr`, `LoadGlobal`,
/// `CallFunctionKw` keyword names, etc.). Called inline at every emission
/// site — overflow only happens when the intern pool exceeded `u16::MAX`
/// during parse/prepare, so the error construction is `#[cold]` and the
/// success path inlines to a single `as u16`.
fn check_name_index_u16(name_id: StringId, position: CodeRange) -> Result<u16, CompileError> {
    u16::try_from(name_id.index()).map_err(|_| name_index_too_large(position))
}

#[cold]
#[inline(never)]
fn name_index_too_large(position: CodeRange) -> CompileError {
    CompileError::new(
        format!(
            "module has too many distinct names; the bytecode format supports up to {} interned strings",
            usize::from(u16::MAX) + 1,
        ),
        position,
    )
}

/// Converts a call-related count (positional args, keyword args, defaults,
/// closure cells) into the `u8` operand used by the corresponding opcodes.
/// `kind` (e.g. "default parameter values") is interpolated into the error
/// message so the diagnostic identifies which kind of count overflowed.
fn check_call_args_u8(count: usize, kind: &'static str, position: CodeRange) -> Result<u8, CompileError> {
    u8::try_from(count).map_err(|_| too_many_call_args(count, kind, position))
}

#[cold]
#[inline(never)]
fn too_many_call_args(count: usize, kind: &'static str, position: CodeRange) -> CompileError {
    CompileError::new(format!("more than {MAX_CALL_ARGS} {kind} ({count})"), position)
}

/// Compiles prepared AST nodes to bytecode.
///
/// The compiler traverses the AST and emits bytecode instructions using
/// `CodeBuilder`. It handles variable scoping, control flow, and expression
/// evaluation order following Python semantics.
///
/// Functions are compiled recursively and collected in the `functions` vector.
/// When a `PreparedFunctionDef` is encountered, its body is compiled first,
/// creating a `Function` struct that is added to the vector. The index of the
/// function in this vector becomes the operand for MakeFunction/MakeClosure opcodes.
pub struct Compiler<'a> {
    /// Current code being built.
    code: CodeBuilder,

    /// Reference to interns for string/function lookups.
    interns: &'a Interns,

    /// Compiled functions, indexed by their position in this vector.
    ///
    /// Functions are added in the order they are encountered during compilation.
    /// Nested functions are compiled before their containing function's code
    /// finishes, so inner functions have lower indices.
    functions: Vec<Function>,

    /// Loop stack for break/continue handling.
    /// Each entry tracks the loop start offset and pending break jumps.
    loop_stack: Vec<LoopInfo>,

    /// Stack of finally targets for handling returns inside try-finally.
    ///
    /// When a return statement is compiled inside a try-finally block, instead
    /// of immediately returning, we store the return value and jump to the
    /// finally block. The finally block will then execute the return.
    finally_targets: Vec<FinallyTarget>,

    /// Tracks nesting depth inside exception handlers.
    ///
    /// When break/continue/return is inside an except handler, we need to
    /// emit one `ClearException` per enclosing handler to drain the per-handler
    /// `exception_stack` entries before jumping to the finally path or loop
    /// target. The exception *value* is already off the operand stack — it's
    /// consumed eagerly at handler entry (stored to the `as` binding or
    /// popped) — so no operand-stack Pop is needed here.
    except_handler_depth: u16,

    /// Whether the compiler is currently compiling module-level code.
    ///
    /// At module level, `Local` scope maps to global opcodes
    /// (`LoadGlobal`/`StoreGlobal`/`DeleteGlobal`) because module locals live in the
    /// globals array. In function bodies this is `false` and these scopes use local
    /// opcodes that index into the stack.
    is_module_scope: bool,

    /// Number of stack-resident locals in the running frame for this code object.
    ///
    /// - Function scope: equals the function's `namespace_size` (params + cells +
    ///   free vars + assigned locals).
    /// - Module scope: `0` — module-level "locals" live in `self.globals`, so
    ///   nothing is stored in the frame's locals region.
    frame_locals: u16,

    /// Operand-stack offsets for comp-var slot IDs currently in scope.
    ///
    /// Indexed by the slot ID stored in the prepared `Identifier`. The value is
    /// the absolute operand-stack offset of that comp-var (set after the
    /// `FOR_ITER` / `UNPACK_SEQUENCE` / `LIFT_TO_TOP` chain that landed it on
    /// the stack, by `compile_comp_target_unpack`). Used by `compile_name` for
    /// `NameScope::CompVar` to emit `LoadLocal/W(frame_locals + offset)`.
    ///
    /// Pushed/truncated with each comprehension via `enter_comprehension` /
    /// `exit_comprehension`, so sibling comps reuse slot IDs cleanly.
    slot_offsets: Vec<u16>,

    /// Slot IDs that are statically known to have been assigned at the current
    /// emission point.
    ///
    /// Updated by `compile_comp_target_unpack` when it records a leaf's offset
    /// (= the moment the comp's `for` clause has stored a value into that
    /// slot). Read by `compile_name` for `NameScope::CompVar`: bound reads
    /// emit `LoadLocal/W`; unbound reads (slot not yet in this set) emit
    /// `RaiseUnboundLocal(name_id)`. The same comprehension's slots are
    /// removed at `exit_comprehension`, so sibling comps start fresh.
    bound_comp_slots: AHashSet<u16>,
}

/// Information about a loop for break/continue handling.
///
/// Tracks the bytecode locations needed for compiling break and continue statements:
/// - `start`: where continue should jump to (the ForIter instruction for `for` loops,
///   or condition evaluation for `while` loops)
/// - `break_jumps`: pending jumps from break statements that need to be patched
///   to jump past the loop's else block
/// - `has_iterator_on_stack`: whether this loop has an iterator on the stack that
///   needs to be popped on break (true for `for` loops, false for `while` loops)
struct LoopInfo {
    /// Bytecode position + stack depth at loop start (for continue).
    /// `emit_jump_to` uses the depth to enforce the backward-jump merge invariant.
    start: JumpTarget,
    /// Jump labels that need patching to loop end (for break).
    /// Entries from breaks emitted in dead state are no-op labels — `patch_jump`
    /// ignores them silently.
    break_jumps: Vec<JumpLabel>,
    /// Whether this loop has an iterator on the stack.
    /// True for `for` loops, false for `while` loops.
    has_iterator_on_stack: bool,
}

/// A break or continue that needs to go through a finally block.
///
/// When break/continue is inside a try-finally, we need to run the finally block
/// before executing the break/continue. This struct tracks the jump and which
/// loop it targets.
struct BreakContinueThruFinally {
    /// The jump instruction that needs to be patched. A no-op label if the
    /// break/continue was emitted from dead state; `patch_jump` ignores it.
    jump: JumpLabel,
    /// The loop depth (index in loop_stack) being targeted.
    target_loop_depth: usize,
}

/// Tracks a finally block for handling returns/break/continue inside try-finally.
///
/// When compiling a try-finally, we push a `FinallyTarget` to track jumps
/// from return/break/continue statements that need to go through the finally block.
struct FinallyTarget {
    /// Jump labels for returns inside the try block that need to go to finally.
    return_jumps: Vec<JumpLabel>,
    /// Break statements that need to go through this finally block.
    break_jumps: Vec<BreakContinueThruFinally>,
    /// Continue statements that need to go through this finally block.
    continue_jumps: Vec<BreakContinueThruFinally>,
    /// The loop depth when this finally was entered.
    /// Used to determine if break/continue targets a loop outside this finally.
    loop_depth_at_entry: usize,
    /// `except_handler_depth` at the try-statement entry — i.e. the number
    /// of enclosing `except` clauses that are still alive while control is
    /// inside this finally's protected region. A `return` that crosses
    /// this finally must NOT pop those handlers' exception state (the
    /// finally body might reference them); cleanup of handlers between
    /// here and the next-outer finally is the responsibility of this
    /// finally's emit_return_routing trailer.
    except_handler_depth_at_entry: u16,
}

/// A simulated entry on the operand stack during comprehension target unpacking.
///
/// The compiler walks each comp target by recursively unpacking tuples,
/// emitting `UnpackSequence`/`UnpackEx`/`LiftToTop` as needed, and tracks the
/// per-step stack state in a `Vec<SimItem>`. Tracking is necessary because
/// `LiftToTop` reorders items; without simulating, the compiler couldn't tell
/// which final operand-stack offset each comp-var leaf ends up at.
enum SimItem<'a> {
    /// A finalized comp-var leaf. The slot ID gets mapped to an absolute
    /// operand-stack offset (via its position in the sim Vec) once all
    /// unpacking has finished.
    Leaf(u16),
    /// A value that still needs to be matched against an `UnpackTarget`
    /// (which may be a nested `Tuple`). The borrowed target tells the
    /// compiler how to drive the next UNPACK / Lift step.
    Pending(&'a UnpackTarget),
}

/// Result of module compilation: the module code and all compiled functions.
pub struct CompileResult {
    /// The compiled module code.
    pub code: Code,
    /// All functions compiled during module compilation, indexed by their function ID.
    pub functions: Vec<Function>,
}

impl<'a> Compiler<'a> {
    /// Creates a new compiler with access to the string interner.
    ///
    /// `frame_locals` is the runtime `locals_count` for this code object:
    /// 0 for module-level code (globals live in `self.globals`, not on the
    /// stack), or the function's namespace size at function scope. Comp-var
    /// load/store opcodes encode `frame_locals + offset` as their slot
    /// operand so plain `LoadLocal/W` and `StoreLocal/W` reach the correct
    /// operand-stack position at runtime.
    fn new(interns: &'a Interns, functions: Vec<Function>, is_module_scope: bool, frame_locals: u16) -> Self {
        let mut code = CodeBuilder::new();
        code.new_code_region(0);
        Self {
            code,
            interns,
            functions,
            loop_stack: Vec::new(),
            finally_targets: Vec::new(),
            except_handler_depth: 0,
            is_module_scope,
            frame_locals,
            slot_offsets: Vec::new(),
            bound_comp_slots: AHashSet::new(),
        }
    }

    /// Compiles module-level code (a sequence of statements).
    ///
    /// Returns the compiled module Code and all compiled Functions, or a compile
    /// error if limits were exceeded. The module implicitly returns the value
    /// of the last expression, or None if empty.
    pub fn compile_module(
        nodes: &[PreparedNode],
        interns: &Interns,
        globals: &NameMap,
    ) -> Result<CompileResult, CompileError> {
        Self::compile_module_with_functions(nodes, interns, globals, Vec::new())
    }

    /// Compiles module-level code while preserving an existing function table prefix.
    ///
    /// This is used by incremental REPL compilation so previously created
    /// `FunctionId`s remain stable: new function IDs are allocated after
    /// `existing_functions.len()`.
    pub fn compile_module_with_functions(
        nodes: &[PreparedNode],
        interns: &Interns,
        globals: &NameMap,
        existing_functions: Vec<Function>,
    ) -> Result<CompileResult, CompileError> {
        let num_locals = check_namespace_size_u16(globals.len(), "module")?;
        // Module frames have `locals_count = 0` at runtime (globals live in
        // `self.globals`), so comp-var offsets are emitted as plain operand-
        // stack indices.
        let mut compiler = Compiler::new(interns, existing_functions, true, 0);

        // All globals are "local names" in the module
        for (slot, name_id) in globals.iter() {
            compiler.code.register_local_name(slot.as_u16(), name_id);
        }

        compiler.compile_block(nodes)?;

        // Module returns None if no explicit return
        compiler.code.emit(Opcode::LoadNone)?;
        compiler.code.emit(Opcode::ReturnValue)?;

        Ok(CompileResult {
            code: compiler.code.build(num_locals),
            functions: compiler.functions,
        })
    }

    /// Compiles a function body to bytecode, returning the Code and any nested functions.
    ///
    /// Used internally when compiling function definitions. The function body is
    /// compiled to bytecode with an implicit `return None` at the end if there's
    /// no explicit return statement.
    ///
    /// The `functions` parameter receives any previously compiled functions, and
    /// any nested functions found in the body will be added to it.
    fn compile_function_body(
        body: &[PreparedNode],
        interns: &Interns,
        functions: Vec<Function>,
        num_locals: u16,
    ) -> Result<(Code, Vec<Function>), CompileError> {
        // Function frames have `locals_count = num_locals` at runtime, so
        // comp-var load/store opcodes use `num_locals + offset` to skip past
        // the locals region into the operand-stack region.
        let mut compiler = Compiler::new(interns, functions, false, num_locals);
        compiler.compile_block(body)?;

        // Implicit return None if no explicit return
        compiler.code.emit(Opcode::LoadNone)?;
        compiler.code.emit(Opcode::ReturnValue)?;

        Ok((compiler.code.build(num_locals), compiler.functions))
    }

    /// Compiles a block of statements.
    fn compile_block(&mut self, nodes: &[PreparedNode]) -> Result<(), CompileError> {
        for node in nodes {
            if self.code.is_dead() {
                // Don't bother compiling dead code
                break;
            }
            self.compile_stmt(node)?;
        }
        Ok(())
    }

    // ========================================================================
    // Statement Compilation
    // ========================================================================

    /// Compiles a single statement.
    fn compile_stmt(&mut self, node: &PreparedNode) -> Result<(), CompileError> {
        // Node is an alias, use qualified path for matching
        match node {
            Node::Expr(expr) => {
                self.compile_expr(expr)?;
                self.code.emit(Opcode::Pop)?; // Discard result
            }
            Node::Return(expr) => {
                self.compile_return(expr.as_ref())?;
            }
            Node::Assign { target, object } => {
                self.compile_expr(object)?;
                self.compile_store(target)?;
            }
            Node::UnpackAssign {
                targets,
                targets_position,
                object,
            } => {
                self.compile_expr(object)?;
                self.emit_unpack_store(targets, *targets_position)?;
            }
            Node::OpAssign { target, op, value } => {
                let Some(opcode) = operator_to_inplace_opcode(op) else {
                    return Err(CompileError::new(
                        "matrix multiplication augmented assignment (@=) is not yet supported",
                        target.position,
                    ));
                };
                self.compile_name(target)?;
                self.compile_expr(value)?;
                self.code.emit(opcode)?;
                self.compile_store(target)?;
            }
            Node::SubscriptOpAssign {
                target,
                index,
                op,
                value,
                target_position,
            } => {
                let Some(opcode) = operator_to_inplace_opcode(op) else {
                    return Err(CompileError::new(
                        "matrix multiplication augmented assignment (@=) is not yet supported",
                        *target_position,
                    ));
                };
                self.compile_expr(target)?;
                self.compile_expr(index)?;
                self.code.emit(Opcode::Dup2)?;
                self.code.set_location(*target_position, None);
                self.code.emit(Opcode::BinarySubscr)?;
                self.compile_expr(value)?;
                self.code.emit(opcode)?;
                self.code.emit(Opcode::Rot3)?;
                self.code.set_location(*target_position, None);
                self.code.emit(Opcode::StoreSubscr)?;
            }
            Node::SubscriptAssign {
                target,
                index,
                value,
                target_position,
            } => {
                self.compile_expr(value)?;
                self.emit_subscript_store(target, index, *target_position)?;
            }
            Node::AttrOpAssign {
                object,
                attr,
                op,
                value,
                target_position,
            } => {
                let Some(opcode) = operator_to_inplace_opcode(op) else {
                    return Err(CompileError::new(
                        "matrix multiplication augmented assignment (@=) is not yet supported",
                        *target_position,
                    ));
                };
                let name_id = attr.string_id().expect("LoadAttr requires interned attr name");
                let name_idx = check_name_index_u16(name_id, *target_position)?;
                // Stack: compile object, dup for later store, load attr, apply op, rotate, store
                self.compile_expr(object)?; // [obj]
                self.code.emit(Opcode::Dup)?; // [obj, obj]
                self.code.set_location(*target_position, None);
                self.code.emit_u16(Opcode::LoadAttr, name_idx)?; // [obj, attr_val]
                self.compile_expr(value)?; // [obj, attr_val, rhs]
                self.code.emit(opcode)?; // [obj, result]
                self.code.emit(Opcode::Rot2)?; // [result, obj]
                self.code.set_location(*target_position, None);
                self.code.emit_u16(Opcode::StoreAttr, name_idx)?; // []
            }
            Node::AttrAssign {
                object,
                attr,
                target_position,
                value,
            } => {
                self.compile_expr(value)?;
                self.emit_attr_store(object, attr, *target_position)?;
            }
            Node::ChainAssign { targets, object } => {
                // Python evaluates the RHS once, then assigns to each target in
                // left-to-right source order. We materialise the value on the stack
                // and, for every target except the last, emit `Dup` to keep a copy
                // underneath the target-specific store logic. The final target
                // consumes the remaining copy, leaving the stack balanced.
                //
                // The parser only produces `ChainAssign` with `targets.len() >= 2`,
                // but because `Node` derives `Deserialize`, untrusted snapshot input
                // could otherwise reach here with 0 or 1 targets. `split_last()`
                // handles both cases safely without an unsigned underflow, and the
                // `is_empty` branch pops the leftover RHS value so the operand stack
                // stays balanced.
                self.compile_expr(object)?;
                if let Some((last, rest)) = targets.split_last() {
                    for target in rest {
                        self.code.emit(Opcode::Dup)?;
                        self.compile_assign_target(target)?;
                    }
                    self.compile_assign_target(last)?;
                } else {
                    self.code.emit(Opcode::Pop)?;
                }
            }
            Node::If { test, body, or_else } => self.compile_if(test, body, or_else)?,
            Node::For {
                target,
                iter,
                body,
                or_else,
            } => self.compile_for(target, iter, body, or_else)?,
            Node::While { test, body, or_else } => self.compile_while(test, body, or_else)?,
            Node::Assert { test, msg } => self.compile_assert(test, msg.as_ref())?,
            Node::Raise(expr) => {
                if let Some(exc) = expr {
                    self.compile_expr(exc)?;
                    self.code.emit(Opcode::Raise)?;
                } else {
                    self.code.emit(Opcode::Reraise)?;
                }
            }
            Node::FunctionDef(func_def) => self.compile_function_def(func_def)?,
            Node::Try(try_block) => self.compile_try(try_block)?,
            Node::With {
                context, target, body, ..
            } => self.compile_with(context, target.as_ref(), body)?,
            Node::Import { names } => {
                for import_name in names {
                    self.compile_import(import_name.module_name, &import_name.binding)?;
                }
            }
            Node::ImportFrom {
                module_name,
                names,
                position,
            } => self.compile_import_from(*module_name, names, *position)?,
            Node::Break { position } => self.compile_break(*position)?,
            Node::Continue { position } => self.compile_continue(*position)?,
            // These are handled during the prepare phase and produce no bytecode
            Node::Pass | Node::Global { .. } | Node::Nonlocal { .. } => {}
        }
        Ok(())
    }

    /// Compiles a function definition.
    ///
    /// This involves:
    /// 1. Recursively compiling the function body to bytecode
    /// 2. Creating a Function struct with the compiled Code
    /// 3. Adding the Function to the compiler's functions vector
    /// 4. Emitting bytecode to evaluate defaults and create the function at runtime
    fn compile_function_def(&mut self, func_def: &PreparedFunctionDef) -> Result<(), CompileError> {
        let func_pos = func_def.name.position;

        // Bound the bytecode-operand counts before compiling — the `u8` casts
        // below depend on these fitting in 255.
        let defaults_count = check_call_args_u8(func_def.default_exprs.len(), "default parameter values", func_pos)?;
        let cell_count = check_call_args_u8(func_def.free_var_enclosing_slots.len(), "closure variables", func_pos)?;

        // 1. Compile the function body recursively
        // Take ownership of functions for the recursive compile, then restore
        let functions = mem::take(&mut self.functions);
        let namespace_size = check_namespace_size_u16(func_def.namespace_size, "function")?;
        let (body_code, mut functions) =
            Self::compile_function_body(&func_def.body, self.interns, functions, namespace_size)?;

        // 2. Create the compiled Function and add to the vector
        let func_id = functions.len();
        let function = Function::new(
            func_def.name,
            func_def.signature.clone(),
            func_def.namespace_size,
            func_def.free_var_enclosing_slots.clone(),
            func_def.free_var_slots.clone(),
            func_def.cell_var_slots.clone(),
            func_def.cell_param_indices.clone(),
            func_def.default_exprs.len(),
            func_def.is_async,
            body_code,
        );
        functions.push(function);

        // Restore functions to self
        self.functions = functions;

        // 3. Compile and push default values (evaluated at definition time)
        for default_expr in &func_def.default_exprs {
            self.compile_expr(default_expr)?;
        }
        let func_id_u16 = check_function_count_u16(func_id, func_pos)?;

        // 4. Emit MakeFunction or MakeClosure (if has free vars)
        if func_def.free_var_enclosing_slots.is_empty() {
            // MakeFunction: func_id (u16) + defaults_count (u8)
            self.code
                .emit_u16_u8(Opcode::MakeFunction, func_id_u16, defaults_count)?;
        } else {
            // Push captured cells from enclosing scope
            for &slot in &func_def.free_var_enclosing_slots {
                // Load the cell reference from the enclosing namespace.
                // `slot` is a `NamespaceId` bound by `check_namespace_size_u16`
                // on the enclosing scope, so the conversion is an invariant
                // rather than a user-input check (panic-on-failure is fine).
                self.code.emit_load_local(slot.as_u16())?;
            }
            // MakeClosure: func_id (u16) + defaults_count (u8) + cell_count (u8)
            self.code
                .emit_u16_u8_u8(Opcode::MakeClosure, func_id_u16, defaults_count, cell_count)?;
        }

        // 5. Store the function object to its name slot
        self.compile_store(&func_def.name)?;

        Ok(())
    }

    /// Compiles a lambda expression.
    ///
    /// This is similar to `compile_function_def` but:
    /// - Does NOT store the function to a name slot (it stays on the stack as an expression result)
    ///
    /// The lambda's `PreparedFunctionDef` already has `<lambda>` as its name.
    fn compile_lambda(&mut self, func_def: &PreparedFunctionDef) -> Result<(), CompileError> {
        let func_pos = func_def.name.position;

        // Bound the bytecode-operand counts before compiling — the `u8` casts
        // below depend on these fitting in 255.
        let defaults_count = check_call_args_u8(func_def.default_exprs.len(), "default parameter values", func_pos)?;
        let cell_count = check_call_args_u8(func_def.free_var_enclosing_slots.len(), "closure variables", func_pos)?;

        // 1. Compile the function body recursively
        let functions = mem::take(&mut self.functions);
        let namespace_size = check_namespace_size_u16(func_def.namespace_size, "lambda")?;
        let (body_code, mut functions) =
            Self::compile_function_body(&func_def.body, self.interns, functions, namespace_size)?;

        // 2. Create the compiled Function and add to the vector
        let func_id = functions.len();
        let function = Function::new(
            func_def.name,
            func_def.signature.clone(),
            func_def.namespace_size,
            func_def.free_var_enclosing_slots.clone(),
            func_def.free_var_slots.clone(),
            func_def.cell_var_slots.clone(),
            func_def.cell_param_indices.clone(),
            func_def.default_exprs.len(),
            func_def.is_async,
            body_code,
        );
        functions.push(function);

        // Restore functions to self
        self.functions = functions;

        // 3. Compile and push default values (evaluated at definition time)
        for default_expr in &func_def.default_exprs {
            self.compile_expr(default_expr)?;
        }
        let func_id_u16 = check_function_count_u16(func_id, func_pos)?;

        // 4. Emit MakeFunction or MakeClosure (if has free vars)
        if func_def.free_var_enclosing_slots.is_empty() {
            // MakeFunction: func_id (u16) + defaults_count (u8)
            self.code
                .emit_u16_u8(Opcode::MakeFunction, func_id_u16, defaults_count)?;
        } else {
            // Push captured cells from enclosing scope. `slot` is a
            // `NamespaceId` from the enclosing scope, bounded by
            // `check_namespace_size_u16`; the conversion is an invariant.
            for &slot in &func_def.free_var_enclosing_slots {
                self.code.emit_load_local(slot.as_u16())?;
            }
            // MakeClosure: func_id (u16) + defaults_count (u8) + cell_count (u8)
            self.code
                .emit_u16_u8_u8(Opcode::MakeClosure, func_id_u16, defaults_count, cell_count)?;
        }

        // NOTE: Unlike compile_function_def, we do NOT call compile_store here.
        // The function object stays on the stack as an expression result.

        Ok(())
    }

    /// Compiles an import statement.
    ///
    /// Emits `LoadModule` to create the module, then stores it to the binding name.
    /// If the module is unknown, emits `RaiseImportError` to defer the error to runtime.
    /// This allows imports inside `if TYPE_CHECKING:` blocks to compile successfully.
    fn compile_import(&mut self, module_name: StringId, binding: &Identifier) -> Result<(), CompileError> {
        let position = binding.position;
        self.code.set_location(position, None);

        // Look up the module by name
        if let Some(builtin_module) = StandardLib::from_string_id(module_name) {
            // Known module - emit LoadModule
            self.code.emit_u8(Opcode::LoadModule, builtin_module as u8)?;
            // Store to the binding (respects Local/Global/Cell scope)
            self.compile_store(binding)?;
        } else {
            // Unknown module - defer error to runtime with RaiseImportError
            // This allows TYPE_CHECKING imports to compile without error
            let name_const = self.code.add_const(Value::InternString(module_name))?;
            self.code.emit_u16(Opcode::RaiseImportError, name_const)?;
        }
        Ok(())
    }

    /// Compiles a `from module import name, ...` statement.
    ///
    /// Creates the module once, then loads each attribute and stores to the binding.
    /// Invalid attribute names will raise `AttributeError` at runtime.
    /// If the module is unknown, emits `RaiseImportError` to defer the error to runtime.
    /// This allows imports inside `if TYPE_CHECKING:` blocks to compile successfully.
    fn compile_import_from(
        &mut self,
        module_name: StringId,
        names: &[(StringId, Identifier)],
        position: CodeRange,
    ) -> Result<(), CompileError> {
        self.code.set_location(position, None);

        // Look up the module
        if let Some(builtin_module) = StandardLib::from_string_id(module_name) {
            // Known module - emit LoadModule
            self.code.emit_u8(Opcode::LoadModule, builtin_module as u8)?;

            // For each name to import
            for (i, (import_name, binding)) in names.iter().enumerate() {
                // Dup the module if this isn't the last import (last one consumes the module)
                if i < names.len() - 1 {
                    self.code.emit(Opcode::Dup)?;
                }

                // Load the attribute from the module (raises ImportError if not found)
                let name_idx = check_name_index_u16(*import_name, position)?;
                self.code.emit_u16(Opcode::LoadAttrImport, name_idx)?;

                // Store to the binding
                self.compile_store(binding)?;
            }
        } else {
            // Unknown module - defer error to runtime with RaiseImportError
            // This allows TYPE_CHECKING imports to compile without error
            let name_const = self.code.add_const(Value::InternString(module_name))?;
            self.code.emit_u16(Opcode::RaiseImportError, name_const)?;
        }
        Ok(())
    }

    // ========================================================================
    // Expression Compilation
    // ========================================================================

    /// Compiles an expression, leaving its value on the stack.
    fn compile_expr(&mut self, expr_loc: &ExprLoc) -> Result<(), CompileError> {
        // Set source location for traceback info
        self.code.set_location(expr_loc.position, None);

        match &expr_loc.expr {
            Expr::Literal(lit) => self.compile_literal(lit)?,

            Expr::Name(ident) => self.compile_name(ident)?,

            Expr::Builtin(builtin) => {
                let idx = self.code.add_const(Value::Builtin(*builtin))?;
                self.code.emit_u16(Opcode::LoadConst, idx)?;
            }

            Expr::Op { left, op, right } => {
                self.compile_binary_op(left, op, right, expr_loc.position)?;
            }

            Expr::CmpOp { left, op, right } => {
                self.compile_expr(left)?;
                self.compile_expr(right)?;
                // Restore the full comparison expression's position for traceback caret range
                self.code.set_location(expr_loc.position, None);
                // ModEq needs special handling - it has a constant operand
                if let CmpOperator::ModEq(value) = op {
                    let const_idx = self.code.add_const(Value::Int(*value))?;
                    self.code.emit_u16(Opcode::CompareModEq, const_idx)?;
                } else {
                    self.code.emit(cmp_operator_to_opcode(op))?;
                }
            }

            Expr::ChainCmp { left, comparisons } => {
                self.compile_chain_comparison(left, comparisons, expr_loc.position)?;
            }

            Expr::Not(operand) => {
                self.compile_expr(operand)?;
                // Restore the full expression's position for traceback caret range
                self.code.set_location(expr_loc.position, None);
                self.code.emit(Opcode::UnaryNot)?;
            }

            Expr::UnaryMinus(operand) => {
                self.compile_expr(operand)?;
                // Restore the full expression's position for traceback caret range
                self.code.set_location(expr_loc.position, None);
                self.code.emit(Opcode::UnaryNeg)?;
            }

            Expr::UnaryPlus(operand) => {
                self.compile_expr(operand)?;
                // Restore the full expression's position for traceback caret range
                self.code.set_location(expr_loc.position, None);
                self.code.emit(Opcode::UnaryPos)?;
            }

            Expr::UnaryInvert(operand) => {
                self.compile_expr(operand)?;
                // Restore the full expression's position for traceback caret range
                self.code.set_location(expr_loc.position, None);
                self.code.emit(Opcode::UnaryInvert)?;
            }

            Expr::List(elements) => {
                if has_unpack_seq(elements) {
                    // Generalized path: build incrementally for PEP 448 *unpacks
                    self.code.emit_u16(Opcode::BuildList, 0)?;
                    for item in elements {
                        match item {
                            SequenceItem::Value(e) => {
                                self.compile_expr(e)?;
                                self.code.emit_u8(Opcode::ListAppend, 0)?;
                            }
                            SequenceItem::Unpack(e) => {
                                self.compile_expr(e)?;
                                self.code.emit(Opcode::ListExtend)?;
                            }
                        }
                    }
                } else {
                    // Fast path: all values, single BuildList.
                    // SAFETY: has_unpack_seq(elements) is false, so every item is Value.
                    for item in elements {
                        let SequenceItem::Value(e) = item else {
                            unreachable!("list fast path: only Value items")
                        };
                        self.compile_expr(e)?;
                    }
                    let count = check_collection_size_u16(elements.len(), expr_loc.position)?;
                    self.code.emit_u16(Opcode::BuildList, count)?;
                }
            }

            Expr::Tuple(elements) => {
                if has_unpack_seq(elements) {
                    // Generalized path: build via list then convert for PEP 448 *unpacks
                    self.code.emit_u16(Opcode::BuildList, 0)?;
                    for item in elements {
                        match item {
                            SequenceItem::Value(e) => {
                                self.compile_expr(e)?;
                                self.code.emit_u8(Opcode::ListAppend, 0)?;
                            }
                            SequenceItem::Unpack(e) => {
                                self.compile_expr(e)?;
                                self.code.emit(Opcode::ListExtend)?;
                            }
                        }
                    }
                    self.code.emit(Opcode::ListToTuple)?;
                } else {
                    // Fast path: all values, single BuildTuple.
                    // SAFETY: has_unpack_seq(elements) is false, so every item is Value.
                    for item in elements {
                        let SequenceItem::Value(e) = item else {
                            unreachable!("tuple fast path: only Value items")
                        };
                        self.compile_expr(e)?;
                    }
                    let count = check_collection_size_u16(elements.len(), expr_loc.position)?;
                    self.code.emit_u16(Opcode::BuildTuple, count)?;
                }
            }

            Expr::Dict(dict_items) => {
                if has_unpack_dict(dict_items) {
                    // Generalized path: build incrementally for PEP 448 **unpacks
                    self.code.emit_u16(Opcode::BuildDict, 0)?;
                    for item in dict_items {
                        match item {
                            DictItem::Pair(key, value) => {
                                self.compile_expr(key)?;
                                self.compile_expr(value)?;
                                // depth=0: dict is at TOS after key/value are popped
                                self.code.emit_u8(Opcode::DictSetItem, 0)?;
                            }
                            DictItem::Unpack(e) => {
                                self.compile_expr(e)?;
                                // depth=0: dict is directly below mapping on stack
                                self.code.emit_u8(Opcode::DictUpdate, 0)?;
                            }
                        }
                    }
                } else {
                    // Fast path: all pairs, single BuildDict.
                    // SAFETY: has_unpack_dict(dict_items) is false, so every item is Pair.
                    for item in dict_items {
                        let DictItem::Pair(key, value) = item else {
                            unreachable!("dict fast path: only Pair items")
                        };
                        self.compile_expr(key)?;
                        self.compile_expr(value)?;
                    }
                    let count = check_collection_size_u16(dict_items.len(), expr_loc.position)?;
                    self.code.emit_u16(Opcode::BuildDict, count)?;
                }
            }

            Expr::Set(elements) => {
                if has_unpack_seq(elements) {
                    // Generalized path: build incrementally for PEP 448 *unpacks
                    self.code.emit_u16(Opcode::BuildSet, 0)?;
                    for item in elements {
                        match item {
                            SequenceItem::Value(e) => {
                                self.compile_expr(e)?;
                                self.code.emit_u8(Opcode::SetAdd, 0)?;
                            }
                            SequenceItem::Unpack(e) => {
                                self.compile_expr(e)?;
                                self.code.emit_u8(Opcode::SetExtend, 0)?;
                            }
                        }
                    }
                } else {
                    // Fast path: all values, single BuildSet.
                    // SAFETY: has_unpack_seq(elements) is false, so every item is Value.
                    for item in elements {
                        let SequenceItem::Value(e) = item else {
                            unreachable!("set fast path: only Value items")
                        };
                        self.compile_expr(e)?;
                    }
                    let count = check_collection_size_u16(elements.len(), expr_loc.position)?;
                    self.code.emit_u16(Opcode::BuildSet, count)?;
                }
            }

            Expr::Subscript { object, index } => {
                self.compile_expr(object)?;
                self.compile_expr(index)?;
                // Restore the full subscript expression's position for traceback
                self.code.set_location(expr_loc.position, None);
                self.code.emit(Opcode::BinarySubscr)?;
            }

            Expr::IfElse { test, body, orelse } => {
                self.compile_if_else_expr(test, body, orelse)?;
            }

            Expr::AttrGet { object, attr } => {
                self.compile_expr(object)?;
                // Restore the full expression's position for traceback caret range
                self.code.set_location(expr_loc.position, None);
                let name_id = attr.string_id().expect("LoadAttr requires interned attr name");
                let name_idx = check_name_index_u16(name_id, expr_loc.position)?;
                self.code.emit_u16(Opcode::LoadAttr, name_idx)?;
            }

            Expr::Call { callable, args } => {
                self.compile_call(callable, args, expr_loc.position)?;
            }

            Expr::AttrCall { object, attr, args } => {
                // Compile the object (will be on the stack)
                self.compile_expr(object)?;

                // Compile the attribute call arguments and emit CallAttr
                self.compile_method_call(attr, args, expr_loc.position)?;
            }

            Expr::IndirectCall { callable, args } => {
                // Compile the callable expression (e.g., a lambda)
                self.compile_expr(callable)?;

                // Compile arguments and emit the call
                self.compile_call_args(args, expr_loc.position)?;
            }

            Expr::FString(parts) => {
                // Compile each part and build the f-string
                let part_count = self.compile_fstring_parts(parts)?;
                self.code.emit_u16(Opcode::BuildFString, part_count)?;
            }

            Expr::ListComp { elt, generators } => {
                self.compile_list_comp(elt, generators)?;
            }

            Expr::SetComp { elt, generators } => {
                self.compile_set_comp(elt, generators)?;
            }

            Expr::DictComp { key, value, generators } => {
                self.compile_dict_comp(key, value, generators)?;
            }

            Expr::Lambda { func_def } => {
                self.compile_lambda(func_def)?;
            }

            Expr::LambdaRaw { .. } => {
                // LambdaRaw should be converted to Lambda during prepare phase
                unreachable!("Expr::LambdaRaw should not exist after prepare phase")
            }

            Expr::Await(value) => {
                // Await expressions: compile the inner expression, then emit Await
                // Await handles ExternalFuture, Coroutine, and GatherFuture
                self.compile_expr(value)?;
                // Restore the full expression's position for traceback caret range
                self.code.set_location(expr_loc.position, None);
                self.code.emit(Opcode::Await)?;
            }

            Expr::Slice { lower, upper, step } => {
                // Compile slice components: start, stop, step (push None for missing)
                if let Some(lower) = lower {
                    self.compile_expr(lower)?;
                } else {
                    self.code.emit(Opcode::LoadNone)?;
                }
                if let Some(upper) = upper {
                    self.compile_expr(upper)?;
                } else {
                    self.code.emit(Opcode::LoadNone)?;
                }
                if let Some(step) = step {
                    self.compile_expr(step)?;
                } else {
                    self.code.emit(Opcode::LoadNone)?;
                }
                self.code.emit(Opcode::BuildSlice)?;
            }

            Expr::Named { target, value } => {
                // Compile the value expression (leaves result on stack)
                self.compile_expr(value)?;
                // Duplicate so value remains after store
                self.code.emit(Opcode::Dup)?;
                // Store to target (pops one copy)
                self.compile_store(target)?;
            }
        }
        Ok(())
    }

    // ========================================================================
    // Literal Compilation
    // ========================================================================

    /// Compiles a literal value.
    fn compile_literal(&mut self, literal: &Literal) -> Result<(), CompileError> {
        match literal {
            Literal::None => self.code.emit(Opcode::LoadNone),
            Literal::Bool(true) => self.code.emit(Opcode::LoadTrue),
            Literal::Bool(false) => self.code.emit(Opcode::LoadFalse),
            Literal::Int(n) => {
                // Use LoadSmallInt for values that fit in i8
                if let Ok(small) = i8::try_from(*n) {
                    self.code.emit_i8(Opcode::LoadSmallInt, small)
                } else {
                    let idx = self.code.add_const(Value::from(*literal))?;
                    self.code.emit_u16(Opcode::LoadConst, idx)
                }
            }
            // For Float, Str, Bytes, Ellipsis - use LoadConst with Value::from
            _ => {
                let idx = self.code.add_const(Value::from(*literal))?;
                self.code.emit_u16(Opcode::LoadConst, idx)
            }
        }
    }

    // ========================================================================
    // Variable Operations
    // ========================================================================

    /// Compiles loading a variable onto the stack.
    ///
    /// At module level, `Local` scopes emits global opcodes
    /// because module-level locals live in the globals array.
    fn compile_name(&mut self, ident: &Identifier) -> Result<(), CompileError> {
        let slot = ident.namespace_id().as_u16();
        match ident.scope {
            NameScope::Local => {
                // True local - register name and mark as assigned for UnboundLocalError
                self.code.register_local_name(slot, ident.name_id);
                if self.is_module_scope {
                    self.code.emit_u16(Opcode::LoadGlobal, slot)
                } else {
                    self.code.emit_load_local(slot)
                }
            }
            NameScope::Global => {
                // Global name - only a "local" name at module scope
                if self.is_module_scope {
                    self.code.register_local_name(slot, ident.name_id);
                }
                self.code.emit_u16(Opcode::LoadGlobal, slot)
            }
            NameScope::Cell => {
                // Register the name for NameError messages (unbound free variable)
                self.code.register_local_name(slot, ident.name_id);
                // Emit local slot index — the VM reads the cell HeapId from the stack
                self.code.emit_u16(Opcode::LoadCell, slot)
            }
            NameScope::CompVar => {
                // Comprehension target read. Static analysis tells us
                // whether the corresponding `for` has stored to this slot
                // yet in the linear emission order:
                //
                // - Bound (`slot ∈ bound_comp_slots`): emit a regular
                //   `LoadLocal/W` — the comp var lives on the operand stack
                //   at `frame_locals + offset` (set up by
                //   `compile_comp_target_unpack` at the corresponding
                //   FOR_ITER / UNPACK step).
                // - Unbound: the corresponding `for` clause hasn't stored
                //   to the slot at this point in the comp (e.g. an earlier
                //   generator's iter references a later target). Emit
                //   `RaiseUnboundLocal(name_id)`; the name lives in the
                //   opcode so sibling comps with different unbound targets
                //   each report the correct variable.
                if self.bound_comp_slots.contains(&slot) {
                    let absolute = self.slot_offsets[slot as usize];
                    self.code.emit_load_local(absolute)
                } else {
                    self.code.emit_raise_unbound_local(ident.name_id)
                }
            }
        }
    }

    /// Compiles loading a variable in call context (e.g., `foo()` loads `foo`).
    ///
    /// For `Global` scope, emits a callable-aware load opcode that pushes
    /// `ExtFunction(name_id)` for undefined names instead of yielding
    /// `NameLookup`. This allows execution to reach `CallFunction`, which naturally
    /// yields `FunctionCall` — giving the host a chance to handle external function calls.
    ///
    /// For `Local` and `Cell` scopes, delegates to `compile_name` since those can't
    /// be external functions (they're always defined locally or captured).
    fn compile_name_callable(&mut self, ident: &Identifier) -> Result<(), CompileError> {
        match ident.scope {
            NameScope::Global => {
                // Global scope - name_id is encoded in the operand because global slot
                // indices are in a different namespace from local slots, so looking up
                // the name from the current frame's local_names would be incorrect
                self.code
                    .emit_load_global_callable(ident.namespace_id().as_u16(), ident.name_id)
            }
            // Local, Cell, and CompVar can't be external functions - use regular load
            NameScope::Local | NameScope::Cell | NameScope::CompVar => self.compile_name(ident),
        }
    }

    /// Compiles storing the top of stack to a variable.
    ///
    /// At module level, `Local` scope emits `StoreGlobal`
    /// because module-level locals live in the globals array.
    fn compile_store(&mut self, target: &Identifier) -> Result<(), CompileError> {
        let slot = target.namespace_id().as_u16();
        match target.scope {
            NameScope::Local => {
                // Module-level `Local` binds the global namespace; function-level
                // `Local` is a genuine local that may freely shadow a dunder name.
                if self.is_module_scope {
                    self.check_reserved_dunder_store(target)?;
                }
                self.code.register_local_name(slot, target.name_id);
                if self.is_module_scope {
                    self.code.emit_u16(Opcode::StoreGlobal, slot)
                } else {
                    self.code.emit_store_local(slot)
                }
            }
            NameScope::Global => {
                self.check_reserved_dunder_store(target)?;
                self.code.emit_u16(Opcode::StoreGlobal, slot)
            }
            NameScope::Cell => {
                // Emit local slot index — the VM reads the cell HeapId from the stack
                self.code.emit_u16(Opcode::StoreCell, slot)
            }
            NameScope::CompVar => {
                // Comp-var stores never go through `compile_store`. They are
                // handled by `compile_comp_target_unpack`, which leaves the
                // value on the operand stack as the natural result of
                // `FOR_ITER` (and any subsequent `UNPACK_SEQUENCE` /
                // `LIFT_TO_TOP` for nested tuples).
                unreachable!(
                    "compile_store called with NameScope::CompVar — comp targets are stored via compile_comp_target_unpack"
                )
            }
        }
    }

    /// Rejects assignment to a read-only module dunder at module/global scope.
    ///
    /// Monty exposes [`RESERVED_MODULE_DUNDERS`] with fixed values for CPython
    /// compatibility but, unlike CPython, has no module namespace to write into,
    /// so rebinding one is unsupported and surfaces as `NotImplementedError`.
    /// Only callers that bind the global namespace (module-`Local` and `Global`
    /// scopes) invoke this — function locals sharing these names are fine.
    fn check_reserved_dunder_store(&self, target: &Identifier) -> Result<(), CompileError> {
        let name = self.interns.get_str(target.name_id);
        if RESERVED_MODULE_DUNDERS.contains(&name) {
            Err(CompileError::not_implemented(
                format!("cannot reassign read-only module attribute '{name}'"),
                target.position,
            ))
        } else {
            Ok(())
        }
    }

    // ========================================================================
    // Binary Operator Compilation
    // ========================================================================

    /// Compiles a binary operation.
    ///
    /// `parent_pos` is the position of the full binary expression (e.g., `1 / 0`),
    /// which we restore before emitting the opcode so tracebacks show the right range.
    fn compile_binary_op(
        &mut self,
        left: &ExprLoc,
        op: &Operator,
        right: &ExprLoc,
        parent_pos: CodeRange,
    ) -> Result<(), CompileError> {
        match op {
            // Short-circuit AND: evaluate left, jump if falsy
            Operator::And => {
                self.compile_expr(left)?;
                let end_jump = self.code.emit_jump(Opcode::JumpIfFalseOrPop)?;
                self.compile_expr(right)?;
                self.code.patch_jump(end_jump)?;
            }

            // Short-circuit OR: evaluate left, jump if truthy
            Operator::Or => {
                self.compile_expr(left)?;
                let end_jump = self.code.emit_jump(Opcode::JumpIfTrueOrPop)?;
                self.compile_expr(right)?;
                self.code.patch_jump(end_jump)?;
            }

            // Regular binary operators
            _ => {
                self.compile_expr(left)?;
                self.compile_expr(right)?;
                // Restore the full expression's position for traceback caret range
                self.code.set_location(parent_pos, None);
                self.code.emit(operator_to_opcode(op))?;
            }
        }
        Ok(())
    }

    /// Compiles a chain comparison expression like `a < b < c < d`.
    ///
    /// Chain comparisons evaluate each intermediate value only once and short-circuit
    /// on the first false result. Uses stack manipulation to avoid namespace pollution.
    ///
    /// Bytecode strategy for `a < b < c`:
    /// ```text
    /// eval a              # Stack: [a]
    /// eval b              # Stack: [a, b]
    /// Dup                 # Stack: [a, b, b]
    /// Rot3                # Stack: [b, a, b]
    /// CompareLt           # Stack: [b, result1]
    /// JumpIfFalseOrPop    # if false: jump to cleanup; if true: pop, stack=[b]
    /// eval c              # Stack: [b, c]
    /// CompareLt           # Stack: [result2]
    /// Jump @end
    /// @cleanup:           # Stack: [b, False]
    /// Rot2                # Stack: [False, b]
    /// Pop                 # Stack: [False]
    /// @end:
    /// ```
    fn compile_chain_comparison(
        &mut self,
        left: &ExprLoc,
        comparisons: &[(CmpOperator, ExprLoc)],
        position: CodeRange,
    ) -> Result<(), CompileError> {
        let n = comparisons.len();

        // Compile leftmost operand
        self.compile_expr(left)?;

        // Track jump targets for short-circuit cleanup
        let mut cleanup_jumps = Vec::with_capacity(n - 1);

        for (i, (op, right)) in comparisons.iter().enumerate() {
            let is_last = i == n - 1;

            // Compile the right operand
            self.compile_expr(right)?;

            if !is_last {
                // Keep a copy of the intermediate for the next comparison
                self.code.emit(Opcode::Dup)?;
                // Reorder: [prev, curr, curr] -> [curr, prev, curr]
                self.code.emit(Opcode::Rot3)?;
            }

            // Emit comparison
            self.code.set_location(position, None);
            if let CmpOperator::ModEq(value) = op {
                let const_idx = self.code.add_const(Value::Int(*value))?;
                self.code.emit_u16(Opcode::CompareModEq, const_idx)?;
            } else {
                self.code.emit(cmp_operator_to_opcode(op))?;
            }

            if !is_last {
                // Short-circuit: if false, jump to cleanup
                let jump = self.code.emit_jump(Opcode::JumpIfFalseOrPop)?;
                cleanup_jumps.push(jump);
            }
        }

        // Jump past cleanup (result already on stack).
        let end_jump = self.code.emit_jump(Opcode::Jump)?;

        // Cleanup: remove the saved intermediate value, keep False result.
        for jump in cleanup_jumps {
            self.code.patch_jump(jump)?;
        }
        self.code.emit(Opcode::Rot2)?; // [False, intermediate]
        self.code.emit(Opcode::Pop)?; // [False]

        self.code.patch_jump(end_jump)?;
        Ok(())
    }

    // ========================================================================
    // Control Flow Compilation
    // ========================================================================

    /// Compiles an if/else statement.
    fn compile_if(
        &mut self,
        test: &ExprLoc,
        body: &[PreparedNode],
        or_else: &[PreparedNode],
    ) -> Result<(), CompileError> {
        self.compile_expr(test)?;

        if or_else.is_empty() {
            // Simple if without else
            let end_jump = self.code.emit_jump(Opcode::JumpIfFalse)?;
            self.compile_block(body)?;
            self.code.patch_jump(end_jump)?;
        } else {
            // If with else
            let else_jump = self.code.emit_jump(Opcode::JumpIfFalse)?;
            self.compile_block(body)?;
            let end_jump = self.code.emit_jump(Opcode::Jump)?;
            self.code.patch_jump(else_jump)?;
            self.compile_block(or_else)?;
            self.code.patch_jump(end_jump)?;
        }
        Ok(())
    }

    /// Compiles a ternary conditional expression.
    fn compile_if_else_expr(&mut self, test: &ExprLoc, body: &ExprLoc, orelse: &ExprLoc) -> Result<(), CompileError> {
        self.compile_expr(test)?;
        let else_jump = self.code.emit_jump(Opcode::JumpIfFalse)?;
        self.compile_expr(body)?;
        let end_jump = self.code.emit_jump(Opcode::Jump)?;
        self.code.patch_jump(else_jump)?;
        self.compile_expr(orelse)?;
        self.code.patch_jump(end_jump)?;
        Ok(())
    }

    /// Compiles a function call expression.
    ///
    /// For builtin calls with positional-only arguments, emits the optimized `CallBuiltin`
    /// opcode which avoids pushing/popping the callable on the stack.
    ///
    /// For other calls, pushes the callable onto the stack, then all arguments, then emits
    /// `CallFunction` or `CallFunctionKw`.
    ///
    /// The `call_pos` is the position of the full call expression for proper traceback caret.
    fn compile_call(&mut self, callable: &Callable, args: &ArgExprs, call_pos: CodeRange) -> Result<(), CompileError> {
        // Check if we can use the optimized CallBuiltinFunction path:
        // - Callable must be a builtin function (known at compile time)
        // - Arguments must be positional-only (Empty, One, Two, or Args)
        if let Callable::Builtin(Builtins::Function(builtin_func)) = callable
            && let Some(arg_count) = self.compile_builtin_call(args, call_pos)?
        {
            // Optimization applied - CallBuiltinFunction emitted
            self.code.set_location(call_pos, None);
            self.code.emit_call_builtin_function(*builtin_func as u8, arg_count)?;
            return Ok(());
        }
        // Fall through to standard path for kwargs/unpacking

        // Check if we can use the optimized CallBuiltinType path:
        // - Callable must be a builtin type constructor (known at compile time)
        // - Arguments must be positional-only (Empty, One, Two, or Args)
        if let Callable::Builtin(Builtins::Type(t)) = callable
            && let Some(type_id) = t.callable_to_u8()
            && let Some(arg_count) = self.compile_builtin_call(args, call_pos)?
        {
            // Optimization applied - CallBuiltinType emitted
            self.code.set_location(call_pos, None);
            self.code.emit_call_builtin_type(type_id, arg_count)?;
            return Ok(());
        }
        // Fall through to standard path for kwargs/unpacking or non-callable types

        // Standard path: push callable, compile args, emit CallFunction/CallFunctionKw
        // Push the callable (use name position for NameError caret range)
        match callable {
            Callable::Builtin(builtin) => {
                let idx = self.code.add_const(Value::Builtin(*builtin))?;
                self.code.emit_u16(Opcode::LoadConst, idx)?;
            }
            Callable::Name(ident) => {
                // Use callable-aware load opcodes so undefined names produce ExtFunction
                // instead of yielding NameLookup, allowing CallFunction to yield FunctionCall
                self.code.set_location(ident.position, None);
                self.compile_name_callable(ident)?;
            }
        }

        // Compile arguments and emit the call
        // Restore full call position before CallFunction for call-related errors
        match args {
            ArgExprs::Empty => {
                self.code.set_location(call_pos, None);
                self.code.emit_u8(Opcode::CallFunction, 0)?;
            }
            ArgExprs::One(arg) => {
                self.compile_expr(arg)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u8(Opcode::CallFunction, 1)?;
            }
            ArgExprs::Two(arg1, arg2) => {
                self.compile_expr(arg1)?;
                self.compile_expr(arg2)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u8(Opcode::CallFunction, 2)?;
            }
            ArgExprs::Args(args) => {
                // Check argument count limit before compiling
                if args.len() > MAX_CALL_ARGS {
                    return Err(CompileError::new(
                        format!("more than {MAX_CALL_ARGS} positional arguments in function call"),
                        call_pos,
                    ));
                }
                for arg in args {
                    self.compile_expr(arg)?;
                }
                let arg_count = u8::try_from(args.len()).expect("argument count exceeds u8");
                self.code.set_location(call_pos, None);
                self.code.emit_u8(Opcode::CallFunction, arg_count)?;
            }
            ArgExprs::Kwargs(kwargs) => {
                // Check keyword argument count limit
                if kwargs.len() > MAX_CALL_ARGS {
                    return Err(CompileError::new(
                        format!("more than {MAX_CALL_ARGS} keyword arguments in function call"),
                        call_pos,
                    ));
                }
                // Keyword-only call: compile kwarg values and emit CallFunctionKw
                let mut kwname_ids = Vec::with_capacity(kwargs.len());
                for kwarg in kwargs {
                    self.compile_expr(&kwarg.value)?;
                    kwname_ids.push(check_name_index_u16(kwarg.key.name_id, call_pos)?);
                }
                self.code.set_location(call_pos, None);
                self.code.emit_call_function_kw(0, &kwname_ids)?;
            }
            ArgExprs::ArgsKargs {
                args,
                var_args,
                kwargs,
                var_kwargs,
            } => {
                // Mixed positional and keyword arguments - may include *args or **kwargs unpacking
                if var_args.is_some() || var_kwargs.is_some() {
                    // Use CallFunctionEx for unpacking - no limit on this path since
                    // args are built into a tuple dynamically at runtime
                    self.compile_call_with_unpacking(
                        callable,
                        args.as_ref(),
                        var_args.as_ref(),
                        kwargs.as_ref(),
                        var_kwargs.as_ref(),
                        call_pos,
                    )?;
                } else {
                    // No unpacking - use CallFunctionKw for efficiency
                    // Check limits before compiling
                    let pos_count = args.as_ref().map_or(0, Vec::len);
                    let kw_count = kwargs.as_ref().map_or(0, Vec::len);

                    if pos_count > MAX_CALL_ARGS {
                        return Err(CompileError::new(
                            format!("more than {MAX_CALL_ARGS} positional arguments in function call"),
                            call_pos,
                        ));
                    }
                    if kw_count > MAX_CALL_ARGS {
                        return Err(CompileError::new(
                            format!("more than {MAX_CALL_ARGS} keyword arguments in function call"),
                            call_pos,
                        ));
                    }

                    // Compile positional args
                    if let Some(args) = args {
                        for arg in args {
                            self.compile_expr(arg)?;
                        }
                    }

                    // Compile kwarg values and collect names
                    let mut kwname_ids = Vec::new();
                    if let Some(kwargs) = kwargs {
                        for kwarg in kwargs {
                            self.compile_expr(&kwarg.value)?;
                            kwname_ids.push(check_name_index_u16(kwarg.key.name_id, call_pos)?);
                        }
                    }

                    self.code.set_location(call_pos, None);
                    self.code.emit_call_function_kw(
                        u8::try_from(pos_count).expect("positional arg count exceeds u8"),
                        &kwname_ids,
                    )?;
                }
            }
            ArgExprs::GeneralizedCall { args, kwargs } => {
                // PEP 448: generalized unpacking — multiple *args or **kwargs.
                // Callable was already pushed above this match; delegate to the helper.
                let func_name_id = self.get_callable_name_id(callable)?;
                self.compile_generalized_call_body(args, kwargs, func_name_id, call_pos)?;
            }
        }
        Ok(())
    }

    /// Compiles function call arguments and emits the call instruction.
    ///
    /// This is used when the callable is already on the stack (e.g., from compiling an expression).
    /// It compiles the arguments, then emits `CallFunction` or `CallFunctionKw` as appropriate.
    fn compile_call_args(&mut self, args: &ArgExprs, call_pos: CodeRange) -> Result<(), CompileError> {
        match args {
            ArgExprs::Empty => {
                self.code.set_location(call_pos, None);
                self.code.emit_u8(Opcode::CallFunction, 0)?;
            }
            ArgExprs::One(arg) => {
                self.compile_expr(arg)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u8(Opcode::CallFunction, 1)?;
            }
            ArgExprs::Two(arg1, arg2) => {
                self.compile_expr(arg1)?;
                self.compile_expr(arg2)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u8(Opcode::CallFunction, 2)?;
            }
            ArgExprs::Args(args) => {
                if args.len() > MAX_CALL_ARGS {
                    return Err(CompileError::new(
                        format!("more than {MAX_CALL_ARGS} positional arguments in function call"),
                        call_pos,
                    ));
                }
                for arg in args {
                    self.compile_expr(arg)?;
                }
                let arg_count = u8::try_from(args.len()).expect("argument count exceeds u8");
                self.code.set_location(call_pos, None);
                self.code.emit_u8(Opcode::CallFunction, arg_count)?;
            }
            ArgExprs::Kwargs(kwargs) => {
                if kwargs.len() > MAX_CALL_ARGS {
                    return Err(CompileError::new(
                        format!("more than {MAX_CALL_ARGS} keyword arguments in function call"),
                        call_pos,
                    ));
                }
                let mut kwname_ids = Vec::with_capacity(kwargs.len());
                for kwarg in kwargs {
                    self.compile_expr(&kwarg.value)?;
                    kwname_ids.push(check_name_index_u16(kwarg.key.name_id, call_pos)?);
                }
                self.code.set_location(call_pos, None);
                self.code.emit_call_function_kw(0, &kwname_ids)?;
            }
            ArgExprs::ArgsKargs {
                args,
                kwargs,
                var_args,
                var_kwargs,
            } => {
                // Mixed positional and keyword arguments - may include *args or **kwargs unpacking
                if var_args.is_some() || var_kwargs.is_some() {
                    // Use CallFunctionExtended for unpacking - no limit on this path since
                    // args are built into a tuple dynamically at runtime.
                    // Callable is already on stack, so we just need to build args and kwargs.
                    self.compile_call_args_with_unpacking(
                        args.as_ref(),
                        var_args.as_ref(),
                        kwargs.as_ref(),
                        var_kwargs.as_ref(),
                        call_pos,
                    )?;
                } else {
                    // No unpacking - use CallFunctionKw for efficiency
                    let pos_args = args.as_deref().unwrap_or(&[]);
                    let kw_args = kwargs.as_deref().unwrap_or(&[]);
                    let pos_count = pos_args.len();
                    let kw_count = kw_args.len();

                    // Check limits separately (same as direct calls)
                    if pos_count > MAX_CALL_ARGS {
                        return Err(CompileError::new(
                            format!("more than {MAX_CALL_ARGS} positional arguments in function call"),
                            call_pos,
                        ));
                    }
                    if kw_count > MAX_CALL_ARGS {
                        return Err(CompileError::new(
                            format!("more than {MAX_CALL_ARGS} keyword arguments in function call"),
                            call_pos,
                        ));
                    }

                    // Compile positional args
                    for arg in pos_args {
                        self.compile_expr(arg)?;
                    }

                    // Compile keyword args
                    let mut kwname_ids = Vec::with_capacity(kw_count);
                    for kwarg in kw_args {
                        self.compile_expr(&kwarg.value)?;
                        kwname_ids.push(check_name_index_u16(kwarg.key.name_id, call_pos)?);
                    }

                    self.code.set_location(call_pos, None);
                    self.code.emit_call_function_kw(
                        u8::try_from(pos_count).expect("positional arg count exceeds u8"),
                        &kwname_ids,
                    )?;
                }
            }
            ArgExprs::GeneralizedCall { args, kwargs } => {
                // PEP 448: generalized unpacking — callable is already on the stack.
                // Use 0xFFFF as func_name_id since we don't know the callee name here.
                self.compile_generalized_call_body(args, kwargs, 0xFFFF, call_pos)?;
            }
        }
        Ok(())
    }

    /// Compiles arguments with `*args` and/or `**kwargs` unpacking when callable is already on stack.
    ///
    /// This is used for expression calls (e.g., `(lambda *a: a)(*xs)`) where the callable
    /// is compiled as an expression and is already on the stack.
    ///
    /// Stack layout: callable (on stack) -> callable, args_tuple, kwargs_dict?
    fn compile_call_args_with_unpacking(
        &mut self,
        args: Option<&Vec<ExprLoc>>,
        var_args: Option<&ExprLoc>,
        kwargs: Option<&Vec<Kwarg>>,
        var_kwargs: Option<&ExprLoc>,
        call_pos: CodeRange,
    ) -> Result<(), CompileError> {
        // 1. Build args tuple
        // Push regular positional args and build list
        let pos_count = args.map_or(0, Vec::len);
        if let Some(args) = args {
            for arg in args {
                self.compile_expr(arg)?;
            }
        }
        let pos_count_u16 = check_collection_size_u16(pos_count, call_pos)?;
        self.code.emit_u16(Opcode::BuildList, pos_count_u16)?;

        // Extend with *args if present
        if let Some(var_args_expr) = var_args {
            self.compile_expr(var_args_expr)?;
            self.code.emit(Opcode::ListExtend)?;
        }

        // Convert list to tuple
        self.code.emit(Opcode::ListToTuple)?;

        // 2. Build kwargs dict (if we have kwargs or var_kwargs)
        let has_kwargs = kwargs.is_some() || var_kwargs.is_some();
        if has_kwargs {
            // Build dict from regular kwargs
            let kw_count = kwargs.map_or(0, Vec::len);
            if let Some(kwargs) = kwargs {
                for kwarg in kwargs {
                    // Push key as interned string constant
                    let key_const = self.code.add_const(Value::InternString(kwarg.key.name_id))?;
                    self.code.emit_u16(Opcode::LoadConst, key_const)?;
                    // Push value
                    self.compile_expr(&kwarg.value)?;
                }
            }
            let kw_count_u16 = check_collection_size_u16(kw_count, call_pos)?;
            self.code.emit_u16(Opcode::BuildDict, kw_count_u16)?;

            // Merge **kwargs if present
            // Use 0xFFFF for func_name_id (like builtins) since we don't have a name
            if let Some(var_kwargs_expr) = var_kwargs {
                self.compile_expr(var_kwargs_expr)?;
                self.code.emit_u16(Opcode::DictMerge, 0xFFFF)?;
            }
        }

        // 3. Call the function
        self.code.set_location(call_pos, None);
        let flags = u8::from(has_kwargs);
        self.code.emit_u8(Opcode::CallFunctionExtended, flags)?;
        Ok(())
    }

    /// Compiles arguments for a builtin call and returns the arg count if optimization can be used.
    ///
    /// Returns `Some(arg_count)` if the call uses positional-only arguments (CallBuiltinFunction applicable).
    /// Returns `None` if the call uses kwargs or unpacking (must use standard CallFunction path).
    ///
    /// When `Some` is returned, arguments have been compiled onto the stack.
    fn compile_builtin_call(&mut self, args: &ArgExprs, call_pos: CodeRange) -> Result<Option<u8>, CompileError> {
        match args {
            ArgExprs::Empty => Ok(Some(0)),
            ArgExprs::One(arg) => {
                self.compile_expr(arg)?;
                Ok(Some(1))
            }
            ArgExprs::Two(arg1, arg2) => {
                self.compile_expr(arg1)?;
                self.compile_expr(arg2)?;
                Ok(Some(2))
            }
            ArgExprs::Args(args) => {
                if args.len() > MAX_CALL_ARGS {
                    return Err(CompileError::new(
                        format!("more than {MAX_CALL_ARGS} positional arguments in function call"),
                        call_pos,
                    ));
                }
                for arg in args {
                    self.compile_expr(arg)?;
                }
                Ok(Some(u8::try_from(args.len()).expect("argument count exceeds u8")))
            }
            // Kwargs or unpacking - fall back to standard path
            ArgExprs::Kwargs(_) | ArgExprs::ArgsKargs { .. } | ArgExprs::GeneralizedCall { .. } => Ok(None),
        }
    }

    /// Compiles a function call with `*args` and/or `**kwargs` unpacking.
    ///
    /// This generates bytecode to build an args tuple and kwargs dict dynamically,
    /// then calls the function using `CallFunctionEx`.
    ///
    /// Stack layout for call:
    /// - callable (already on stack)
    /// - args tuple
    /// - kwargs dict (if present)
    fn compile_call_with_unpacking(
        &mut self,
        callable: &Callable,
        args: Option<&Vec<ExprLoc>>,
        var_args: Option<&ExprLoc>,
        kwargs: Option<&Vec<Kwarg>>,
        var_kwargs: Option<&ExprLoc>,
        call_pos: CodeRange,
    ) -> Result<(), CompileError> {
        // Get function name for error messages. Builtins use their real interned name
        // so duplicate-kwargs errors from **unpacking match CPython.
        let func_name_id = self.get_callable_name_id(callable)?;

        // 1. Build args tuple
        // Push regular positional args and build list
        let pos_count = args.map_or(0, Vec::len);
        if let Some(args) = args {
            for arg in args {
                self.compile_expr(arg)?;
            }
        }
        let pos_count_u16 = check_collection_size_u16(pos_count, call_pos)?;
        self.code.emit_u16(Opcode::BuildList, pos_count_u16)?;

        // Extend with *args if present
        if let Some(var_args_expr) = var_args {
            self.compile_expr(var_args_expr)?;
            self.code.emit(Opcode::ListExtend)?;
        }

        // Convert list to tuple
        self.code.emit(Opcode::ListToTuple)?;

        // 2. Build kwargs dict (if we have kwargs or var_kwargs)
        let has_kwargs = kwargs.is_some() || var_kwargs.is_some();
        if has_kwargs {
            // Build dict from regular kwargs
            let kw_count = kwargs.map_or(0, Vec::len);
            if let Some(kwargs) = kwargs {
                for kwarg in kwargs {
                    // Push key as interned string constant
                    let key_const = self.code.add_const(Value::InternString(kwarg.key.name_id))?;
                    self.code.emit_u16(Opcode::LoadConst, key_const)?;
                    // Push value
                    self.compile_expr(&kwarg.value)?;
                }
            }
            let kw_count_u16 = check_collection_size_u16(kw_count, call_pos)?;
            self.code.emit_u16(Opcode::BuildDict, kw_count_u16)?;

            // Merge **kwargs if present
            if let Some(var_kwargs_expr) = var_kwargs {
                self.compile_expr(var_kwargs_expr)?;
                self.code.emit_u16(Opcode::DictMerge, func_name_id)?;
            }
        }

        // 3. Call the function
        self.code.set_location(call_pos, None);
        let flags = u8::from(has_kwargs);
        self.code.emit_u8(Opcode::CallFunctionExtended, flags)?;
        Ok(())
    }

    /// Returns the best available function name id for call-site error messages.
    ///
    /// This is primarily used by `DictMerge` during `**kwargs` unpacking so
    /// duplicate-key and non-mapping errors can mention the actual callee name.
    /// When the callable is not a named local/global, we still try to resolve
    /// builtin functions, builtin exception constructors, and builtin types to
    /// their interned public names.
    fn get_callable_name_id(&self, callable: &Callable) -> Result<u16, CompileError> {
        match callable {
            Callable::Name(ident) => check_name_index_u16(ident.name_id, ident.position),
            Callable::Builtin(builtin) => Ok(self.get_builtin_name_id(*builtin).unwrap_or(0xFFFF)),
        }
    }

    /// Resolves a builtin callable to its interned public name, if available.
    ///
    /// Returning `None` falls back to `<unknown>` in the VM, which is still
    /// correct but less helpful. In practice these names should already be
    /// interned during preparation because builtin names are resolved from source.
    fn get_builtin_name_id(&self, builtin: Builtins) -> Option<u16> {
        let name_id = match builtin {
            Builtins::Function(function) => {
                let name: &'static str = function.into();
                self.interns.get_string_id_by_name(name)?
            }
            Builtins::ExcType(exc_type) => self.interns.get_string_id_by_name(&exc_type.to_string())?,
            Builtins::Type(type_) => {
                let name = type_.builtin_name()?;
                self.interns.get_string_id_by_name(name)?
            }
        };

        u16::try_from(name_id.index()).ok()
    }

    /// Compiles an attribute call on an object.
    ///
    /// The object should already be on the stack. This compiles the arguments
    /// and emits a CallAttr opcode with the attribute name and arg count.
    fn compile_method_call(
        &mut self,
        attr: &EitherStr,
        args: &ArgExprs,
        call_pos: CodeRange,
    ) -> Result<(), CompileError> {
        // Get the interned attribute name, converted up-front so the limit check
        // happens once per method call rather than at every emit-site below.
        let name_id = attr.string_id().expect("CallAttr requires interned attr name");
        let name_idx = check_name_index_u16(name_id, call_pos)?;

        // Compile arguments based on the argument type
        match args {
            ArgExprs::Empty => {
                self.code.set_location(call_pos, None);
                self.code.emit_u16_u8(Opcode::CallAttr, name_idx, 0)?;
            }
            ArgExprs::One(arg) => {
                self.compile_expr(arg)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u16_u8(Opcode::CallAttr, name_idx, 1)?;
            }
            ArgExprs::Two(arg1, arg2) => {
                self.compile_expr(arg1)?;
                self.compile_expr(arg2)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u16_u8(Opcode::CallAttr, name_idx, 2)?;
            }
            ArgExprs::Args(args) => {
                // Check argument count limit
                if args.len() > MAX_CALL_ARGS {
                    return Err(CompileError::new(
                        format!("more than {MAX_CALL_ARGS} arguments in method call"),
                        call_pos,
                    ));
                }
                for arg in args {
                    self.compile_expr(arg)?;
                }
                let arg_count = u8::try_from(args.len()).expect("argument count exceeds u8");
                self.code.set_location(call_pos, None);
                self.code.emit_u16_u8(Opcode::CallAttr, name_idx, arg_count)?;
            }
            ArgExprs::Kwargs(kwargs) => {
                // Keyword-only method call
                if kwargs.len() > MAX_CALL_ARGS {
                    return Err(CompileError::new(
                        format!("more than {MAX_CALL_ARGS} keyword arguments in method call"),
                        call_pos,
                    ));
                }
                // Compile kwarg values and collect names
                let mut kwname_ids = Vec::with_capacity(kwargs.len());
                for kwarg in kwargs {
                    self.compile_expr(&kwarg.value)?;
                    kwname_ids.push(check_name_index_u16(kwarg.key.name_id, call_pos)?);
                }
                self.code.set_location(call_pos, None);
                self.code.emit_call_attr_kw(name_idx, 0, &kwname_ids)?;
            }
            ArgExprs::ArgsKargs {
                args,
                kwargs,
                var_args,
                var_kwargs,
            } => {
                // Check if there's unpacking - use CallAttrExtended
                if var_args.is_some() || var_kwargs.is_some() {
                    return self.compile_method_call_with_unpacking(
                        name_id,
                        args.as_ref(),
                        var_args.as_ref(),
                        kwargs.as_ref(),
                        var_kwargs.as_ref(),
                        call_pos,
                    );
                }

                // No unpacking - use CallAttrKw for efficiency
                let pos_count = args.as_ref().map_or(0, Vec::len);
                let kw_count = kwargs.as_ref().map_or(0, Vec::len);

                if pos_count > MAX_CALL_ARGS {
                    return Err(CompileError::new(
                        format!("more than {MAX_CALL_ARGS} positional arguments in method call"),
                        call_pos,
                    ));
                }
                if kw_count > MAX_CALL_ARGS {
                    return Err(CompileError::new(
                        format!("more than {MAX_CALL_ARGS} keyword arguments in method call"),
                        call_pos,
                    ));
                }

                // Compile positional args
                if let Some(args) = args {
                    for arg in args {
                        self.compile_expr(arg)?;
                    }
                }

                // Compile kwarg values and collect names
                let mut kwname_ids = Vec::new();
                if let Some(kwargs) = kwargs {
                    for kwarg in kwargs {
                        self.compile_expr(&kwarg.value)?;
                        kwname_ids.push(check_name_index_u16(kwarg.key.name_id, call_pos)?);
                    }
                }

                self.code.set_location(call_pos, None);
                self.code.emit_call_attr_kw(
                    name_idx,
                    u8::try_from(pos_count).expect("positional arg count exceeds u8"),
                    &kwname_ids,
                )?;
            }
            ArgExprs::GeneralizedCall { args, kwargs } => {
                // PEP 448: generalized unpacking on a method call.
                // Receiver is already on the stack; build args tuple and kwargs dict,
                // then emit CallAttrExtended.
                let func_name_id = name_idx;
                let has_kwargs = !kwargs.is_empty();

                // 1. Build args tuple
                self.code.emit_u16(Opcode::BuildList, 0)?;
                for arg in args {
                    match arg {
                        CallArg::Value(e) => {
                            self.compile_expr(e)?;
                            self.code.emit_u8(Opcode::ListAppend, 0)?;
                        }
                        CallArg::Unpack(e) => {
                            self.compile_expr(e)?;
                            self.code.emit(Opcode::ListExtend)?;
                        }
                    }
                }
                self.code.emit(Opcode::ListToTuple)?;

                // 2. Build kwargs dict (if any)
                if has_kwargs {
                    self.code.emit_u16(Opcode::BuildDict, 0)?;
                    for kwarg in kwargs {
                        match kwarg {
                            CallKwarg::Named(kw) => {
                                let key_const = self.code.add_const(Value::InternString(kw.key.name_id))?;
                                self.code.emit_u16(Opcode::LoadConst, key_const)?;
                                self.compile_expr(&kw.value)?;
                                self.code.emit_u16(Opcode::BuildDict, 1)?;
                                self.code.emit_u16(Opcode::MethodDictMerge, func_name_id)?;
                            }
                            CallKwarg::Unpack(e) => {
                                self.compile_expr(e)?;
                                self.code.emit_u16(Opcode::MethodDictMerge, func_name_id)?;
                            }
                        }
                    }
                }

                // 3. Emit CallAttrExtended
                self.code.set_location(call_pos, None);
                let flags = u8::from(has_kwargs);
                self.code.emit_u16_u8(Opcode::CallAttrExtended, func_name_id, flags)?;
            }
        }
        Ok(())
    }

    /// Compiles a method call with `*args` and/or `**kwargs` unpacking.
    ///
    /// The receiver object should already be on the stack. This builds the args tuple
    /// and optional kwargs dict, then emits `CallAttrExtended`.
    fn compile_method_call_with_unpacking(
        &mut self,
        name_id: StringId,
        args: Option<&Vec<ExprLoc>>,
        var_args: Option<&ExprLoc>,
        kwargs: Option<&Vec<Kwarg>>,
        var_kwargs: Option<&ExprLoc>,
        call_pos: CodeRange,
    ) -> Result<(), CompileError> {
        // Convert the attribute name id up front so the overflow check happens
        // once and both `DictMerge` (for error messages) and `CallAttrExtended`
        // can reuse the converted value.
        let name_idx = check_name_index_u16(name_id, call_pos)?;
        // 1. Build args tuple
        // Push regular positional args and build list
        let pos_count = args.map_or(0, Vec::len);
        if let Some(args) = args {
            for arg in args {
                self.compile_expr(arg)?;
            }
        }
        let pos_count_u16 = check_collection_size_u16(pos_count, call_pos)?;
        self.code.emit_u16(Opcode::BuildList, pos_count_u16)?;

        // Extend with *args if present
        if let Some(var_args_expr) = var_args {
            self.compile_expr(var_args_expr)?;
            self.code.emit(Opcode::ListExtend)?;
        }

        // Convert list to tuple
        self.code.emit(Opcode::ListToTuple)?;

        // 2. Build kwargs dict (if we have kwargs or var_kwargs)
        let has_kwargs = kwargs.is_some() || var_kwargs.is_some();
        if has_kwargs {
            // Build dict from regular kwargs
            let kw_count = kwargs.map_or(0, Vec::len);
            if let Some(kwargs) = kwargs {
                for kwarg in kwargs {
                    // Push key as interned string constant
                    let key_const = self.code.add_const(Value::InternString(kwarg.key.name_id))?;
                    self.code.emit_u16(Opcode::LoadConst, key_const)?;
                    // Push value
                    self.compile_expr(&kwarg.value)?;
                }
            }
            let kw_count_u16 = check_collection_size_u16(kw_count, call_pos)?;
            self.code.emit_u16(Opcode::BuildDict, kw_count_u16)?;

            // Merge **kwargs if present
            if let Some(var_kwargs_expr) = var_kwargs {
                self.compile_expr(var_kwargs_expr)?;
                // Method-call form — `MethodDictMerge` qualifies the duplicate-
                // kwarg error with the receiver's type (e.g. `list.sort()`).
                self.code.emit_u16(Opcode::MethodDictMerge, name_idx)?;
            }
        }

        // 3. Call the method with CallAttrExtended
        self.code.set_location(call_pos, None);
        let flags = u8::from(has_kwargs);
        self.code.emit_u16_u8(Opcode::CallAttrExtended, name_idx, flags)?;
        Ok(())
    }

    /// Shared body for PEP 448 generalized calls with multiple `*args` and/or `**kwargs`.
    ///
    /// Assumes the callable is already on the stack (pushed by the caller).
    /// Emits:
    ///   1. `BuildList(0)` + per-item `ListAppend`/`ListExtend` + `ListToTuple` for args.
    ///   2. `BuildDict(0)` + per-item `BuildDict(1)+DictMerge`/`DictMerge` for kwargs (if any).
    ///   3. `CallFunctionExtended(flags)`.
    ///
    /// `func_name_id` is used in `DictMerge` error messages; pass `0xFFFF` when unknown.
    ///
    /// Stack transition (callable already on stack):
    ///   `[callable]` → `[callable, args_tuple]` → `[callable, args_tuple, kwargs_dict?]`
    ///   → `[result]`
    fn compile_generalized_call_body(
        &mut self,
        args: &[CallArg],
        kwargs: &[CallKwarg],
        func_name_id: u16,
        call_pos: CodeRange,
    ) -> Result<(), CompileError> {
        // 1. Build args tuple
        self.code.emit_u16(Opcode::BuildList, 0)?;
        for arg in args {
            match arg {
                CallArg::Value(e) => {
                    self.compile_expr(e)?;
                    self.code.emit_u8(Opcode::ListAppend, 0)?;
                }
                CallArg::Unpack(e) => {
                    self.compile_expr(e)?;
                    self.code.emit(Opcode::ListExtend)?;
                }
            }
        }
        self.code.emit(Opcode::ListToTuple)?;

        // 2. Build kwargs dict (if any)
        let has_kwargs = !kwargs.is_empty();
        if has_kwargs {
            // Start with an empty dict, then merge each kwarg one at a time via DictMerge
            // so that duplicates (including Named+Unpack ordering) raise TypeError correctly.
            self.code.emit_u16(Opcode::BuildDict, 0)?;
            for kwarg in kwargs {
                match kwarg {
                    CallKwarg::Named(kw) => {
                        // Wrap key+value in a single-item dict, then merge into kwargs dict.
                        let key_const = self.code.add_const(Value::InternString(kw.key.name_id))?;
                        self.code.emit_u16(Opcode::LoadConst, key_const)?;
                        self.compile_expr(&kw.value)?;
                        self.code.emit_u16(Opcode::BuildDict, 1)?;
                        self.code.emit_u16(Opcode::DictMerge, func_name_id)?;
                    }
                    CallKwarg::Unpack(e) => {
                        self.compile_expr(e)?;
                        self.code.emit_u16(Opcode::DictMerge, func_name_id)?;
                    }
                }
            }
        }

        // 3. Emit the extended call
        self.code.set_location(call_pos, None);
        let flags = u8::from(has_kwargs);
        self.code.emit_u8(Opcode::CallFunctionExtended, flags)?;
        Ok(())
    }

    /// Compiles a for loop.
    fn compile_for(
        &mut self,
        target: &UnpackTarget,
        iter: &ExprLoc,
        body: &[PreparedNode],
        or_else: &[PreparedNode],
    ) -> Result<(), CompileError> {
        // Compile iterator expression
        self.compile_expr(iter)?;
        // Convert to iterator
        self.code.emit(Opcode::GetIter)?;

        // Loop start
        let loop_start = self.code.current_jump_target();

        // Push loop info for break/continue
        self.loop_stack.push(LoopInfo {
            start: loop_start,
            break_jumps: Vec::new(),
            has_iterator_on_stack: true,
        });

        // ForIter: advance iterator or jump to end
        let end_jump = self.code.emit_jump(Opcode::ForIter)?;

        // Store current value to target (handles both single identifiers and tuple unpacking)
        self.compile_unpack_target(target)?;

        // Compile body
        self.compile_block(body)?;

        // Jump back to loop start
        self.code.emit_jump_to(Opcode::Jump, loop_start)?;
        // End of loop - ForIter jumps here when iterator is exhausted
        self.code.patch_jump(end_jump)?;

        // Pop loop info before compiling else block
        let loop_info = self.loop_stack.pop().expect("loop stack underflow");

        // Compile else block (runs if loop completed without break)
        if !or_else.is_empty() {
            self.compile_block(or_else)?;
        }

        // Patch break jumps to here - AFTER the else block so break skips else
        for break_jump in loop_info.break_jumps {
            self.code.patch_jump(break_jump)?;
        }

        Ok(())
    }

    /// Compiles a while loop.
    ///
    /// The bytecode structure:
    /// ```text
    /// loop_start:
    ///   [evaluate condition]
    ///   JumpIfFalse -> end_jump
    ///   [body]
    ///   Jump -> loop_start
    /// end_jump:
    ///   [else block]
    /// [break patches here]
    /// ```
    ///
    /// Key differences from `for` loops:
    /// - No `GetIter` (no iterator)
    /// - No `ForIter` (use `JumpIfFalse` instead)
    /// - `continue` jumps to condition evaluation
    /// - `break` doesn't need to pop iterator (nothing extra on stack)
    fn compile_while(
        &mut self,
        test: &ExprLoc,
        body: &[PreparedNode],
        or_else: &[PreparedNode],
    ) -> Result<(), CompileError> {
        let loop_start = self.code.current_jump_target();

        self.loop_stack.push(LoopInfo {
            start: loop_start,
            break_jumps: Vec::new(),
            has_iterator_on_stack: false,
        });

        self.compile_expr(test)?;
        let end_jump = self.code.emit_jump(Opcode::JumpIfFalse)?;

        self.compile_block(body)?;
        self.code.emit_jump_to(Opcode::Jump, loop_start)?;

        self.code.patch_jump(end_jump)?;
        let loop_info = self.loop_stack.pop().expect("loop stack underflow");

        if !or_else.is_empty() {
            self.compile_block(or_else)?;
        }

        for break_jump in loop_info.break_jumps {
            self.code.patch_jump(break_jump)?;
        }

        Ok(())
    }

    /// Compiles a break statement.
    ///
    /// Break exits the innermost loop and skips its else block. If inside a
    /// try-finally, the finally block must run first.
    ///
    /// The bytecode without finally:
    /// 1. Clean up exception state if inside except handler
    /// 2. Pop the iterator if in a `for` loop (still on stack during loop body)
    /// 3. Jump to after the else block
    ///
    /// With finally:
    /// 1. Clean up exception state if inside except handler
    /// 2. Pop the iterator if in a `for` loop
    /// 3. Jump to "finally with break" path (patched when try compilation completes)
    /// 4. That path runs finally, then jumps to after the else block
    fn compile_break(&mut self, position: CodeRange) -> Result<(), CompileError> {
        if self.loop_stack.is_empty() {
            return Err(CompileError::new("'break' outside loop", position));
        }

        let target_loop_depth = self.loop_stack.len() - 1;

        // If inside except handlers, clear each enclosing exception_stack
        // entry.
        for _ in 0..self.except_handler_depth {
            self.code.emit(Opcode::ClearException)?;
        }

        // Check if we need to go through any finally blocks
        // We need to run finally if break crosses the try boundary, i.e., if
        // we're breaking from a loop that existed before the try started.
        let routes_through_finally = self
            .finally_targets
            .last()
            .is_some_and(|ft| target_loop_depth < ft.loop_depth_at_entry);

        if routes_through_finally {
            // Routed path: leave the iterator on the stack — the chain of
            // finally trailers may have its own per-block stack contributions
            // (e.g. a `with` block sits on top of the iterator), and popping
            // the iterator here would pop the wrong slot. The outermost
            // trailer (`compile_control_flow_after_finally`) pops the
            // iterator just before jumping to the loop's break target.
            let jump = self.code.emit_jump(Opcode::Jump)?;
            self.finally_targets
                .last_mut()
                .expect("checked above")
                .break_jumps
                .push(BreakContinueThruFinally {
                    jump,
                    target_loop_depth,
                });
        } else {
            // Direct path: pop the iterator (if any) and jump to loop end.
            if self.loop_stack[target_loop_depth].has_iterator_on_stack {
                self.code.emit(Opcode::Pop)?;
            }
            let jump = self.code.emit_jump(Opcode::Jump)?;
            self.loop_stack[target_loop_depth].break_jumps.push(jump);
        }

        Ok(())
    }

    /// Compiles a continue statement.
    ///
    /// Continue jumps back to the loop start (the ForIter instruction) which
    /// advances the iterator and either enters the next iteration or exits the loop.
    /// If inside a try-finally, the finally block must run first.
    fn compile_continue(&mut self, position: CodeRange) -> Result<(), CompileError> {
        if self.loop_stack.is_empty() {
            return Err(CompileError::new("'continue' not properly in loop", position));
        }

        let target_loop_depth = self.loop_stack.len() - 1;

        // If inside except handlers, clear each enclosing exception_stack
        // entry.
        for _ in 0..self.except_handler_depth {
            self.code.emit(Opcode::ClearException)?;
        }

        // Check if we need to go through any finally blocks
        // We need to run finally if continue crosses the try boundary
        if let Some(finally_target) = self.finally_targets.last_mut()
            && target_loop_depth < finally_target.loop_depth_at_entry
        {
            // Continuing a loop that's outside (or at the start of) this try-finally,
            // so finally must run before the continue
            let jump = self.code.emit_jump(Opcode::Jump)?;
            finally_target.continue_jumps.push(BreakContinueThruFinally {
                jump,
                target_loop_depth,
            });
        }

        // No finally to go through, jump directly to loop start
        let loop_start = self.loop_stack[target_loop_depth].start;
        self.code.emit_jump_to(Opcode::Jump, loop_start)?;

        Ok(())
    }

    /// Compiles break or continue after a finally block has run.
    ///
    /// Called from `compile_try` after the finally block code. Each control flow
    /// statement may target a different loop, so we check if there's another finally
    /// to go through or if we can jump directly to the loop's target.
    ///
    /// Note: All items in the list jumped to the same finally block, so they all
    /// have the same starting point. After finally runs, we need to route each
    /// to its target loop, potentially through more finally blocks.
    fn compile_control_flow_after_finally(
        &mut self,
        items: &[BreakContinueThruFinally],
        is_break: bool,
    ) -> Result<(), CompileError> {
        // All items went through the same finally, now we need to dispatch to
        // potentially different loops. For simplicity, we assume all items in
        // a single finally target the same loop (the innermost one at the time).
        // This is always true since break/continue only targets the innermost loop.
        let Some(first) = items.first() else {
            return Ok(());
        };
        let target_loop_depth = first.target_loop_depth;

        // Check if there's another finally between us and the target loop
        if let Some(finally_target) = self.finally_targets.last_mut()
            && target_loop_depth < finally_target.loop_depth_at_entry
        {
            // Need to go through another finally
            let jump = self.code.emit_jump(Opcode::Jump)?;
            let jump_info = BreakContinueThruFinally {
                jump,
                target_loop_depth,
            };
            if is_break {
                finally_target.break_jumps.push(jump_info);
            } else {
                // else continue
                finally_target.continue_jumps.push(jump_info);
            }
            return Ok(());
        }

        // No more finally blocks, jump directly to the loop target. For
        // break paths we pop the for-loop iterator here (rather than at the
        // break statement) so that intervening per-block stack contributions
        // — e.g. a `with` block sitting on top of the iterator — get to clean
        // themselves up in their own trailers first. Continue paths leave the
        // iterator on top because the loop start (`ForIter`) expects it.
        if is_break {
            if self.loop_stack[target_loop_depth].has_iterator_on_stack {
                self.code.emit(Opcode::Pop)?;
            }
            let jump = self.code.emit_jump(Opcode::Jump)?;
            self.loop_stack[target_loop_depth].break_jumps.push(jump);
        } else {
            // else continue
            let loop_start = self.loop_stack[target_loop_depth].start;
            self.code.emit_jump_to(Opcode::Jump, loop_start)?;
        }
        Ok(())
    }

    // ========================================================================
    // Comprehension Compilation
    // ========================================================================

    /// Compiles a list comprehension: `[elt for target in iter if cond...]`
    ///
    /// Bytecode structure:
    /// ```text
    /// BUILD_LIST 0
    /// <compile first iter>
    /// GET_ITER
    /// loop_start:
    ///   FOR_ITER end_loop        ; pushes the iter's value
    ///   [UNPACK / LIFT_TO_TOP]   ; comp-var leaves end up on operand stack
    ///   <compile filters - jump back to loop_start if any fails>
    ///   [nested generators...]
    ///   <compile elt>
    ///   LIST_APPEND depth        ; reaches list by counting items between
    ///   POP × K_this_generator   ; remove this generator's comp vars
    ///   JUMP loop_start
    /// end_loop:                  ; FOR_ITER popped the iter on exhaustion
    /// ; result list on stack
    /// ```
    ///
    /// Comprehension targets live on the operand stack as the values pushed
    /// by `FOR_ITER` (plus unpacked sub-values). The compiler tracks each
    /// leaf's absolute operand-stack offset in `Compiler::slot_offsets` so
    /// that `compile_name` for `NameScope::CompVar` can emit
    /// `LoadLocal/W(frame_locals + offset)`. Per-iteration `POP`s clean the
    /// comp vars before the JUMP so the loop's stack discipline is preserved.
    fn compile_list_comp(&mut self, elt: &ExprLoc, generators: &[Comprehension]) -> Result<(), CompileError> {
        if self.code.is_dead() {
            return Ok(());
        }
        check_comp_generators(generators.len(), elt.position)?;
        self.code.emit_u16(Opcode::BuildList, 0)?;
        let depth_after_collection = self
            .code
            .stack_depth()
            .expect("list comp: BuildList kept us live, stack_depth must be Some");

        self.compile_comprehension_generators(generators, 0, |compiler| {
            compiler.compile_expr(elt)?;
            if compiler.code.is_dead() {
                return Ok(());
            }
            let depth = compiler.compute_append_depth(depth_after_collection, 1, elt.position)?;
            compiler.code.emit_u8(Opcode::ListAppend, depth)
        })?;

        Ok(())
    }

    /// Compiles a set comprehension: `{elt for target in iter if cond...}`
    fn compile_set_comp(&mut self, elt: &ExprLoc, generators: &[Comprehension]) -> Result<(), CompileError> {
        if self.code.is_dead() {
            return Ok(());
        }
        check_comp_generators(generators.len(), elt.position)?;
        self.code.emit_u16(Opcode::BuildSet, 0)?;
        let depth_after_collection = self
            .code
            .stack_depth()
            .expect("set comp: BuildSet kept us live, stack_depth must be Some");

        self.compile_comprehension_generators(generators, 0, |compiler| {
            compiler.compile_expr(elt)?;
            if compiler.code.is_dead() {
                return Ok(());
            }
            let depth = compiler.compute_append_depth(depth_after_collection, 1, elt.position)?;
            compiler.code.emit_u8(Opcode::SetAdd, depth)
        })?;

        Ok(())
    }

    /// Compiles a dict comprehension: `{key: value for target in iter if cond...}`
    fn compile_dict_comp(
        &mut self,
        key: &ExprLoc,
        value: &ExprLoc,
        generators: &[Comprehension],
    ) -> Result<(), CompileError> {
        if self.code.is_dead() {
            return Ok(());
        }
        check_comp_generators(generators.len(), key.position)?;
        self.code.emit_u16(Opcode::BuildDict, 0)?;
        let depth_after_collection = self
            .code
            .stack_depth()
            .expect("dict comp: BuildDict kept us live, stack_depth must be Some");

        self.compile_comprehension_generators(generators, 0, |compiler| {
            compiler.compile_expr(key)?;
            compiler.compile_expr(value)?;
            if compiler.code.is_dead() {
                return Ok(());
            }
            // DictSetItem pops 2 (key+value), so the post-pop offset for the
            // collection is one deeper than the list/set case.
            let depth = compiler.compute_append_depth(depth_after_collection, 2, key.position)?;
            compiler.code.emit_u8(Opcode::DictSetItem, depth)
        })?;

        Ok(())
    }

    /// Computes the `depth` operand for `ListAppend` / `SetAdd` / `DictSetItem`.
    ///
    /// All three opcodes pop their value(s) first and then index the
    /// collection at `len_post_pop - 1 - depth`. We want the collection at
    /// its known position (`depth_after_collection - 1`), so the operand is
    /// `current_stack_depth - depth_after_collection - 1` for list/set (pops 1)
    /// or `current_stack_depth - depth_after_collection - 2` for dict (pops 2).
    /// The caller passes the pop count.
    fn compute_append_depth(
        &self,
        depth_after_collection: u16,
        pops: u16,
        position: CodeRange,
    ) -> Result<u8, CompileError> {
        let current = self.code.stack_depth().expect("compute_append_depth in dead code");
        let depth = current
            .checked_sub(depth_after_collection)
            .and_then(|d| d.checked_sub(pops))
            .ok_or_else(|| CompileError::new("comprehension stack-depth bookkeeping went negative", position))?;
        u8::try_from(depth).map_err(|_| {
            CompileError::new(
                "comprehension target + iterator count exceeds u8 depth operand",
                position,
            )
        })
    }

    /// Recursively compiles comprehension generators (the for/if clauses).
    ///
    /// For each generator:
    /// 1. Compile the iterator expression and `GET_ITER`.
    /// 2. Start loop: `FOR_ITER` pushes the iter's value (or pops iter and
    ///    jumps to end on exhaustion).
    /// 3. Unpack the comp target — `compile_comp_target_unpack` emits any
    ///    `UNPACK_SEQUENCE` / `UNPACK_EX` / `LIFT_TO_TOP` needed and records
    ///    each leaf's operand-stack offset.
    /// 4. Compile filter conditions; on false, jump back to loop start
    ///    (skipping per-iter POPs and the body — the per-iter operand-stack
    ///    items live below the filter result, so this works the same way it
    ///    did with the dedicated-region scheme).
    /// 5. Either recurse for the next generator, or call `body_fn` at the
    ///    innermost level (which emits the element expression and
    ///    `LIST_APPEND` / `SET_ADD` / `DICT_SET_ITEM`).
    /// 6. Per-iteration `POP` for each comp-var leaf produced by this
    ///    generator's target, restoring the loop-start stack shape.
    /// 7. Jump back to loop start.
    fn compile_comprehension_generators(
        &mut self,
        generators: &[Comprehension],
        index: usize,
        body_fn: impl FnOnce(&mut Self) -> Result<(), CompileError>,
    ) -> Result<(), CompileError> {
        let generator = &generators[index];

        // Compile iterator expression
        self.compile_expr(&generator.iter)?;
        self.code.emit(Opcode::GetIter)?;

        // Loop start
        let loop_start = self.code.current_jump_target();

        // FOR_ITER: pushes value, or pops iter and jumps to end on exhaustion.
        let end_jump = self.code.emit_jump(Opcode::ForIter)?;

        // Unpack target — leaves the comp vars on the operand stack at offsets
        // recorded in `self.slot_offsets`, and marks them in `bound_comp_slots`.
        let comp_var_slots = self.compile_comp_target_unpack(&generator.target)?;

        // Filters: any false → forward-jump to the per-iter cleanup block
        // below. We can't jump directly to `loop_start`: the comp vars are
        // on the operand stack, so we must pop them first to keep the
        // loop-start stack shape consistent. `JumpIfFalse` pops `cond`, so
        // arrival depth at the cleanup label matches the post-body depth
        // (both are `loop_start + K`).
        let mut filter_skip_jumps = Vec::with_capacity(generator.ifs.len());
        for cond in &generator.ifs {
            self.compile_expr(cond)?;
            filter_skip_jumps.push(self.code.emit_jump(Opcode::JumpIfFalse)?);
        }

        // Recurse or emit body.
        if index + 1 < generators.len() {
            self.compile_comprehension_generators(generators, index + 1, body_fn)?;
        } else {
            body_fn(self)?;
        }

        // Per-iteration cleanup block: pop this generator's comp vars so the
        // JUMP back to `loop_start` lands at the same stack shape as the
        // previous iteration's entry. Filter-failure jumps also land here.
        for jmp in filter_skip_jumps {
            self.code.patch_jump(jmp)?;
        }
        for _ in 0..comp_var_slots.len() {
            self.code.emit(Opcode::Pop)?;
        }

        // Jump back to loop start
        self.code.emit_jump_to(Opcode::Jump, loop_start)?;
        self.code.patch_jump(end_jump)?;

        // Comp vars are out of scope after the loop body; clear their
        // bound-state so a sibling comprehension that reuses the same slot
        // IDs sees its own targets as unbound at iter-prep time.
        // `slot_offsets` entries are simply overwritten by whoever uses the
        // slot next, so we leave them as-is.
        for slot in &comp_var_slots {
            self.bound_comp_slots.remove(slot);
        }

        Ok(())
    }

    /// Compiles the unpacking of a comprehension target.
    ///
    /// At entry, `FOR_ITER` has pushed the iter's value at TOS. This emits
    /// `UNPACK_SEQUENCE` / `UNPACK_EX` / `LIFT_TO_TOP` as needed (nested
    /// tuples force `LIFT_TO_TOP` to bring sub-iterables to TOS for
    /// further unpacking) and records each leaf's absolute operand-stack
    /// offset in `self.slot_offsets`. Also marks each leaf's slot ID in
    /// `self.bound_comp_slots`.
    ///
    /// Returns the slot IDs for this target's leaves so the caller can
    /// emit a matching `POP` per leaf for per-iteration cleanup.
    fn compile_comp_target_unpack(&mut self, target: &UnpackTarget) -> Result<Vec<u16>, CompileError> {
        // `FOR_ITER` just pushed; current depth's TOS index is the value's
        // operand-stack offset, which is also the offset of the first leaf
        // produced by this unpack.
        //
        // If we're already in dead-code state (e.g. an earlier generator's
        // iter expression contained a `RaiseUnboundLocal` that terminated the
        // current code region), no bytecode emission would have any effect.
        // Return an empty slot list — `compile_comprehension_generators` then
        // emits its `POP`s and `JUMP` in dead state (also no-ops). The
        // comp-var slots stay out of `bound_comp_slots`, so any subsequent
        // `CompVar` read would dispatch to `RaiseUnboundLocal` — also a
        // no-op in dead code.
        let Some(stack_depth) = self.code.stack_depth() else {
            return Ok(Vec::new());
        };
        let base_offset = stack_depth - 1;

        let mut sim: Vec<SimItem<'_>> = vec![SimItem::Pending(target)];
        self.process_unpack_sim(&mut sim)?;

        // All items should be Leafs now. Record offsets in order.
        let mut slot_ids = Vec::with_capacity(sim.len());
        for (i, item) in sim.into_iter().enumerate() {
            let SimItem::Leaf(slot) = item else {
                unreachable!("process_unpack_sim left a Pending on the sim");
            };
            let i_u16 = u16::try_from(i).expect("comp-var index bounded by u8 unpack count");
            let offset = base_offset.checked_add(i_u16).ok_or_else(|| {
                CompileError::new(
                    "comprehension operand-stack offset exceeds u16",
                    target_position(target),
                )
            })?;
            let slot_idx = slot as usize;
            if slot_idx >= self.slot_offsets.len() {
                self.slot_offsets.resize(slot_idx + 1, 0);
            }
            self.slot_offsets[slot_idx] = self.frame_locals.checked_add(offset).ok_or_else(|| {
                CompileError::new(
                    "comprehension comp-var slot exceeds u16 (frame_locals + offset)",
                    target_position(target),
                )
            })?;
            self.bound_comp_slots.insert(slot);
            slot_ids.push(slot);
        }

        Ok(slot_ids)
    }

    /// Drives one step of the unpack simulation: takes the topmost `Pending`
    /// off `sim` and either marks it `Leaf` (for `Name`/`Starred`) or emits
    /// `UNPACK_SEQUENCE`/`UNPACK_EX` and recursively processes sub-targets,
    /// using `LIFT_TO_TOP` to bring each sub-target to TOS before recursion.
    ///
    /// Precondition: `sim`'s topmost item is `Pending`.
    fn process_unpack_sim(&mut self, sim: &mut Vec<SimItem<'_>>) -> Result<(), CompileError> {
        let target = match sim.pop() {
            Some(SimItem::Pending(t)) => t,
            Some(SimItem::Leaf(_)) => unreachable!("process_unpack_sim called with Leaf at TOS"),
            None => unreachable!("process_unpack_sim called on empty sim"),
        };

        match target {
            UnpackTarget::Name(ident) | UnpackTarget::Starred(ident) => {
                sim.push(SimItem::Leaf(ident.namespace_id().as_u16()));
            }
            UnpackTarget::Tuple { targets, position } => {
                // Pick UNPACK_EX vs UNPACK_SEQUENCE based on whether a starred
                // sub-target is present (same logic as the regular assignment
                // path in `compile_unpack_target`).
                let star_idx = targets.iter().position(|t| matches!(t, UnpackTarget::Starred(_)));
                self.code.set_location(*position, None);
                if let Some(star_idx) = star_idx {
                    let before = check_unpack_targets(star_idx, *position)?;
                    let after = check_unpack_targets(targets.len() - star_idx - 1, *position)?;
                    self.code.emit_u8_u8(Opcode::UnpackEx, before, after)?;
                } else {
                    let count = check_unpack_targets(targets.len(), *position)?;
                    self.code.emit_u8(Opcode::UnpackSequence, count)?;
                }

                // UNPACK pushes sub-targets in reverse source order: sub n-1
                // ends up at the bottom of the new region, sub 0 at TOS.
                let base = sim.len();
                for sub in targets.iter().rev() {
                    sim.push(SimItem::Pending(sub));
                }

                // Process sub-targets in source order. Each lift only moves
                // items at or above the source-index, so subs we haven't
                // processed yet (lower indices, deeper in the sim) keep
                // their position.
                let n = targets.len();
                for i in 0..n {
                    let target_idx = base + (n - 1 - i);
                    let tos_idx = sim.len() - 1;
                    if tos_idx > target_idx {
                        let lift_n = tos_idx - target_idx;
                        let lift_n_u8 = u8::try_from(lift_n).map_err(|_| {
                            CompileError::new("comprehension nesting requires lift offset > u8", *position)
                        })?;
                        self.code.emit_u8(Opcode::LiftToTop, lift_n_u8)?;
                        let item = sim.remove(target_idx);
                        sim.push(item);
                    }
                    // Now sub i is at TOS. Recurse to either mark Leaf or
                    // unpack further.
                    self.process_unpack_sim(sim)?;
                }
            }
        }
        Ok(())
    }

    /// Compiles storage of an unpack target - either a single identifier, nested tuple, or starred.
    ///
    /// For single identifiers: emits a simple store.
    /// For nested tuples: emits `UnpackSequence` (or `UnpackEx` with starred) and recursively
    /// handles each sub-target.
    fn compile_unpack_target(&mut self, target: &UnpackTarget) -> Result<(), CompileError> {
        match target {
            UnpackTarget::Name(ident) => {
                // Single identifier - just store directly
                self.compile_store(ident)?;
            }
            UnpackTarget::Starred(ident) => {
                // Starred target by itself (shouldn't happen at top level normally)
                // Just store as if it were a name
                self.compile_store(ident)?;
            }
            UnpackTarget::Tuple { targets, position } => {
                // Check if there's a starred target
                let star_idx = targets.iter().position(|t| matches!(t, UnpackTarget::Starred(_)));

                self.code.set_location(*position, None);

                if let Some(star_idx) = star_idx {
                    // Has starred target - use UnpackEx
                    let before = check_unpack_targets(star_idx, *position)?;
                    let after = check_unpack_targets(targets.len() - star_idx - 1, *position)?;
                    self.code.emit_u8_u8(Opcode::UnpackEx, before, after)?;
                } else {
                    // No starred target - use UnpackSequence
                    let count = check_unpack_targets(targets.len(), *position)?;
                    self.code.emit_u8(Opcode::UnpackSequence, count)?;
                }

                // After UnpackSequence/UnpackEx, values are on stack with first item on top
                // Store them in order, recursively handling further nesting
                for target in targets {
                    self.compile_unpack_target(target)?;
                }
            }
        }
        Ok(())
    }

    /// Compiles a single assignment step, assuming the value to assign is on top of stack.
    ///
    /// Central per-shape dispatch for assignment stores. Called once per step of a chained
    /// assignment, and also by the single-target `Node::SubscriptAssign`/`AttrAssign`/
    /// `UnpackAssign`/`Assign` handlers (after they push the RHS). Keeping this dispatch
    /// in one place ensures the store sequences stay in sync across single-target and
    /// chained forms.
    fn compile_assign_target(&mut self, target: &AssignTarget) -> Result<(), CompileError> {
        match target {
            AssignTarget::Name(ident) => self.compile_store(ident)?,
            AssignTarget::Subscript {
                target,
                index,
                target_position,
            } => self.emit_subscript_store(target, index, *target_position)?,
            AssignTarget::Attr {
                object,
                attr,
                target_position,
            } => self.emit_attr_store(object, attr, *target_position)?,
            AssignTarget::Unpack {
                targets,
                targets_position,
            } => self.emit_unpack_store(targets, *targets_position)?,
        }
        Ok(())
    }

    /// Emits the bytecode for `container[index] = value`, assuming `value` is on top of stack.
    ///
    /// `StoreSubscr` expects the stack to be `[.., value, container, index]` with `index`
    /// on top, so this evaluates `target` (container) and then `index` above the incoming
    /// value. Used by both `Node::SubscriptAssign` and chained-assignment subscript steps.
    fn emit_subscript_store(
        &mut self,
        target: &ExprLoc,
        index: &ExprLoc,
        target_position: CodeRange,
    ) -> Result<(), CompileError> {
        self.compile_expr(target)?;
        self.compile_expr(index)?;
        self.code.set_location(target_position, None);
        self.code.emit(Opcode::StoreSubscr)?;
        Ok(())
    }

    /// Emits the bytecode for `object.attr = value`, assuming `value` is on top of stack.
    ///
    /// `StoreAttr` expects `[.., value, object]` with `object` on top, so this evaluates
    /// `object` above the incoming value. Used by both `Node::AttrAssign` and chained-
    /// assignment attribute steps.
    ///
    /// The parser always stores attribute names as `EitherStr::Interned`, so the hot
    /// path never hits the `Heap` branch. We still check it explicitly rather than
    /// panicking because `Node` derives `Deserialize` — an untrusted snapshot could
    /// carry a `Heap` attribute name, and defense-in-depth says the compiler should
    /// surface that as a graceful `CompileError` instead of aborting the process.
    fn emit_attr_store(
        &mut self,
        object: &ExprLoc,
        attr: &EitherStr,
        target_position: CodeRange,
    ) -> Result<(), CompileError> {
        let Some(name_id) = attr.string_id() else {
            return Err(CompileError::new(
                "internal error: attribute name in AST must be interned",
                target_position,
            ));
        };
        let name_idx = check_name_index_u16(name_id, target_position)?;
        self.compile_expr(object)?;
        self.code.set_location(target_position, None);
        self.code.emit_u16(Opcode::StoreAttr, name_idx)?;
        Ok(())
    }

    /// Emits the bytecode for unpacking assignments (`a, b = value`, `[a, *rest] = value`).
    ///
    /// Assumes the iterable is already on top of stack, chooses between `UnpackSequence`
    /// (no starred target) and `UnpackEx` (exactly one starred target), then stores the
    /// unpacked values into each sub-target — recursing through nested tuple patterns.
    /// Shared between `Node::UnpackAssign` and chained-assignment unpack steps.
    fn emit_unpack_store(&mut self, targets: &[UnpackTarget], targets_position: CodeRange) -> Result<(), CompileError> {
        let star_idx = targets.iter().position(|t| matches!(t, UnpackTarget::Starred(_)));
        self.code.set_location(targets_position, None);
        if let Some(star_idx) = star_idx {
            let before = check_unpack_targets(star_idx, targets_position)?;
            let after = check_unpack_targets(targets.len() - star_idx - 1, targets_position)?;
            self.code.emit_u8_u8(Opcode::UnpackEx, before, after)?;
        } else {
            let count = check_unpack_targets(targets.len(), targets_position)?;
            self.code.emit_u8(Opcode::UnpackSequence, count)?;
        }
        for t in targets {
            self.compile_unpack_target(t)?;
        }
        Ok(())
    }

    // ========================================================================
    // Statement Helpers
    // ========================================================================

    /// Compiles an assert statement.
    fn compile_assert(&mut self, test: &ExprLoc, msg: Option<&ExprLoc>) -> Result<(), CompileError> {
        // Compile test
        self.compile_expr(test)?;
        // Jump over raise if truthy
        let skip_jump = self.code.emit_jump(Opcode::JumpIfTrue)?;

        // Raise AssertionError
        let exc_idx = self
            .code
            .add_const(Value::Builtin(Builtins::ExcType(ExcType::AssertionError)))?;
        self.code.emit_u16(Opcode::LoadConst, exc_idx)?;

        if let Some(msg_expr) = msg {
            // Call AssertionError(msg)
            self.compile_expr(msg_expr)?;
            self.code.emit_u8(Opcode::CallFunction, 1)?;
        } else {
            // Call AssertionError()
            self.code.emit_u8(Opcode::CallFunction, 0)?;
        }

        self.code.emit(Opcode::Raise)?;
        self.code.patch_jump(skip_jump)?;
        Ok(())
    }

    /// Compiles f-string parts, returning the number of string parts to concatenate.
    ///
    /// Each part is compiled to leave a string value on the stack:
    /// - `Literal(StringId)`: Push the interned string directly
    /// - `Interpolation`: Compile expr, emit FormatValue to convert to string
    fn compile_fstring_parts(&mut self, parts: &[FStringPart]) -> Result<u16, CompileError> {
        let mut count = 0u16;

        for part in parts {
            match part {
                FStringPart::Literal(string_id) => {
                    // Push the interned string as a constant
                    let const_idx = self.code.add_const(Value::InternString(*string_id))?;
                    self.code.emit_u16(Opcode::LoadConst, const_idx)?;
                    count += 1;
                }
                FStringPart::Interpolation {
                    expr,
                    conversion,
                    format_spec,
                    debug_prefix,
                } => {
                    // If debug prefix present, push it first
                    if let Some(prefix_id) = debug_prefix {
                        let const_idx = self.code.add_const(Value::InternString(*prefix_id))?;
                        self.code.emit_u16(Opcode::LoadConst, const_idx)?;
                        count += 1;
                    }

                    // Compile the expression
                    self.compile_expr(expr)?;

                    // A debug expression (`{x=}`) defaults to `repr`, but ONLY
                    // when it has neither an explicit conversion nor a format
                    // spec. With a spec (`{x=:.3f}`) the spec applies to the
                    // value directly (not to its repr string), matching CPython.
                    let effective_conversion = if debug_prefix.is_some()
                        && matches!(conversion, ConversionFlag::None)
                        && format_spec.is_none()
                    {
                        ConversionFlag::Repr
                    } else {
                        *conversion
                    };

                    // Emit FormatValue with appropriate flags
                    let flags = self.compile_format_value(effective_conversion, format_spec.as_ref())?;
                    self.code.emit_u8(Opcode::FormatValue, flags)?;
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Compiles format value flags and optionally pushes format spec to stack.
    ///
    /// Returns the flags byte encoding conversion, spec presence, and (for
    /// static specs) that the on-stack spec is the encoded `Int` form rather
    /// than a string. See [`FORMAT_VALUE_HAS_SPEC`]/[`FORMAT_VALUE_STATIC_SPEC`]
    /// for the bit layout. If a format spec is present it's pushed to the
    /// stack before the value.
    fn compile_format_value(
        &mut self,
        conversion: ConversionFlag,
        format_spec: Option<&FormatSpec>,
    ) -> Result<u8, CompileError> {
        // Conversion flag: bits 0-1
        let conv_bits = match conversion {
            ConversionFlag::None => 0,
            ConversionFlag::Str => 1,
            ConversionFlag::Repr => 2,
            ConversionFlag::Ascii => 3,
        };

        match format_spec {
            None => Ok(conv_bits),
            Some(FormatSpec::Static(encoded)) => {
                // Push the raw encoded form; the static-spec flag tells the
                // VM to read it back via decode_format_spec without inspecting
                // the Value variant.
                let const_idx = self.code.add_const(Value::Int(*encoded))?;
                self.code.emit_u16(Opcode::LoadConst, const_idx)?;
                Ok(conv_bits | FORMAT_VALUE_HAS_SPEC | FORMAT_VALUE_STATIC_SPEC)
            }
            Some(FormatSpec::Dynamic(dynamic_parts)) => {
                // Compile dynamic format spec parts to build a format spec string
                // Then parse it at runtime
                let part_count = self.compile_fstring_parts(dynamic_parts)?;
                if part_count > 1 {
                    self.code.emit_u16(Opcode::BuildFString, part_count)?;
                }
                // Format spec string is now on stack
                Ok(conv_bits | FORMAT_VALUE_HAS_SPEC)
            }
        }
    }

    // ========================================================================
    // Exception Handling Compilation
    // ========================================================================

    /// Compiles a return statement.
    ///
    /// `expr` is the expression after `return` (`None` for a bare `return`).
    fn compile_return(&mut self, expr: Option<&ExprLoc>) -> Result<(), CompileError> {
        if let Some(expr) = expr {
            self.compile_expr(expr)?;
        } else {
            self.code.emit(Opcode::LoadNone)?;
        }

        self.compile_return_routing()?;

        Ok(())
    }

    /// Used for returning from current function. The return value must already
    /// be on the top of the stack.
    ///
    /// Will either emit a direct `ReturnValue`, or jump to the next enclosing
    /// finally block (if we're inside one).
    ///
    /// Clears active-exception state for every `except` handler we're
    /// exiting up to (but not past) the next enclosing finally — finally
    /// bodies between us and the next-outer finally need to run with their
    /// textually-enclosing exception state intact, e.g.:
    ///
    /// ```python
    /// try:
    ///     raise ValueError
    /// except ValueError:
    ///     try:
    ///         return  # inner finally below must STILL see ValueError as
    ///     finally:    # the active exception so bare `raise` re-raises it.
    ///         ...
    /// ```
    ///
    /// The remaining handlers are cleared further out by the finally
    /// trailers in [`compile_try`] as control flows through them.
    fn compile_return_routing(&mut self) -> Result<(), CompileError> {
        let target_depth = self
            .finally_targets
            .last()
            .map_or(0, |t| t.except_handler_depth_at_entry);

        for _ in 0..(self.except_handler_depth - target_depth) {
            self.code.emit(Opcode::ClearException)?;
        }

        if let Some(finally_target) = self.finally_targets.last_mut() {
            let jump = self.code.emit_jump(Opcode::Jump)?;
            finally_target.return_jumps.push(jump);
        } else {
            self.code.emit(Opcode::ReturnValue)?;
        }

        Ok(())
    }

    /// Compiles a try/except/else/finally block.
    ///
    /// The bytecode structure is:
    /// ```text
    /// <try_body>                     # protected range
    /// JUMP to_else_or_finally        # skip handlers if no exception
    /// handler_dispatch:              # exception pushed by VM
    ///   # for each handler:
    ///   <check exception type>
    ///   <handler body>
    ///   CLEAR_EXCEPTION
    ///   JUMP to_finally
    /// reraise:
    ///   RERAISE                      # no handler matched
    /// else_block:
    ///   <else_body>
    /// finally_block:
    ///   <finally_body>
    /// end:
    /// ```
    ///
    /// For finally blocks, exceptions that propagate through the handler dispatch
    /// (including RERAISE when no handler matches) are caught by a second exception
    /// entry that ensures finally runs before propagation.
    ///
    /// Returns inside try/except/else jump to a "finally with return" path that
    /// runs the finally code then returns the value.
    ///
    /// **Note:** The finally block code is emitted multiple times (once for each
    /// control flow path: normal, exception, return, break, continue). This is the
    /// same approach CPython uses - each path has different stack state at entry
    /// (e.g., return has a value on stack, break has popped the iterator), so we
    /// can't easily share a single copy. The duplication is intentional.
    fn compile_try(&mut self, try_block: &Try<PreparedNode>) -> Result<(), CompileError> {
        let has_finally = !try_block.finally.is_empty();
        let has_handlers = !try_block.handlers.is_empty();
        let has_else = !try_block.or_else.is_empty();

        // Record stack depth at try entry (for unwinding on exception)
        let Some(stack_depth) = self.code.stack_depth() else {
            // Compiling dead code, don't need to emit anything
            return Ok(());
        };

        // Record `except_handler_depth` at try entry — the count of this
        // frame's exception_stack entries that should be active inside the
        // try body. The VM uses this on unwind to drain entries left
        // behind by abandoned-but-trailer-skipped handlers.
        let try_exc_stack_count = self.except_handler_depth;

        // If there's a finally block, track returns/break/continue inside try/handlers/else
        if has_finally {
            self.finally_targets.push(FinallyTarget {
                return_jumps: Vec::new(),
                break_jumps: Vec::new(),
                continue_jumps: Vec::new(),
                loop_depth_at_entry: self.loop_stack.len(),
                except_handler_depth_at_entry: self.except_handler_depth,
            });
        }

        // === Compile try body ===
        let try_start = self.code.current_offset();
        self.compile_block(&try_block.body)?;

        // Jump to else/finally if no exception (skip handlers)
        let after_try_jump = self.code.emit_jump(Opcode::Jump)?;
        // End of the try-body region for the exception table. This is past
        // the `after_try_jump` if it was emitted, so an exception that fires
        // up to and including that Jump still routes to the handler.
        let try_end = self.code.current_offset();

        // === Handler dispatch starts here ===
        let handler_start = self.code.current_offset();

        // Track jumps that go to finally (for patching later)
        let mut finally_jumps: Vec<JumpLabel> = Vec::new();

        self.compile_exception_handlers(stack_depth, &try_block.handlers, &mut finally_jumps)?;

        // After handler dispatch, each handler path either:
        // 1. Matched and popped the exception (via Pop), then jumped to finally
        // 2. Didn't match and reraised (for last handler)
        // The handlers' Pop instructions already account for the exception,
        // so no additional stack depth adjustment is needed here.

        // Mark end of handler dispatch (for finally exception entry)
        let handler_dispatch_end = self.code.current_offset();

        // === Finally cleanup handler (for exceptions during handler dispatch) ===
        // This catches exceptions from RERAISE (and any other exceptions in handlers)
        // and ensures finally runs before the exception propagates.
        let finally_cleanup_start = if has_finally {
            let cleanup_start = self.code.current_offset();
            // Exception value is on stack (pushed by VM), so stack = stack_depth + 1
            self.code.new_code_region(stack_depth + 1);
            // We need to pop it, run finally, then reraise
            // But we can't easily save the exception, so we use a different approach:
            // The exception is already on the exception_stack from handle_exception,
            // so we can just pop from operand stack, run finally, then reraise.
            self.code.emit(Opcode::Pop)?; // Pop exception from operand stack
            self.compile_block(&try_block.finally)?;
            self.code.emit(Opcode::Reraise)?; // Re-raise from exception_stack
            Some(cleanup_start)
        } else {
            None
        };

        // === Finally with return/break/continue paths ===
        // Pop finally target and get all the jumps that need to go through finally
        let finally_with_return_start = if has_finally {
            let finally_target = self.finally_targets.pop().expect("finally_targets should not be empty");

            // === Finally with return path ===
            let return_start = if finally_target.return_jumps.is_empty() {
                None
            } else {
                let start = self.code.current_offset();
                for jump in finally_target.return_jumps {
                    self.code.patch_jump(jump)?;
                }
                self.compile_block(&try_block.finally)?;
                self.compile_return_routing()?;
                Some(start)
            };

            // === Finally with break path ===
            // For each break, run finally then either:
            // - Jump to outer finally's break path (if there's an outer finally between us and the loop)
            // - Jump directly to the loop's break target
            if !finally_target.break_jumps.is_empty() {
                for break_info in &finally_target.break_jumps {
                    self.code.patch_jump(break_info.jump)?;
                }
                self.compile_block(&try_block.finally)?;
                // After finally, compile the break again (handles nested finally or direct jump)
                self.compile_control_flow_after_finally(&finally_target.break_jumps, true)?;
            }

            // === Finally with continue path ===
            if !finally_target.continue_jumps.is_empty() {
                for continue_info in &finally_target.continue_jumps {
                    self.code.patch_jump(continue_info.jump)?;
                }
                self.compile_block(&try_block.finally)?;
                // After finally, compile the continue again (handles nested finally or direct jump)
                self.compile_control_flow_after_finally(&finally_target.continue_jumps, false)?;
            }

            return_start
        } else {
            None
        };

        // === Else block (runs if no exception) ===
        self.code.patch_jump(after_try_jump)?;
        let else_start = self.code.current_offset();
        if has_else {
            self.compile_block(&try_block.or_else)?;
        }
        let else_end = self.code.current_offset();

        // === Normal finally path (no exception pending, no return) ===
        // Patch all jumps from handlers to go here
        for jump in finally_jumps {
            self.code.patch_jump(jump)?;
        }

        if has_finally {
            self.compile_block(&try_block.finally)?;
        }

        // === Add exception table entries ===
        // Order matters: entries are searched in order, so inner entries must come first.

        // Entry 1: Try body -> handler dispatch.
        // exception_stack_count = try_exc_stack_count: entering the try body
        // adds no handler entries.
        if has_handlers || has_finally {
            self.code
                .add_exception_entry(try_start, try_end, handler_start, stack_depth, try_exc_stack_count)?;
        }

        // Entry 2: Handler dispatch -> finally cleanup (only if has_finally).
        // exception_stack_count = try_exc_stack_count + 1: the original
        // exception was pushed onto exception_stack by entry 1's catch and
        // is still active throughout handler dispatch.
        if let Some(cleanup_start) = finally_cleanup_start {
            self.code.add_exception_entry(
                handler_start,
                handler_dispatch_end,
                cleanup_start,
                stack_depth,
                try_exc_stack_count + 1,
            )?;
        }

        // Entry 3: Finally with return -> finally cleanup
        // If an exception occurs while running finally (in the return path), catch it
        if let (Some(return_start), Some(cleanup_start)) = (finally_with_return_start, finally_cleanup_start) {
            // End at else_start (before else block).
            self.code.add_exception_entry(
                return_start,
                else_start,
                cleanup_start,
                stack_depth,
                try_exc_stack_count,
            )?;
        }

        // Entry 4: Else block -> finally cleanup (only if has_finally and
        // has_else). Else runs when no exception was raised, so no handler
        // pushed an entry: exception_stack_count = try_exc_stack_count.
        if has_else && let Some(cleanup_start) = finally_cleanup_start {
            self.code
                .add_exception_entry(else_start, else_end, cleanup_start, stack_depth, try_exc_stack_count)?;
        }

        Ok(())
    }

    /// Compiles a `with` statement: `with EXPR [as TARGET]: BODY`.
    ///
    /// The bytecode shape is:
    /// ```text
    /// <compile context expr>            ; [ctx]
    /// BEFORE_WITH                       ; [ctx, value]
    /// try_start:
    ///   <store target or POP>           ; [ctx]
    ///   <compile body>                  ; [ctx]
    ///   WITH_EXIT                       ; []
    ///   JUMP end                        ; skip the exception handler
    /// try_end:
    /// handler_start:
    ///   ; VM pushes the exception: stack is [ctx, exc]
    ///   WITH_EXCEPT_START               ; [ctx, exc, suppress]
    ///   JUMP_IF_TRUE swallow            ; pops suppress; falsy = continue
    ///   POP                             ; [ctx]
    ///   POP                             ; []
    ///   RERAISE                         ; propagate
    /// swallow:                          ; [ctx, exc]
    ///   POP                             ; [ctx]
    ///   POP                             ; []
    ///   CLEAR_EXCEPTION
    ///   JUMP end
    /// <return / break / continue trailers, each running WITH_EXIT before
    ///  routing to the outer target>
    /// end:
    /// ```
    ///
    /// The `<store target or POP>` step lives *inside* the protected region so
    /// `with f() as (a, b):` invokes `__exit__` when the unpack fails —
    /// matching CPython, which similarly places `UNPACK_SEQUENCE` inside the
    /// `BEFORE_WITH` exception-table entry. If the store raises, the unwinder
    /// drops any partial unpack state down to the handler's expected depth
    /// (`stack_depth + 1`) before pushing `exc` and entering `handler_start`.
    ///
    /// A single exception-table entry covers the body, routing exceptions to
    /// `handler_start` with stack depth `outer + 1` (the context manager). If
    /// the cleanup itself (`__exit__` invocation) raises, the new exception
    /// replaces the original one and propagates via the surrounding frame's
    /// exception table — this matches CPython's behavior.
    ///
    /// `return`/`break`/`continue` inside the body are routed through this
    /// method's trailers (analogous to `try`/`finally`) so `__exit__` is called
    /// before propagating the early exit. The `return` trailer uses `Rot2` to
    /// preserve the return value while invoking `WithExit` on the context
    /// manager underneath.
    fn compile_with(
        &mut self,
        context: &ExprLoc,
        target: Option<&UnpackTarget>,
        body: &[PreparedNode],
    ) -> Result<(), CompileError> {
        // Record outer stack depth for the exception-table entry. If we are in
        // dead-code state there's nothing to emit.
        let Some(stack_depth) = self.code.stack_depth() else {
            return Ok(());
        };

        let try_exc_stack_count = self.except_handler_depth;

        // Evaluate context expr and invoke __enter__.
        self.compile_expr(context)?;
        self.code.emit(Opcode::BeforeWith)?;

        // Track early exits inside the body so we can call __exit__ before
        // they propagate. Mirrors the FinallyTarget push in `compile_try`.
        self.finally_targets.push(FinallyTarget {
            return_jumps: Vec::new(),
            break_jumps: Vec::new(),
            continue_jumps: Vec::new(),
            loop_depth_at_entry: self.loop_stack.len(),
            except_handler_depth_at_entry: self.except_handler_depth,
        });

        // === Body (protected region) ===
        let try_start = self.code.current_offset();
        // Bind the __enter__ result to the `as` target (or discard it). This
        // lives inside the protected region so `with f() as (a, b):` calls
        // `__exit__` when the unpack fails — matching CPython, which similarly
        // covers UNPACK_SEQUENCE with the with-block's exception table entry.
        if let Some(target) = target {
            self.compile_unpack_target(target)?;
        } else {
            self.code.emit(Opcode::Pop)?;
        }
        self.compile_block(body)?;
        // Close the protected range BEFORE `WithExit` so the normal-exit
        // cleanup is outside the body's exception-table entry. If
        // `__exit__` raises here, the new exception should propagate to
        // the outer frame's exception table (matching CPython, where an
        // `__exit__` exception replaces any prior state). Routing it
        // back to our own handler would invoke `__exit__` a second time
        // with the ctx already popped, blowing up the stack-depth
        // bookkeeping in the unwinder.
        let try_end = self.code.current_offset();

        // Normal exit: __exit__(None, None, None); pop the (discarded) result;
        // skip the handler.
        self.code.emit(Opcode::WithExit)?;
        self.code.emit(Opcode::Pop)?;
        let after_body_jump = self.code.emit_jump(Opcode::Jump)?;

        // === Exception handler ===
        let handler_start = self.code.current_offset();
        // VM unwinds to `stack_depth + 1` (the ctx) and then pushes the exception
        // value itself, so we enter at depth `stack_depth + 2` with [ctx, exc].
        self.code.new_code_region(stack_depth + 2);

        self.code.emit(Opcode::WithExceptStart)?;
        // Stack: [ctx, exc, suppress]
        let swallow_jump = self.code.emit_jump(Opcode::JumpIfTrue)?;
        // Falsy path: stack = [ctx, exc]. Drop both and re-raise.
        self.code.emit(Opcode::Pop)?;
        self.code.emit(Opcode::Pop)?;
        self.code.emit(Opcode::Reraise)?;

        // Swallow path: stack = [ctx, exc]. Drop both, clear current exception,
        // jump to end.
        self.code.patch_jump(swallow_jump)?;
        self.code.emit(Opcode::Pop)?;
        self.code.emit(Opcode::Pop)?;
        self.code.emit(Opcode::ClearException)?;
        let after_swallow_jump = self.code.emit_jump(Opcode::Jump)?;

        // === Early-exit trailers (return/break/continue inside body) ===
        let finally_target = self.finally_targets.pop().expect("finally_targets should not be empty");

        // === Return path ===
        // Stack at patch site: [ctx, return_value]. Swap so ctx is on top for
        // WithExit, discard its result, then route the (preserved) return value.
        if !finally_target.return_jumps.is_empty() {
            for jump in finally_target.return_jumps {
                self.code.patch_jump(jump)?;
            }
            self.code.emit(Opcode::Rot2)?;
            self.code.emit(Opcode::WithExit)?;
            self.code.emit(Opcode::Pop)?;
            self.compile_return_routing()?;
        }

        // === Break path ===
        // Stack at patch site: [ctx] (compile_break already popped any for-loop
        // iterator). Call __exit__, discard its result, then route to the break target.
        if !finally_target.break_jumps.is_empty() {
            for break_info in &finally_target.break_jumps {
                self.code.patch_jump(break_info.jump)?;
            }
            self.code.emit(Opcode::WithExit)?;
            self.code.emit(Opcode::Pop)?;
            self.compile_control_flow_after_finally(&finally_target.break_jumps, true)?;
        }

        // === Continue path ===
        // Stack at patch site: [ctx]. Same shape as the break path.
        if !finally_target.continue_jumps.is_empty() {
            for continue_info in &finally_target.continue_jumps {
                self.code.patch_jump(continue_info.jump)?;
            }
            self.code.emit(Opcode::WithExit)?;
            self.code.emit(Opcode::Pop)?;
            self.compile_control_flow_after_finally(&finally_target.continue_jumps, false)?;
        }

        // === Merge point for the normal-exit and swallowed-exception paths ===
        self.code.patch_jump(after_body_jump)?;
        self.code.patch_jump(after_swallow_jump)?;

        // === Exception-table entry: body -> handler ===
        // `stack_depth + 1` accounts for the ctx left on the stack; the VM
        // pushes the exception value itself on top of that. `exception_stack_count`
        // is unchanged because the with-block does not push to exception_stack.
        self.code
            .add_exception_entry(try_start, try_end, handler_start, stack_depth + 1, try_exc_stack_count)?;

        Ok(())
    }

    /// Compiles the exception handlers for a try block.
    ///
    /// Each handler checks if the exception matches its type, and if so,
    /// executes the handler body. If no handler matches, the exception is re-raised.
    ///
    /// The caller is responsible for calling this from a dead-code region; otherwise
    /// the attempt to create a new code region will panic.
    ///
    /// The region is closed at the end of this function, so the caller will need
    /// to start a new code region for any code that follows the handlers.
    fn compile_exception_handlers(
        &mut self,
        stack_depth: u16,
        handlers: &[ExceptHandler<PreparedNode>],
        finally_jumps: &mut Vec<JumpLabel>,
    ) -> Result<(), CompileError> {
        // Start a new code region for the exception handlers, +1 for
        // the exception value pushed by the VM on entry to the handler dispatch
        self.code.new_code_region(stack_depth + 1);

        for handler in handlers {
            let no_match_jump = if let Some(exc_type) = &handler.exc_type {
                // Typed handler: `except ExcType:` or `except ExcType as e:`.
                // Stack on entry: [exception]. `CheckExcMatch` peeks the
                // exception (doesn't pop it), so [exception] stays on the
                // stack across the check on both match and no-match paths.
                self.compile_expr(exc_type)?;
                self.code.emit(Opcode::CheckExcMatch)?;
                Some(self.code.emit_jump(Opcode::JumpIfFalse)?)
            } else {
                // Bare `except:` (must be the last handler per Python rules).
                None
            };

            // Match path: consume exception from the stack and store
            // to target if present.
            if let Some(name) = &handler.name {
                self.compile_store(name)?;
            } else {
                self.code.emit(Opcode::Pop)?;
            }

            self.except_handler_depth += 1;
            self.compile_block(&handler.body)?;
            self.except_handler_depth -= 1;

            if let Some(name) = &handler.name {
                self.compile_delete(name)?;
            }

            self.code.emit(Opcode::ClearException)?;
            finally_jumps.push(self.code.emit_jump(Opcode::Jump)?);

            if let Some(no_match_jump) = no_match_jump {
                // No-match landing: stack is [exception]. Falls through into
                // the next handler's check (or the post-loop `Reraise`).
                self.code.patch_jump(no_match_jump)?;
            }
        }

        // No handler matched - reraise the exception
        self.code.emit(Opcode::Reraise)?;

        Ok(())
    }

    /// Compiles deletion of a variable.
    ///
    /// At module level, `Local` scope emits `DeleteGlobal`
    /// because module-level locals live in the globals array.
    ///
    /// Function-scope `Local` deletes are limited to the first 256 slots
    /// because the only available opcode (`DeleteLocal`) takes a `u8`
    /// operand; a wide variant has not been added because slot-255 deletes
    /// are essentially unreachable in real code (each `except ... as e`
    /// implicitly emits a delete on the bound name, but functions with 256+
    /// locals plus an `except as` are exotic enough that we surface a
    /// `SyntaxError` rather than introduce a new opcode just for this).
    fn compile_delete(&mut self, target: &Identifier) -> Result<(), CompileError> {
        let slot = target.namespace_id().as_u16();
        match target.scope {
            NameScope::Local => {
                if self.is_module_scope {
                    self.code.emit_u16(Opcode::DeleteGlobal, slot)?;
                } else if let Ok(s) = u8::try_from(slot) {
                    self.code.emit_u8(Opcode::DeleteLocal, s)?;
                } else {
                    return Err(CompileError::new(
                        format!(
                            "cannot delete local variable in function with more than {} locals (slot {slot})",
                            u16::from(u8::MAX) + 1,
                        ),
                        target.position,
                    ));
                }
            }
            NameScope::Global => {
                self.code.emit_u16(Opcode::DeleteGlobal, slot)?;
            }
            NameScope::Cell => {
                // Delete cell not commonly needed
                // For now, just store None
                self.code.emit(Opcode::LoadNone)?;
                self.compile_store(target)?;
            }
            NameScope::CompVar => {
                unreachable!("no syntax exists to `del` a comprehension variable")
            }
        }
        Ok(())
    }
}

/// Error that can occur during bytecode compilation.
///
/// These are typically limit violations that can't be represented in the bytecode
/// format (e.g., too many arguments, too many local variables), or import errors
/// detected at compile time.
#[derive(Debug, Clone)]
pub struct CompileError {
    /// Error message describing the issue.
    message: Cow<'static, str>,
    /// Source location where the error occurred.
    position: CodeRange,
    /// Exception type to use (defaults to SyntaxError).
    exc_type: ExcType,
}

impl CompileError {
    /// Creates a new compile error with the given message and position.
    ///
    /// Defaults to `SyntaxError` exception type.
    pub(super) fn new(message: impl Into<Cow<'static, str>>, position: CodeRange) -> Self {
        Self {
            message: message.into(),
            position,
            exc_type: ExcType::SyntaxError,
        }
    }

    /// Creates a compile error that surfaces as `NotImplementedError`.
    ///
    /// Used for Python constructs Monty deliberately rejects rather than
    /// supports (e.g. reassigning a reserved module dunder), matching the
    /// `NotImplementedError` Monty raises for other unsupported syntax.
    pub(super) fn not_implemented(message: impl Into<Cow<'static, str>>, position: CodeRange) -> Self {
        Self {
            message: message.into(),
            position,
            exc_type: ExcType::NotImplementedError,
        }
    }

    /// Converts this compile error into a Python exception.
    ///
    /// Uses the stored exception type (SyntaxError or ModuleNotFoundError).
    /// - SyntaxError: hides the `, in <module>` part (CPython's format)
    /// - ModuleNotFoundError: hides caret markers (CPython doesn't show them)
    pub fn into_python_exc(self, filename: &str, source: &str) -> MontyException {
        let mut source_map = SourceMap::new(source);
        let mut frame = if self.exc_type == ExcType::SyntaxError {
            // SyntaxError uses different format: no `, in <module>`
            StackFrame::from_position_syntax_error(self.position, filename, &mut source_map)
        } else {
            StackFrame::from_position(self.position, filename, &mut source_map)
        };
        // CPython doesn't show carets for module not found errors
        if self.exc_type == ExcType::ModuleNotFoundError {
            frame.hide_caret = true;
        }
        MontyException::new_full(self.exc_type, Some(self.message.into_owned()), vec![frame])
    }
}

// ============================================================================
// Operator Mapping Functions
// ============================================================================

/// Maps a binary `Operator` to its corresponding `Opcode`.
fn operator_to_opcode(op: &Operator) -> Opcode {
    match op {
        Operator::Add => Opcode::BinaryAdd,
        Operator::Sub => Opcode::BinarySub,
        Operator::Mult => Opcode::BinaryMul,
        Operator::Div => Opcode::BinaryDiv,
        Operator::FloorDiv => Opcode::BinaryFloorDiv,
        Operator::Mod => Opcode::BinaryMod,
        Operator::Pow => Opcode::BinaryPow,
        Operator::MatMult => Opcode::BinaryMatMul,
        Operator::LShift => Opcode::BinaryLShift,
        Operator::RShift => Opcode::BinaryRShift,
        Operator::BitOr => Opcode::BinaryOr,
        Operator::BitXor => Opcode::BinaryXor,
        Operator::BitAnd => Opcode::BinaryAnd,
        // And/Or are handled separately for short-circuit evaluation
        Operator::And | Operator::Or => {
            unreachable!("And/Or operators handled in compile_binary_op")
        }
    }
}

/// Maps an `Operator` to its in-place (augmented assignment) `Opcode`.
///
/// Returns `None` for operators that don't have an in-place opcode (currently `MatMult`,
/// since matrix multiplication is not yet supported). Returns `Some(opcode)` for all
/// other valid augmented assignment operators.
///
/// # Panics
///
/// Panics if called with `And` or `Or` operators, which cannot be used in augmented
/// assignments (this would be a parser bug).
fn operator_to_inplace_opcode(op: &Operator) -> Option<Opcode> {
    match op {
        Operator::Add => Some(Opcode::InplaceAdd),
        Operator::Sub => Some(Opcode::InplaceSub),
        Operator::Mult => Some(Opcode::InplaceMul),
        Operator::Div => Some(Opcode::InplaceDiv),
        Operator::FloorDiv => Some(Opcode::InplaceFloorDiv),
        Operator::Mod => Some(Opcode::InplaceMod),
        Operator::Pow => Some(Opcode::InplacePow),
        Operator::BitAnd => Some(Opcode::InplaceAnd),
        Operator::BitOr => Some(Opcode::InplaceOr),
        Operator::BitXor => Some(Opcode::InplaceXor),
        Operator::LShift => Some(Opcode::InplaceLShift),
        Operator::RShift => Some(Opcode::InplaceRShift),
        Operator::MatMult => None,
        Operator::And | Operator::Or => {
            unreachable!("And/Or operators cannot be used in augmented assignment")
        }
    }
}

/// Maps a `CmpOperator` to its corresponding `Opcode`.
fn cmp_operator_to_opcode(op: &CmpOperator) -> Opcode {
    match op {
        CmpOperator::Eq => Opcode::CompareEq,
        CmpOperator::NotEq => Opcode::CompareNe,
        CmpOperator::Lt => Opcode::CompareLt,
        CmpOperator::LtE => Opcode::CompareLe,
        CmpOperator::Gt => Opcode::CompareGt,
        CmpOperator::GtE => Opcode::CompareGe,
        CmpOperator::Is => Opcode::CompareIs,
        CmpOperator::IsNot => Opcode::CompareIsNot,
        CmpOperator::In => Opcode::CompareIn,
        CmpOperator::NotIn => Opcode::CompareNotIn,
        // ModEq is handled specially at the call site (needs constant operand)
        CmpOperator::ModEq(_) => unreachable!("ModEq handled at call site"),
    }
}

/// Returns `true` if any item in the sequence is a PEP 448 unpack (`*expr`).
///
/// Used to choose between the fast single-`Build*(N)` path and the generalized
/// incremental `Build*(0)` + `ListAppend`/`ListExtend` (or `SetAdd`/`SetExtend`) path.
/// Only the generalized path is needed when at least one `Unpack` variant is present.
fn has_unpack_seq(items: &[SequenceItem]) -> bool {
    items.iter().any(|i| matches!(i, SequenceItem::Unpack(_)))
}

/// Returns `true` if any item in the dict literal is a PEP 448 `**expr` unpack.
///
/// Used to choose between the fast single-`BuildDict(N)` path and the generalized
/// incremental `BuildDict(0)` + `DictSetItem`/`DictUpdate` path.
fn has_unpack_dict(items: &[DictItem]) -> bool {
    items.iter().any(|i| matches!(i, DictItem::Unpack(_)))
}
