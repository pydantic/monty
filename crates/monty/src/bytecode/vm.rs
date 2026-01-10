//! Bytecode virtual machine for executing compiled Python code.
//!
//! The VM uses a stack-based execution model with an operand stack for computation
//! and a call stack for function frames. Each frame owns its instruction pointer (IP).

use crate::{
    args::ArgValues,
    builtins::Builtins,
    bytecode::{code::Code, op::Opcode},
    exception_private::{ExcType, ExceptionRaise, RunError, SimpleException},
    for_iterator::ForIterator,
    heap::{Heap, HeapData, HeapId},
    intern::{FunctionId, Interns, StringId},
    io::PrintWriter,
    namespace::{NamespaceId, Namespaces, GLOBAL_NS_IDX},
    parse::CodeRange,
    resource::ResourceTracker,
    types::{Dict, List, PyTrait, Set, Tuple},
    value::Value,
};

// ============================================================================
// VM Result Types
// ============================================================================

/// Result of VM execution.
pub enum VMResult {
    /// Execution completed successfully with a return value.
    Complete(Value),

    /// Execution encountered an error.
    ///
    /// This can be a catchable exception (`RunError::Exc`), an uncatchable
    /// resource limit error (`RunError::UncatchableExc`), or an internal
    /// interpreter error (`RunError::Internal`).
    Error(RunError),

    /// Execution paused for an external function call.
    ///
    /// The caller should execute the external function and call `resume()`
    /// with the result.
    ExternalCall {
        /// ID of the external function to call.
        function_id: u16,
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
        }
    }

    /// Pushes an initial frame for module-level code and runs the VM.
    pub fn run_module(&mut self, code: &'a Code) -> VMResult {
        self.frames.push(CallFrame::new_module(code, GLOBAL_NS_IDX));
        self.run()
    }

    /// Main execution loop.
    ///
    /// Fetches opcodes from the current frame's bytecode and executes them.
    /// Returns when execution completes, an error occurs, or an external
    /// call is needed.
    pub fn run(&mut self) -> VMResult {
        loop {
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
                    let len = self.stack.len();
                    let c = self.stack[len - 1].clone_immediate();
                    self.stack[len - 1] = self.stack[len - 2].clone_immediate();
                    self.stack[len - 2] = self.stack[len - 3].clone_immediate();
                    self.stack[len - 3] = c;
                }

                // ============================================================
                // Constants & Literals
                // ============================================================
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

                Opcode::LoadNone => {
                    self.push(Value::None);
                }

                Opcode::LoadTrue => {
                    self.push(Value::Bool(true));
                }

                Opcode::LoadFalse => {
                    self.push(Value::Bool(false));
                }

                Opcode::LoadSmallInt => {
                    let n = self.fetch_i8();
                    self.push(Value::Int(i64::from(n)));
                }

                // ============================================================
                // Variables - Specialized Local Loads (no operand)
                // ============================================================
                Opcode::LoadLocal0 => self.load_local(0),
                Opcode::LoadLocal1 => self.load_local(1),
                Opcode::LoadLocal2 => self.load_local(2),
                Opcode::LoadLocal3 => self.load_local(3),

                // ============================================================
                // Variables - General Local Operations
                // ============================================================
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

                // ============================================================
                // Variables - Global Operations
                // ============================================================
                Opcode::LoadGlobal => {
                    let slot = self.fetch_u16();
                    self.load_global(slot);
                }

                Opcode::StoreGlobal => {
                    let slot = self.fetch_u16();
                    self.store_global(slot);
                }

                // ============================================================
                // Variables - Cell Operations (closures)
                // ============================================================
                Opcode::LoadCell => {
                    let slot = self.fetch_u16();
                    self.load_cell(slot);
                }

                Opcode::StoreCell => {
                    let slot = self.fetch_u16();
                    self.store_cell(slot);
                }

                // ============================================================
                // Binary Operations
                // ============================================================
                Opcode::BinaryAdd => {
                    if let Err(e) = self.binary_add() {
                        return VMResult::Error(e);
                    }
                }

                Opcode::BinarySub => {
                    if let Err(e) = self.binary_sub() {
                        return VMResult::Error(e);
                    }
                }

                Opcode::BinaryMul => {
                    if let Err(e) = self.binary_mult() {
                        return VMResult::Error(e);
                    }
                }

                Opcode::BinaryDiv => {
                    if let Err(e) = self.binary_div() {
                        return VMResult::Error(e);
                    }
                }

                Opcode::BinaryFloorDiv => {
                    if let Err(e) = self.binary_floordiv() {
                        return VMResult::Error(e);
                    }
                }

                Opcode::BinaryMod => {
                    self.binary_mod();
                }

                Opcode::BinaryPow => {
                    if let Err(e) = self.binary_pow() {
                        return VMResult::Error(e);
                    }
                }

                // Bitwise operations - not yet implemented in PyTrait
                Opcode::BinaryAnd => todo!("BinaryAnd not implemented in PyTrait"),
                Opcode::BinaryOr => todo!("BinaryOr not implemented in PyTrait"),
                Opcode::BinaryXor => todo!("BinaryXor not implemented in PyTrait"),
                Opcode::BinaryLShift => todo!("BinaryLShift not implemented in PyTrait"),
                Opcode::BinaryRShift => todo!("BinaryRShift not implemented in PyTrait"),
                Opcode::BinaryMatMul => todo!("BinaryMatMul not implemented"),

                // ============================================================
                // Comparison Operations
                // ============================================================
                Opcode::CompareEq => self.compare_eq(),
                Opcode::CompareNe => self.compare_ne(),
                Opcode::CompareLt => self.compare_ord(std::cmp::Ordering::is_lt),
                Opcode::CompareLe => self.compare_ord(std::cmp::Ordering::is_le),
                Opcode::CompareGt => self.compare_ord(std::cmp::Ordering::is_gt),
                Opcode::CompareGe => self.compare_ord(std::cmp::Ordering::is_ge),
                Opcode::CompareIs => self.compare_is(false),
                Opcode::CompareIsNot => self.compare_is(true),
                Opcode::CompareIn => {
                    if let Err(e) = self.compare_in(false) {
                        return VMResult::Error(e);
                    }
                }
                Opcode::CompareNotIn => {
                    if let Err(e) = self.compare_in(true) {
                        return VMResult::Error(e);
                    }
                }

                // ============================================================
                // Unary Operations
                // ============================================================
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
                        None => {
                            return VMResult::Error(ExcType::type_error("bad operand type for unary -"));
                        }
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
                        None => {
                            return VMResult::Error(ExcType::type_error("bad operand type for unary +"));
                        }
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
                            return VMResult::Error(ExcType::type_error("bad operand type for unary ~"));
                        }
                    }
                }

                // ============================================================
                // In-place Operations
                // ============================================================
                Opcode::InplaceAdd => {
                    if let Err(e) = self.inplace_add() {
                        return VMResult::Error(e);
                    }
                }

                // Other in-place ops use the same logic as binary ops for now
                Opcode::InplaceSub => {
                    if let Err(e) = self.binary_sub() {
                        return VMResult::Error(e);
                    }
                }

                Opcode::InplaceMul => {
                    if let Err(e) = self.binary_mult() {
                        return VMResult::Error(e);
                    }
                }

                Opcode::InplaceDiv => {
                    if let Err(e) = self.binary_div() {
                        return VMResult::Error(e);
                    }
                }

                Opcode::InplaceFloorDiv => {
                    if let Err(e) = self.binary_floordiv() {
                        return VMResult::Error(e);
                    }
                }

                Opcode::InplaceMod => {
                    self.binary_mod();
                }

                Opcode::InplacePow => {
                    if let Err(e) = self.binary_pow() {
                        return VMResult::Error(e);
                    }
                }

                Opcode::InplaceAnd => todo!("InplaceAnd not implemented"),
                Opcode::InplaceOr => todo!("InplaceOr not implemented"),
                Opcode::InplaceXor => todo!("InplaceXor not implemented"),
                Opcode::InplaceLShift => todo!("InplaceLShift not implemented"),
                Opcode::InplaceRShift => todo!("InplaceRShift not implemented"),

                // ============================================================
                // Collection Building
                // ============================================================
                Opcode::BuildList => {
                    let count = self.fetch_u16() as usize;
                    if let Err(e) = self.build_list(count) {
                        return VMResult::Error(e);
                    }
                }

                Opcode::BuildTuple => {
                    let count = self.fetch_u16() as usize;
                    if let Err(e) = self.build_tuple(count) {
                        return VMResult::Error(e);
                    }
                }

                Opcode::BuildDict => {
                    let count = self.fetch_u16() as usize;
                    if let Err(e) = self.build_dict(count) {
                        return VMResult::Error(e);
                    }
                }

                Opcode::BuildSet => {
                    let count = self.fetch_u16() as usize;
                    if let Err(e) = self.build_set(count) {
                        return VMResult::Error(e);
                    }
                }

                Opcode::BuildFString => {
                    todo!("BuildFString not implemented")
                }

                // ============================================================
                // Subscript & Attribute
                // ============================================================
                Opcode::BinarySubscr => {
                    let index = self.pop();
                    let obj = self.pop();
                    let result = obj.py_getitem(&index, self.heap, self.interns);
                    obj.drop_with_heap(self.heap);
                    index.drop_with_heap(self.heap);
                    match result {
                        Ok(v) => self.push(v),
                        Err(e) => return VMResult::Error(e),
                    }
                }

                Opcode::StoreSubscr => {
                    // Stack order: value, obj, index (TOS)
                    let index = self.pop();
                    let mut obj = self.pop();
                    let value = self.pop();
                    let result = obj.py_setitem(index, value, self.heap, self.interns);
                    obj.drop_with_heap(self.heap);
                    if let Err(e) = result {
                        return VMResult::Error(e);
                    }
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
                        Err(e) => return VMResult::Error(e),
                    }
                }

                Opcode::StoreAttr => {
                    let name_idx = self.fetch_u16();
                    let name_id = StringId::from_index(name_idx);
                    let obj = self.pop();
                    let value = self.pop();
                    let result = self.set_attr(&obj, name_id, value);
                    obj.drop_with_heap(self.heap);
                    if let Err(e) = result {
                        return VMResult::Error(e);
                    }
                }

                Opcode::DeleteAttr => {
                    todo!("DeleteAttr not implemented")
                }

                // ============================================================
                // Control Flow
                // ============================================================
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

                // ============================================================
                // Iteration
                // ============================================================
                Opcode::GetIter => {
                    let value = self.pop();
                    // Create a ForIterator from the value and store on heap
                    match ForIterator::new(value, self.heap, self.interns) {
                        Ok(iter) => match self.heap.allocate(HeapData::Iterator(iter)) {
                            Ok(heap_id) => self.push(Value::Ref(heap_id)),
                            Err(e) => return VMResult::Error(e.into()),
                        },
                        Err(e) => return VMResult::Error(e),
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
                            return VMResult::Error(RunError::internal("ForIter: expected iterator on stack"));
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
                                return VMResult::Error(e);
                            }
                        }
                    } else {
                        return VMResult::Error(RunError::internal("ForIter: expected iterator ref on stack"));
                    }
                }

                // ============================================================
                // Function Calls (Step 4)
                // ============================================================
                Opcode::CallFunction => {
                    let arg_count = self.fetch_u8() as usize;

                    // Pop arguments in reverse order (TOS is last arg)
                    let args = self.pop_n_args(arg_count);

                    // Pop the callable
                    let callable = self.pop();

                    // Call the function and handle the result
                    match self.call_function(callable, args) {
                        Ok(result) => self.push(result),
                        Err(err) => return VMResult::Error(err),
                    }
                }

                Opcode::CallFunctionKw => {
                    todo!("CallFunctionKw (Step 4)")
                }

                Opcode::CallMethod => {
                    todo!("CallMethod (Step 4)")
                }

                Opcode::CallExternal => {
                    todo!("CallExternal (Step 6)")
                }

                // ============================================================
                // Function Definition (Step 4)
                // ============================================================
                Opcode::MakeFunction => {
                    todo!("MakeFunction (Step 4)")
                }

                Opcode::MakeClosure => {
                    todo!("MakeClosure (Step 4)")
                }

                // ============================================================
                // Exception Handling (Step 5)
                // ============================================================
                Opcode::Raise => {
                    let exc = self.pop();
                    return VMResult::Error(self.make_exception(exc));
                }

                Opcode::RaiseFrom => {
                    todo!("RaiseFrom (Step 5)")
                }

                Opcode::Reraise => {
                    if let Some(exc) = self.current_exception.take() {
                        return VMResult::Error(self.make_exception(exc));
                    }
                    // No active exception - create a RuntimeError
                    let exc = SimpleException::new(
                        ExcType::RuntimeError,
                        Some("No active exception to re-raise".to_string()),
                    );
                    return VMResult::Error(exc.into());
                }

                Opcode::ClearException => {
                    if let Some(exc) = self.current_exception.take() {
                        exc.drop_with_heap(self.heap);
                    }
                }

                // ============================================================
                // Return
                // ============================================================
                Opcode::ReturnValue => {
                    let value = self.pop();
                    if self.frames.len() == 1 {
                        // Module-level return - we're done
                        return VMResult::Complete(value);
                    }
                    // Pop current frame and push return value
                    self.pop_frame();
                    self.push(value);
                }

                // ============================================================
                // Unpacking
                // ============================================================
                Opcode::UnpackSequence => {
                    let count = self.fetch_u8() as usize;
                    if let Err(e) = self.unpack_sequence(count) {
                        return VMResult::Error(e);
                    }
                }

                Opcode::UnpackEx => {
                    todo!("UnpackEx not implemented")
                }

                // ============================================================
                // Special
                // ============================================================
                Opcode::Nop => {
                    // No operation
                }
            }
        }
    }

    /// Resumes execution after an external call completes.
    ///
    /// Pushes the return value onto the stack and continues execution.
    pub fn resume(&mut self, result: Value) -> VMResult {
        self.push(result);
        self.run()
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
    fn pop_frame(&mut self) {
        let frame = self.frames.pop().expect("no frame to pop");
        // Clean up frame's stack region (locals are in namespace)
        while self.stack.len() > frame.stack_base {
            let value = self.stack.pop().unwrap();
            value.drop_with_heap(self.heap);
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
        // TODO: Implement py_contains on Value
        let rhs = self.pop(); // container
        let lhs = self.pop(); // item
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        let _ = negate;
        todo!("compare_in: py_contains not yet implemented")
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
        for chunk in items.chunks(2) {
            let key = chunk[0].clone_immediate();
            let value = chunk[1].clone_immediate();
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
    fn get_attr(&self, _obj: &Value, _name_id: StringId) -> Result<Value, RunError> {
        // TODO: Implement py_getattr on Value
        todo!("get_attr: py_getattr not yet implemented")
    }

    /// Sets an attribute on an object.
    fn set_attr(&mut self, _obj: &Value, _name_id: StringId, _value: Value) -> Result<(), RunError> {
        // TODO: Implement py_setattr on Value
        todo!("set_attr: py_setattr not yet implemented")
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

    /// Calls a callable value with the given arguments.
    fn call_function(&mut self, callable: Value, args: ArgValues) -> Result<Value, RunError> {
        match callable {
            Value::Builtin(builtin) => {
                // Call the builtin function
                let result = builtin.call(self.heap, args, self.interns, self.print_writer)?;
                Ok(result)
            }
            Value::Ref(heap_id) => {
                // Could be a closure or function - check heap
                match self.heap.get(heap_id) {
                    HeapData::Closure(_, _, _) | HeapData::FunctionDefaults(_, _) => {
                        // Drop the callable ref
                        callable.drop_with_heap(self.heap);
                        // Drop args since we're not using them
                        args.drop_with_heap(self.heap);
                        todo!("User-defined function calls not yet implemented")
                    }
                    _ => {
                        callable.drop_with_heap(self.heap);
                        args.drop_with_heap(self.heap);
                        Err(ExcType::type_error("object is not callable"))
                    }
                }
            }
            _ => {
                args.drop_with_heap(self.heap);
                Err(ExcType::type_error("object is not callable"))
            }
        }
    }

    // ========================================================================
    // Exception Handling
    // ========================================================================

    /// Creates a RunError from a Value that should be an exception.
    fn make_exception(&self, exc_value: Value) -> RunError {
        // For now, create a simple exception. Full traceback support in Step 5.
        if let Value::Ref(heap_id) = &exc_value {
            if let HeapData::Exception(exc) = self.heap.get(*heap_id) {
                // Clone the exception and convert to RunError via ExceptionRaise
                let exc_clone = exc.clone();
                let raise: ExceptionRaise = exc_clone.into();
                return raise.into();
            }
        }
        // Invalid exception value - create a TypeError
        SimpleException::new(
            ExcType::TypeError,
            Some("exceptions must derive from BaseException".to_string()),
        )
        .into()
    }
}
