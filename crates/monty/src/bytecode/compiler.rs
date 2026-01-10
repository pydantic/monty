//! Bytecode compiler for transforming AST to bytecode.
//!
//! The compiler traverses the prepared AST (`Node` and `Expr` types from `expressions.rs`)
//! and emits bytecode instructions using `CodeBuilder`. It handles variable scoping,
//! control flow, and expression evaluation order following Python semantics.

use super::{
    builder::{CodeBuilder, JumpLabel},
    code::Code,
    op::Opcode,
};
use crate::{
    args::ArgExprs,
    builtins::Builtins,
    callable::Callable,
    expressions::{Expr, ExprLoc, Identifier, Literal, NameScope, Node},
    intern::Interns,
    operators::{CmpOperator, Operator},
    value::Value,
};

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
}

/// Information about a loop for break/continue handling.
///
/// Note: break/continue are not yet implemented in the parser,
/// so this is currently unused but included for future use.
struct LoopInfo {
    /// Bytecode offset of loop start (for continue).
    start: usize,
    /// Jump labels that need patching to loop end (for break).
    break_jumps: Vec<JumpLabel>,
}

impl<'a> Compiler<'a> {
    /// Creates a new compiler with access to the string interner.
    pub fn new(interns: &'a Interns) -> Self {
        Self {
            code: CodeBuilder::new(),
            interns,
            loop_stack: Vec::new(),
        }
    }

    /// Compiles module-level code (a sequence of statements).
    ///
    /// Returns a Code object for the module. The module implicitly returns
    /// the value of the last expression, or None if empty.
    pub fn compile_module(nodes: &[Node], interns: &Interns, num_locals: u16) -> Code {
        let mut compiler = Compiler::new(interns);
        compiler.compile_block(nodes);

        // Module returns None if no explicit return
        compiler.code.emit(Opcode::LoadNone);
        compiler.code.emit(Opcode::ReturnValue);

        compiler.code.build(num_locals)
    }

    /// Compiles a function body to bytecode.
    ///
    /// Used during eager compilation to compile each function definition.
    /// The function body is compiled to bytecode with an implicit `return None`
    /// at the end if there's no explicit return statement.
    pub fn compile_function(body: &[Node], interns: &Interns, num_locals: u16) -> Code {
        let mut compiler = Compiler::new(interns);
        compiler.compile_block(body);

        // Implicit return None if no explicit return
        compiler.code.emit(Opcode::LoadNone);
        compiler.code.emit(Opcode::ReturnValue);

        compiler.code.build(num_locals)
    }

    /// Compiles a block of statements.
    fn compile_block(&mut self, nodes: &[Node]) {
        for node in nodes {
            self.compile_stmt(node);
        }
    }

    // ========================================================================
    // Statement Compilation
    // ========================================================================

    /// Compiles a single statement.
    fn compile_stmt(&mut self, node: &Node) {
        match node {
            Node::Expr(expr) => {
                self.compile_expr(expr);
                self.code.emit(Opcode::Pop); // Discard result
            }

            Node::Return(expr) => {
                self.compile_expr(expr);
                self.code.emit(Opcode::ReturnValue);
            }

            Node::ReturnNone => {
                self.code.emit(Opcode::LoadNone);
                self.code.emit(Opcode::ReturnValue);
            }

            Node::Assign { target, object } => {
                self.compile_expr(object);
                self.compile_store(target);
            }

            Node::OpAssign { target, op, object } => {
                self.compile_name(target);
                self.compile_expr(object);
                self.code.emit(operator_to_inplace_opcode(op));
                self.compile_store(target);
            }

            Node::SubscriptAssign { target, index, value } => {
                // Stack order for StoreSubscr: value, obj, index
                self.compile_expr(value);
                self.compile_name(target);
                self.compile_expr(index);
                self.code.emit(Opcode::StoreSubscr);
            }

            Node::AttrAssign {
                object, attr, value, ..
            } => {
                // Stack order for StoreAttr: value, obj
                self.compile_expr(value);
                self.compile_expr(object);
                let name_id = attr.string_id().expect("StoreAttr requires interned attr name");
                self.code.emit_u16(Opcode::StoreAttr, name_id.index() as u16);
            }

            Node::If { test, body, or_else } => {
                self.compile_if(test, body, or_else);
            }

            Node::For {
                target,
                iter,
                body,
                or_else,
            } => {
                self.compile_for(target, iter, body, or_else);
            }

            Node::Assert { test, msg } => {
                self.compile_assert(test, msg.as_ref());
            }

            Node::Raise(expr) => {
                if let Some(exc) = expr {
                    self.compile_expr(exc);
                    self.code.emit(Opcode::Raise);
                } else {
                    self.code.emit(Opcode::Reraise);
                }
            }

            Node::FunctionDef(func_id) => {
                let func = self.interns.get_function(*func_id);

                // 1. Compile and push default values (evaluated at definition time)
                for default_expr in &func.default_exprs {
                    self.compile_expr(default_expr);
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

            Node::Try(_) => {
                todo!("Try/except compilation (Step 5)")
            }
        }
    }

    // ========================================================================
    // Expression Compilation
    // ========================================================================

    /// Compiles an expression, leaving its value on the stack.
    fn compile_expr(&mut self, expr_loc: &ExprLoc) {
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
                self.compile_binary_op(left, op, right);
            }

            Expr::CmpOp { left, op, right } => {
                self.compile_expr(left);
                self.compile_expr(right);
                self.code.emit(cmp_operator_to_opcode(op));
            }

            Expr::Not(operand) => {
                self.compile_expr(operand);
                self.code.emit(Opcode::UnaryNot);
            }

            Expr::UnaryMinus(operand) => {
                self.compile_expr(operand);
                self.code.emit(Opcode::UnaryNeg);
            }

            Expr::List(elements) => {
                for elem in elements {
                    self.compile_expr(elem);
                }
                self.code.emit_u16(Opcode::BuildList, elements.len() as u16);
            }

            Expr::Tuple(elements) => {
                for elem in elements {
                    self.compile_expr(elem);
                }
                self.code.emit_u16(Opcode::BuildTuple, elements.len() as u16);
            }

            Expr::Dict(pairs) => {
                for (key, value) in pairs {
                    self.compile_expr(key);
                    self.compile_expr(value);
                }
                self.code.emit_u16(Opcode::BuildDict, pairs.len() as u16);
            }

            Expr::Set(elements) => {
                for elem in elements {
                    self.compile_expr(elem);
                }
                self.code.emit_u16(Opcode::BuildSet, elements.len() as u16);
            }

            Expr::Subscript { object, index } => {
                self.compile_expr(object);
                self.compile_expr(index);
                self.code.emit(Opcode::BinarySubscr);
            }

            Expr::IfElse { test, body, orelse } => {
                self.compile_if_else_expr(test, body, orelse);
            }

            Expr::AttrGet { object, attr } => {
                self.compile_expr(object);
                let name_id = attr.string_id().expect("LoadAttr requires interned attr name");
                self.code.emit_u16(Opcode::LoadAttr, name_id.index() as u16);
            }

            Expr::Call { callable, args } => {
                self.compile_call(callable, args);
            }

            Expr::AttrCall { .. } => {
                todo!("AttrCall compilation (Step 4)")
            }

            Expr::FString(parts) => {
                // Compile each part and build the f-string
                let part_count = self.compile_fstring_parts(parts);
                self.code.emit_u16(Opcode::BuildFString, part_count);
            }
        }
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
                self.code.emit_load_local(slot);
            }
            NameScope::Global => {
                self.code.emit_u16(Opcode::LoadGlobal, slot);
            }
            NameScope::Cell => {
                self.code.emit_u16(Opcode::LoadCell, slot);
            }
        }
    }

    /// Compiles storing the top of stack to a variable.
    fn compile_store(&mut self, target: &Identifier) {
        let slot = target.namespace_id().index() as u16;
        match target.scope {
            NameScope::Local => {
                self.code.emit_store_local(slot);
            }
            NameScope::Global => {
                self.code.emit_u16(Opcode::StoreGlobal, slot);
            }
            NameScope::Cell => {
                self.code.emit_u16(Opcode::StoreCell, slot);
            }
        }
    }

    // ========================================================================
    // Binary Operator Compilation
    // ========================================================================

    /// Compiles a binary operation.
    fn compile_binary_op(&mut self, left: &ExprLoc, op: &Operator, right: &ExprLoc) {
        match op {
            // Short-circuit AND: evaluate left, jump if falsy
            Operator::And => {
                self.compile_expr(left);
                let end_jump = self.code.emit_jump(Opcode::JumpIfFalseOrPop);
                self.compile_expr(right);
                self.code.patch_jump(end_jump);
            }

            // Short-circuit OR: evaluate left, jump if truthy
            Operator::Or => {
                self.compile_expr(left);
                let end_jump = self.code.emit_jump(Opcode::JumpIfTrueOrPop);
                self.compile_expr(right);
                self.code.patch_jump(end_jump);
            }

            // Regular binary operators
            _ => {
                self.compile_expr(left);
                self.compile_expr(right);
                self.code.emit(operator_to_opcode(op));
            }
        }
    }

    // ========================================================================
    // Control Flow Compilation
    // ========================================================================

    /// Compiles an if/else statement.
    fn compile_if(&mut self, test: &ExprLoc, body: &[Node], or_else: &[Node]) {
        self.compile_expr(test);

        if or_else.is_empty() {
            // Simple if without else
            let end_jump = self.code.emit_jump(Opcode::JumpIfFalse);
            self.compile_block(body);
            self.code.patch_jump(end_jump);
        } else {
            // If with else
            let else_jump = self.code.emit_jump(Opcode::JumpIfFalse);
            self.compile_block(body);
            let end_jump = self.code.emit_jump(Opcode::Jump);
            self.code.patch_jump(else_jump);
            self.compile_block(or_else);
            self.code.patch_jump(end_jump);
        }
    }

    /// Compiles a ternary conditional expression.
    fn compile_if_else_expr(&mut self, test: &ExprLoc, body: &ExprLoc, orelse: &ExprLoc) {
        self.compile_expr(test);
        let else_jump = self.code.emit_jump(Opcode::JumpIfFalse);
        self.compile_expr(body);
        let end_jump = self.code.emit_jump(Opcode::Jump);
        self.code.patch_jump(else_jump);
        self.compile_expr(orelse);
        self.code.patch_jump(end_jump);
    }

    /// Compiles a function call expression.
    ///
    /// Pushes the callable onto the stack, then all arguments, then emits CallFunction.
    fn compile_call(&mut self, callable: &Callable, args: &ArgExprs) {
        // Push the callable
        match callable {
            Callable::Builtin(builtin) => {
                let idx = self.code.add_const(Value::Builtin(*builtin));
                self.code.emit_u16(Opcode::LoadConst, idx);
            }
            Callable::Name(ident) => {
                self.compile_name(ident);
            }
        }

        // Compile arguments and emit the call
        match args {
            ArgExprs::Empty => {
                self.code.emit_u8(Opcode::CallFunction, 0);
            }
            ArgExprs::One(arg) => {
                self.compile_expr(arg);
                self.code.emit_u8(Opcode::CallFunction, 1);
            }
            ArgExprs::Two(arg1, arg2) => {
                self.compile_expr(arg1);
                self.compile_expr(arg2);
                self.code.emit_u8(Opcode::CallFunction, 2);
            }
            ArgExprs::Args(args) => {
                for arg in args {
                    self.compile_expr(arg);
                }
                // CallFunction takes u8 for arg count (max 255 positional args)
                let arg_count = args.len().min(255) as u8;
                self.code.emit_u8(Opcode::CallFunction, arg_count);
            }
            ArgExprs::Kwargs(_) | ArgExprs::ArgsKargs { .. } => {
                // Keyword arguments require CallFunctionKw opcode
                todo!("Keyword argument calls (CallFunctionKw) not yet implemented")
            }
        }
    }

    /// Compiles a for loop.
    fn compile_for(&mut self, target: &Identifier, iter: &ExprLoc, body: &[Node], or_else: &[Node]) {
        // Compile iterator expression
        self.compile_expr(iter);
        // Convert to iterator
        self.code.emit(Opcode::GetIter);

        // Loop start
        let loop_start = self.code.current_offset();

        // Push loop info for break/continue (future use)
        self.loop_stack.push(LoopInfo {
            start: loop_start,
            break_jumps: Vec::new(),
        });

        // ForIter: advance iterator or jump to end
        let end_jump = self.code.emit_jump(Opcode::ForIter);

        // Store current value to target
        self.compile_store(target);

        // Compile body
        self.compile_block(body);

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
            self.compile_block(or_else);
        }
    }

    // ========================================================================
    // Statement Helpers
    // ========================================================================

    /// Compiles an assert statement.
    fn compile_assert(&mut self, test: &ExprLoc, msg: Option<&ExprLoc>) {
        // Compile test
        self.compile_expr(test);
        // Jump over raise if truthy
        let skip_jump = self.code.emit_jump(Opcode::JumpIfTrue);

        // Raise AssertionError
        let exc_idx = self.code.add_const(Value::Builtin(Builtins::ExcType(
            crate::exception_private::ExcType::AssertionError,
        )));
        self.code.emit_u16(Opcode::LoadConst, exc_idx);

        if let Some(msg_expr) = msg {
            // Call AssertionError(msg)
            self.compile_expr(msg_expr);
            self.code.emit_u8(Opcode::CallFunction, 1);
        } else {
            // Call AssertionError()
            self.code.emit_u8(Opcode::CallFunction, 0);
        }

        self.code.emit(Opcode::Raise);
        self.code.patch_jump(skip_jump);
    }

    /// Compiles f-string parts, returning the number of parts.
    ///
    /// Note: F-string literal parts contain raw `String`s rather than `StringId`s,
    /// so full f-string support requires either interning during compilation or
    /// a different representation. Deferred for now.
    fn compile_fstring_parts(&mut self, _parts: &[crate::fstring::FStringPart]) -> u16 {
        // F-string literals are stored as raw Strings (not StringIds) in FStringPart::Literal.
        // Since we can't intern new strings during compilation (Interns is read-only),
        // full f-string support requires changes to how f-strings are parsed.
        // For now, defer to runtime (or panic at compile time for f-strings).
        todo!("F-string compilation requires changes to f-string representation")
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
        CmpOperator::ModEq(_) => todo!("ModEq requires special handling"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intern::InternerBuilder;

    /// Creates an empty Interns for testing.
    fn test_interns() -> Interns {
        let builder = InternerBuilder::new();
        Interns::new(builder, Vec::new(), Vec::new())
    }

    // Basic smoke test - more comprehensive tests will come with the VM
    #[test]
    fn test_compiler_creates_code() {
        let interns = test_interns();
        let code = Compiler::compile_module(&[], &interns, 0);
        // Empty module should have LoadNone + ReturnValue
        assert_eq!(code.bytecode().len(), 2);
        assert_eq!(code.bytecode()[0], Opcode::LoadNone as u8);
        assert_eq!(code.bytecode()[1], Opcode::ReturnValue as u8);
    }
}
