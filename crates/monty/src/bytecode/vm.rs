//! Bytecode virtual machine for executing compiled Python code.
//!
//! The VM uses a stack-based execution model with an operand stack for computation
//! and a call stack for function frames. Each frame owns its instruction pointer (IP).

use crate::{
    args::ArgValues,
    bytecode::{code::Code, op::Opcode},
    exception_private::{ExcType, ExceptionRaise, RunError, SimpleException},
    for_iterator::ForIterator,
    heap::{Heap, HeapData, HeapId},
    intern::{ExtFunctionId, FunctionId, Interns, StringId},
    io::PrintWriter,
    namespace::{NamespaceId, Namespaces, GLOBAL_NS_IDX},
    parse::CodeRange,
    resource::ResourceTracker,
    types::{Dict, List, PyTrait, Set, Tuple},
    value::{Attr, Value},
};

// ============================================================================
// VM Result Types
// ============================================================================

/// Result of calling a function.
///
/// Distinguishes between builtin function calls (which return a value immediately),
/// user function calls (which push a frame and continue execution), and external
/// function calls (which pause the VM).
enum CallResult {
    /// Builtin function returned a value - push it onto the stack.
    Builtin(Value),
    /// User function call - frame was pushed, continue execution in VM loop.
    /// The return value will be pushed by ReturnValue opcode.
    UserFunction,
    /// External function call - VM should pause and return to caller.
    /// Contains (ext_function_id, args).
    ExternalCall(ExtFunctionId, Vec<Value>),
}

/// Bitwise operation type for binary_bitwise helper.
enum BitwiseOp {
    And,
    Or,
    Xor,
    LShift,
    RShift,
}

/// Result of VM execution.
pub enum VMSuccess {
    /// Execution completed successfully with a return value.
    Complete(Value),

    /// Execution paused for an external function call.
    ///
    /// The caller should execute the external function and call `resume()`
    /// with the result.
    ExternalCall {
        /// ID of the external function to call.
        ext_function_id: ExtFunctionId,
        /// Arguments for the external function.
        args: Vec<Value>,
    },
}

// ============================================================================
// Call Frame
// ============================================================================

/// A single function activation record.
///
/// Each frame represents one level in the call stack and owns its own
/// instruction pointer. This design avoids sync bugs on call/return.
pub struct CallFrame<'code> {
    /// Bytecode being executed.
    code: &'code Code,

    /// Instruction pointer within this frame's bytecode.
    ip: usize,

    /// Base index into operand stack for this frame.
    ///
    /// Used to identify where this frame's stack region begins.
    stack_base: usize,

    /// Namespace index for this frame's locals.
    namespace_idx: NamespaceId,

    /// Function ID (for tracebacks). None for module-level code.
    function_id: Option<FunctionId>,

    /// Captured cells for closures.
    cells: Vec<HeapId>,

    /// Call site position (for tracebacks).
    call_position: Option<CodeRange>,
}

impl<'code> CallFrame<'code> {
    /// Creates a new call frame for module-level code.
    pub fn new_module(code: &'code Code, namespace_idx: NamespaceId) -> Self {
        Self {
            code,
            ip: 0,
            stack_base: 0,
            namespace_idx,
            function_id: None,
            cells: Vec::new(),
            call_position: None,
        }
    }

    /// Creates a new call frame for a function call.
    pub fn new_function(
        code: &'code Code,
        stack_base: usize,
        namespace_idx: NamespaceId,
        function_id: FunctionId,
        cells: Vec<HeapId>,
        call_position: CodeRange,
    ) -> Self {
        Self {
            code,
            ip: 0,
            stack_base,
            namespace_idx,
            function_id: Some(function_id),
            cells,
            call_position: Some(call_position),
        }
    }
}

// ============================================================================
// VM Snapshot for Pause/Resume
// ============================================================================

/// Serializable representation of a call frame.
///
/// Cannot store `&Code` (a reference) - instead stores `FunctionId` to look up
/// the pre-compiled Code object on resume. Module-level code uses `None`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SerializedFrame {
    /// Which function's code this frame executes (None = module-level).
    function_id: Option<FunctionId>,

    /// Instruction pointer within this frame's bytecode.
    ip: usize,

    /// Base index into operand stack for this frame's locals.
    stack_base: usize,

    /// Namespace index for this frame's locals.
    namespace_idx: NamespaceId,

    /// Captured cells for closures (HeapIds remain valid after heap deserialization).
    cells: Vec<HeapId>,

    /// Call site position (for tracebacks).
    call_position: Option<CodeRange>,
}

impl CallFrame<'_> {
    /// Converts this frame to a serializable representation.
    fn serialize(&self) -> SerializedFrame {
        SerializedFrame {
            function_id: self.function_id,
            ip: self.ip,
            stack_base: self.stack_base,
            namespace_idx: self.namespace_idx,
            cells: self.cells.clone(),
            call_position: self.call_position,
        }
    }
}

/// VM state for pause/resume at external function calls.
///
/// **Ownership:** This struct OWNS the values (refcounts were already incremented).
/// Must be used with the serialized Heap - HeapId values are indices into that heap.
///
/// **Usage:** When the VM pauses for an external call, call `into_snapshot()` to
/// create this snapshot. The snapshot can be serialized and stored. On resume,
/// use `restore()` to reconstruct the VM and continue execution.
///
/// Note: This struct does not implement `Clone` because `Value` uses manual
/// reference counting. Snapshots transfer ownership - they are not copied.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct VMSnapshot {
    /// Operand stack (may contain Value::Ref(HeapId) pointing to heap).
    stack: Vec<Value>,

    /// Call frames (serializable form - stores FunctionId, not &Code).
    frames: Vec<SerializedFrame>,

    /// Current exception being handled (if any).
    current_exception: Option<Value>,

    /// IP of the instruction that caused the pause (for exception handling).
    instruction_ip: usize,
}

// ============================================================================
// Virtual Machine
// ============================================================================

/// The bytecode virtual machine.
///
/// Executes compiled bytecode using a stack-based execution model.
/// The instruction pointer (IP) lives in each `CallFrame`, not here,
/// to avoid sync bugs on call/return.
pub struct VM<'a, T: ResourceTracker, P: PrintWriter> {
    /// Operand stack - values being computed.
    stack: Vec<Value>,

    /// Call stack - function frames (each frame has its own IP).
    frames: Vec<CallFrame<'a>>,

    /// Heap for reference-counted objects.
    heap: &'a mut Heap<T>,

    /// Namespace stack for variable storage.
    namespaces: &'a mut Namespaces,

    /// Interned strings/bytes.
    interns: &'a Interns,

    /// Print output writer.
    print_writer: &'a mut P,

    /// Current exception being handled (if any).
    ///
    /// Used by bare `raise` to re-raise the current exception.
    /// Set when entering an except handler, cleared when exiting.
    current_exception: Option<Value>,

    /// IP of the instruction being executed (for exception table lookup).
    ///
    /// Updated at the start of each instruction before operands are fetched.
    /// This allows us to find the correct exception handler when an error occurs.
    instruction_ip: usize,
}

impl<'a, T: ResourceTracker, P: PrintWriter> VM<'a, T, P> {
    /// Creates a new VM with the given runtime context.
    pub fn new(
        heap: &'a mut Heap<T>,
        namespaces: &'a mut Namespaces,
        interns: &'a Interns,
        print_writer: &'a mut P,
    ) -> Self {
        Self {
            stack: Vec::with_capacity(64),
            frames: Vec::with_capacity(16),
            heap,
            namespaces,
            interns,
            print_writer,
            current_exception: None,
            instruction_ip: 0,
        }
    }

    /// Pushes an initial frame for module-level code and runs the VM.
    pub fn run_module(&mut self, code: &'a Code) -> Result<VMSuccess, RunError> {
        self.frames.push(CallFrame::new_module(code, GLOBAL_NS_IDX));
        self.run()
    }

    /// Cleans up VM state before the VM is dropped.
    ///
    /// This method must be called before the VM goes out of scope to ensure
    /// proper reference counting cleanup for any exception values.
    pub fn cleanup(&mut self) {
        // Drop current_exception if present
        if let Some(exc) = self.current_exception.take() {
            exc.drop_with_heap(self.heap);
        }
        // Stack should be empty, but clean up just in case
        for value in self.stack.drain(..) {
            value.drop_with_heap(self.heap);
        }
    }

    /// Main execution loop.
    ///
    /// Fetches opcodes from the current frame's bytecode and executes them.
    /// Returns when execution completes, an error occurs, or an external
    /// call is needed.
    pub fn run(&mut self) -> Result<VMSuccess, RunError> {
        loop {
            // Track instruction IP for exception table lookup
            self.instruction_ip = self.current_frame().ip;
            let opcode = self.fetch_opcode();

            match opcode {
                // ============================================================
                // Stack Operations
                // ============================================================
                Opcode::Pop => {
                    let value = self.pop();
                    value.drop_with_heap(self.heap);
                }
                Opcode::Dup => {
                    // Copy without incrementing refcount first (avoids borrow conflict)
                    let value = self.peek().copy_for_extend();
                    // Now we can safely increment refcount and push
                    if let Value::Ref(id) = &value {
                        self.heap.inc_ref(*id);
                    }
                    self.push(value);
                }
                Opcode::Rot2 => {
                    // Swap top two: [a, b] → [b, a]
                    let len = self.stack.len();
                    self.stack.swap(len - 1, len - 2);
                }
                Opcode::Rot3 => {
                    // Rotate top three: [a, b, c] → [c, a, b]
                    // Uses in-place rotation without cloning
                    let len = self.stack.len();
                    // Move c out, then shift a→b→c, then put c at a's position
                    // Equivalent to: [..rest, a, b, c] → [..rest, c, a, b]
                    self.stack[len - 3..].rotate_right(1);
                }
                // Constants & Literals
                Opcode::LoadConst => {
                    let idx = self.fetch_u16();
                    // Copy without incrementing refcount first (avoids borrow conflict)
                    let value = self.current_frame().code.constants().get(idx).copy_for_extend();
                    // Now we can safely increment refcount and push
                    if let Value::Ref(id) = &value {
                        self.heap.inc_ref(*id);
                    }
                    self.push(value);
                }
                Opcode::LoadNone => self.push(Value::None),
                Opcode::LoadTrue => self.push(Value::Bool(true)),
                Opcode::LoadFalse => self.push(Value::Bool(false)),
                Opcode::LoadSmallInt => {
                    let n = self.fetch_i8();
                    self.push(Value::Int(i64::from(n)));
                }
                // Variables - Specialized Local Loads (no operand)
                Opcode::LoadLocal0 => self.load_local(0),
                Opcode::LoadLocal1 => self.load_local(1),
                Opcode::LoadLocal2 => self.load_local(2),
                Opcode::LoadLocal3 => self.load_local(3),
                // Variables - General Local Operations
                Opcode::LoadLocal => {
                    let slot = u16::from(self.fetch_u8());
                    self.load_local(slot);
                }
                Opcode::LoadLocalW => {
                    let slot = self.fetch_u16();
                    self.load_local(slot);
                }
                Opcode::StoreLocal => {
                    let slot = u16::from(self.fetch_u8());
                    self.store_local(slot);
                }
                Opcode::StoreLocalW => {
                    let slot = self.fetch_u16();
                    self.store_local(slot);
                }
                Opcode::DeleteLocal => {
                    let slot = u16::from(self.fetch_u8());
                    self.delete_local(slot);
                }
                // Variables - Global Operations
                Opcode::LoadGlobal => {
                    let slot = self.fetch_u16();
                    self.load_global(slot);
                }
                Opcode::StoreGlobal => {
                    let slot = self.fetch_u16();
                    self.store_global(slot);
                }
                // Variables - Cell Operations (closures)
                Opcode::LoadCell => {
                    let slot = self.fetch_u16();
                    self.load_cell(slot);
                }
                Opcode::StoreCell => {
                    let slot = self.fetch_u16();
                    self.store_cell(slot);
                }
                // Binary Operations
                Opcode::BinaryAdd => self.binary_add()?,
                Opcode::BinarySub => self.binary_sub()?,
                Opcode::BinaryMul => self.binary_mult()?,
                Opcode::BinaryDiv => self.binary_div()?,
                Opcode::BinaryFloorDiv => self.binary_floordiv()?,
                Opcode::BinaryMod => self.binary_mod(),
                Opcode::BinaryPow => self.binary_pow()?,
                // Bitwise operations - only work on integers
                Opcode::BinaryAnd => self.binary_bitwise(BitwiseOp::And)?,
                Opcode::BinaryOr => self.binary_bitwise(BitwiseOp::Or)?,
                Opcode::BinaryXor => self.binary_bitwise(BitwiseOp::Xor)?,
                Opcode::BinaryLShift => self.binary_bitwise(BitwiseOp::LShift)?,
                Opcode::BinaryRShift => self.binary_bitwise(BitwiseOp::RShift)?,
                Opcode::BinaryMatMul => todo!("BinaryMatMul not implemented"),
                // Comparison Operations
                Opcode::CompareEq => self.compare_eq(),
                Opcode::CompareNe => self.compare_ne(),
                Opcode::CompareLt => self.compare_ord(std::cmp::Ordering::is_lt),
                Opcode::CompareLe => self.compare_ord(std::cmp::Ordering::is_le),
                Opcode::CompareGt => self.compare_ord(std::cmp::Ordering::is_gt),
                Opcode::CompareGe => self.compare_ord(std::cmp::Ordering::is_ge),
                Opcode::CompareIs => self.compare_is(false),
                Opcode::CompareIsNot => self.compare_is(true),
                Opcode::CompareIn => self.compare_in(false)?,
                Opcode::CompareNotIn => self.compare_in(true)?,
                // Unary Operations
                Opcode::UnaryNot => {
                    let value = self.pop();
                    let result = !value.py_bool(self.heap, self.interns);
                    value.drop_with_heap(self.heap);
                    self.push(Value::Bool(result));
                }
                Opcode::UnaryNeg => {
                    // Unary minus - negate numeric value
                    let value = self.pop();
                    let result = match &value {
                        Value::Int(n) => Some(Value::Int(-n)),
                        Value::Float(f) => Some(Value::Float(-f)),
                        Value::Bool(b) => Some(Value::Int(if *b { -1 } else { 0 })),
                        _ => None,
                    };
                    value.drop_with_heap(self.heap);
                    match result {
                        Some(v) => self.push(v),
                        None => return Err(ExcType::type_error("bad operand type for unary -")),
                    }
                }
                Opcode::UnaryPos => {
                    // Unary plus - typically a no-op for numbers
                    let value = self.pop();
                    let result = match &value {
                        Value::Int(_) | Value::Float(_) | Value::Bool(_) => Some(value.clone_immediate()),
                        _ => None,
                    };
                    value.drop_with_heap(self.heap);
                    match result {
                        Some(v) => self.push(v),
                        None => return Err(ExcType::type_error("bad operand type for unary +")),
                    }
                }
                Opcode::UnaryInvert => {
                    // Bitwise NOT
                    let value = self.pop();
                    let result = match &value {
                        Value::Int(n) => Some(Value::Int(!n)),
                        Value::Bool(b) => Some(Value::Int(!i64::from(*b))),
                        _ => None,
                    };
                    value.drop_with_heap(self.heap);
                    match result {
                        Some(v) => self.push(v),
                        None => {
                            return Err(ExcType::type_error("bad operand type for unary ~"));
                        }
                    }
                }
                // In-place Operations
                Opcode::InplaceAdd => self.inplace_add()?,
                // Other in-place ops use the same logic as binary ops for now
                Opcode::InplaceSub => self.binary_sub()?,
                Opcode::InplaceMul => self.binary_mult()?,
                Opcode::InplaceDiv => self.binary_div()?,
                Opcode::InplaceFloorDiv => self.binary_floordiv()?,
                Opcode::InplaceMod => self.binary_mod(),
                Opcode::InplacePow => self.binary_pow()?,
                Opcode::InplaceAnd => self.binary_bitwise(BitwiseOp::And)?,
                Opcode::InplaceOr => self.binary_bitwise(BitwiseOp::Or)?,
                Opcode::InplaceXor => self.binary_bitwise(BitwiseOp::Xor)?,
                Opcode::InplaceLShift => self.binary_bitwise(BitwiseOp::LShift)?,
                Opcode::InplaceRShift => self.binary_bitwise(BitwiseOp::RShift)?,
                // Collection Building
                Opcode::BuildList => {
                    let count = self.fetch_u16() as usize;
                    self.build_list(count)?;
                }
                Opcode::BuildTuple => {
                    let count = self.fetch_u16() as usize;
                    self.build_tuple(count)?;
                }
                Opcode::BuildDict => {
                    let count = self.fetch_u16() as usize;
                    self.build_dict(count)?;
                }
                Opcode::BuildSet => {
                    let count = self.fetch_u16() as usize;
                    self.build_set(count)?;
                }
                Opcode::BuildFString => {
                    todo!("BuildFString not implemented")
                }
                // Subscript & Attribute
                Opcode::BinarySubscr => {
                    let index = self.pop();
                    let obj = self.pop();
                    let result = obj.py_getitem(&index, self.heap, self.interns);
                    obj.drop_with_heap(self.heap);
                    index.drop_with_heap(self.heap);
                    match result {
                        Ok(v) => self.push(v),
                        Err(e) => return Err(e),
                    }
                }
                Opcode::StoreSubscr => {
                    // Stack order: value, obj, index (TOS)
                    let index = self.pop();
                    let mut obj = self.pop();
                    let value = self.pop();
                    let result = obj.py_setitem(index, value, self.heap, self.interns);
                    obj.drop_with_heap(self.heap);
                    result?;
                }
                Opcode::DeleteSubscr => {
                    // TODO: Implement py_delitem on Value
                    let index = self.pop();
                    let obj = self.pop();
                    obj.drop_with_heap(self.heap);
                    index.drop_with_heap(self.heap);
                    todo!("DeleteSubscr: py_delitem not yet implemented")
                }
                Opcode::LoadAttr => {
                    let name_idx = self.fetch_u16();
                    let name_id = StringId::from_index(name_idx);
                    let obj = self.pop();
                    let result = self.get_attr(&obj, name_id);
                    obj.drop_with_heap(self.heap);
                    match result {
                        Ok(v) => self.push(v),
                        Err(e) => return Err(e),
                    }
                }
                Opcode::StoreAttr => {
                    let name_idx = self.fetch_u16();
                    let name_id = StringId::from_index(name_idx);
                    let obj = self.pop();
                    let value = self.pop();
                    let result = self.set_attr(&obj, name_id, value);
                    obj.drop_with_heap(self.heap);
                    result?;
                }
                Opcode::DeleteAttr => {
                    todo!("DeleteAttr not implemented")
                }
                // Control Flow
                Opcode::Jump => {
                    let offset = self.fetch_i16();
                    self.jump_relative(offset);
                }
                Opcode::JumpIfTrue => {
                    let offset = self.fetch_i16();
                    let cond = self.pop();
                    if cond.py_bool(self.heap, self.interns) {
                        self.jump_relative(offset);
                    }
                    cond.drop_with_heap(self.heap);
                }
                Opcode::JumpIfFalse => {
                    let offset = self.fetch_i16();
                    let cond = self.pop();
                    if !cond.py_bool(self.heap, self.interns) {
                        self.jump_relative(offset);
                    }
                    cond.drop_with_heap(self.heap);
                }
                Opcode::JumpIfTrueOrPop => {
                    let offset = self.fetch_i16();
                    if self.peek().py_bool(self.heap, self.interns) {
                        self.jump_relative(offset);
                    } else {
                        let value = self.pop();
                        value.drop_with_heap(self.heap);
                    }
                }
                Opcode::JumpIfFalseOrPop => {
                    let offset = self.fetch_i16();
                    if self.peek().py_bool(self.heap, self.interns) {
                        let value = self.pop();
                        value.drop_with_heap(self.heap);
                    } else {
                        self.jump_relative(offset);
                    }
                }
                // Iteration
                Opcode::GetIter => {
                    let value = self.pop();
                    // Create a ForIterator from the value and store on heap
                    match ForIterator::new(value, self.heap, self.interns) {
                        Ok(iter) => match self.heap.allocate(HeapData::Iterator(iter)) {
                            Ok(heap_id) => self.push(Value::Ref(heap_id)),
                            Err(e) => return Err(e.into()),
                        },
                        Err(e) => return Err(e),
                    }
                }
                Opcode::ForIter => {
                    let offset = self.fetch_i16();
                    // Peek at the iterator on TOS
                    let iter_ref = self.peek().copy_for_extend();
                    if let Value::Ref(heap_id) = iter_ref {
                        // Take the iterator out of the heap temporarily to avoid borrow conflict
                        let HeapData::Iterator(mut iter) = std::mem::replace(
                            self.heap.get_mut(heap_id),
                            HeapData::Iterator(ForIterator::placeholder()),
                        ) else {
                            return Err(RunError::internal("ForIter: expected iterator on stack"));
                        };

                        // Get next value from iterator
                        let next_result = iter.for_next(self.heap, self.interns);

                        // Put the iterator back
                        *self.heap.get_mut(heap_id) = HeapData::Iterator(iter);

                        match next_result {
                            Ok(Some(value)) => {
                                // Push the next value
                                self.push(value);
                            }
                            Ok(None) => {
                                // Iterator exhausted - pop it and jump to end
                                let iter = self.pop();
                                iter.drop_with_heap(self.heap);
                                self.jump_relative(offset);
                            }
                            Err(e) => {
                                // Error during iteration (e.g., dict size changed)
                                let iter = self.pop();
                                iter.drop_with_heap(self.heap);
                                return Err(e);
                            }
                        }
                    } else {
                        return Err(RunError::internal("ForIter: expected iterator ref on stack"));
                    }
                }
                // Function Calls
                Opcode::CallFunction => {
                    let arg_count = self.fetch_u8() as usize;

                    // Pop arguments in reverse order (TOS is last arg)
                    let args = self.pop_n_args(arg_count);

                    // Pop the callable
                    let callable = self.pop();

                    // Call the function and handle the result
                    match self.call_function(callable, args) {
                        Ok(CallResult::Builtin(result)) => self.push(result),
                        Ok(CallResult::UserFunction) => {
                            // Frame was pushed, continue execution in VM loop.
                            // Return value will be pushed by ReturnValue opcode.
                        }
                        Ok(CallResult::ExternalCall(ext_id, args_vec)) => {
                            // External function call - pause VM and return to caller
                            return Ok(VMSuccess::ExternalCall {
                                ext_function_id: ext_id,
                                args: args_vec,
                            });
                        }
                        Err(err) => {
                            // Try to handle the exception
                            if let Some(result) = self.handle_exception(err) {
                                return Err(result);
                            }
                            // Exception was handled, continue execution
                        }
                    }
                }
                Opcode::CallFunctionKw => {
                    todo!("CallFunctionKw (Step 4)")
                }
                Opcode::CallMethod => {
                    // CallMethod: u16 name_id, u8 arg_count
                    // Stack: [obj, arg1, arg2, ..., argN] -> [result]
                    let name_idx = self.fetch_u16();
                    let arg_count = self.fetch_u8() as usize;
                    let name_id = StringId::from_index(name_idx);

                    // Pop arguments in reverse order (TOS is last arg)
                    let args = self.pop_n_args(arg_count);

                    // Pop the object
                    let obj = self.pop();

                    // Call the method on the object
                    match self.call_method(obj, name_id, args) {
                        Ok(result) => self.push(result),
                        Err(err) => {
                            // Try to handle the exception
                            if let Some(result) = self.handle_exception(err) {
                                return Err(result);
                            }
                            // Exception was handled, continue execution
                        }
                    }
                }
                Opcode::CallExternal => {
                    todo!("CallExternal (Step 6)")
                }
                // Function Definition (Step 4)
                Opcode::MakeFunction => {
                    let func_idx = self.fetch_u16();
                    let defaults_count = self.fetch_u8() as usize;
                    let func_id = FunctionId::from_index(func_idx);

                    // Pop default values from stack (they were pushed in order, so reverse)
                    let defaults = if defaults_count > 0 {
                        let mut defaults = self.pop_n(defaults_count);
                        defaults.reverse();
                        defaults
                    } else {
                        Vec::new()
                    };

                    // Create FunctionDefaults on heap and push reference
                    let heap_id = self.heap.allocate(HeapData::FunctionDefaults(func_id, defaults))?;
                    self.push(Value::Ref(heap_id));
                }
                Opcode::MakeClosure => {
                    let func_idx = self.fetch_u16();
                    let defaults_count = self.fetch_u8() as usize;
                    let cell_count = self.fetch_u8() as usize;
                    let func_id = FunctionId::from_index(func_idx);

                    // Pop cells from stack (pushed after defaults, so on top)
                    // Cells are Value::Ref pointing to HeapData::Cell
                    let mut cells = Vec::with_capacity(cell_count);
                    for _ in 0..cell_count {
                        let cell_val = self.pop();
                        match cell_val {
                            Value::Ref(heap_id) => {
                                // Keep the reference - don't drop, Closure will own it
                                cells.push(heap_id);
                            }
                            _ => {
                                return Err(RunError::internal("MakeClosure: expected cell reference on stack"));
                            }
                        }
                    }
                    // Reverse to get original order (first cell pushed is first in vector)
                    cells.reverse();

                    // Pop default values from stack (they were pushed in order, so reverse)
                    let defaults = if defaults_count > 0 {
                        let mut defaults = self.pop_n(defaults_count);
                        defaults.reverse();
                        defaults
                    } else {
                        Vec::new()
                    };

                    // Create Closure on heap and push reference
                    let heap_id = self.heap.allocate(HeapData::Closure(func_id, cells, defaults))?;
                    self.push(Value::Ref(heap_id));
                }
                // Exception Handling
                Opcode::Raise => {
                    let exc = self.pop();
                    let error = self.make_exception(exc);
                    if let Some(result) = self.handle_exception(error) {
                        return Err(result);
                    }
                    // Exception was handled, continue execution
                }
                Opcode::RaiseFrom => {
                    todo!("RaiseFrom (Step 5)")
                }
                Opcode::Reraise => {
                    let error = if let Some(exc) = self.current_exception.take() {
                        self.make_exception(exc)
                    } else {
                        // No active exception - create a RuntimeError
                        SimpleException::new(
                            ExcType::RuntimeError,
                            Some("No active exception to reraise".to_string()),
                        )
                        .into()
                    };
                    if let Some(result) = self.handle_exception(error) {
                        return Err(result);
                    }
                    // Exception was handled, continue execution
                }
                Opcode::ClearException => {
                    if let Some(exc) = self.current_exception.take() {
                        exc.drop_with_heap(self.heap);
                    }
                }
                // Return
                Opcode::ReturnValue => {
                    let value = self.pop();
                    if self.frames.len() == 1 {
                        // Module-level return - we're done
                        return Ok(VMSuccess::Complete(value));
                    }
                    // Pop current frame and push return value
                    self.pop_frame();
                    self.push(value);
                }
                // Unpacking
                Opcode::UnpackSequence => {
                    let count = self.fetch_u8() as usize;
                    self.unpack_sequence(count)?;
                }
                Opcode::UnpackEx => {
                    todo!("UnpackEx not implemented")
                }
                // Special
                Opcode::Nop => {
                    // No operation
                }
            }
        }
    }

    /// Resumes execution after an external call completes.
    ///
    /// Pushes the return value onto the stack and continues execution.
    pub fn resume(&mut self, result: Value) -> Result<VMSuccess, RunError> {
        self.push(result);
        self.run()
    }

    /// Consumes the VM and creates a snapshot for pause/resume.
    ///
    /// **Ownership transfer:** This method takes `self` by value, consuming the VM.
    /// The snapshot owns all Values (refcounts already correct from the live VM).
    /// The heap and namespaces must be serialized alongside this snapshot.
    ///
    /// This is NOT a clone - it's a transfer. After calling this, the original VM
    /// is gone and only the snapshot (+ serialized heap/namespaces) represents the state.
    pub fn into_snapshot(self) -> VMSnapshot {
        VMSnapshot {
            // Move values directly - no clone, no refcount increment needed
            // (the VM owned them, now the snapshot owns them)
            stack: self.stack,
            frames: self.frames.into_iter().map(|f| f.serialize()).collect(),
            current_exception: self.current_exception,
            instruction_ip: self.instruction_ip,
        }
    }

    /// Reconstructs a VM from a snapshot.
    ///
    /// The heap and namespaces must already be deserialized. `FunctionId` values
    /// in frames are used to look up pre-compiled `Code` objects from the `Interns`.
    /// The `module_code` is used for frames with `function_id = None`.
    ///
    /// # Arguments
    /// * `snapshot` - The VM snapshot to restore
    /// * `module_code` - Compiled module code (for frames with function_id = None)
    /// * `heap` - The deserialized heap
    /// * `namespaces` - The deserialized namespaces
    /// * `interns` - Interns for looking up function code
    /// * `print_writer` - Writer for print output
    pub fn restore(
        snapshot: VMSnapshot,
        module_code: &'a Code,
        heap: &'a mut Heap<T>,
        namespaces: &'a mut Namespaces,
        interns: &'a Interns,
        print_writer: &'a mut P,
    ) -> Self {
        // Reconstruct call frames from serialized form
        let frames = snapshot
            .frames
            .into_iter()
            .map(|sf| {
                let code = match sf.function_id {
                    Some(func_id) => interns
                        .get_function(func_id)
                        .code
                        .as_ref()
                        .expect("function should be compiled"),
                    None => module_code,
                };
                CallFrame {
                    code,
                    ip: sf.ip,
                    stack_base: sf.stack_base,
                    namespace_idx: sf.namespace_idx,
                    function_id: sf.function_id,
                    cells: sf.cells,
                    call_position: sf.call_position,
                }
            })
            .collect();

        Self {
            stack: snapshot.stack,
            frames,
            heap,
            namespaces,
            interns,
            print_writer,
            current_exception: snapshot.current_exception,
            instruction_ip: snapshot.instruction_ip,
        }
    }

    // ========================================================================
    // Fetch Helpers
    // ========================================================================

    /// Fetches the next byte from current frame's bytecode and advances IP.
    #[inline]
    fn fetch_byte(&mut self) -> u8 {
        let frame = self.current_frame_mut();
        let byte = frame.code.bytecode()[frame.ip];
        frame.ip += 1;
        byte
    }

    /// Fetches and decodes the next opcode.
    #[inline]
    fn fetch_opcode(&mut self) -> Opcode {
        let byte = self.fetch_byte();
        Opcode::try_from(byte).expect("invalid opcode in bytecode")
    }

    /// Fetches a u8 operand.
    #[inline]
    fn fetch_u8(&mut self) -> u8 {
        self.fetch_byte()
    }

    /// Fetches an i8 operand.
    #[inline]
    fn fetch_i8(&mut self) -> i8 {
        self.fetch_byte() as i8
    }

    /// Fetches a u16 operand (little-endian).
    #[inline]
    fn fetch_u16(&mut self) -> u16 {
        let lo = self.fetch_byte();
        let hi = self.fetch_byte();
        u16::from_le_bytes([lo, hi])
    }

    /// Fetches an i16 operand (little-endian).
    #[inline]
    fn fetch_i16(&mut self) -> i16 {
        let lo = self.fetch_byte();
        let hi = self.fetch_byte();
        i16::from_le_bytes([lo, hi])
    }

    // ========================================================================
    // Stack Operations
    // ========================================================================

    /// Pushes a value onto the operand stack.
    #[inline]
    fn push(&mut self, value: Value) {
        self.stack.push(value);
    }

    /// Pops a value from the operand stack.
    #[inline]
    fn pop(&mut self) -> Value {
        self.stack.pop().expect("stack underflow")
    }

    /// Peeks at the top of the operand stack without removing it.
    #[inline]
    fn peek(&self) -> &Value {
        self.stack.last().expect("stack underflow")
    }

    /// Pops n values from the stack in reverse order (first popped is last in vec).
    fn pop_n(&mut self, n: usize) -> Vec<Value> {
        let start = self.stack.len() - n;
        self.stack.drain(start..).collect()
    }

    // ========================================================================
    // Frame Operations
    // ========================================================================

    /// Returns a reference to the current (topmost) call frame.
    #[inline]
    fn current_frame(&self) -> &CallFrame<'a> {
        self.frames.last().expect("no active frame")
    }

    /// Returns a mutable reference to the current call frame.
    #[inline]
    fn current_frame_mut(&mut self) -> &mut CallFrame<'a> {
        self.frames.last_mut().expect("no active frame")
    }

    /// Pops the current frame from the call stack.
    ///
    /// Cleans up the frame's stack region and namespace (except for global namespace).
    fn pop_frame(&mut self) {
        let frame = self.frames.pop().expect("no frame to pop");
        // Clean up frame's stack region
        while self.stack.len() > frame.stack_base {
            let value = self.stack.pop().unwrap();
            value.drop_with_heap(self.heap);
        }
        // Clean up the namespace (but not the global namespace)
        if frame.namespace_idx != GLOBAL_NS_IDX {
            self.namespaces.drop_with_heap(frame.namespace_idx, self.heap);
        }
    }

    /// Applies a relative jump offset to the current frame's IP.
    #[inline]
    fn jump_relative(&mut self, offset: i16) {
        let frame = self.current_frame_mut();
        frame.ip = (frame.ip as isize + offset as isize) as usize;
    }

    // ========================================================================
    // Variable Operations
    // ========================================================================

    /// Loads a local variable and pushes it onto the stack.
    fn load_local(&mut self, slot: u16) {
        let ns_idx = self.current_frame().namespace_idx;
        let namespace = self.namespaces.get(ns_idx);
        // Copy without incrementing refcount first (avoids borrow conflict)
        let value = namespace.get(NamespaceId::new(slot as usize)).copy_for_extend();
        // Now we can safely increment refcount and push
        if let Value::Ref(id) = &value {
            self.heap.inc_ref(*id);
        }
        self.push(value);
    }

    /// Pops the top of stack and stores it in a local variable.
    fn store_local(&mut self, slot: u16) {
        let value = self.pop();
        let ns_idx = self.current_frame().namespace_idx;
        let namespace = self.namespaces.get_mut(ns_idx);
        let ns_slot = NamespaceId::new(slot as usize);
        let old_value = std::mem::replace(namespace.get_mut(ns_slot), value);
        old_value.drop_with_heap(self.heap);
    }

    /// Deletes a local variable (sets it to Undefined).
    fn delete_local(&mut self, slot: u16) {
        let ns_idx = self.current_frame().namespace_idx;
        let namespace = self.namespaces.get_mut(ns_idx);
        let ns_slot = NamespaceId::new(slot as usize);
        let old_value = std::mem::replace(namespace.get_mut(ns_slot), Value::Undefined);
        old_value.drop_with_heap(self.heap);
    }

    /// Loads a global variable and pushes it onto the stack.
    fn load_global(&mut self, slot: u16) {
        let namespace = self.namespaces.get(GLOBAL_NS_IDX);
        // Copy without incrementing refcount first (avoids borrow conflict)
        let value = namespace.get(NamespaceId::new(slot as usize)).copy_for_extend();
        // Now we can safely increment refcount and push
        if let Value::Ref(id) = &value {
            self.heap.inc_ref(*id);
        }
        self.push(value);
    }

    /// Pops the top of stack and stores it in a global variable.
    fn store_global(&mut self, slot: u16) {
        let value = self.pop();
        let namespace = self.namespaces.get_mut(GLOBAL_NS_IDX);
        let ns_slot = NamespaceId::new(slot as usize);
        let old_value = std::mem::replace(namespace.get_mut(ns_slot), value);
        old_value.drop_with_heap(self.heap);
    }

    /// Loads from a closure cell and pushes onto the stack.
    fn load_cell(&mut self, slot: u16) {
        let cell_id = self.current_frame().cells[slot as usize];
        // Copy without incrementing refcount first (avoids borrow conflict)
        let value = self.heap.get_cell_value(cell_id).copy_for_extend();
        // Now we can safely increment refcount and push
        if let Value::Ref(id) = &value {
            self.heap.inc_ref(*id);
        }
        self.push(value);
    }

    /// Pops the top of stack and stores it in a closure cell.
    fn store_cell(&mut self, slot: u16) {
        let value = self.pop();
        let cell_id = self.current_frame().cells[slot as usize];
        self.heap.set_cell_value(cell_id, value);
    }

    // ========================================================================
    // Binary Operation Helpers
    // ========================================================================

    /// Binary addition with proper refcount handling.
    fn binary_add(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_add(&rhs, self.heap, self.interns);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Ok(Some(v)) => {
                self.push(v);
                Ok(())
            }
            Ok(None) => Err(ExcType::type_error("unsupported operand type(s) for +")),
            Err(e) => Err(e.into()),
        }
    }

    /// Binary subtraction with proper refcount handling.
    fn binary_sub(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_sub(&rhs, self.heap);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Ok(Some(v)) => {
                self.push(v);
                Ok(())
            }
            Ok(None) => Err(ExcType::type_error("unsupported operand type(s) for -")),
            Err(e) => Err(e.into()),
        }
    }

    /// Binary multiplication with proper refcount handling.
    fn binary_mult(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_mult(&rhs, self.heap, self.interns);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Ok(Some(v)) => {
                self.push(v);
                Ok(())
            }
            Ok(None) => Err(ExcType::type_error("unsupported operand type(s) for *")),
            Err(e) => Err(e),
        }
    }

    /// Binary division with proper refcount handling.
    fn binary_div(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_div(&rhs, self.heap);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Ok(Some(v)) => {
                self.push(v);
                Ok(())
            }
            Ok(None) => Err(ExcType::type_error("unsupported operand type(s) for /")),
            Err(e) => Err(e),
        }
    }

    /// Binary floor division with proper refcount handling.
    fn binary_floordiv(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_floordiv(&rhs, self.heap);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Ok(Some(v)) => {
                self.push(v);
                Ok(())
            }
            Ok(None) => Err(ExcType::type_error("unsupported operand type(s) for //")),
            Err(e) => Err(e),
        }
    }

    /// Binary modulo (no Result, just Option).
    fn binary_mod(&mut self) {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_mod(&rhs);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Some(v) => self.push(v),
            None => self.push(Value::None), // Type error - simplified for now
        }
    }

    /// Binary power with proper refcount handling.
    fn binary_pow(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_pow(&rhs, self.heap);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Ok(Some(v)) => {
                self.push(v);
                Ok(())
            }
            Ok(None) => Err(ExcType::type_error("unsupported operand type(s) for **")),
            Err(e) => Err(e),
        }
    }

    /// Binary bitwise operation on integers.
    ///
    /// Python only supports bitwise operations on integers (and bools, which coerce to int).
    fn binary_bitwise(&mut self, op: BitwiseOp) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();

        // Get integer values from lhs and rhs
        let lhs_int = match &lhs {
            Value::Int(i) => Some(*i),
            Value::Bool(b) => Some(i64::from(*b)),
            _ => None,
        };
        let rhs_int = match &rhs {
            Value::Int(i) => Some(*i),
            Value::Bool(b) => Some(i64::from(*b)),
            _ => None,
        };

        // Drop operands before returning error
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);

        if let (Some(l), Some(r)) = (lhs_int, rhs_int) {
            let result = match op {
                BitwiseOp::And => l & r,
                BitwiseOp::Or => l | r,
                BitwiseOp::Xor => l ^ r,
                BitwiseOp::LShift => {
                    // Python raises ValueError for negative shift, OverflowError for too large
                    if r < 0 {
                        return Err(ExcType::value_error_negative_shift_count());
                    }
                    // Limit shift to avoid overflow
                    if r > 63 {
                        return Err(ExcType::overflow_shift_count());
                    }
                    l << r
                }
                BitwiseOp::RShift => {
                    if r < 0 {
                        return Err(ExcType::value_error_negative_shift_count());
                    }
                    // Large right shifts just give 0 or -1 for negative numbers
                    if r > 63 {
                        if l < 0 {
                            -1
                        } else {
                            0
                        }
                    } else {
                        l >> r
                    }
                }
            };
            self.push(Value::Int(result));
            Ok(())
        } else {
            let op_str = match op {
                BitwiseOp::And => "&",
                BitwiseOp::Or => "|",
                BitwiseOp::Xor => "^",
                BitwiseOp::LShift => "<<",
                BitwiseOp::RShift => ">>",
            };
            Err(ExcType::type_error(&format!(
                "unsupported operand type(s) for {op_str}"
            )))
        }
    }

    /// In-place addition (uses py_iadd for mutable containers, falls back to py_add).
    ///
    /// For mutable types like lists, `py_iadd` mutates in place and returns true.
    /// For immutable types, we fall back to regular addition.
    fn inplace_add(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let mut lhs = self.pop();

        // Try in-place operation first (for mutable types like lists)
        // py_iadd takes owned `other` and mutates `self` in place
        let lhs_id = if let Value::Ref(id) = &lhs { Some(*id) } else { None };

        let succeeded = lhs.py_iadd(rhs.clone_with_heap(self.heap), self.heap, lhs_id, self.interns)?;

        if succeeded {
            // In-place operation succeeded - drop rhs and push lhs back
            rhs.drop_with_heap(self.heap);
            self.push(lhs);
            Ok(())
        } else {
            // Fall back to regular addition
            let result = lhs.py_add(&rhs, self.heap, self.interns);
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            match result {
                Ok(Some(v)) => {
                    self.push(v);
                    Ok(())
                }
                Ok(None) => Err(ExcType::type_error("unsupported operand type(s) for +=")),
                Err(e) => Err(e.into()),
            }
        }
    }

    // ========================================================================
    // Comparison Helpers
    // ========================================================================

    /// Equality comparison.
    fn compare_eq(&mut self) {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_eq(&rhs, self.heap, self.interns);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(result));
    }

    /// Inequality comparison.
    fn compare_ne(&mut self) {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = !lhs.py_eq(&rhs, self.heap, self.interns);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(result));
    }

    /// Ordering comparison with a predicate.
    fn compare_ord<F>(&mut self, check: F)
    where
        F: FnOnce(std::cmp::Ordering) -> bool,
    {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_cmp(&rhs, self.heap, self.interns).is_some_and(check);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(result));
    }

    /// Identity comparison (is/is not).
    ///
    /// Two values are identical if:
    /// - Both are `None` (singleton)
    /// - Both are the same boolean value (True/False are singletons)
    /// - Both are `Ref` pointing to the same `HeapId`
    /// - For small integers and interned strings, Python uses object caching,
    ///   but we simplify by comparing values for immediate types.
    fn compare_is(&mut self, negate: bool) {
        let rhs = self.pop();
        let lhs = self.pop();

        let result = match (&lhs, &rhs) {
            // Singletons
            (Value::None, Value::None) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            // Heap references - same identity if same HeapId
            (Value::Ref(a), Value::Ref(b)) => a == b,
            // Different types or different values are not identical
            _ => false,
        };

        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(if negate { !result } else { result }));
    }

    /// Membership test (in/not in).
    fn compare_in(&mut self, negate: bool) -> Result<(), RunError> {
        let container = self.pop(); // container (rhs)
        let item = self.pop(); // item to find (lhs)

        let result = self.contains(&item, &container)?;

        item.drop_with_heap(self.heap);
        container.drop_with_heap(self.heap);

        self.push(Value::Bool(if negate { !result } else { result }));
        Ok(())
    }

    /// TODO this should call methods on the types!
    /// Check if item is contained in container.
    ///
    /// Handles different container types:
    /// - List/Tuple: linear search with equality
    /// - Dict: key lookup
    /// - Set/FrozenSet: element lookup
    /// - Str: substring search
    fn contains(&mut self, item: &Value, container: &Value) -> Result<bool, RunError> {
        match container {
            Value::Ref(heap_id) => {
                let heap_id = *heap_id;
                match self.heap.get(heap_id) {
                    HeapData::List(list) => {
                        // Get the length first, then iterate by index to avoid borrow conflicts
                        let len = list.len();
                        for i in 0..len {
                            // Re-borrow heap each iteration, copy element for comparison
                            let elem = match self.heap.get(heap_id) {
                                HeapData::List(l) => l.as_vec().get(i).map(Value::copy_for_extend),
                                _ => unreachable!(),
                            };
                            if let Some(elem) = elem {
                                let found = item.py_eq(&elem, self.heap, self.interns);
                                // Don't drop elem - it's a borrowed copy, not owned.
                                // Value has a Drop impl only with ref-count-panic feature.
                                #[allow(clippy::forget_non_drop)]
                                std::mem::forget(elem);
                                if found {
                                    return Ok(true);
                                }
                            }
                        }
                        Ok(false)
                    }
                    HeapData::Tuple(tuple) => {
                        // Get the length first, then iterate by index to avoid borrow conflicts
                        let len = tuple.as_vec().len();
                        for i in 0..len {
                            // Re-borrow heap each iteration, copy element for comparison
                            let elem = match self.heap.get(heap_id) {
                                HeapData::Tuple(t) => t.as_vec().get(i).map(Value::copy_for_extend),
                                _ => unreachable!(),
                            };
                            if let Some(elem) = elem {
                                let found = item.py_eq(&elem, self.heap, self.interns);
                                // Don't drop elem - it's a borrowed copy, not owned.
                                // Value has a Drop impl only with ref-count-panic feature.
                                #[allow(clippy::forget_non_drop)]
                                std::mem::forget(elem);
                                if found {
                                    return Ok(true);
                                }
                            }
                        }
                        Ok(false)
                    }
                    HeapData::Dict(_) => {
                        // Check if item is a key in dict
                        self.heap.with_entry_mut(heap_id, |heap, data| {
                            if let HeapData::Dict(dict) = data {
                                match dict.get(item, heap, self.interns) {
                                    Ok(Some(_)) => Ok(true),
                                    Ok(None) => Ok(false),
                                    Err(e) => Err(e),
                                }
                            } else {
                                unreachable!("type changed during borrow")
                            }
                        })
                    }
                    HeapData::Set(_) => {
                        // Check if item is in set
                        self.heap.with_entry_mut(heap_id, |heap, data| {
                            if let HeapData::Set(set) = data {
                                set.contains(item, heap, self.interns)
                            } else {
                                unreachable!("type changed during borrow")
                            }
                        })
                    }
                    HeapData::FrozenSet(_) => {
                        // Check if item is in frozenset
                        self.heap.with_entry_mut(heap_id, |heap, data| {
                            if let HeapData::FrozenSet(fset) = data {
                                fset.contains(item, heap, self.interns)
                            } else {
                                unreachable!("type changed during borrow")
                            }
                        })
                    }
                    HeapData::Str(s) => {
                        // Substring check for str in str
                        match item {
                            Value::InternString(item_id) => {
                                let item_str = self.interns.get_str(*item_id);
                                Ok(s.as_str().contains(item_str))
                            }
                            Value::Ref(item_heap_id) => {
                                if let HeapData::Str(item_str) = self.heap.get(*item_heap_id) {
                                    Ok(s.as_str().contains(item_str.as_str()))
                                } else {
                                    let type_name = container.py_type(Some(self.heap));
                                    Err(ExcType::type_error(&format!(
                                        "'in <{type_name}>' requires string as left operand"
                                    )))
                                }
                            }
                            _ => {
                                let type_name = container.py_type(Some(self.heap));
                                Err(ExcType::type_error(&format!(
                                    "'in <{type_name}>' requires string as left operand"
                                )))
                            }
                        }
                    }
                    other => {
                        let type_name = other.py_type(Some(self.heap));
                        Err(ExcType::type_error(&format!(
                            "argument of type '{type_name}' is not iterable"
                        )))
                    }
                }
            }
            Value::InternString(string_id) => {
                // Substring check for str in str
                let container_str = self.interns.get_str(*string_id);
                match item {
                    Value::InternString(item_id) => {
                        let item_str = self.interns.get_str(*item_id);
                        Ok(container_str.contains(item_str))
                    }
                    Value::Ref(item_heap_id) => {
                        if let HeapData::Str(item_str) = self.heap.get(*item_heap_id) {
                            Ok(container_str.contains(item_str.as_str()))
                        } else {
                            Err(ExcType::type_error("'in <str>' requires string as left operand"))
                        }
                    }
                    _ => Err(ExcType::type_error("'in <str>' requires string as left operand")),
                }
            }
            _ => {
                let type_name = container.py_type(Some(self.heap));
                Err(ExcType::type_error(&format!(
                    "argument of type '{type_name}' is not iterable"
                )))
            }
        }
    }

    // ========================================================================
    // Collection Building
    // ========================================================================

    /// Builds a list from the top n stack values.
    fn build_list(&mut self, count: usize) -> Result<(), RunError> {
        let items = self.pop_n(count);
        let list = List::new(items);
        let heap_id = self.heap.allocate(HeapData::List(list)).map_err(RunError::from)?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Builds a tuple from the top n stack values.
    fn build_tuple(&mut self, count: usize) -> Result<(), RunError> {
        let items = self.pop_n(count);
        let tuple = Tuple::new(items);
        let heap_id = self.heap.allocate(HeapData::Tuple(tuple)).map_err(RunError::from)?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Builds a dict from the top 2n stack values (key/value pairs).
    fn build_dict(&mut self, count: usize) -> Result<(), RunError> {
        let items = self.pop_n(count * 2);
        let mut dict = Dict::new();
        // Use into_iter to consume items by value, avoiding clone and proper ownership transfer
        let mut iter = items.into_iter();
        while let (Some(key), Some(value)) = (iter.next(), iter.next()) {
            dict.set(key, value, self.heap, self.interns)?;
        }
        let heap_id = self.heap.allocate(HeapData::Dict(dict)).map_err(RunError::from)?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Builds a set from the top n stack values.
    fn build_set(&mut self, count: usize) -> Result<(), RunError> {
        let items = self.pop_n(count);
        let mut set = Set::new();
        for item in items {
            set.add(item, self.heap, self.interns)?;
        }
        let heap_id = self.heap.allocate(HeapData::Set(set)).map_err(RunError::from)?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    // ========================================================================
    // Attribute Access
    // ========================================================================

    /// Gets an attribute from an object.
    ///
    /// Currently only Dataclass objects support attribute access. For other types,
    /// returns AttributeError.
    fn get_attr(&mut self, obj: &Value, name_id: StringId) -> Result<Value, RunError> {
        let attr_name = self.interns.get_str(name_id);

        if let Value::Ref(heap_id) = obj {
            let heap_id = *heap_id;
            // Check if heap object is a Dataclass (need to check type first)
            let is_dataclass = matches!(self.heap.get(heap_id), HeapData::Dataclass(_));

            if is_dataclass {
                // Use with_entry_mut to get mutable access to the dataclass
                let name_value = Value::InternString(name_id);
                self.heap.with_entry_mut(heap_id, |heap, data| {
                    if let HeapData::Dataclass(dc) = data {
                        match dc.get_attr(&name_value, heap, self.interns) {
                            Ok(Some(value)) => {
                                // Clone the value and increment its refcount
                                Ok(value.clone_with_heap(heap))
                            }
                            Ok(None) => {
                                // Attribute not found
                                let type_name = dc.py_type(Some(heap));
                                Err(ExcType::attribute_error(type_name, attr_name))
                            }
                            Err(e) => Err(e),
                        }
                    } else {
                        unreachable!("type changed during borrow")
                    }
                })
            } else {
                // Other heap types don't support attribute access
                let type_name = self.heap.get(heap_id).py_type(Some(self.heap));
                Err(ExcType::attribute_error(type_name, attr_name))
            }
        } else {
            // Non-heap values don't support attribute access
            let type_name = obj.py_type(Some(self.heap));
            Err(ExcType::attribute_error(type_name, attr_name))
        }
    }

    /// Sets an attribute on an object.
    ///
    /// Currently only Dataclass objects support attribute setting. For other types,
    /// returns AttributeError.
    fn set_attr(&mut self, obj: &Value, name_id: StringId, value: Value) -> Result<(), RunError> {
        let attr_name = self.interns.get_str(name_id);

        if let Value::Ref(heap_id) = obj {
            let heap_id = *heap_id;
            // Check if heap object is a Dataclass (need to check type first)
            let is_dataclass = matches!(self.heap.get(heap_id), HeapData::Dataclass(_));

            if is_dataclass {
                // Use with_entry_mut to get mutable access to the dataclass
                let name_value = Value::InternString(name_id);
                self.heap.with_entry_mut(heap_id, |heap, data| {
                    if let HeapData::Dataclass(dc) = data {
                        match dc.set_attr(name_value, value, heap, self.interns) {
                            Ok(old_value) => {
                                // Drop old value if there was one
                                if let Some(old) = old_value {
                                    old.drop_with_heap(heap);
                                }
                                Ok(())
                            }
                            Err(e) => Err(e),
                        }
                    } else {
                        unreachable!("type changed during borrow")
                    }
                })
            } else {
                // Other heap types don't support attribute setting
                let type_name = self.heap.get(heap_id).py_type(Some(self.heap));
                value.drop_with_heap(self.heap);
                Err(ExcType::attribute_error(type_name, attr_name))
            }
        } else {
            // Non-heap values don't support attribute setting
            let type_name = obj.py_type(Some(self.heap));
            value.drop_with_heap(self.heap);
            Err(ExcType::attribute_error(type_name, attr_name))
        }
    }

    // ========================================================================
    // Unpacking
    // ========================================================================

    /// Unpacks a sequence into n values on the stack.
    fn unpack_sequence(&mut self, count: usize) -> Result<(), RunError> {
        let value = self.pop();

        // First, copy values without incrementing refcounts (avoids borrow conflict)
        let items: Vec<Value> = if let Value::Ref(heap_id) = &value {
            match self.heap.get(*heap_id) {
                HeapData::List(list) => {
                    let list_len = list.len();
                    if list_len != count {
                        value.drop_with_heap(self.heap);
                        return Err(SimpleException::new(
                            ExcType::ValueError,
                            Some(format!(
                                "not enough values to unpack (expected {count}, got {list_len})"
                            )),
                        )
                        .into());
                    }
                    list.as_vec().iter().map(Value::copy_for_extend).collect()
                }
                HeapData::Tuple(tuple) => {
                    let tuple_len = tuple.as_vec().len();
                    if tuple_len != count {
                        value.drop_with_heap(self.heap);
                        return Err(SimpleException::new(
                            ExcType::ValueError,
                            Some(format!(
                                "not enough values to unpack (expected {count}, got {tuple_len})"
                            )),
                        )
                        .into());
                    }
                    tuple.as_vec().iter().map(Value::copy_for_extend).collect()
                }
                _ => {
                    value.drop_with_heap(self.heap);
                    return Err(ExcType::type_error("cannot unpack non-sequence"));
                }
            }
        } else {
            value.drop_with_heap(self.heap);
            return Err(ExcType::type_error("cannot unpack non-sequence"));
        };

        value.drop_with_heap(self.heap);

        // Now increment refcounts for all copied values
        for item in &items {
            if let Value::Ref(id) = item {
                self.heap.inc_ref(*id);
            }
        }

        // Push items in reverse order so first item is on top
        for item in items.into_iter().rev() {
            self.push(item);
        }
        Ok(())
    }

    // ========================================================================
    // Function Call Helpers
    // ========================================================================

    /// Pops n arguments from the stack and wraps them in ArgValues.
    fn pop_n_args(&mut self, n: usize) -> ArgValues {
        match n {
            0 => ArgValues::Empty,
            1 => ArgValues::One(self.pop()),
            2 => {
                let b = self.pop();
                let a = self.pop();
                ArgValues::Two(a, b)
            }
            _ => {
                let args = self.pop_n(n);
                ArgValues::ArgsKargs {
                    args,
                    kwargs: crate::args::KwargsValues::Empty,
                }
            }
        }
    }

    /// Calls a method on an object.
    ///
    /// For heap-allocated objects (Value::Ref), dispatches to the type's
    /// `py_call_attr` implementation via `heap.call_attr()`.
    fn call_method(&mut self, obj: Value, name_id: StringId, args: ArgValues) -> Result<Value, RunError> {
        let attr = Attr::Interned(name_id);

        if let Value::Ref(heap_id) = obj {
            // Call the method on the heap object
            let result = self.heap.call_attr(heap_id, &attr, args, self.interns);
            // Drop the object reference after the call
            obj.drop_with_heap(self.heap);
            result
        } else {
            // Non-heap values don't support method calls
            let type_name = obj.py_type(Some(self.heap));
            args.drop_with_heap(self.heap);
            Err(ExcType::attribute_error(type_name, self.interns.get_str(name_id)))
        }
    }

    /// Calls a callable value with the given arguments.
    ///
    /// Returns `CallResult::Builtin(value)` for builtin functions,
    /// `CallResult::UserFunction` for user functions (frame was pushed), or
    /// `CallResult::ExternalCall` for external functions (VM should pause).
    fn call_function(&mut self, callable: Value, args: ArgValues) -> Result<CallResult, RunError> {
        match callable {
            Value::Builtin(builtin) => {
                // Call the builtin function
                let result = builtin.call(self.heap, args, self.interns, self.print_writer)?;
                Ok(CallResult::Builtin(result))
            }
            Value::ExtFunction(ext_id) => {
                // External function - return to caller to execute
                // Convert ArgValues to Vec<Value> for external call
                let args_vec = args.into_vec();
                Ok(CallResult::ExternalCall(ext_id, args_vec))
            }
            Value::Ref(heap_id) => {
                // Could be a closure or function - check heap and extract info.
                // Two-phase approach to avoid borrow conflicts:
                // 1. Copy data without incrementing refcounts
                // 2. Increment refcounts after the borrow ends

                // Phase 1: Copy data (func_id, cells, defaults) without refcount changes
                let (func_id, cells, defaults) = match self.heap.get(heap_id) {
                    HeapData::Closure(fid, cells, defaults) => {
                        let cloned_cells = cells.clone();
                        // Use copy_for_extend to avoid refcount increment during borrow
                        let cloned_defaults: Vec<Value> = defaults.iter().map(Value::copy_for_extend).collect();
                        (*fid, cloned_cells, cloned_defaults)
                    }
                    HeapData::FunctionDefaults(fid, defaults) => {
                        let cloned_defaults: Vec<Value> = defaults.iter().map(Value::copy_for_extend).collect();
                        (*fid, Vec::new(), cloned_defaults)
                    }
                    _ => {
                        callable.drop_with_heap(self.heap);
                        args.drop_with_heap(self.heap);
                        return Err(ExcType::type_error("object is not callable"));
                    }
                };

                // Phase 2: Increment refcounts now that the heap borrow has ended
                for &cell_id in &cells {
                    self.heap.inc_ref(cell_id);
                }
                for default in &defaults {
                    if let Value::Ref(id) = default {
                        self.heap.inc_ref(*id);
                    }
                }

                // Drop the callable ref (cloned data has its own refcounts)
                callable.drop_with_heap(self.heap);

                // Call the user function
                self.call_user_function(func_id, cells, defaults, args)?;
                Ok(CallResult::UserFunction)
            }
            _ => {
                args.drop_with_heap(self.heap);
                Err(ExcType::type_error("object is not callable"))
            }
        }
    }

    /// Calls a user-defined function by pushing a new frame.
    ///
    /// Sets up the function's namespace with bound arguments, cell variables,
    /// and free variables (captured from enclosing scope for closures).
    fn call_user_function(
        &mut self,
        func_id: FunctionId,
        cells: Vec<HeapId>,
        defaults: Vec<Value>,
        args: ArgValues,
    ) -> Result<(), RunError> {
        // Get call position BEFORE borrowing namespaces mutably
        let call_position = self.current_position();

        // Get function info (interns is a shared reference so no conflict)
        let func = self.interns.get_function(func_id);
        let namespace_size = func.namespace_size;
        let param_count = func.signature.total_slots();
        let cell_var_count = func.cell_var_count;
        let cell_param_indices = func.cell_param_indices.clone();
        let code = func.code.as_ref().expect("function should be compiled");

        // 1. Create new namespace for function
        let namespace_idx = self.namespaces.new_namespace(namespace_size, self.heap)?;

        // 2. Bind arguments to parameters
        {
            let namespace = self.namespaces.get_mut(namespace_idx).mut_vec();
            let bind_result = func
                .signature
                .bind(args, &defaults, self.heap, self.interns, func.name, namespace);

            if let Err(e) = bind_result {
                self.namespaces.drop_with_heap(namespace_idx, self.heap);
                return Err(e);
            }
        }

        // Track created cell HeapIds for the frame
        let mut frame_cells: Vec<HeapId> = Vec::with_capacity(cell_var_count + cells.len());

        // 3. Create cells for variables captured by nested functions
        {
            let namespace = self.namespaces.get_mut(namespace_idx).mut_vec();
            for (i, maybe_param_idx) in cell_param_indices.iter().enumerate() {
                let cell_slot = param_count + i;
                let cell_value = if let Some(param_idx) = maybe_param_idx {
                    // Cell is for a parameter - copy its value
                    namespace[*param_idx].clone_with_heap(self.heap)
                } else {
                    Value::Undefined
                };
                let cell_id = self.heap.allocate(HeapData::Cell(cell_value))?;
                frame_cells.push(cell_id);
                // Extend namespace to fit cell if needed
                while namespace.len() <= cell_slot {
                    namespace.push(Value::Undefined);
                }
                namespace[cell_slot] = Value::Ref(cell_id);
            }

            // 4. Copy captured cells (free vars) into namespace
            let free_var_start = param_count + cell_var_count;
            for (i, &cell_id) in cells.iter().enumerate() {
                self.heap.inc_ref(cell_id);
                frame_cells.push(cell_id);
                let slot = free_var_start + i;
                // Extend namespace to fit free var if needed
                while namespace.len() <= slot {
                    namespace.push(Value::Undefined);
                }
                namespace[slot] = Value::Ref(cell_id);
            }

            // 5. Fill remaining slots with Undefined
            while namespace.len() < namespace_size {
                namespace.push(Value::Undefined);
            }
        }

        // 6. Push new frame
        self.frames.push(CallFrame::new_function(
            code,
            self.stack.len(),
            namespace_idx,
            func_id,
            frame_cells,
            call_position,
        ));

        Ok(())
    }

    /// Returns the current source position for traceback generation.
    fn current_position(&self) -> CodeRange {
        let frame = self.current_frame();
        // Get the position from the current instruction (IP points to next instruction)
        // Look up in location table
        let ip = frame.ip.saturating_sub(1);
        frame
            .code
            .location_for_offset(ip)
            .map(super::code::LocationEntry::range)
            .unwrap_or_default()
    }

    // ========================================================================
    // Exception Handling
    // ========================================================================

    /// Creates a RunError from a Value that should be an exception.
    ///
    /// Takes ownership of the exception value and drops it properly.
    fn make_exception(&mut self, exc_value: Value) -> RunError {
        // For now, create a simple exception. Full traceback support in Step 5.
        if let Value::Ref(heap_id) = &exc_value {
            if let HeapData::Exception(exc) = self.heap.get(*heap_id) {
                // Clone the exception and convert to RunError via ExceptionRaise
                let exc_clone = exc.clone();
                // Drop the value with proper heap cleanup
                exc_value.drop_with_heap(self.heap);
                let raise: ExceptionRaise = exc_clone.into();
                return raise.into();
            }
        }
        // Drop the value (even if not an exception)
        exc_value.drop_with_heap(self.heap);
        // Invalid exception value - create a TypeError
        SimpleException::new(
            ExcType::TypeError,
            Some("exceptions must derive from BaseException".to_string()),
        )
        .into()
    }

    /// Handles an exception by searching for a handler in the exception table.
    ///
    /// Returns:
    /// - `Some(VMResult)` if the exception was not caught (should return from run loop)
    /// - `None` if the exception was caught (continue execution)
    ///
    /// When an exception is caught:
    /// 1. Unwinds the stack to the handler's expected depth
    /// 2. Pushes the exception value onto the stack
    /// 3. Sets `current_exception` for bare `raise`
    /// 4. Jumps to the handler code
    fn handle_exception(&mut self, error: RunError) -> Option<RunError> {
        // Only catchable exceptions can be handled
        let exc_info = match &error {
            RunError::Exc(exc) => exc.clone(),
            RunError::UncatchableExc(_) | RunError::Internal(_) => {
                return Some(error);
            }
        };

        // Create exception value to push on stack
        let exc_value = self.create_exception_value(&exc_info);
        let exc_value = match exc_value {
            Ok(v) => v,
            Err(e) => return Some(e),
        };

        // Search for handler in current and outer frames
        loop {
            let frame = self.current_frame();
            let ip = self.instruction_ip as u32;

            // Search exception table for a handler covering this IP
            if let Some(entry) = frame.code.find_exception_handler(ip) {
                // Found a handler! Unwind stack and jump to it.
                let handler_offset = entry.handler() as usize;
                let target_stack_depth = frame.stack_base + entry.stack_depth() as usize;

                // Unwind stack to target depth (drop excess values)
                while self.stack.len() > target_stack_depth {
                    let value = self.stack.pop().unwrap();
                    value.drop_with_heap(self.heap);
                }

                // Push exception value onto stack (handler expects it)
                let exc_for_stack = exc_value.clone_with_heap(self.heap);
                self.push(exc_for_stack);

                // Set current_exception for bare raise
                if let Some(old) = self.current_exception.replace(exc_value) {
                    old.drop_with_heap(self.heap);
                }

                // Jump to handler
                self.current_frame_mut().ip = handler_offset;

                return None; // Continue execution at handler
            }

            // No handler in this frame - pop frame and try outer
            if self.frames.len() <= 1 {
                // No more frames - exception is unhandled
                exc_value.drop_with_heap(self.heap);
                return Some(error);
            }

            // Pop this frame and continue searching
            self.pop_frame();
            // Update instruction_ip to the call site in the outer frame
            self.instruction_ip = self
                .current_frame()
                .call_position
                .map_or(0, |p| p.start().line as usize);
        }
    }

    /// Creates an exception Value from exception info.
    ///
    /// Allocates an Exception on the heap and returns a Value::Ref to it.
    fn create_exception_value(&mut self, exc: &ExceptionRaise) -> Result<Value, RunError> {
        let exception = exc.exc.clone();
        let heap_id = self.heap.allocate(HeapData::Exception(exception))?;
        Ok(Value::Ref(heap_id))
    }
}
