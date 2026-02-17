//! Bytecode compiler for transforming AST to bytecode.
//!
//! The compiler traverses the prepared AST (`PreparedNode` and `Expr` types from `expressions.rs`)
//! and emits bytecode instructions using `CodeBuilder`. It handles variable scoping,
//! control flow, and expression evaluation order following Python semantics.
//!
//! Functions are compiled recursively: when a `PreparedFunctionDef` is encountered,
//! its body is compiled to bytecode and a `Function` struct is created. All compiled
//! functions are collected and returned along with the module code.

use std::borrow::Cow;

use super::{
    builder::{CodeBuilder, JumpLabel},
    code::{Code, ExceptionEntry},
    op::Opcode,
};
use crate::{
    args::{ArgExprs, Kwarg},
    builtins::Builtins,
    exception_private::ExcType,
    exception_public::{MontyException, StackFrame},
    expressions::{
        Callable, CmpOperator, Comprehension, Expr, ExprLoc, Identifier, Literal, NameScope, Node, Operator,
        PreparedFunctionDef, PreparedNode, UnpackTarget,
    },
    fstring::{ConversionFlag, FStringPart, FormatSpec, ParsedFormatSpec, encode_format_spec},
    function::Function,
    intern::{Interns, StringId},
    modules::BuiltinModule,
    parse::{CodeRange, ExceptHandler, Try},
    value::{EitherStr, Value},
};

/// Maximum number of arguments allowed in a function call.
///
/// This limit comes from the bytecode format: `CallFunction` and `CallAttr`
/// use a u8 operand for the argument count, so max 255. Python itself has no
/// such limit but we need one for our bytecode encoding.
const MAX_CALL_ARGS: usize = 255;

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

    /// Base namespace slot for cell variables.
    ///
    /// For functions, this is the parameter count. Cell/free variable namespace slots
    /// start at `cell_base`, so we subtract this when emitting LoadCell/StoreCell
    /// to convert to the cells array index.
    cell_base: u16,

    /// Stack of finally targets for handling returns inside try-finally.
    ///
    /// When a return statement is compiled inside a try-finally block, instead
    /// of immediately returning, we store the return value and jump to the
    /// finally block. The finally block will then execute the return.
    finally_targets: Vec<FinallyTarget>,

    /// Tracks nesting depth inside exception handlers.
    ///
    /// When break/continue/return is inside an except handler, we need to
    /// clear the current exception (`ClearException`) and pop the exception
    /// value from the stack before jumping to the finally path or loop target.
    except_handler_depth: usize,
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
    /// Bytecode offset of loop start (for continue).
    start: usize,
    /// Jump labels that need patching to loop end (for break).
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
    /// The jump instruction that needs to be patched.
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
    fn new(interns: &'a Interns, functions: Vec<Function>) -> Self {
        Self {
            code: CodeBuilder::new(),
            interns,
            functions,
            loop_stack: Vec::new(),
            cell_base: 0,
            finally_targets: Vec::new(),
            except_handler_depth: 0,
        }
    }

    /// Creates a new compiler with a specific cell base offset.
    fn new_with_cell_base(interns: &'a Interns, functions: Vec<Function>, cell_base: u16) -> Self {
        Self {
            code: CodeBuilder::new(),
            interns,
            functions,
            loop_stack: Vec::new(),
            cell_base,
            finally_targets: Vec::new(),
            except_handler_depth: 0,
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
        num_locals: u16,
    ) -> Result<CompileResult, CompileError> {
        Self::compile_module_with_functions(nodes, interns, num_locals, Vec::new())
    }

    /// Compiles module-level code while preserving an existing function table prefix.
    ///
    /// This is used by incremental REPL compilation so previously created
    /// `FunctionId`s remain stable: new function IDs are allocated after
    /// `existing_functions.len()`.
    pub fn compile_module_with_functions(
        nodes: &[PreparedNode],
        interns: &Interns,
        num_locals: u16,
        existing_functions: Vec<Function>,
    ) -> Result<CompileResult, CompileError> {
        let mut compiler = Compiler::new(interns, Vec::new());
        compiler.functions = existing_functions;
        compiler.compile_block(nodes)?;

        // Module returns None if no explicit return
        compiler.code.emit(Opcode::LoadNone);
        compiler.code.emit(Opcode::ReturnValue);

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
    /// The `cell_base` parameter is the number of parameter slots, used to convert
    /// cell variable namespace slots to cells array indices.
    ///
    /// The `functions` parameter receives any previously compiled functions, and
    /// any nested functions found in the body will be added to it.
    fn compile_function_body(
        body: &[PreparedNode],
        interns: &Interns,
        functions: Vec<Function>,
        num_locals: u16,
        cell_base: u16,
    ) -> Result<(Code, Vec<Function>), CompileError> {
        let mut compiler = Compiler::new_with_cell_base(interns, functions, cell_base);
        compiler.compile_block(body)?;

        // Implicit return None if no explicit return
        compiler.code.emit(Opcode::LoadNone);
        compiler.code.emit(Opcode::ReturnValue);

        Ok((compiler.code.build(num_locals), compiler.functions))
    }

    /// Compiles a block of statements.
    fn compile_block(&mut self, nodes: &[PreparedNode]) -> Result<(), CompileError> {
        for node in nodes {
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
                self.code.emit(Opcode::Pop); // Discard result
            }
            Node::Return(expr) => {
                self.compile_expr(expr)?;
                self.compile_return();
            }
            Node::ReturnNone => {
                self.code.emit(Opcode::LoadNone);
                self.compile_return();
            }
            Node::Assign { target, object } => {
                self.compile_expr(object)?;
                self.compile_store(target);
            }
            Node::UnpackAssign {
                targets,
                targets_position,
                object,
            } => {
                self.compile_expr(object)?;

                // Check if there's a starred target
                let star_idx = targets.iter().position(|t| matches!(t, UnpackTarget::Starred(_)));

                // Set location to targets for proper caret in tracebacks
                self.code.set_location(*targets_position, None);

                if let Some(star_idx) = star_idx {
                    // Has starred target - use UnpackEx
                    let before = u8::try_from(star_idx).expect("too many targets before star");
                    let after = u8::try_from(targets.len() - star_idx - 1).expect("too many targets after star");
                    self.code.emit_u8_u8(Opcode::UnpackEx, before, after);
                } else {
                    // No starred target - use UnpackSequence
                    let count = u8::try_from(targets.len()).expect("too many targets in unpack");
                    self.code.emit_u8(Opcode::UnpackSequence, count);
                }

                // After UnpackSequence/UnpackEx, values are on stack with first item on top
                // Store them in order (first target gets first item), handling nesting
                for target in targets {
                    self.compile_unpack_target(target);
                }
            }
            Node::OpAssign { target, op, object } => {
                let Some(opcode) = operator_to_inplace_opcode(op) else {
                    return Err(CompileError::new(
                        "matrix multiplication augmented assignment (@=) is not yet supported",
                        target.position,
                    ));
                };
                self.compile_name(target);
                self.compile_expr(object)?;
                self.code.emit(opcode);
                self.compile_store(target);
            }
            Node::SubscriptAssign {
                target,
                index,
                value,
                target_position,
            } => {
                // Stack order for StoreSubscr: value, obj, index
                self.compile_expr(value)?;
                self.compile_name(target);
                self.compile_expr(index)?;
                // Set location to the target (e.g., `lst[10]`) for proper caret in tracebacks
                self.code.set_location(*target_position, None);
                self.code.emit(Opcode::StoreSubscr);
            }
            Node::AttrAssign {
                object,
                attr,
                target_position,
                value,
            } => {
                // Stack order for StoreAttr: value, obj
                self.compile_expr(value)?;
                self.compile_expr(object)?;
                let name_id = attr.string_id().expect("StoreAttr requires interned attr name");
                // Set location to the target (e.g., `x.foo`) for proper caret in tracebacks
                self.code.set_location(*target_position, None);
                self.code.emit_u16(
                    Opcode::StoreAttr,
                    u16::try_from(name_id.index()).expect("name index exceeds u16"),
                );
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
                    self.code.emit(Opcode::Raise);
                } else {
                    self.code.emit(Opcode::Reraise);
                }
            }
            Node::FunctionDef(func_def) => self.compile_function_def(func_def)?,
            Node::Try(try_block) => self.compile_try(try_block)?,
            Node::Import { module_name, binding } => self.compile_import(*module_name, binding),
            Node::ImportFrom {
                module_name,
                names,
                position,
            } => self.compile_import_from(*module_name, names, *position),
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

        // Check bytecode operand limits
        if func_def.default_exprs.len() > MAX_CALL_ARGS {
            return Err(CompileError::new(
                format!("more than {MAX_CALL_ARGS} default parameter values"),
                func_pos,
            ));
        }
        if func_def.free_var_enclosing_slots.len() > MAX_CALL_ARGS {
            return Err(CompileError::new(
                format!("more than {MAX_CALL_ARGS} closure variables"),
                func_pos,
            ));
        }

        // 1. Compile the function body recursively
        // Take ownership of functions for the recursive compile, then restore
        let functions = std::mem::take(&mut self.functions);
        let cell_base = u16::try_from(func_def.signature.param_count()).expect("function parameter count exceeds u16");
        let namespace_size = u16::try_from(func_def.namespace_size).expect("function namespace size exceeds u16");
        let (body_code, mut functions) =
            Self::compile_function_body(&func_def.body, self.interns, functions, namespace_size, cell_base)?;

        // 2. Create the compiled Function and add to the vector
        let func_id = functions.len();
        let function = Function::new(
            func_def.name,
            func_def.signature.clone(),
            func_def.namespace_size,
            func_def.free_var_enclosing_slots.clone(),
            func_def.cell_var_count,
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
        let defaults_count =
            u8::try_from(func_def.default_exprs.len()).expect("function default argument count exceeds u8");
        let func_id_u16 = u16::try_from(func_id).expect("function count exceeds u16");

        // 4. Emit MakeFunction or MakeClosure (if has free vars)
        if func_def.free_var_enclosing_slots.is_empty() {
            // MakeFunction: func_id (u16) + defaults_count (u8)
            self.code.emit_u16_u8(Opcode::MakeFunction, func_id_u16, defaults_count);
        } else {
            // Push captured cells from enclosing scope
            for &slot in &func_def.free_var_enclosing_slots {
                // Load the cell reference from the enclosing namespace
                let slot_u16 = u16::try_from(slot.index()).expect("closure slot index exceeds u16");
                self.code.emit_load_local(slot_u16);
            }
            let cell_count =
                u8::try_from(func_def.free_var_enclosing_slots.len()).expect("closure cell count exceeds u8");
            // MakeClosure: func_id (u16) + defaults_count (u8) + cell_count (u8)
            self.code
                .emit_u16_u8_u8(Opcode::MakeClosure, func_id_u16, defaults_count, cell_count);
        }

        // 5. Store the function object to its name slot
        self.compile_store(&func_def.name);

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

        // Check bytecode operand limits
        if func_def.default_exprs.len() > MAX_CALL_ARGS {
            return Err(CompileError::new(
                format!("more than {MAX_CALL_ARGS} default parameter values"),
                func_pos,
            ));
        }
        if func_def.free_var_enclosing_slots.len() > MAX_CALL_ARGS {
            return Err(CompileError::new(
                format!("more than {MAX_CALL_ARGS} closure variables"),
                func_pos,
            ));
        }

        // 1. Compile the function body recursively
        let functions = std::mem::take(&mut self.functions);
        let cell_base = u16::try_from(func_def.signature.param_count()).expect("function parameter count exceeds u16");
        let namespace_size = u16::try_from(func_def.namespace_size).expect("function namespace size exceeds u16");
        let (body_code, mut functions) =
            Self::compile_function_body(&func_def.body, self.interns, functions, namespace_size, cell_base)?;

        // 2. Create the compiled Function and add to the vector
        let func_id = functions.len();
        let function = Function::new(
            func_def.name,
            func_def.signature.clone(),
            func_def.namespace_size,
            func_def.free_var_enclosing_slots.clone(),
            func_def.cell_var_count,
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
        let defaults_count =
            u8::try_from(func_def.default_exprs.len()).expect("function default argument count exceeds u8");
        let func_id_u16 = u16::try_from(func_id).expect("function count exceeds u16");

        // 4. Emit MakeFunction or MakeClosure (if has free vars)
        if func_def.free_var_enclosing_slots.is_empty() {
            // MakeFunction: func_id (u16) + defaults_count (u8)
            self.code.emit_u16_u8(Opcode::MakeFunction, func_id_u16, defaults_count);
        } else {
            // Push captured cells from enclosing scope
            for &slot in &func_def.free_var_enclosing_slots {
                let slot_u16 = u16::try_from(slot.index()).expect("closure slot index exceeds u16");
                self.code.emit_load_local(slot_u16);
            }
            let cell_count =
                u8::try_from(func_def.free_var_enclosing_slots.len()).expect("closure cell count exceeds u8");
            // MakeClosure: func_id (u16) + defaults_count (u8) + cell_count (u8)
            self.code
                .emit_u16_u8_u8(Opcode::MakeClosure, func_id_u16, defaults_count, cell_count);
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
    fn compile_import(&mut self, module_name: StringId, binding: &Identifier) {
        let position = binding.position;
        self.code.set_location(position, None);

        // Look up the module by name
        if let Some(builtin_module) = BuiltinModule::from_string_id(module_name) {
            // Known module - emit LoadModule
            self.code.emit_u8(Opcode::LoadModule, builtin_module as u8);
            // Store to the binding (respects Local/Global/Cell scope)
            self.compile_store(binding);
        } else {
            // Unknown module - defer error to runtime with RaiseImportError
            // This allows TYPE_CHECKING imports to compile without error
            let name_const = self.code.add_const(Value::InternString(module_name));
            self.code.emit_u16(Opcode::RaiseImportError, name_const);
        }
    }

    /// Compiles a `from module import name, ...` statement.
    ///
    /// Creates the module once, then loads each attribute and stores to the binding.
    /// Invalid attribute names will raise `AttributeError` at runtime.
    /// If the module is unknown, emits `RaiseImportError` to defer the error to runtime.
    /// This allows imports inside `if TYPE_CHECKING:` blocks to compile successfully.
    fn compile_import_from(&mut self, module_name: StringId, names: &[(StringId, Identifier)], position: CodeRange) {
        self.code.set_location(position, None);

        // Look up the module
        if let Some(builtin_module) = BuiltinModule::from_string_id(module_name) {
            // Known module - emit LoadModule
            self.code.emit_u8(Opcode::LoadModule, builtin_module as u8);

            // For each name to import
            for (i, (import_name, binding)) in names.iter().enumerate() {
                // Dup the module if this isn't the last import (last one consumes the module)
                if i < names.len() - 1 {
                    self.code.emit(Opcode::Dup);
                }

                // Load the attribute from the module (raises ImportError if not found)
                let name_idx = u16::try_from(import_name.index()).expect("name index exceeds u16");
                self.code.emit_u16(Opcode::LoadAttrImport, name_idx);

                // Store to the binding
                self.compile_store(binding);
            }
        } else {
            // Unknown module - defer error to runtime with RaiseImportError
            // This allows TYPE_CHECKING imports to compile without error
            let name_const = self.code.add_const(Value::InternString(module_name));
            self.code.emit_u16(Opcode::RaiseImportError, name_const);
        }
    }

    // ========================================================================
    // Expression Compilation
    // ========================================================================

    /// Compiles an expression, leaving its value on the stack.
    fn compile_expr(&mut self, expr_loc: &ExprLoc) -> Result<(), CompileError> {
        // Set source location for traceback info
        self.code.set_location(expr_loc.position, None);

        match &expr_loc.expr {
            Expr::Literal(lit) => self.compile_literal(lit),

            Expr::Name(ident) => self.compile_name(ident),

            Expr::Builtin(builtin) => {
                let idx = self.code.add_const(Value::Builtin(*builtin));
                self.code.emit_u16(Opcode::LoadConst, idx);
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
                    let const_idx = self.code.add_const(Value::Int(*value));
                    self.code.emit_u16(Opcode::CompareModEq, const_idx);
                } else {
                    self.code.emit(cmp_operator_to_opcode(op));
                }
            }

            Expr::ChainCmp { left, comparisons } => {
                self.compile_chain_comparison(left, comparisons, expr_loc.position)?;
            }

            Expr::Not(operand) => {
                self.compile_expr(operand)?;
                // Restore the full expression's position for traceback caret range
                self.code.set_location(expr_loc.position, None);
                self.code.emit(Opcode::UnaryNot);
            }

            Expr::UnaryMinus(operand) => {
                self.compile_expr(operand)?;
                // Restore the full expression's position for traceback caret range
                self.code.set_location(expr_loc.position, None);
                self.code.emit(Opcode::UnaryNeg);
            }

            Expr::UnaryPlus(operand) => {
                self.compile_expr(operand)?;
                // Restore the full expression's position for traceback caret range
                self.code.set_location(expr_loc.position, None);
                self.code.emit(Opcode::UnaryPos);
            }

            Expr::UnaryInvert(operand) => {
                self.compile_expr(operand)?;
                // Restore the full expression's position for traceback caret range
                self.code.set_location(expr_loc.position, None);
                self.code.emit(Opcode::UnaryInvert);
            }

            Expr::List(elements) => {
                for elem in elements {
                    self.compile_expr(elem)?;
                }
                self.code.emit_u16(
                    Opcode::BuildList,
                    u16::try_from(elements.len()).expect("elements count exceeds u16"),
                );
            }

            Expr::Tuple(elements) => {
                for elem in elements {
                    self.compile_expr(elem)?;
                }
                self.code.emit_u16(
                    Opcode::BuildTuple,
                    u16::try_from(elements.len()).expect("elements count exceeds u16"),
                );
            }

            Expr::Dict(pairs) => {
                for (key, value) in pairs {
                    self.compile_expr(key)?;
                    self.compile_expr(value)?;
                }
                self.code.emit_u16(
                    Opcode::BuildDict,
                    u16::try_from(pairs.len()).expect("pairs count exceeds u16"),
                );
            }

            Expr::Set(elements) => {
                for elem in elements {
                    self.compile_expr(elem)?;
                }
                self.code.emit_u16(
                    Opcode::BuildSet,
                    u16::try_from(elements.len()).expect("elements count exceeds u16"),
                );
            }

            Expr::Subscript { object, index } => {
                self.compile_expr(object)?;
                self.compile_expr(index)?;
                // Restore the full subscript expression's position for traceback
                self.code.set_location(expr_loc.position, None);
                self.code.emit(Opcode::BinarySubscr);
            }

            Expr::IfElse { test, body, orelse } => {
                self.compile_if_else_expr(test, body, orelse)?;
            }

            Expr::AttrGet { object, attr } => {
                self.compile_expr(object)?;
                // Restore the full expression's position for traceback caret range
                self.code.set_location(expr_loc.position, None);
                let name_id = attr.string_id().expect("LoadAttr requires interned attr name");
                self.code.emit_u16(
                    Opcode::LoadAttr,
                    u16::try_from(name_id.index()).expect("name index exceeds u16"),
                );
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
                self.code.emit_u16(Opcode::BuildFString, part_count);
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
                self.code.emit(Opcode::Await);
            }

            Expr::Slice { lower, upper, step } => {
                // Compile slice components: start, stop, step (push None for missing)
                if let Some(lower) = lower {
                    self.compile_expr(lower)?;
                } else {
                    self.code.emit(Opcode::LoadNone);
                }
                if let Some(upper) = upper {
                    self.compile_expr(upper)?;
                } else {
                    self.code.emit(Opcode::LoadNone);
                }
                if let Some(step) = step {
                    self.compile_expr(step)?;
                } else {
                    self.code.emit(Opcode::LoadNone);
                }
                self.code.emit(Opcode::BuildSlice);
            }

            Expr::Named { target, value } => {
                // Compile the value expression (leaves result on stack)
                self.compile_expr(value)?;
                // Duplicate so value remains after store
                self.code.emit(Opcode::Dup);
                // Store to target (pops one copy)
                self.compile_store(target);
            }
        }
        Ok(())
    }

    // ========================================================================
    // Literal Compilation
    // ========================================================================

    /// Compiles a literal value.
    fn compile_literal(&mut self, literal: &Literal) {
        match literal {
            Literal::None => {
                self.code.emit(Opcode::LoadNone);
            }

            Literal::Bool(true) => {
                self.code.emit(Opcode::LoadTrue);
            }

            Literal::Bool(false) => {
                self.code.emit(Opcode::LoadFalse);
            }

            Literal::Int(n) => {
                // Use LoadSmallInt for values that fit in i8
                if let Ok(small) = i8::try_from(*n) {
                    self.code.emit_i8(Opcode::LoadSmallInt, small);
                } else {
                    let idx = self.code.add_const(Value::from(*literal));
                    self.code.emit_u16(Opcode::LoadConst, idx);
                }
            }

            // For Float, Str, Bytes, Ellipsis - use LoadConst with Value::from
            _ => {
                let idx = self.code.add_const(Value::from(*literal));
                self.code.emit_u16(Opcode::LoadConst, idx);
            }
        }
    }

    // ========================================================================
    // Variable Operations
    // ========================================================================

    /// Compiles loading a variable onto the stack.
    fn compile_name(&mut self, ident: &Identifier) {
        let slot = u16::try_from(ident.namespace_id().index()).expect("local slot exceeds u16");
        match ident.scope {
            NameScope::Local => {
                // True local - register name and mark as assigned for UnboundLocalError
                self.code.register_local_name(slot, ident.name_id);
                self.code.register_assigned_local(slot);
                self.code.emit_load_local(slot);
            }
            NameScope::LocalUnassigned => {
                // Undefined reference - register name but NOT as assigned for NameError
                self.code.register_local_name(slot, ident.name_id);
                self.code.emit_load_local(slot);
            }
            NameScope::Global => {
                self.code.emit_u16(Opcode::LoadGlobal, slot);
            }
            NameScope::Cell => {
                // Convert namespace slot to cells array index
                let cell_index = slot.saturating_sub(self.cell_base);
                // Register the name for NameError messages (unbound free variable)
                self.code.register_local_name(cell_index, ident.name_id);
                self.code.emit_u16(Opcode::LoadCell, cell_index);
            }
        }
    }

    /// Compiles loading a variable with position tracking for proper traceback ranges.
    ///
    /// Sets the identifier's position before loading, so NameErrors show the correct caret.
    fn compile_name_with_position(&mut self, ident: &Identifier) {
        // Set the identifier's position for proper traceback caret range
        self.code.set_location(ident.position, None);
        self.compile_name(ident);
    }

    /// Compiles storing the top of stack to a variable.
    fn compile_store(&mut self, target: &Identifier) {
        let slot = u16::try_from(target.namespace_id().index()).expect("local slot exceeds u16");
        match target.scope {
            NameScope::Local | NameScope::LocalUnassigned => {
                // Both true locals and initially-unassigned slots use local storage
                self.code.register_local_name(slot, target.name_id);
                self.code.emit_store_local(slot);
            }
            NameScope::Global => {
                self.code.emit_u16(Opcode::StoreGlobal, slot);
            }
            NameScope::Cell => {
                // Convert namespace slot to cells array index
                let cell_index = slot.saturating_sub(self.cell_base);
                self.code.emit_u16(Opcode::StoreCell, cell_index);
            }
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
                let end_jump = self.code.emit_jump(Opcode::JumpIfFalseOrPop);
                self.compile_expr(right)?;
                self.code.patch_jump(end_jump);
            }

            // Short-circuit OR: evaluate left, jump if truthy
            Operator::Or => {
                self.compile_expr(left)?;
                let end_jump = self.code.emit_jump(Opcode::JumpIfTrueOrPop);
                self.compile_expr(right)?;
                self.code.patch_jump(end_jump);
            }

            // Regular binary operators
            _ => {
                self.compile_expr(left)?;
                self.compile_expr(right)?;
                // Restore the full expression's position for traceback caret range
                self.code.set_location(parent_pos, None);
                self.code.emit(operator_to_opcode(op));
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

        // Remember stack depth before the chain for cleanup calculation
        let base_depth = self.code.stack_depth();

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
                self.code.emit(Opcode::Dup);
                // Reorder: [prev, curr, curr] -> [curr, prev, curr]
                self.code.emit(Opcode::Rot3);
            }

            // Emit comparison
            self.code.set_location(position, None);
            if let CmpOperator::ModEq(value) = op {
                let const_idx = self.code.add_const(Value::Int(*value));
                self.code.emit_u16(Opcode::CompareModEq, const_idx);
            } else {
                self.code.emit(cmp_operator_to_opcode(op));
            }

            if !is_last {
                // Short-circuit: if false, jump to cleanup
                let jump = self.code.emit_jump(Opcode::JumpIfFalseOrPop);
                cleanup_jumps.push(jump);
            }
        }

        // Jump past cleanup (result already on stack)
        let end_jump = self.code.emit_jump(Opcode::Jump);

        // Cleanup: remove the saved intermediate value, keep False result
        // The cleanup is only reached via JumpIfFalseOrPop which doesn't pop,
        // so the stack has: [intermediate, False] (2 extra items from base)
        for jump in cleanup_jumps {
            self.code.patch_jump(jump);
        }
        self.code.set_stack_depth(base_depth + 2); // [intermediate, False]
        self.code.emit(Opcode::Rot2); // [False, intermediate]
        self.code.emit(Opcode::Pop); // [False]

        self.code.patch_jump(end_jump);
        // Final result is on stack: base_depth + 1
        self.code.set_stack_depth(base_depth + 1);

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
            let end_jump = self.code.emit_jump(Opcode::JumpIfFalse);
            self.compile_block(body)?;
            self.code.patch_jump(end_jump);
        } else {
            // If with else
            let else_jump = self.code.emit_jump(Opcode::JumpIfFalse);
            self.compile_block(body)?;
            let end_jump = self.code.emit_jump(Opcode::Jump);
            self.code.patch_jump(else_jump);
            self.compile_block(or_else)?;
            self.code.patch_jump(end_jump);
        }
        Ok(())
    }

    /// Compiles a ternary conditional expression.
    fn compile_if_else_expr(&mut self, test: &ExprLoc, body: &ExprLoc, orelse: &ExprLoc) -> Result<(), CompileError> {
        self.compile_expr(test)?;
        let else_jump = self.code.emit_jump(Opcode::JumpIfFalse);
        self.compile_expr(body)?;
        let end_jump = self.code.emit_jump(Opcode::Jump);
        self.code.patch_jump(else_jump);
        self.compile_expr(orelse)?;
        self.code.patch_jump(end_jump);
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
            self.code.emit_call_builtin_function(*builtin_func as u8, arg_count);
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
            self.code.emit_call_builtin_type(type_id, arg_count);
            return Ok(());
        }
        // Fall through to standard path for kwargs/unpacking or non-callable types

        // Standard path: push callable, compile args, emit CallFunction/CallFunctionKw
        // Push the callable (use name position for NameError caret range)
        match callable {
            Callable::Builtin(builtin) => {
                let idx = self.code.add_const(Value::Builtin(*builtin));
                self.code.emit_u16(Opcode::LoadConst, idx);
            }
            Callable::Name(ident) => {
                // Use identifier position so NameError shows caret under just the name
                self.compile_name_with_position(ident);
            }
        }

        // Compile arguments and emit the call
        // Restore full call position before CallFunction for call-related errors
        match args {
            ArgExprs::Empty => {
                self.code.set_location(call_pos, None);
                self.code.emit_u8(Opcode::CallFunction, 0);
            }
            ArgExprs::One(arg) => {
                self.compile_expr(arg)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u8(Opcode::CallFunction, 1);
            }
            ArgExprs::Two(arg1, arg2) => {
                self.compile_expr(arg1)?;
                self.compile_expr(arg2)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u8(Opcode::CallFunction, 2);
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
                self.code.emit_u8(Opcode::CallFunction, arg_count);
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
                    kwname_ids.push(u16::try_from(kwarg.key.name_id.index()).expect("name index exceeds u16"));
                }
                self.code.set_location(call_pos, None);
                self.code.emit_call_function_kw(0, &kwname_ids);
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
                            kwname_ids.push(u16::try_from(kwarg.key.name_id.index()).expect("name index exceeds u16"));
                        }
                    }

                    self.code.set_location(call_pos, None);
                    self.code.emit_call_function_kw(
                        u8::try_from(pos_count).expect("positional arg count exceeds u8"),
                        &kwname_ids,
                    );
                }
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
                self.code.emit_u8(Opcode::CallFunction, 0);
            }
            ArgExprs::One(arg) => {
                self.compile_expr(arg)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u8(Opcode::CallFunction, 1);
            }
            ArgExprs::Two(arg1, arg2) => {
                self.compile_expr(arg1)?;
                self.compile_expr(arg2)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u8(Opcode::CallFunction, 2);
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
                self.code.emit_u8(Opcode::CallFunction, arg_count);
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
                    kwname_ids.push(u16::try_from(kwarg.key.name_id.index()).expect("name index exceeds u16"));
                }
                self.code.set_location(call_pos, None);
                self.code.emit_call_function_kw(0, &kwname_ids);
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
                        kwname_ids.push(u16::try_from(kwarg.key.name_id.index()).expect("name index exceeds u16"));
                    }

                    self.code.set_location(call_pos, None);
                    self.code.emit_call_function_kw(
                        u8::try_from(pos_count).expect("positional arg count exceeds u8"),
                        &kwname_ids,
                    );
                }
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
        self.code.emit_u16(
            Opcode::BuildList,
            u16::try_from(pos_count).expect("positional arg count exceeds u16"),
        );

        // Extend with *args if present
        if let Some(var_args_expr) = var_args {
            self.compile_expr(var_args_expr)?;
            self.code.emit(Opcode::ListExtend);
        }

        // Convert list to tuple
        self.code.emit(Opcode::ListToTuple);

        // 2. Build kwargs dict (if we have kwargs or var_kwargs)
        let has_kwargs = kwargs.is_some() || var_kwargs.is_some();
        if has_kwargs {
            // Build dict from regular kwargs
            let kw_count = kwargs.map_or(0, Vec::len);
            if let Some(kwargs) = kwargs {
                for kwarg in kwargs {
                    // Push key as interned string constant
                    let key_const = self.code.add_const(Value::InternString(kwarg.key.name_id));
                    self.code.emit_u16(Opcode::LoadConst, key_const);
                    // Push value
                    self.compile_expr(&kwarg.value)?;
                }
            }
            self.code.emit_u16(
                Opcode::BuildDict,
                u16::try_from(kw_count).expect("keyword count exceeds u16"),
            );

            // Merge **kwargs if present
            // Use 0xFFFF for func_name_id (like builtins) since we don't have a name
            if let Some(var_kwargs_expr) = var_kwargs {
                self.compile_expr(var_kwargs_expr)?;
                self.code.emit_u16(Opcode::DictMerge, 0xFFFF);
            }
        }

        // 3. Call the function
        self.code.set_location(call_pos, None);
        let flags = u8::from(has_kwargs);
        self.code.emit_u8(Opcode::CallFunctionExtended, flags);
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
            ArgExprs::Kwargs(_) | ArgExprs::ArgsKargs { .. } => Ok(None),
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
        // Get function name for error messages (0xFFFF for builtins)
        let func_name_id = match callable {
            Callable::Name(ident) => u16::try_from(ident.name_id.index()).expect("name index exceeds u16"),
            Callable::Builtin(_) => 0xFFFF,
        };

        // 1. Build args tuple
        // Push regular positional args and build list
        let pos_count = args.map_or(0, Vec::len);
        if let Some(args) = args {
            for arg in args {
                self.compile_expr(arg)?;
            }
        }
        self.code.emit_u16(
            Opcode::BuildList,
            u16::try_from(pos_count).expect("positional arg count exceeds u16"),
        );

        // Extend with *args if present
        if let Some(var_args_expr) = var_args {
            self.compile_expr(var_args_expr)?;
            self.code.emit(Opcode::ListExtend);
        }

        // Convert list to tuple
        self.code.emit(Opcode::ListToTuple);

        // 2. Build kwargs dict (if we have kwargs or var_kwargs)
        let has_kwargs = kwargs.is_some() || var_kwargs.is_some();
        if has_kwargs {
            // Build dict from regular kwargs
            let kw_count = kwargs.map_or(0, Vec::len);
            if let Some(kwargs) = kwargs {
                for kwarg in kwargs {
                    // Push key as interned string constant
                    let key_const = self.code.add_const(Value::InternString(kwarg.key.name_id));
                    self.code.emit_u16(Opcode::LoadConst, key_const);
                    // Push value
                    self.compile_expr(&kwarg.value)?;
                }
            }
            self.code.emit_u16(
                Opcode::BuildDict,
                u16::try_from(kw_count).expect("keyword count exceeds u16"),
            );

            // Merge **kwargs if present
            if let Some(var_kwargs_expr) = var_kwargs {
                self.compile_expr(var_kwargs_expr)?;
                self.code.emit_u16(Opcode::DictMerge, func_name_id);
            }
        }

        // 3. Call the function
        self.code.set_location(call_pos, None);
        let flags = u8::from(has_kwargs);
        self.code.emit_u8(Opcode::CallFunctionExtended, flags);
        Ok(())
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
        // Get the interned attribute name
        let name_id = attr.string_id().expect("CallAttr requires interned attr name");

        // Compile arguments based on the argument type
        match args {
            ArgExprs::Empty => {
                self.code.set_location(call_pos, None);
                self.code.emit_u16_u8(
                    Opcode::CallAttr,
                    u16::try_from(name_id.index()).expect("name index exceeds u16"),
                    0,
                );
            }
            ArgExprs::One(arg) => {
                self.compile_expr(arg)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u16_u8(
                    Opcode::CallAttr,
                    u16::try_from(name_id.index()).expect("name index exceeds u16"),
                    1,
                );
            }
            ArgExprs::Two(arg1, arg2) => {
                self.compile_expr(arg1)?;
                self.compile_expr(arg2)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u16_u8(
                    Opcode::CallAttr,
                    u16::try_from(name_id.index()).expect("name index exceeds u16"),
                    2,
                );
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
                self.code.emit_u16_u8(
                    Opcode::CallAttr,
                    u16::try_from(name_id.index()).expect("name index exceeds u16"),
                    arg_count,
                );
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
                    kwname_ids.push(u16::try_from(kwarg.key.name_id.index()).expect("name index exceeds u16"));
                }
                self.code.set_location(call_pos, None);
                self.code.emit_call_attr_kw(
                    u16::try_from(name_id.index()).expect("name index exceeds u16"),
                    0, // no positional args
                    &kwname_ids,
                );
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
                        kwname_ids.push(u16::try_from(kwarg.key.name_id.index()).expect("name index exceeds u16"));
                    }
                }

                self.code.set_location(call_pos, None);
                self.code.emit_call_attr_kw(
                    u16::try_from(name_id.index()).expect("name index exceeds u16"),
                    u8::try_from(pos_count).expect("positional arg count exceeds u8"),
                    &kwname_ids,
                );
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
        // 1. Build args tuple
        // Push regular positional args and build list
        let pos_count = args.map_or(0, Vec::len);
        if let Some(args) = args {
            for arg in args {
                self.compile_expr(arg)?;
            }
        }
        self.code.emit_u16(
            Opcode::BuildList,
            u16::try_from(pos_count).expect("positional arg count exceeds u16"),
        );

        // Extend with *args if present
        if let Some(var_args_expr) = var_args {
            self.compile_expr(var_args_expr)?;
            self.code.emit(Opcode::ListExtend);
        }

        // Convert list to tuple
        self.code.emit(Opcode::ListToTuple);

        // 2. Build kwargs dict (if we have kwargs or var_kwargs)
        let has_kwargs = kwargs.is_some() || var_kwargs.is_some();
        if has_kwargs {
            // Build dict from regular kwargs
            let kw_count = kwargs.map_or(0, Vec::len);
            if let Some(kwargs) = kwargs {
                for kwarg in kwargs {
                    // Push key as interned string constant
                    let key_const = self.code.add_const(Value::InternString(kwarg.key.name_id));
                    self.code.emit_u16(Opcode::LoadConst, key_const);
                    // Push value
                    self.compile_expr(&kwarg.value)?;
                }
            }
            self.code.emit_u16(
                Opcode::BuildDict,
                u16::try_from(kw_count).expect("keyword count exceeds u16"),
            );

            // Merge **kwargs if present
            if let Some(var_kwargs_expr) = var_kwargs {
                self.compile_expr(var_kwargs_expr)?;
                // Use the method name for error messages
                self.code.emit_u16(
                    Opcode::DictMerge,
                    u16::try_from(name_id.index()).expect("name index exceeds u16"),
                );
            }
        }

        // 3. Call the method with CallAttrExtended
        self.code.set_location(call_pos, None);
        let name_idx = u16::try_from(name_id.index()).expect("name index exceeds u16");
        let flags = u8::from(has_kwargs);
        self.code.emit_u16_u8(Opcode::CallAttrExtended, name_idx, flags);
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
        self.code.emit(Opcode::GetIter);

        // Loop start
        let loop_start = self.code.current_offset();

        // Push loop info for break/continue
        self.loop_stack.push(LoopInfo {
            start: loop_start,
            break_jumps: Vec::new(),
            has_iterator_on_stack: true,
        });

        // ForIter: advance iterator or jump to end
        let end_jump = self.code.emit_jump(Opcode::ForIter);

        // Store current value to target (handles both single identifiers and tuple unpacking)
        self.compile_unpack_target(target);

        // Compile body
        self.compile_block(body)?;

        // Jump back to loop start
        self.code.emit_jump_to(Opcode::Jump, loop_start);

        // End of loop - ForIter jumps here when iterator is exhausted
        self.code.patch_jump(end_jump);

        // Pop loop info before compiling else block
        let loop_info = self.loop_stack.pop().expect("loop stack underflow");

        // Compile else block (runs if loop completed without break)
        if !or_else.is_empty() {
            self.compile_block(or_else)?;
        }

        // Patch break jumps to here - AFTER the else block so break skips else
        for break_jump in loop_info.break_jumps {
            self.code.patch_jump(break_jump);
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
        let loop_start = self.code.current_offset();

        self.loop_stack.push(LoopInfo {
            start: loop_start,
            break_jumps: Vec::new(),
            has_iterator_on_stack: false,
        });

        self.compile_expr(test)?;
        let end_jump = self.code.emit_jump(Opcode::JumpIfFalse);

        self.compile_block(body)?;
        self.code.emit_jump_to(Opcode::Jump, loop_start);

        self.code.patch_jump(end_jump);
        let loop_info = self.loop_stack.pop().expect("loop stack underflow");

        if !or_else.is_empty() {
            self.compile_block(or_else)?;
        }

        for break_jump in loop_info.break_jumps {
            self.code.patch_jump(break_jump);
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

        // If inside except handlers, clean up ALL exception states
        // Each nested except handler has pushed an exception onto the stack,
        // so we need to clear/pop each one when breaking out
        for _ in 0..self.except_handler_depth {
            self.code.emit(Opcode::ClearException);
            self.code.emit(Opcode::Pop); // Pop the exception value
        }

        // Pop the iterator only for `for` loops (has iterator on stack)
        // `while` loops don't have an iterator to pop
        if self.loop_stack[target_loop_depth].has_iterator_on_stack {
            self.code.emit(Opcode::Pop);
        }

        // Check if we need to go through any finally blocks
        // We need to run finally if break crosses the try boundary, i.e., if
        // we're breaking from a loop that existed before the try started.
        if let Some(finally_target) = self.finally_targets.last_mut()
            && target_loop_depth < finally_target.loop_depth_at_entry
        {
            // Breaking from a loop that's outside (or at the start of) this try-finally,
            // so finally must run before the break
            let jump = self.code.emit_jump(Opcode::Jump);
            finally_target.break_jumps.push(BreakContinueThruFinally {
                jump,
                target_loop_depth,
            });
            // Set stack depth for unreachable cleanup code (see comment below)
            if self.except_handler_depth > 0 {
                self.code
                    .set_stack_depth(u16::try_from(self.except_handler_depth).unwrap_or(u16::MAX));
            }
            return Ok(());
        }

        // No finally to go through, jump directly to loop end
        let jump = self.code.emit_jump(Opcode::Jump);
        self.loop_stack[target_loop_depth].break_jumps.push(jump);

        // The code following this break is unreachable at runtime, but the compiler
        // will still emit cleanup code for each enclosing except handler (ClearException + Pop).
        // Set stack depth so those unreachable pops don't cause negative stack tracking.
        if self.except_handler_depth > 0 {
            self.code
                .set_stack_depth(u16::try_from(self.except_handler_depth).unwrap_or(u16::MAX));
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

        // If inside except handlers, clean up ALL exception states
        // Each nested except handler has pushed an exception onto the stack,
        // so we need to clear/pop each one when continuing
        for _ in 0..self.except_handler_depth {
            self.code.emit(Opcode::ClearException);
            self.code.emit(Opcode::Pop); // Pop the exception value
        }

        // Check if we need to go through any finally blocks
        // We need to run finally if continue crosses the try boundary
        if let Some(finally_target) = self.finally_targets.last_mut()
            && target_loop_depth < finally_target.loop_depth_at_entry
        {
            // Continuing a loop that's outside (or at the start of) this try-finally,
            // so finally must run before the continue
            let jump = self.code.emit_jump(Opcode::Jump);
            finally_target.continue_jumps.push(BreakContinueThruFinally {
                jump,
                target_loop_depth,
            });
            // Set stack depth for unreachable cleanup code (see comment below)
            if self.except_handler_depth > 0 {
                self.code
                    .set_stack_depth(u16::try_from(self.except_handler_depth).unwrap_or(u16::MAX));
            }
            return Ok(());
        }

        // No finally to go through, jump directly to loop start
        let loop_start = self.loop_stack[target_loop_depth].start;
        self.code.emit_jump_to(Opcode::Jump, loop_start);

        // The code following this continue is unreachable at runtime, but the compiler
        // will still emit cleanup code for each enclosing except handler (ClearException + Pop).
        // Set stack depth so those unreachable pops don't cause negative stack tracking.
        if self.except_handler_depth > 0 {
            self.code
                .set_stack_depth(u16::try_from(self.except_handler_depth).unwrap_or(u16::MAX));
        }

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
    fn compile_control_flow_after_finally(&mut self, items: &[BreakContinueThruFinally], is_break: bool) {
        // All items went through the same finally, now we need to dispatch to
        // potentially different loops. For simplicity, we assume all items in
        // a single finally target the same loop (the innermost one at the time).
        // This is always true since break/continue only targets the innermost loop.
        let Some(first) = items.first() else {
            return;
        };
        let target_loop_depth = first.target_loop_depth;

        // Check if there's another finally between us and the target loop
        if let Some(finally_target) = self.finally_targets.last_mut()
            && target_loop_depth < finally_target.loop_depth_at_entry
        {
            // Need to go through another finally
            let jump = self.code.emit_jump(Opcode::Jump);
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
            return;
        }

        // No more finally blocks, jump directly to the loop target
        if is_break {
            let jump = self.code.emit_jump(Opcode::Jump);
            self.loop_stack[target_loop_depth].break_jumps.push(jump);
        } else {
            // else continue
            let loop_start = self.loop_stack[target_loop_depth].start;
            self.code.emit_jump_to(Opcode::Jump, loop_start);
        }
    }

    // ========================================================================
    // Comprehension Compilation
    // ========================================================================

    /// Compiles a list comprehension: `[elt for target in iter if cond...]`
    ///
    /// Bytecode structure:
    /// ```text
    /// BUILD_LIST 0          ; empty result
    /// <compile first iter>
    /// GET_ITER
    /// loop_start:
    ///   FOR_ITER end_loop
    ///   STORE_LOCAL target
    ///   <compile filters - jump back to loop_start if any fails>
    ///   [nested generators...]
    ///   <compile elt>
    ///   LIST_APPEND depth
    ///   JUMP loop_start
    /// end_loop:
    /// ; result list on stack
    /// ```
    fn compile_list_comp(&mut self, elt: &ExprLoc, generators: &[Comprehension]) -> Result<(), CompileError> {
        // Build empty list
        self.code.emit_u16(Opcode::BuildList, 0);

        // Compile the nested generators, which will eventually append to the list
        let depth = u8::try_from(generators.len()).expect("too many generators in list comprehension");
        self.compile_comprehension_generators(generators, 0, |compiler| {
            compiler.compile_expr(elt)?;
            compiler.code.emit_u8(Opcode::ListAppend, depth);
            Ok(())
        })?;

        Ok(())
    }

    /// Compiles a set comprehension: `{elt for target in iter if cond...}`
    fn compile_set_comp(&mut self, elt: &ExprLoc, generators: &[Comprehension]) -> Result<(), CompileError> {
        // Build empty set
        self.code.emit_u16(Opcode::BuildSet, 0);

        // Compile the nested generators, which will eventually add to the set
        let depth = u8::try_from(generators.len()).expect("too many generators in set comprehension");
        self.compile_comprehension_generators(generators, 0, |compiler| {
            compiler.compile_expr(elt)?;
            compiler.code.emit_u8(Opcode::SetAdd, depth);
            Ok(())
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
        // Build empty dict
        self.code.emit_u16(Opcode::BuildDict, 0);

        // Compile the nested generators, which will eventually set items in the dict
        let depth = u8::try_from(generators.len()).expect("too many generators in dict comprehension");
        self.compile_comprehension_generators(generators, 0, |compiler| {
            compiler.compile_expr(key)?;
            compiler.compile_expr(value)?;
            compiler.code.emit_u8(Opcode::DictSetItem, depth);
            Ok(())
        })?;

        Ok(())
    }

    /// Recursively compiles comprehension generators (the for/if clauses).
    ///
    /// For each generator:
    /// 1. Compile the iterator expression and get iterator
    /// 2. Start loop: FOR_ITER to get next value or exit
    /// 3. Store to target variable
    /// 4. Compile filter conditions (jump back to loop start if any fails)
    /// 5. Either recurse for inner generator, or call the body callback
    /// 6. Jump back to loop start
    ///
    /// The `body_fn` callback is called at the innermost level to emit the element/key-value code.
    fn compile_comprehension_generators(
        &mut self,
        generators: &[Comprehension],
        index: usize,
        body_fn: impl FnOnce(&mut Self) -> Result<(), CompileError>,
    ) -> Result<(), CompileError> {
        let generator = &generators[index];

        // Compile iterator expression
        self.compile_expr(&generator.iter)?;
        self.code.emit(Opcode::GetIter);

        // Loop start
        let loop_start = self.code.current_offset();

        // FOR_ITER: advance iterator or jump to end
        let end_jump = self.code.emit_jump(Opcode::ForIter);

        // Store current value to target (single variable or tuple unpacking)
        self.compile_unpack_target(&generator.target);

        // Compile filter conditions - jump back to loop start if any fails
        for cond in &generator.ifs {
            self.compile_expr(cond)?;
            // If condition is false, skip to next iteration
            self.code.emit_jump_to(Opcode::JumpIfFalse, loop_start);
        }

        // Either recurse for inner generator, or emit body
        if index + 1 < generators.len() {
            // Recurse for inner generator
            self.compile_comprehension_generators(generators, index + 1, body_fn)?;
        } else {
            // Innermost level - emit body (the element/key-value expression and append/add/set)
            body_fn(self)?;
        }

        // Jump back to loop start
        self.code.emit_jump_to(Opcode::Jump, loop_start);

        // End of loop
        self.code.patch_jump(end_jump);

        Ok(())
    }

    /// Compiles storage of an unpack target - either a single identifier, nested tuple, or starred.
    ///
    /// For single identifiers: emits a simple store.
    /// For nested tuples: emits `UnpackSequence` (or `UnpackEx` with starred) and recursively
    /// handles each sub-target.
    fn compile_unpack_target(&mut self, target: &UnpackTarget) {
        match target {
            UnpackTarget::Name(ident) => {
                // Single identifier - just store directly
                self.compile_store(ident);
            }
            UnpackTarget::Starred(ident) => {
                // Starred target by itself (shouldn't happen at top level normally)
                // Just store as if it were a name
                self.compile_store(ident);
            }
            UnpackTarget::Tuple { targets, position } => {
                // Check if there's a starred target
                let star_idx = targets.iter().position(|t| matches!(t, UnpackTarget::Starred(_)));

                self.code.set_location(*position, None);

                if let Some(star_idx) = star_idx {
                    // Has starred target - use UnpackEx
                    let before = u8::try_from(star_idx).expect("too many targets before star");
                    let after = u8::try_from(targets.len() - star_idx - 1).expect("too many targets after star");
                    self.code.emit_u8_u8(Opcode::UnpackEx, before, after);
                } else {
                    // No starred target - use UnpackSequence
                    let count = u8::try_from(targets.len()).expect("too many targets in nested unpack");
                    self.code.emit_u8(Opcode::UnpackSequence, count);
                }

                // After UnpackSequence/UnpackEx, values are on stack with first item on top
                // Store them in order, recursively handling further nesting
                for target in targets {
                    self.compile_unpack_target(target);
                }
            }
        }
    }

    // ========================================================================
    // Statement Helpers
    // ========================================================================

    /// Compiles an assert statement.
    fn compile_assert(&mut self, test: &ExprLoc, msg: Option<&ExprLoc>) -> Result<(), CompileError> {
        // Compile test
        self.compile_expr(test)?;
        // Jump over raise if truthy
        let skip_jump = self.code.emit_jump(Opcode::JumpIfTrue);

        // Raise AssertionError
        let exc_idx = self
            .code
            .add_const(Value::Builtin(Builtins::ExcType(ExcType::AssertionError)));
        self.code.emit_u16(Opcode::LoadConst, exc_idx);

        if let Some(msg_expr) = msg {
            // Call AssertionError(msg)
            self.compile_expr(msg_expr)?;
            self.code.emit_u8(Opcode::CallFunction, 1);
        } else {
            // Call AssertionError()
            self.code.emit_u8(Opcode::CallFunction, 0);
        }

        self.code.emit(Opcode::Raise);
        self.code.patch_jump(skip_jump);
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
                    let const_idx = self.code.add_const(Value::InternString(*string_id));
                    self.code.emit_u16(Opcode::LoadConst, const_idx);
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
                        let const_idx = self.code.add_const(Value::InternString(*prefix_id));
                        self.code.emit_u16(Opcode::LoadConst, const_idx);
                        count += 1;
                    }

                    // Compile the expression
                    self.compile_expr(expr)?;

                    // For debug expressions without explicit conversion, Python uses repr by default
                    let effective_conversion = if debug_prefix.is_some() && matches!(conversion, ConversionFlag::None) {
                        ConversionFlag::Repr
                    } else {
                        *conversion
                    };

                    // Emit FormatValue with appropriate flags
                    let flags = self.compile_format_value(effective_conversion, format_spec.as_ref())?;
                    self.code.emit_u8(Opcode::FormatValue, flags);
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Compiles format value flags and optionally pushes format spec to stack.
    ///
    /// Returns the flags byte encoding conversion and format spec presence.
    /// If a format spec is present, it's pushed to the stack before the value.
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
            Some(FormatSpec::Static(parsed)) => {
                // Static format spec - push a marker constant with the parsed spec info
                // We store this as a special format spec value in the constant pool
                // The VM will recognize this and use the pre-parsed spec
                let const_idx = self.add_format_spec_const(parsed);
                self.code.emit_u16(Opcode::LoadConst, const_idx);
                Ok(conv_bits | 0x04) // has format spec on stack
            }
            Some(FormatSpec::Dynamic(dynamic_parts)) => {
                // Compile dynamic format spec parts to build a format spec string
                // Then parse it at runtime
                let part_count = self.compile_fstring_parts(dynamic_parts)?;
                if part_count > 1 {
                    self.code.emit_u16(Opcode::BuildFString, part_count);
                }
                // Format spec string is now on stack
                Ok(conv_bits | 0x04) // has format spec on stack
            }
        }
    }

    /// Adds a format spec to the constant pool as an encoded integer.
    ///
    /// Uses the encoding from `fstring::encode_format_spec` and stores it as
    /// a negative integer to distinguish from regular ints.
    fn add_format_spec_const(&mut self, spec: &ParsedFormatSpec) -> u16 {
        let encoded = encode_format_spec(spec);
        // Use negative to distinguish from regular ints (format spec marker)
        // We negate and subtract 1 to ensure it's negative and recoverable
        let encoded_i64 = i64::try_from(encoded).expect("format spec encoding exceeds i64::MAX");
        let marker = -(encoded_i64 + 1);
        self.code.add_const(Value::Int(marker))
    }

    // ========================================================================
    // Exception Handling Compilation
    // ========================================================================

    /// Compiles a return statement, handling finally blocks properly.
    ///
    /// If we're inside a try-finally block, the return value is kept on the stack
    /// and we jump to a "finally with return" section that runs finally then returns.
    /// Otherwise, we emit a direct `ReturnValue`.
    fn compile_return(&mut self) {
        if let Some(finally_target) = self.finally_targets.last_mut() {
            // Inside a try-finally: jump to finally, then return
            // Return value is already on stack
            let jump = self.code.emit_jump(Opcode::Jump);
            finally_target.return_jumps.push(jump);
        } else {
            // Normal return
            self.code.emit(Opcode::ReturnValue);
        }
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
        let stack_depth = self.code.stack_depth();

        // If there's a finally block, track returns/break/continue inside try/handlers/else
        if has_finally {
            self.finally_targets.push(FinallyTarget {
                return_jumps: Vec::new(),
                break_jumps: Vec::new(),
                continue_jumps: Vec::new(),
                loop_depth_at_entry: self.loop_stack.len(),
            });
        }

        // === Compile try body ===
        let try_start = self.code.current_offset();
        self.compile_block(&try_block.body)?;
        let try_end = self.code.current_offset();

        // Jump to else/finally if no exception (skip handlers)
        let after_try_jump = self.code.emit_jump(Opcode::Jump);

        // === Handler dispatch starts here ===
        let handler_start = self.code.current_offset();

        // VM pushes exception onto stack when entering handler.
        // Adjust compiler's stack depth tracking to reflect this.
        self.code.adjust_stack_depth(1);

        // Track jumps that go to finally (for patching later)
        let mut finally_jumps: Vec<JumpLabel> = Vec::new();

        if has_handlers {
            // Compile exception handlers
            // handler_entry_depth = stack_depth + 1 (exception on stack)
            let handler_entry_depth = stack_depth + 1;
            self.compile_exception_handlers(&try_block.handlers, &mut finally_jumps, handler_entry_depth)?;
        } else {
            // No handlers - just reraise (this only happens with try-finally)
            self.code.emit(Opcode::Reraise);
        }

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
            self.code.set_stack_depth(stack_depth + 1);
            // We need to pop it, run finally, then reraise
            // But we can't easily save the exception, so we use a different approach:
            // The exception is already on the exception_stack from handle_exception,
            // so we can just pop from operand stack, run finally, then reraise.
            self.code.emit(Opcode::Pop); // Pop exception from operand stack
            self.compile_block(&try_block.finally)?;
            self.code.emit(Opcode::Reraise); // Re-raise from exception_stack
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
                    self.code.patch_jump(jump);
                }
                // Return value is on stack, stack = stack_depth + 1
                self.code.set_stack_depth(stack_depth + 1);
                self.compile_block(&try_block.finally)?;
                self.compile_return();
                Some(start)
            };

            // === Finally with break path ===
            // For each break, run finally then either:
            // - Jump to outer finally's break path (if there's an outer finally between us and the loop)
            // - Jump directly to the loop's break target
            if !finally_target.break_jumps.is_empty() {
                for break_info in &finally_target.break_jumps {
                    self.code.patch_jump(break_info.jump);
                }
                // Break already popped the iterator, so stack = stack_depth - 1
                // (the iterator was on stack at try entry, break removed it)
                self.code.set_stack_depth(stack_depth.saturating_sub(1));
                self.compile_block(&try_block.finally)?;
                // After finally, compile the break again (handles nested finally or direct jump)
                self.compile_control_flow_after_finally(&finally_target.break_jumps, true);
            }

            // === Finally with continue path ===
            if !finally_target.continue_jumps.is_empty() {
                for continue_info in &finally_target.continue_jumps {
                    self.code.patch_jump(continue_info.jump);
                }
                // Continue doesn't pop the iterator, stack = stack_depth
                self.code.set_stack_depth(stack_depth);
                self.compile_block(&try_block.finally)?;
                // After finally, compile the continue again (handles nested finally or direct jump)
                self.compile_control_flow_after_finally(&finally_target.continue_jumps, false);
            }

            return_start
        } else {
            None
        };

        // === Else block (runs if no exception) ===
        self.code.patch_jump(after_try_jump);
        // Normal path from try body, stack = stack_depth
        self.code.set_stack_depth(stack_depth);
        let else_start = self.code.current_offset();
        if has_else {
            self.compile_block(&try_block.or_else)?;
        }
        let else_end = self.code.current_offset();

        // === Normal finally path (no exception pending, no return) ===
        // Patch all jumps from handlers to go here
        for jump in finally_jumps {
            self.code.patch_jump(jump);
        }

        if has_finally {
            // Stack = stack_depth (no exception, no return value)
            self.code.set_stack_depth(stack_depth);
            self.compile_block(&try_block.finally)?;
        }

        // === Add exception table entries ===
        // Order matters: entries are searched in order, so inner entries must come first.

        // Entry 1: Try body -> handler dispatch
        if has_handlers || has_finally {
            self.code.add_exception_entry(ExceptionEntry::new(
                u32::try_from(try_start).expect("bytecode offset exceeds u32"),
                u32::try_from(try_end).expect("bytecode offset exceeds u32") + 3, // +3 to include the JUMP instruction
                u32::try_from(handler_start).expect("bytecode offset exceeds u32"),
                stack_depth,
            ));
        }

        // Entry 2: Handler dispatch -> finally cleanup (only if has_finally)
        // This ensures finally runs when RERAISE is executed or any exception occurs in handlers
        if let Some(cleanup_start) = finally_cleanup_start {
            self.code.add_exception_entry(ExceptionEntry::new(
                u32::try_from(handler_start).expect("bytecode offset exceeds u32"),
                u32::try_from(handler_dispatch_end).expect("bytecode offset exceeds u32"),
                u32::try_from(cleanup_start).expect("bytecode offset exceeds u32"),
                stack_depth,
            ));
        }

        // Entry 3: Finally with return -> finally cleanup
        // If an exception occurs while running finally (in the return path), catch it
        if let (Some(return_start), Some(cleanup_start)) = (finally_with_return_start, finally_cleanup_start) {
            self.code.add_exception_entry(ExceptionEntry::new(
                u32::try_from(return_start).expect("bytecode offset exceeds u32"),
                u32::try_from(else_start).expect("bytecode offset exceeds u32"), // End at else_start (before else block)
                u32::try_from(cleanup_start).expect("bytecode offset exceeds u32"),
                stack_depth,
            ));
        }

        // Entry 4: Else block -> finally cleanup (only if has_finally and has_else)
        // Exceptions in else block should go through finally
        if has_else && let Some(cleanup_start) = finally_cleanup_start {
            self.code.add_exception_entry(ExceptionEntry::new(
                u32::try_from(else_start).expect("bytecode offset exceeds u32"),
                u32::try_from(else_end).expect("bytecode offset exceeds u32"),
                u32::try_from(cleanup_start).expect("bytecode offset exceeds u32"),
                stack_depth,
            ));
        }

        Ok(())
    }

    /// Compiles the exception handlers for a try block.
    ///
    /// Each handler checks if the exception matches its type, and if so,
    /// executes the handler body. If no handler matches, the exception is re-raised.
    ///
    /// `handler_entry_depth` is the stack depth when entering handler dispatch
    /// (i.e., base stack_depth + 1 for the exception value).
    fn compile_exception_handlers(
        &mut self,
        handlers: &[ExceptHandler<PreparedNode>],
        finally_jumps: &mut Vec<JumpLabel>,
        handler_entry_depth: u16,
    ) -> Result<(), CompileError> {
        // Track jumps from non-matching handlers to next handler
        let mut next_handler_jumps: Vec<JumpLabel> = Vec::new();

        for (i, handler) in handlers.iter().enumerate() {
            let is_last = i == handlers.len() - 1;

            // Patch jumps from previous handler's non-match to here
            // If jumping from a previous handler's no-match, stack has [exc, exc] (duplicate)
            // We need to pop the duplicate before starting this handler's check
            if !next_handler_jumps.is_empty() {
                for jump in next_handler_jumps.drain(..) {
                    self.code.patch_jump(jump);
                }
                // Reset stack depth for jump target: [exc, exc] = handler_entry_depth + 1
                self.code.set_stack_depth(handler_entry_depth + 1);
                // Pop the duplicate from previous handler's check
                self.code.emit(Opcode::Pop);
            }

            if let Some(exc_type) = &handler.exc_type {
                // Typed handler: except ExcType: or except ExcType as e:
                // Stack: [exception]

                // Duplicate exception for type check
                self.code.emit(Opcode::Dup);
                // Stack: [exception, exception]

                // Load the exception type to match against
                self.compile_expr(exc_type)?;
                // Stack: [exception, exception, exc_type]

                // Check if exception matches the type
                // This validates exc_type is a valid exception type and performs the match
                // CheckExcMatch pops exc_type, peeks exception, pushes bool
                self.code.emit(Opcode::CheckExcMatch);
                // Stack: [exception, exception, bool]

                // Jump to next handler if match returned False
                // JumpIfFalse pops the bool, leaving [exception, exception]
                let no_match_jump = self.code.emit_jump(Opcode::JumpIfFalse);

                if is_last {
                    // Last handler - if no match, reraise
                    // But first we need to handle the exception var cleanup
                } else {
                    next_handler_jumps.push(no_match_jump);
                }

                // After JumpIfFalse (match succeeded), stack is [exception, exception]
                // Pop the duplicate that was used for the type check
                self.code.emit(Opcode::Pop);
                // Stack: [exception]

                // Exception matched! Bind to variable if needed
                if let Some(name) = &handler.name {
                    // Stack: [exception]
                    // Store to variable (don't pop - we still need it for current_exception)
                    self.code.emit(Opcode::Dup);
                    self.compile_store(name);
                }

                // Track that we're inside an except handler (for break/continue cleanup)
                self.except_handler_depth += 1;

                // Compile handler body
                self.compile_block(&handler.body)?;

                // Exit except handler context
                self.except_handler_depth -= 1;

                // Delete exception variable (Python 3 behavior)
                if let Some(name) = &handler.name {
                    self.compile_delete(name);
                }

                // Clear current_exception
                self.code.emit(Opcode::ClearException);

                // Pop the exception from stack
                self.code.emit(Opcode::Pop);

                // Jump to finally
                finally_jumps.push(self.code.emit_jump(Opcode::Jump));

                // If this was last handler and no match, we need to reraise
                if is_last {
                    self.code.patch_jump(no_match_jump);
                    // Coming from JumpIfFalse no-match path, stack has [exception, exception]
                    // Reset stack depth for jump target
                    self.code.set_stack_depth(handler_entry_depth + 1);
                    // We need to pop the duplicate before reraising
                    self.code.emit(Opcode::Pop);
                    self.code.emit(Opcode::Reraise);
                }
            } else {
                // Bare except: catches everything
                // Stack: [exception]

                // Bind to variable if needed
                if let Some(name) = &handler.name {
                    self.code.emit(Opcode::Dup);
                    self.compile_store(name);
                }

                // Track that we're inside an except handler (for break/continue cleanup)
                self.except_handler_depth += 1;

                // Compile handler body
                self.compile_block(&handler.body)?;

                // Exit except handler context
                self.except_handler_depth -= 1;

                // Delete exception variable
                if let Some(name) = &handler.name {
                    self.compile_delete(name);
                }

                // Clear current_exception
                self.code.emit(Opcode::ClearException);

                // Pop the exception from stack
                self.code.emit(Opcode::Pop);

                // Jump to finally
                finally_jumps.push(self.code.emit_jump(Opcode::Jump));
            }
        }

        Ok(())
    }

    /// Compiles deletion of a variable.
    fn compile_delete(&mut self, target: &Identifier) {
        let slot = u16::try_from(target.namespace_id().index()).expect("local slot exceeds u16");
        match target.scope {
            NameScope::Local | NameScope::LocalUnassigned => {
                if let Ok(s) = u8::try_from(slot) {
                    self.code.emit_u8(Opcode::DeleteLocal, s);
                } else {
                    // Wide variant not implemented yet
                    todo!("DeleteLocalW for slot > 255");
                }
            }
            NameScope::Global | NameScope::Cell => {
                // Delete global/cell not commonly needed
                // For now, just store Undefined
                self.code.emit(Opcode::LoadNone);
                self.compile_store(target);
            }
        }
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
    fn new(message: impl Into<Cow<'static, str>>, position: CodeRange) -> Self {
        Self {
            message: message.into(),
            position,
            exc_type: ExcType::SyntaxError,
        }
    }

    /// Converts this compile error into a Python exception.
    ///
    /// Uses the stored exception type (SyntaxError or ModuleNotFoundError).
    /// - SyntaxError: hides the `, in <module>` part (CPython's format)
    /// - ModuleNotFoundError: hides caret markers (CPython doesn't show them)
    pub fn into_python_exc(self, filename: &str, source: &str) -> MontyException {
        let mut frame = if self.exc_type == ExcType::SyntaxError {
            // SyntaxError uses different format: no `, in <module>`
            StackFrame::from_position_syntax_error(self.position, filename, source)
        } else {
            StackFrame::from_position(self.position, filename, source)
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
