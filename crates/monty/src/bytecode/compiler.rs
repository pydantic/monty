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
    value::{Attr, Value},
};

/// Maximum number of arguments allowed in a function call.
///
/// This limit comes from the bytecode format: `CallFunction` and `CallMethod`
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
}

/// Information about a loop for break/continue handling.
///
/// Note: break/continue are not yet implemented in the parser,
/// so this is currently unused but included for future use.
struct LoopInfo {
    /// Bytecode offset of loop start (for continue).
    _start: usize,
    /// Jump labels that need patching to loop end (for break).
    break_jumps: Vec<JumpLabel>,
}

/// Tracks a finally block for handling returns inside try-finally.
///
/// When compiling a try-finally, we push a `FinallyTarget` to track jumps
/// from return statements that need to go through the finally block.
struct FinallyTarget {
    /// Jump labels for returns inside the try block that need to go to finally.
    return_jumps: Vec<JumpLabel>,
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
        let mut compiler = Compiler::new(interns, Vec::new());
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
                let count = u8::try_from(targets.len()).expect("too many targets in unpack");
                // Set location to targets for proper caret in tracebacks
                self.code.set_location(*targets_position, None);
                self.code.emit_u8(Opcode::UnpackSequence, count);
                // After UnpackSequence, values are on stack with first item on top
                // Store them in order (first target gets first item), handling nesting
                for target in targets {
                    self.compile_unpack_target(target);
                }
            }

            Node::OpAssign { target, op, object } => {
                self.compile_name(target);
                self.compile_expr(object)?;
                self.code.emit(operator_to_inplace_opcode(op));
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

            Node::If { test, body, or_else } => {
                self.compile_if(test, body, or_else)?;
            }

            Node::For {
                target,
                iter,
                body,
                or_else,
            } => {
                self.compile_for(target, iter, body, or_else)?;
            }

            Node::Assert { test, msg } => {
                self.compile_assert(test, msg.as_ref())?;
            }

            Node::Raise(expr) => {
                if let Some(exc) = expr {
                    self.compile_expr(exc)?;
                    self.code.emit(Opcode::Raise);
                } else {
                    self.code.emit(Opcode::Reraise);
                }
            }

            Node::FunctionDef(func_def) => {
                self.compile_function_def(func_def)?;
            }

            Node::Try(try_block) => {
                self.compile_try(try_block)?;
            }

            Node::Import { module_name, binding } => {
                self.compile_import(*module_name, binding)?;
            }

            Node::ImportFrom {
                module_name,
                names,
                position,
            } => {
                self.compile_import_from(*module_name, names, *position)?;
            }

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
    fn compile_import(&mut self, module_name: StringId, binding: &Identifier) -> Result<(), CompileError> {
        let position = binding.position;
        // Look up the module by name
        let builtin_module = BuiltinModule::from_string_id(module_name)
            .ok_or_else(|| CompileError::new_module_not_found(self.interns.get_str(module_name), position))?;

        // Emit LoadModule with the module ID
        self.code.set_location(position, None);
        self.code.emit_u8(Opcode::LoadModule, builtin_module as u8);

        // Store to the binding (respects Local/Global/Cell scope)
        self.compile_store(binding);

        Ok(())
    }

    /// Compiles a `from module import name, ...` statement.
    ///
    /// Creates the module once, then loads each attribute and stores to the binding.
    /// Invalid attribute names will raise `AttributeError` at runtime.
    fn compile_import_from(
        &mut self,
        module_name: StringId,
        names: &[(StringId, Identifier)],
        position: CodeRange,
    ) -> Result<(), CompileError> {
        let module_name_str = self.interns.get_str(module_name);

        // Look up the module
        let builtin_module = BuiltinModule::from_string_id(module_name)
            .ok_or_else(|| CompileError::new_module_not_found(module_name_str, position))?;

        // Load the module once
        self.code.set_location(position, None);
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

                // Compile the method call arguments and emit CallMethod
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

    /// Compiles a method call on an object.
    ///
    /// The object should already be on the stack. This compiles the arguments
    /// and emits a CallMethod opcode with the method name and arg count.
    fn compile_method_call(&mut self, attr: &Attr, args: &ArgExprs, call_pos: CodeRange) -> Result<(), CompileError> {
        // Get the interned attribute name
        let name_id = attr.string_id().expect("CallMethod requires interned attr name");

        // Compile arguments based on the argument type
        match args {
            ArgExprs::Empty => {
                self.code.set_location(call_pos, None);
                self.code.emit_u16_u8(
                    Opcode::CallMethod,
                    u16::try_from(name_id.index()).expect("name index exceeds u16"),
                    0,
                );
            }
            ArgExprs::One(arg) => {
                self.compile_expr(arg)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u16_u8(
                    Opcode::CallMethod,
                    u16::try_from(name_id.index()).expect("name index exceeds u16"),
                    1,
                );
            }
            ArgExprs::Two(arg1, arg2) => {
                self.compile_expr(arg1)?;
                self.compile_expr(arg2)?;
                self.code.set_location(call_pos, None);
                self.code.emit_u16_u8(
                    Opcode::CallMethod,
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
                    Opcode::CallMethod,
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
                self.code.emit_call_method_kw(
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
                // Check if there's unpacking - we don't support that for method calls yet
                if var_args.is_some() || var_kwargs.is_some() {
                    return Err(CompileError::new(
                        "method calls with *args or **kwargs unpacking not yet supported".to_owned(),
                        call_pos,
                    ));
                }

                // No unpacking - use CallMethodKw for efficiency
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
                self.code.emit_call_method_kw(
                    u16::try_from(name_id.index()).expect("name index exceeds u16"),
                    u8::try_from(pos_count).expect("positional arg count exceeds u8"),
                    &kwname_ids,
                );
            }
        }
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

        // Push loop info for break/continue (future use)
        self.loop_stack.push(LoopInfo {
            _start: loop_start,
            break_jumps: Vec::new(),
        });

        // ForIter: advance iterator or jump to end
        let end_jump = self.code.emit_jump(Opcode::ForIter);

        // Store current value to target (handles both single identifiers and tuple unpacking)
        self.compile_unpack_target(target);

        // Compile body
        self.compile_block(body)?;

        // Jump back to loop start
        self.code.emit_jump_to(Opcode::Jump, loop_start);

        // End of loop
        self.code.patch_jump(end_jump);

        // Pop loop info and patch break jumps (future use)
        let loop_info = self.loop_stack.pop().expect("loop stack underflow");
        for break_jump in loop_info.break_jumps {
            self.code.patch_jump(break_jump);
        }

        // Compile else block (runs if loop completed without break)
        if !or_else.is_empty() {
            self.compile_block(or_else)?;
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

    /// Compiles storage of an unpack target - either a single identifier or nested tuple.
    ///
    /// For single identifiers: emits a simple store.
    /// For nested tuples: emits `UnpackSequence` and recursively handles each sub-target.
    fn compile_unpack_target(&mut self, target: &UnpackTarget) {
        match target {
            UnpackTarget::Name(ident) => {
                // Single identifier - just store directly
                self.compile_store(ident);
            }
            UnpackTarget::Tuple { targets, position } => {
                // Nested tuple - emit UnpackSequence then recursively store each
                let count = u8::try_from(targets.len()).expect("too many targets in nested unpack");
                // Set location to targets for proper caret in tracebacks
                self.code.set_location(*position, None);
                self.code.emit_u8(Opcode::UnpackSequence, count);
                // After UnpackSequence, values are on stack with first item on top
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
    fn compile_try(&mut self, try_block: &Try<PreparedNode>) -> Result<(), CompileError> {
        let has_finally = !try_block.finally.is_empty();
        let has_handlers = !try_block.handlers.is_empty();
        let has_else = !try_block.or_else.is_empty();

        // Record stack depth at try entry (for unwinding on exception)
        let stack_depth = self.code.stack_depth();

        // If there's a finally block, track returns inside try/handlers/else
        if has_finally {
            self.finally_targets.push(FinallyTarget {
                return_jumps: Vec::new(),
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

        // Track jumps that go to finally (for patching later)
        let mut finally_jumps: Vec<JumpLabel> = Vec::new();

        if has_handlers {
            // Compile exception handlers
            self.compile_exception_handlers(&try_block.handlers, &mut finally_jumps)?;
        } else {
            // No handlers - just reraise (this only happens with try-finally)
            self.code.emit(Opcode::Reraise);
        }

        // Mark end of handler dispatch (for finally exception entry)
        let handler_dispatch_end = self.code.current_offset();

        // === Finally cleanup handler (for exceptions during handler dispatch) ===
        // This catches exceptions from RERAISE (and any other exceptions in handlers)
        // and ensures finally runs before the exception propagates.
        let finally_cleanup_start = if has_finally {
            let cleanup_start = self.code.current_offset();
            // Exception value is on stack (pushed by VM)
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

        // === Finally with return path ===
        // Returns from try/handler/else come here (return value is on stack)
        // Pop finally target and get the return jumps
        let finally_with_return_start = if has_finally {
            let finally_target = self.finally_targets.pop().expect("finally_targets should not be empty");
            if finally_target.return_jumps.is_empty() {
                None
            } else {
                let start = self.code.current_offset();
                // Patch all return jumps to come here
                for jump in finally_target.return_jumps {
                    self.code.patch_jump(jump);
                }
                // Return value is on stack, run finally, then return (or continue to outer finally)
                self.compile_block(&try_block.finally)?;
                // Use compile_return() to handle nested try-finally correctly
                // If there's an outer finally, this jumps there; otherwise it returns
                self.compile_return();
                Some(start)
            }
        } else {
            None
        };

        // === Else block (runs if no exception) ===
        self.code.patch_jump(after_try_jump);
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
    fn compile_exception_handlers(
        &mut self,
        handlers: &[ExceptHandler<PreparedNode>],
        finally_jumps: &mut Vec<JumpLabel>,
    ) -> Result<(), CompileError> {
        // Track jumps from non-matching handlers to next handler
        let mut next_handler_jumps: Vec<JumpLabel> = Vec::new();

        for (i, handler) in handlers.iter().enumerate() {
            let is_last = i == handlers.len() - 1;

            // Patch jumps from previous handler's non-match to here
            for jump in next_handler_jumps.drain(..) {
                self.code.patch_jump(jump);
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
                self.code.emit(Opcode::CheckExcMatch);
                // Stack: [exception, bool]

                // Jump to next handler if match returned False
                let no_match_jump = self.code.emit_jump(Opcode::JumpIfFalse);

                if is_last {
                    // Last handler - if no match, reraise
                    // But first we need to handle the exception var cleanup
                } else {
                    next_handler_jumps.push(no_match_jump);
                }

                // Exception matched! Bind to variable if needed
                if let Some(name) = &handler.name {
                    // Stack: [exception]
                    // Store to variable (don't pop - we still need it for current_exception)
                    self.code.emit(Opcode::Dup);
                    self.compile_store(name);
                }

                // Compile handler body
                self.compile_block(&handler.body)?;

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

                // Compile handler body
                self.compile_block(&handler.body)?;

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

    /// Creates a ModuleNotFoundError for when a module cannot be found.
    ///
    /// Matches CPython's format: `ModuleNotFoundError: No module named 'name'`
    fn new_module_not_found(module_name: &str, position: CodeRange) -> Self {
        Self {
            message: format!("No module named '{module_name}'").into(),
            position,
            exc_type: ExcType::ModuleNotFoundError,
        }
    }

    /// Converts this compile error into a Python exception.
    ///
    /// Uses the stored exception type (SyntaxError or ModuleNotFoundError).
    /// Module errors have `hide_caret: true` since CPython doesn't show carets for these.
    pub fn into_python_exc(self, filename: &str, source: &str) -> MontyException {
        let mut frame = StackFrame::from_position(self.position, filename, source);
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
fn operator_to_inplace_opcode(op: &Operator) -> Opcode {
    match op {
        Operator::Add => Opcode::InplaceAdd,
        Operator::Sub => Opcode::InplaceSub,
        Operator::Mult => Opcode::InplaceMul,
        Operator::Div => Opcode::InplaceDiv,
        Operator::FloorDiv => Opcode::InplaceFloorDiv,
        Operator::Mod => Opcode::InplaceMod,
        Operator::Pow => Opcode::InplacePow,
        Operator::BitAnd => Opcode::InplaceAnd,
        Operator::BitOr => Opcode::InplaceOr,
        Operator::BitXor => Opcode::InplaceXor,
        Operator::LShift => Opcode::InplaceLShift,
        Operator::RShift => Opcode::InplaceRShift,
        Operator::MatMult => todo!("InplaceMatMul not yet defined"),
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
