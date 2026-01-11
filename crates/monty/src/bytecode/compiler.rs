//! Bytecode compiler for transforming AST to bytecode.
//!
//! The compiler traverses the prepared AST (`Node` and `Expr` types from `expressions.rs`)
//! and emits bytecode instructions using `CodeBuilder`. It handles variable scoping,
//! control flow, and expression evaluation order following Python semantics.

use std::borrow::Cow;

use super::{
    builder::{CodeBuilder, JumpLabel},
    code::{Code, ExceptionEntry},
    op::Opcode,
};
use crate::{
    args::{ArgExprs, Kwarg},
    builtins::Builtins,
    callable::Callable,
    exception_private::ExcType,
    exception_public::{MontyException, StackFrame},
    expressions::{Expr, ExprLoc, Identifier, Literal, NameScope, Node},
    fstring::{encode_format_spec, ConversionFlag, FStringPart, FormatSpec},
    intern::Interns,
    operators::{CmpOperator, Operator},
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
pub struct Compiler<'a> {
    /// Current code being built.
    code: CodeBuilder,

    /// Reference to interns for string/function lookups.
    interns: &'a Interns,

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

impl<'a> Compiler<'a> {
    /// Creates a new compiler with access to the string interner.
    fn new(interns: &'a Interns) -> Self {
        Self {
            code: CodeBuilder::new(),
            interns,
            loop_stack: Vec::new(),
            cell_base: 0,
            finally_targets: Vec::new(),
        }
    }

    /// Creates a new compiler with a specific cell base offset.
    fn new_with_cell_base(interns: &'a Interns, cell_base: u16) -> Self {
        Self {
            code: CodeBuilder::new(),
            interns,
            loop_stack: Vec::new(),
            cell_base,
            finally_targets: Vec::new(),
        }
    }

    /// Compiles module-level code (a sequence of statements).
    ///
    /// Returns a Code object for the module, or a compile error if limits
    /// were exceeded. The module implicitly returns the value of the last
    /// expression, or None if empty.
    pub fn compile_module(nodes: &[Node], interns: &Interns, num_locals: u16) -> Result<Code, CompileError> {
        let mut compiler = Compiler::new(interns);
        compiler.compile_block(nodes)?;

        // Module returns None if no explicit return
        compiler.code.emit(Opcode::LoadNone);
        compiler.code.emit(Opcode::ReturnValue);

        Ok(compiler.code.build(num_locals))
    }

    /// Compiles a function body to bytecode.
    ///
    /// Used during eager compilation to compile each function definition.
    /// The function body is compiled to bytecode with an implicit `return None`
    /// at the end if there's no explicit return statement.
    ///
    /// The `cell_base` parameter is the number of parameter slots, used to convert
    /// cell variable namespace slots to cells array indices.
    pub fn compile_function(
        body: &[Node],
        interns: &Interns,
        num_locals: u16,
        cell_base: u16,
    ) -> Result<Code, CompileError> {
        let mut compiler = Compiler::new_with_cell_base(interns, cell_base);
        compiler.compile_block(body)?;

        // Implicit return None if no explicit return
        compiler.code.emit(Opcode::LoadNone);
        compiler.code.emit(Opcode::ReturnValue);

        Ok(compiler.code.build(num_locals))
    }

    /// Compiles a block of statements.
    fn compile_block(&mut self, nodes: &[Node]) -> Result<(), CompileError> {
        for node in nodes {
            self.compile_stmt(node)?;
        }
        Ok(())
    }

    // ========================================================================
    // Statement Compilation
    // ========================================================================

    /// Compiles a single statement.
    fn compile_stmt(&mut self, node: &Node) -> Result<(), CompileError> {
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

            Node::OpAssign { target, op, object } => {
                self.compile_name(target);
                self.compile_expr(object)?;
                self.code.emit(operator_to_inplace_opcode(op));
                self.compile_store(target);
            }

            Node::SubscriptAssign { target, index, value } => {
                // Stack order for StoreSubscr: value, obj, index
                self.compile_expr(value)?;
                self.compile_name(target);
                self.compile_expr(index)?;
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
                self.code.emit_u16(Opcode::StoreAttr, name_id.index() as u16);
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

            Node::FunctionDef(func_id) => {
                let func = self.interns.get_function(*func_id);
                let func_pos = func.name.position;

                // Check bytecode operand limits
                if func.default_exprs.len() > MAX_CALL_ARGS {
                    return Err(CompileError::new(
                        format!("more than {MAX_CALL_ARGS} default parameter values"),
                        func_pos,
                    ));
                }
                if func.free_var_enclosing_slots.len() > MAX_CALL_ARGS {
                    return Err(CompileError::new(
                        format!("more than {MAX_CALL_ARGS} closure variables"),
                        func_pos,
                    ));
                }

                // 1. Compile and push default values (evaluated at definition time)
                for default_expr in &func.default_exprs {
                    self.compile_expr(default_expr)?;
                }
                let defaults_count = func.default_exprs.len() as u8;

                // 2. Emit MakeFunction or MakeClosure (if has free vars)
                if func.free_var_enclosing_slots.is_empty() {
                    // MakeFunction: func_id (u16) + defaults_count (u8)
                    self.code
                        .emit_u16_u8(Opcode::MakeFunction, func_id.index() as u16, defaults_count);
                } else {
                    // Push captured cells from enclosing scope
                    for &slot in &func.free_var_enclosing_slots {
                        // Load the cell reference from the enclosing namespace
                        self.code.emit_load_local(slot.index() as u16);
                    }
                    let cell_count = func.free_var_enclosing_slots.len() as u8;
                    // MakeClosure: func_id (u16) + defaults_count (u8) + cell_count (u8)
                    self.code
                        .emit_u16_u8_u8(Opcode::MakeClosure, func_id.index() as u16, defaults_count, cell_count);
                }

                // 3. Store the function object to its name slot
                self.compile_store(&func.name);
            }

            Node::Try(try_block) => {
                self.compile_try(try_block)?;
            }
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

            Expr::List(elements) => {
                for elem in elements {
                    self.compile_expr(elem)?;
                }
                self.code.emit_u16(Opcode::BuildList, elements.len() as u16);
            }

            Expr::Tuple(elements) => {
                for elem in elements {
                    self.compile_expr(elem)?;
                }
                self.code.emit_u16(Opcode::BuildTuple, elements.len() as u16);
            }

            Expr::Dict(pairs) => {
                for (key, value) in pairs {
                    self.compile_expr(key)?;
                    self.compile_expr(value)?;
                }
                self.code.emit_u16(Opcode::BuildDict, pairs.len() as u16);
            }

            Expr::Set(elements) => {
                for elem in elements {
                    self.compile_expr(elem)?;
                }
                self.code.emit_u16(Opcode::BuildSet, elements.len() as u16);
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
                self.code.emit_u16(Opcode::LoadAttr, name_id.index() as u16);
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

            Expr::FString(parts) => {
                // Compile each part and build the f-string
                let part_count = self.compile_fstring_parts(parts)?;
                self.code.emit_u16(Opcode::BuildFString, part_count);
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
        let slot = ident.namespace_id().index() as u16;
        match ident.scope {
            NameScope::Local => {
                // Register the name for NameError messages
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
        let slot = target.namespace_id().index() as u16;
        match target.scope {
            NameScope::Local => {
                // Register the name for NameError messages
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
    fn compile_if(&mut self, test: &ExprLoc, body: &[Node], or_else: &[Node]) -> Result<(), CompileError> {
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
    /// Pushes the callable onto the stack, then all arguments, then emits CallFunction.
    /// The `call_pos` is the position of the full call expression for proper traceback caret.
    fn compile_call(&mut self, callable: &Callable, args: &ArgExprs, call_pos: CodeRange) -> Result<(), CompileError> {
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
                let arg_count = args.len() as u8;
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
                    kwname_ids.push(kwarg.key.name_id.index() as u16);
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
                            kwname_ids.push(kwarg.key.name_id.index() as u16);
                        }
                    }

                    self.code.set_location(call_pos, None);
                    self.code.emit_call_function_kw(pos_count as u8, &kwname_ids);
                }
            }
        }
        Ok(())
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
            Callable::Name(ident) => ident.name_id.index() as u16,
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
        self.code.emit_u16(Opcode::BuildList, pos_count as u16);

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
            self.code.emit_u16(Opcode::BuildDict, kw_count as u16);

            // Merge **kwargs if present
            if let Some(var_kwargs_expr) = var_kwargs {
                self.compile_expr(var_kwargs_expr)?;
                self.code.emit_u16(Opcode::DictMerge, func_name_id);
            }
        }

        // 3. Call the function
        self.code.set_location(call_pos, None);
        let flags = u8::from(has_kwargs);
        self.code.emit_u8(Opcode::CallFunctionEx, flags);
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
                self.code.emit_u16_u8(Opcode::CallMethod, name_id.index() as u16, 0);
            }
            ArgExprs::One(arg) => {
                self.compile_expr(arg)?;
                self.code.emit_u16_u8(Opcode::CallMethod, name_id.index() as u16, 1);
            }
            ArgExprs::Two(arg1, arg2) => {
                self.compile_expr(arg1)?;
                self.compile_expr(arg2)?;
                self.code.emit_u16_u8(Opcode::CallMethod, name_id.index() as u16, 2);
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
                let arg_count = args.len() as u8;
                self.code
                    .emit_u16_u8(Opcode::CallMethod, name_id.index() as u16, arg_count);
            }
            ArgExprs::Kwargs(_) | ArgExprs::ArgsKargs { .. } => {
                // TODO: Need CallMethodKw for keyword arguments
                todo!("Method calls with keyword arguments not yet implemented")
            }
        }
        Ok(())
    }

    /// Compiles a for loop.
    fn compile_for(
        &mut self,
        target: &Identifier,
        iter: &ExprLoc,
        body: &[Node],
        or_else: &[Node],
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

        // Store current value to target
        self.compile_store(target);

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
    // Statement Helpers
    // ========================================================================

    /// Compiles an assert statement.
    fn compile_assert(&mut self, test: &ExprLoc, msg: Option<&ExprLoc>) -> Result<(), CompileError> {
        // Compile test
        self.compile_expr(test)?;
        // Jump over raise if truthy
        let skip_jump = self.code.emit_jump(Opcode::JumpIfTrue);

        // Raise AssertionError
        let exc_idx = self.code.add_const(Value::Builtin(Builtins::ExcType(
            crate::exception_private::ExcType::AssertionError,
        )));
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
    fn add_format_spec_const(&mut self, spec: &crate::fstring::ParsedFormatSpec) -> u16 {
        let encoded = encode_format_spec(spec);
        // Use negative to distinguish from regular ints (format spec marker)
        // We negate and subtract 1 to ensure it's negative and recoverable
        let marker = -((encoded as i64) + 1);
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
    fn compile_try(&mut self, try_block: &Try<Node>) -> Result<(), CompileError> {
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
                try_start as u32,
                try_end as u32 + 3, // +3 to include the JUMP instruction
                handler_start as u32,
                stack_depth,
            ));
        }

        // Entry 2: Handler dispatch -> finally cleanup (only if has_finally)
        // This ensures finally runs when RERAISE is executed or any exception occurs in handlers
        if let Some(cleanup_start) = finally_cleanup_start {
            self.code.add_exception_entry(ExceptionEntry::new(
                handler_start as u32,
                handler_dispatch_end as u32,
                cleanup_start as u32,
                stack_depth,
            ));
        }

        // Entry 3: Finally with return -> finally cleanup
        // If an exception occurs while running finally (in the return path), catch it
        if let (Some(return_start), Some(cleanup_start)) = (finally_with_return_start, finally_cleanup_start) {
            self.code.add_exception_entry(ExceptionEntry::new(
                return_start as u32,
                else_start as u32, // End at else_start (before else block)
                cleanup_start as u32,
                stack_depth,
            ));
        }

        // Entry 4: Else block -> finally cleanup (only if has_finally and has_else)
        // Exceptions in else block should go through finally
        if has_else {
            if let Some(cleanup_start) = finally_cleanup_start {
                self.code.add_exception_entry(ExceptionEntry::new(
                    else_start as u32,
                    else_end as u32,
                    cleanup_start as u32,
                    stack_depth,
                ));
            }
        }

        Ok(())
    }

    /// Compiles the exception handlers for a try block.
    ///
    /// Each handler checks if the exception matches its type, and if so,
    /// executes the handler body. If no handler matches, the exception is re-raised.
    fn compile_exception_handlers(
        &mut self,
        handlers: &[ExceptHandler<Node>],
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
        let slot = target.namespace_id().index() as u16;
        match target.scope {
            NameScope::Local => {
                if slot <= 255 {
                    self.code.emit_u8(Opcode::DeleteLocal, slot as u8);
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
/// format (e.g., too many arguments, too many local variables).
#[derive(Debug, Clone)]
pub struct CompileError {
    /// Error message describing what limit was exceeded.
    message: Cow<'static, str>,
    /// Source location where the error occurred.
    position: CodeRange,
}

impl CompileError {
    /// Creates a new compile error with the given message and position.
    fn new(message: impl Into<Cow<'static, str>>, position: CodeRange) -> Self {
        Self {
            message: message.into(),
            position,
        }
    }

    /// Converts this compile error into a Python SyntaxError exception.
    pub fn into_python_exc(self, filename: &str, source: &str) -> MontyException {
        MontyException::new_full(
            ExcType::SyntaxError,
            Some(self.message.into_owned()),
            vec![StackFrame::from_position(self.position, filename, source)],
        )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intern::InternerBuilder;

    /// Creates an empty Interns for testing.
    fn test_interns() -> Interns {
        let builder = InternerBuilder::default();
        Interns::new(builder, Vec::new(), Vec::new())
    }

    // Basic smoke test - more comprehensive tests will come with the VM
    #[test]
    fn test_compiler_creates_code() {
        let interns = test_interns();
        let code = Compiler::compile_module(&[], &interns, 0).unwrap();
        // Empty module should have LoadNone + ReturnValue
        assert_eq!(code.bytecode().len(), 2);
        assert_eq!(code.bytecode()[0], Opcode::LoadNone as u8);
        assert_eq!(code.bytecode()[1], Opcode::ReturnValue as u8);
    }
}
