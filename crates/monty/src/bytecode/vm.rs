//! Bytecode virtual machine for executing compiled Python code.
//!
//! The VM uses a stack-based execution model with an operand stack for computation
//! and a call stack for function frames. Each frame owns its instruction pointer (IP).

use crate::{
    args::{ArgValues, KwargsValues},
    builtins::Builtins,
    bytecode::{code::Code, op::Opcode},
    exception_private::{ExcType, ExceptionRaise, RawStackFrame, RunError, RunResult, SimpleException},
    for_iterator::ForIterator,
    fstring::{decode_format_spec, format_string, format_with_spec, ParsedFormatSpec},
    heap::{Heap, HeapData, HeapId},
    intern::{ExtFunctionId, FunctionId, Interns, StringId, MODULE_STRING_ID},
    io::PrintWriter,
    namespace::{NamespaceId, Namespaces, GLOBAL_NS_IDX},
    parse::CodeRange,
    resource::ResourceTracker,
    types::{Dict, List, PyTrait, Set, Str, Tuple, Type},
    value::{Attr, BitwiseOp, Value},
};

// ============================================================================
// Exception Handling Macro
// ============================================================================

/// Tries an operation and handles any exception using the VM's exception handler.
///
/// If the operation returns an error, passes it to `handle_exception`. If the
/// exception is caught by a Python handler, execution continues. Otherwise,
/// returns the error from the enclosing function.
///
/// This macro is used throughout the VM dispatch loop to handle operations
/// that may raise Python exceptions (e.g., `NameError`, `TypeError`).
macro_rules! try_catch {
    ($self:expr, $expr:expr) => {
        if let Err(e) = $expr {
            catch!($self, e);
        }
    };
}

/// Handles an already-created exception using the VM's exception handler.
///
/// If the exception is caught by a Python handler, execution continues.
/// Otherwise, returns the error from the enclosing function.
///
/// Use this when you have an error value directly (not wrapped in `Result`).
macro_rules! catch {
    ($self:expr, $err:expr) => {
        if let Some(result) = $self.handle_exception($err) {
            return Err(result);
        }
    };
}

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

    /// Stack of exceptions being handled for nested except blocks.
    ///
    /// When entering an except handler, the exception is pushed onto this stack.
    /// When exiting via `ClearException`, the top is popped. This allows nested
    /// except handlers to restore the outer exception context.
    exception_stack: Vec<Value>,

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

    /// Stack of exceptions being handled for nested except blocks.
    ///
    /// Used by bare `raise` to re-raise the current exception.
    /// When entering an except handler, the exception is pushed onto this stack.
    /// When exiting via `ClearException`, the top is popped. This allows nested
    /// except handlers to restore the outer exception context.
    exception_stack: Vec<Value>,

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
            exception_stack: Vec::new(),
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
        // Drop all exceptions in the exception stack
        for exc in self.exception_stack.drain(..) {
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
                Opcode::LoadLocal0 => try_catch!(self, self.load_local(0)),
                Opcode::LoadLocal1 => try_catch!(self, self.load_local(1)),
                Opcode::LoadLocal2 => try_catch!(self, self.load_local(2)),
                Opcode::LoadLocal3 => try_catch!(self, self.load_local(3)),
                // Variables - General Local Operations
                Opcode::LoadLocal => {
                    let slot = u16::from(self.fetch_u8());
                    try_catch!(self, self.load_local(slot));
                }
                Opcode::LoadLocalW => {
                    let slot = self.fetch_u16();
                    try_catch!(self, self.load_local(slot));
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
                    try_catch!(self, self.load_global(slot));
                }
                Opcode::StoreGlobal => {
                    let slot = self.fetch_u16();
                    self.store_global(slot);
                }
                // Variables - Cell Operations (closures)
                Opcode::LoadCell => {
                    let slot = self.fetch_u16();
                    try_catch!(self, self.load_cell(slot));
                }
                Opcode::StoreCell => {
                    let slot = self.fetch_u16();
                    self.store_cell(slot);
                }
                // Binary Operations - route through exception handling for tracebacks
                Opcode::BinaryAdd => try_catch!(self, self.binary_add()),
                Opcode::BinarySub => try_catch!(self, self.binary_sub()),
                Opcode::BinaryMul => try_catch!(self, self.binary_mult()),
                Opcode::BinaryDiv => try_catch!(self, self.binary_div()),
                Opcode::BinaryFloorDiv => try_catch!(self, self.binary_floordiv()),
                Opcode::BinaryMod => try_catch!(self, self.binary_mod()),
                Opcode::BinaryPow => try_catch!(self, self.binary_pow()),
                // Bitwise operations - only work on integers
                Opcode::BinaryAnd => try_catch!(self, self.binary_bitwise(BitwiseOp::And)),
                Opcode::BinaryOr => try_catch!(self, self.binary_bitwise(BitwiseOp::Or)),
                Opcode::BinaryXor => try_catch!(self, self.binary_bitwise(BitwiseOp::Xor)),
                Opcode::BinaryLShift => try_catch!(self, self.binary_bitwise(BitwiseOp::LShift)),
                Opcode::BinaryRShift => try_catch!(self, self.binary_bitwise(BitwiseOp::RShift)),
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
                Opcode::CompareIn => try_catch!(self, self.compare_in(false)),
                Opcode::CompareNotIn => try_catch!(self, self.compare_in(true)),
                Opcode::CompareModEq => try_catch!(self, self.compare_mod_eq()),
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
                    let value_type = value.py_type(Some(self.heap));
                    let result = match &value {
                        Value::Int(n) => Some(Value::Int(-n)),
                        Value::Float(f) => Some(Value::Float(-f)),
                        Value::Bool(b) => Some(Value::Int(if *b { -1 } else { 0 })),
                        _ => None,
                    };
                    value.drop_with_heap(self.heap);
                    if let Some(v) = result {
                        self.push(v);
                    } else {
                        catch!(self, ExcType::unary_type_error("-", value_type));
                    }
                }
                Opcode::UnaryPos => {
                    // Unary plus - typically a no-op for numbers
                    let value = self.pop();
                    let value_type = value.py_type(Some(self.heap));
                    let result = match &value {
                        Value::Int(_) | Value::Float(_) | Value::Bool(_) => Some(value.clone_immediate()),
                        _ => None,
                    };
                    value.drop_with_heap(self.heap);
                    if let Some(v) = result {
                        self.push(v);
                    } else {
                        catch!(self, ExcType::unary_type_error("+", value_type));
                    }
                }
                Opcode::UnaryInvert => {
                    // Bitwise NOT
                    let value = self.pop();
                    let value_type = value.py_type(Some(self.heap));
                    let result = match &value {
                        Value::Int(n) => Some(Value::Int(!n)),
                        Value::Bool(b) => Some(Value::Int(!i64::from(*b))),
                        _ => None,
                    };
                    value.drop_with_heap(self.heap);
                    if let Some(v) = result {
                        self.push(v);
                    } else {
                        catch!(self, ExcType::unary_type_error("~", value_type));
                    }
                }
                // In-place Operations - route through exception handling
                Opcode::InplaceAdd => try_catch!(self, self.inplace_add()),
                // Other in-place ops use the same logic as binary ops for now
                Opcode::InplaceSub => try_catch!(self, self.binary_sub()),
                Opcode::InplaceMul => try_catch!(self, self.binary_mult()),
                Opcode::InplaceDiv => try_catch!(self, self.binary_div()),
                Opcode::InplaceFloorDiv => try_catch!(self, self.binary_floordiv()),
                Opcode::InplaceMod => try_catch!(self, self.binary_mod()),
                Opcode::InplacePow => try_catch!(self, self.binary_pow()),
                Opcode::InplaceAnd => try_catch!(self, self.binary_bitwise(BitwiseOp::And)),
                Opcode::InplaceOr => try_catch!(self, self.binary_bitwise(BitwiseOp::Or)),
                Opcode::InplaceXor => try_catch!(self, self.binary_bitwise(BitwiseOp::Xor)),
                Opcode::InplaceLShift => try_catch!(self, self.binary_bitwise(BitwiseOp::LShift)),
                Opcode::InplaceRShift => try_catch!(self, self.binary_bitwise(BitwiseOp::RShift)),
                // Collection Building - route through exception handling
                Opcode::BuildList => {
                    let count = self.fetch_u16() as usize;
                    try_catch!(self, self.build_list(count));
                }
                Opcode::BuildTuple => {
                    let count = self.fetch_u16() as usize;
                    try_catch!(self, self.build_tuple(count));
                }
                Opcode::BuildDict => {
                    let count = self.fetch_u16() as usize;
                    try_catch!(self, self.build_dict(count));
                }
                Opcode::BuildSet => {
                    let count = self.fetch_u16() as usize;
                    try_catch!(self, self.build_set(count));
                }
                Opcode::FormatValue => {
                    let flags = self.fetch_u8();
                    try_catch!(self, self.format_value(flags));
                }
                Opcode::BuildFString => {
                    let count = self.fetch_u16() as usize;
                    try_catch!(self, self.build_fstring(count));
                }
                Opcode::ListExtend => {
                    try_catch!(self, self.list_extend());
                }
                Opcode::ListToTuple => {
                    try_catch!(self, self.list_to_tuple());
                }
                Opcode::DictMerge => {
                    let func_name_id = self.fetch_u16();
                    try_catch!(self, self.dict_merge(func_name_id));
                }
                // Subscript & Attribute - route through exception handling
                Opcode::BinarySubscr => {
                    let index = self.pop();
                    let obj = self.pop();
                    let result = obj.py_getitem(&index, self.heap, self.interns);
                    obj.drop_with_heap(self.heap);
                    index.drop_with_heap(self.heap);
                    match result {
                        Ok(v) => self.push(v),
                        Err(e) => catch!(self, e),
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
                    try_catch!(self, self.load_attr(name_id));
                }
                Opcode::StoreAttr => {
                    let name_idx = self.fetch_u16();
                    let name_id = StringId::from_index(name_idx);
                    try_catch!(self, self.store_attr(name_id));
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
                // Iteration - route through exception handling
                Opcode::GetIter => {
                    let value = self.pop();
                    // Create a ForIterator from the value and store on heap
                    match ForIterator::new(value, self.heap, self.interns) {
                        Ok(iter) => match self.heap.allocate(HeapData::Iterator(iter)) {
                            Ok(heap_id) => self.push(Value::Ref(heap_id)),
                            Err(e) => catch!(self, e.into()),
                        },
                        Err(e) => catch!(self, e),
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
                            Ok(Some(value)) => self.push(value),
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
                                catch!(self, e);
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
                        Ok(CallResult::UserFunction) => {} // Frame pushed, continue in VM loop
                        Ok(CallResult::ExternalCall(ext_id, args_vec)) => {
                            return Ok(VMSuccess::ExternalCall {
                                ext_function_id: ext_id,
                                args: args_vec,
                            });
                        }
                        Err(err) => catch!(self, err),
                    }
                }
                Opcode::CallFunctionKw => {
                    // Fetch operands: pos_count, kw_count, then kw_count name indices
                    let pos_count = self.fetch_u8() as usize;
                    let kw_count = self.fetch_u8() as usize;

                    // Read keyword name StringIds
                    let mut kwname_ids = Vec::with_capacity(kw_count);
                    for _ in 0..kw_count {
                        kwname_ids.push(StringId::from_index(self.fetch_u16()));
                    }

                    // Pop keyword values (TOS is last kwarg value)
                    let kw_values = self.pop_n(kw_count);

                    // Pop positional arguments
                    let pos_args = self.pop_n(pos_count);

                    // Pop the callable
                    let callable = self.pop();

                    // Build kwargs as Vec<(StringId, Value)>
                    let kwargs_inline: Vec<(StringId, Value)> = kwname_ids.into_iter().zip(kw_values).collect();

                    // Build ArgValues with both positional and keyword args
                    let args = if pos_args.is_empty() && kwargs_inline.is_empty() {
                        ArgValues::Empty
                    } else if pos_args.is_empty() {
                        ArgValues::Kwargs(KwargsValues::Inline(kwargs_inline))
                    } else {
                        ArgValues::ArgsKargs {
                            args: pos_args,
                            kwargs: KwargsValues::Inline(kwargs_inline),
                        }
                    };

                    // Call the function and handle the result
                    match self.call_function(callable, args) {
                        Ok(CallResult::Builtin(result)) => self.push(result),
                        Ok(CallResult::UserFunction) => {} // Frame pushed, continue
                        Ok(CallResult::ExternalCall(ext_id, args_vec)) => {
                            return Ok(VMSuccess::ExternalCall {
                                ext_function_id: ext_id,
                                args: args_vec,
                            });
                        }
                        Err(err) => catch!(self, err),
                    }
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
                        Err(err) => catch!(self, err),
                    }
                }
                Opcode::CallExternal => {
                    todo!("CallExternal")
                }
                Opcode::CallFunctionEx => {
                    let flags = self.fetch_u8();
                    let has_kwargs = (flags & 0x01) != 0;

                    // Pop kwargs dict if present
                    let kwargs = if has_kwargs { Some(self.pop()) } else { None };

                    // Pop args tuple
                    let args_tuple = self.pop();

                    // Pop callable
                    let callable = self.pop();

                    // Call the function with unpacked args
                    match self.call_function_ex(callable, args_tuple, kwargs) {
                        Ok(CallResult::Builtin(result)) => self.push(result),
                        Ok(CallResult::UserFunction) => {} // Frame pushed, continue
                        Ok(CallResult::ExternalCall(ext_id, args_vec)) => {
                            return Ok(VMSuccess::ExternalCall {
                                ext_function_id: ext_id,
                                args: args_vec,
                            });
                        }
                        Err(err) => catch!(self, err),
                    }
                }
                // Function Definition
                Opcode::MakeFunction => {
                    let func_idx = self.fetch_u16();
                    let defaults_count = self.fetch_u8() as usize;
                    let func_id = FunctionId::from_index(func_idx);

                    // Pop default values from stack (drain maintains order: first pushed = first in vec)
                    let defaults = self.pop_n(defaults_count);

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
                    // We use individual pops which reverses order, so we need to reverse back
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
                    // Reverse to get original order (individual pops reverse the order)
                    cells.reverse();

                    // Pop default values from stack (drain maintains order: first pushed = first in vec)
                    let defaults = self.pop_n(defaults_count);

                    // Create Closure on heap and push reference
                    let heap_id = self.heap.allocate(HeapData::Closure(func_id, cells, defaults))?;
                    self.push(Value::Ref(heap_id));
                }
                // Exception Handling
                Opcode::Raise => {
                    let exc = self.pop();
                    let error = self.make_exception(exc, true); // is_raise=true, hide caret
                    catch!(self, error);
                }
                Opcode::RaiseFrom => {
                    todo!("RaiseFrom")
                }
                Opcode::Reraise => {
                    // Pop the current exception from the stack to re-raise it
                    // If caught, handle_exception will push it back
                    let error = if let Some(exc) = self.exception_stack.pop() {
                        self.make_exception(exc, true) // is_raise=true for reraise
                    } else {
                        // No active exception - create a RuntimeError
                        SimpleException::new(
                            ExcType::RuntimeError,
                            Some("No active exception to reraise".to_string()),
                        )
                        .into()
                    };
                    catch!(self, error);
                }
                Opcode::ClearException => {
                    // Pop the current exception from the stack
                    // This restores the previous exception context (if any)
                    if let Some(exc) = self.exception_stack.pop() {
                        exc.drop_with_heap(self.heap);
                    }
                }
                Opcode::CheckExcMatch => {
                    // Stack: [exception, exc_type] -> [exception, bool]
                    let exc_type = self.pop();
                    let exception = self.peek();
                    let result = self.check_exc_match(exception, &exc_type);
                    exc_type.drop_with_heap(self.heap);
                    let result = result?;
                    self.push(Value::Bool(result));
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
                // Unpacking - route through exception handling
                Opcode::UnpackSequence => {
                    let count = self.fetch_u8() as usize;
                    try_catch!(self, self.unpack_sequence(count));
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

    /// Resumes execution after an external call raised an exception.
    ///
    /// Uses the exception handling mechanism to try to catch the exception.
    /// If caught, continues execution at the handler. If not, propagates the error.
    pub fn resume_with_exception(&mut self, error: RunError) -> Result<VMSuccess, RunError> {
        // Use the normal exception handling mechanism
        // handle_exception returns None if caught, Some(error) if not caught
        if let Some(uncaught_error) = self.handle_exception(error) {
            return Err(uncaught_error);
        }
        // Exception was caught, continue execution
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
            exception_stack: self.exception_stack,
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
            exception_stack: snapshot.exception_stack,
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
    ///
    /// Returns a NameError if the variable is undefined (never assigned).
    fn load_local(&mut self, slot: u16) -> RunResult<()> {
        let ns_idx = self.current_frame().namespace_idx;
        let namespace = self.namespaces.get(ns_idx);
        // Copy without incrementing refcount first (avoids borrow conflict)
        let value = namespace.get(NamespaceId::new(slot as usize)).copy_for_extend();

        // Check for undefined value - raise NameError if so
        if matches!(value, Value::Undefined) {
            let name = self.current_frame().code.local_name(slot);
            return Err(self.name_error(slot, name));
        }

        // Now we can safely increment refcount and push
        if let Value::Ref(id) = &value {
            self.heap.inc_ref(*id);
        }
        self.push(value);
        Ok(())
    }

    /// Creates a NameError for an undefined variable.
    fn name_error(&self, slot: u16, name: Option<StringId>) -> RunError {
        let name_str = match name {
            Some(id) => self.interns.get_str(id).to_string(),
            None => format!("<local {slot}>"),
        };
        ExcType::name_error(&name_str).into()
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
    ///
    /// Returns a NameError if the variable is undefined.
    fn load_global(&mut self, slot: u16) -> RunResult<()> {
        let namespace = self.namespaces.get(GLOBAL_NS_IDX);
        // Copy without incrementing refcount first (avoids borrow conflict)
        let value = namespace.get(NamespaceId::new(slot as usize)).copy_for_extend();

        // Check for undefined value - raise NameError if so
        if matches!(value, Value::Undefined) {
            // For globals, we'd need a global_names table too, but for now use a placeholder
            let name = self.current_frame().code.local_name(slot);
            return Err(self.name_error(slot, name));
        }

        // Now we can safely increment refcount and push
        if let Value::Ref(id) = &value {
            self.heap.inc_ref(*id);
        }
        self.push(value);
        Ok(())
    }

    /// Pops the top of stack and stores it in a global variable.
    fn store_global(&mut self, slot: u16) {
        let value = self.pop();
        let namespace = self.namespaces.get_mut(GLOBAL_NS_IDX);
        let ns_slot = NamespaceId::new(slot as usize);
        let old_value = std::mem::replace(namespace.get_mut(ns_slot), value);
        old_value.drop_with_heap(self.heap);
    }

    /// Loads an attribute from an object and pushes it onto the stack.
    ///
    /// Returns an AttributeError if the attribute doesn't exist.
    fn load_attr(&mut self, name_id: StringId) -> RunResult<()> {
        let obj = self.pop();
        let result = obj.py_get_attr(name_id, self.heap, self.interns);
        obj.drop_with_heap(self.heap);
        self.push(result?);
        Ok(())
    }

    /// Stores a value as an attribute on an object.
    ///
    /// Returns an AttributeError if the attribute cannot be set.
    fn store_attr(&mut self, name_id: StringId) -> RunResult<()> {
        let obj = self.pop();
        let value = self.pop();
        // py_set_attr takes ownership of value and drops it on error
        let result = obj.py_set_attr(name_id, value, self.heap, self.interns);
        obj.drop_with_heap(self.heap);
        result
    }

    /// Loads from a closure cell and pushes onto the stack.
    /// Loads from a closure cell and pushes onto the stack.
    ///
    /// Returns a NameError if the cell value is undefined (free variable not bound).
    fn load_cell(&mut self, slot: u16) -> RunResult<()> {
        let cell_id = self.current_frame().cells[slot as usize];
        // Copy without incrementing refcount first (avoids borrow conflict)
        let value = self.heap.get_cell_value(cell_id).copy_for_extend();

        // Check for undefined value - raise NameError for unbound free variable
        if matches!(value, Value::Undefined) {
            let name = self.current_frame().code.local_name(slot);
            return Err(self.free_var_error(name));
        }

        // Now we can safely increment refcount and push
        if let Value::Ref(id) = &value {
            self.heap.inc_ref(*id);
        }
        self.push(value);
        Ok(())
    }

    /// Creates a NameError for an unbound free variable.
    fn free_var_error(&self, name: Option<StringId>) -> RunError {
        let name_str = match name {
            Some(id) => self.interns.get_str(id).to_string(),
            None => "<free var>".to_string(),
        };
        ExcType::name_error_free_variable(&name_str).into()
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
        // Capture types before operation for error messages
        let lhs_type = lhs.py_type(Some(self.heap));
        let rhs_type = rhs.py_type(Some(self.heap));
        let result = lhs.py_add(&rhs, self.heap, self.interns);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Ok(Some(v)) => {
                self.push(v);
                Ok(())
            }
            Ok(None) => Err(ExcType::binary_type_error("+", lhs_type, rhs_type)),
            Err(e) => Err(e.into()),
        }
    }

    /// Binary subtraction with proper refcount handling.
    fn binary_sub(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let lhs_type = lhs.py_type(Some(self.heap));
        let rhs_type = rhs.py_type(Some(self.heap));
        let result = lhs.py_sub(&rhs, self.heap);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Ok(Some(v)) => {
                self.push(v);
                Ok(())
            }
            Ok(None) => Err(ExcType::binary_type_error("-", lhs_type, rhs_type)),
            Err(e) => Err(e.into()),
        }
    }

    /// Binary multiplication with proper refcount handling.
    fn binary_mult(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let lhs_type = lhs.py_type(Some(self.heap));
        let rhs_type = rhs.py_type(Some(self.heap));
        let result = lhs.py_mult(&rhs, self.heap, self.interns);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Ok(Some(v)) => {
                self.push(v);
                Ok(())
            }
            Ok(None) => Err(ExcType::binary_type_error("*", lhs_type, rhs_type)),
            Err(e) => Err(e),
        }
    }

    /// Binary division with proper refcount handling.
    fn binary_div(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let lhs_type = lhs.py_type(Some(self.heap));
        let rhs_type = rhs.py_type(Some(self.heap));
        let result = lhs.py_div(&rhs, self.heap);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Ok(Some(v)) => {
                self.push(v);
                Ok(())
            }
            Ok(None) => Err(ExcType::binary_type_error("/", lhs_type, rhs_type)),
            Err(e) => Err(e),
        }
    }

    /// Binary floor division with proper refcount handling.
    fn binary_floordiv(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let lhs_type = lhs.py_type(Some(self.heap));
        let rhs_type = rhs.py_type(Some(self.heap));
        let result = lhs.py_floordiv(&rhs, self.heap);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Ok(Some(v)) => {
                self.push(v);
                Ok(())
            }
            Ok(None) => Err(ExcType::binary_type_error("//", lhs_type, rhs_type)),
            Err(e) => Err(e),
        }
    }

    /// Binary modulo with proper refcount handling.
    fn binary_mod(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let lhs_type = lhs.py_type(Some(self.heap));
        let rhs_type = rhs.py_type(Some(self.heap));
        let result = lhs.py_mod(&rhs);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Some(v) => {
                self.push(v);
                Ok(())
            }
            None => Err(ExcType::binary_type_error("%", lhs_type, rhs_type)),
        }
    }

    /// Binary power with proper refcount handling.
    fn binary_pow(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        let lhs_type = lhs.py_type(Some(self.heap));
        let rhs_type = rhs.py_type(Some(self.heap));
        let result = lhs.py_pow(&rhs, self.heap);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        match result {
            Ok(Some(v)) => {
                self.push(v);
                Ok(())
            }
            Ok(None) => Err(ExcType::binary_type_error("**", lhs_type, rhs_type)),
            Err(e) => Err(e),
        }
    }

    /// Binary bitwise operation on integers.
    ///
    /// Pops two values, performs the bitwise operation, and pushes the result.
    fn binary_bitwise(&mut self, op: BitwiseOp) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();

        // Compute result before dropping operands (py_bitwise only reads values)
        let result = lhs.py_bitwise(&rhs, op, self.heap);

        // Drop operands before propagating error
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);

        self.push(result?);
        Ok(())
    }

    /// In-place addition (uses py_iadd for mutable containers, falls back to py_add).
    ///
    /// For mutable types like lists, `py_iadd` mutates in place and returns true.
    /// For immutable types, we fall back to regular addition.
    fn inplace_add(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let mut lhs = self.pop();

        // Capture types early for error messages (needed if fallback fails)
        let lhs_type = lhs.py_type(Some(self.heap));
        let rhs_type = rhs.py_type(Some(self.heap));

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
                Ok(None) => Err(ExcType::binary_type_error("+=", lhs_type, rhs_type)),
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
    /// Compares identity using `Value::is()` which compares IDs.
    ///
    /// Identity is determined by `Value::id()` which uses:
    /// - Fixed IDs for singletons (None, True, False, Ellipsis)
    /// - Interned string/bytes index for InternString/InternBytes
    /// - HeapId for heap-allocated values (Ref)
    /// - Value-based hashing for immediate types (Int, Float, Function, etc.)
    fn compare_is(&mut self, negate: bool) {
        let rhs = self.pop();
        let lhs = self.pop();

        let result = lhs.is(&rhs);

        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(if negate { !result } else { result }));
    }

    /// Membership test (in/not in).
    fn compare_in(&mut self, negate: bool) -> Result<(), RunError> {
        let container = self.pop(); // container (rhs)
        let item = self.pop(); // item to find (lhs)

        let result = container.py_contains(&item, self.heap, self.interns);

        item.drop_with_heap(self.heap);
        container.drop_with_heap(self.heap);

        let contained = result?;
        self.push(Value::Bool(if negate { !contained } else { contained }));
        Ok(())
    }

    /// Modulo equality comparison: a % b == k
    ///
    /// This is an optimization for patterns like `x % 3 == 0`. The constant k
    /// is stored in the constant pool and referenced by the u16 operand.
    fn compare_mod_eq(&mut self) -> Result<(), RunError> {
        let const_idx = self.fetch_u16();
        let k = self.current_frame().code.constants().get(const_idx).copy_for_extend();

        let rhs = self.pop(); // divisor (b)
        let lhs = self.pop(); // dividend (a)

        // Compute a % b
        let mod_result = match (&lhs, &rhs) {
            (Value::Int(a), Value::Int(b)) => {
                if *b == 0 {
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    return Err(ExcType::zero_division().into());
                }
                Some(Value::Int(a.rem_euclid(*b)))
            }
            _ => None,
        };

        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);

        match mod_result {
            Some(result) => {
                // Compare with k
                let is_equal = result.py_eq(&k, self.heap, self.interns);
                self.push(Value::Bool(is_equal));
                Ok(())
            }
            None => Err(ExcType::type_error("unsupported operand type(s) for %")),
        }
    }

    // ========================================================================
    // Collection Building
    // ========================================================================

    /// Builds a list from the top n stack values.
    fn build_list(&mut self, count: usize) -> Result<(), RunError> {
        let items = self.pop_n(count);
        let list = List::new(items);
        let heap_id = self.heap.allocate(HeapData::List(list))?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Builds a tuple from the top n stack values.
    fn build_tuple(&mut self, count: usize) -> Result<(), RunError> {
        let items = self.pop_n(count);
        let tuple = Tuple::new(items);
        let heap_id = self.heap.allocate(HeapData::Tuple(tuple))?;
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
        let heap_id = self.heap.allocate(HeapData::Dict(dict))?;
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
        let heap_id = self.heap.allocate(HeapData::Set(set))?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Builds an f-string by concatenating n string parts from the stack.
    fn build_fstring(&mut self, count: usize) -> Result<(), RunError> {
        let parts = self.pop_n(count);
        let mut result = String::new();

        for part in parts {
            // Each part should be a string (interned or heap-allocated)
            let part_str = part.py_str(self.heap, self.interns);
            result.push_str(&part_str);
            part.drop_with_heap(self.heap);
        }

        let heap_id = self.heap.allocate(HeapData::Str(Str::new(result)))?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Extends a list with items from an iterable.
    ///
    /// Stack: [list, iterable] -> [list]
    /// Pops the iterable, extends the list in place, leaves list on stack.
    fn list_extend(&mut self) -> Result<(), RunError> {
        let iterable = self.pop();
        let list_ref = self.pop();

        // Two-phase approach to avoid borrow conflicts:
        // Phase 1: Copy items without refcount changes
        let copied_items: Vec<Value> = match &iterable {
            Value::Ref(id) => match self.heap.get(*id) {
                HeapData::List(list) => list.as_vec().iter().map(Value::copy_for_extend).collect(),
                HeapData::Tuple(tuple) => tuple.as_vec().iter().map(Value::copy_for_extend).collect(),
                HeapData::Set(set) => set.storage().iter().map(Value::copy_for_extend).collect(),
                HeapData::Dict(dict) => dict.iter().map(|(k, _)| Value::copy_for_extend(k)).collect(),
                HeapData::Str(s) => {
                    // Need to allocate strings for each character
                    let chars: Vec<char> = s.as_str().chars().collect();
                    let mut items = Vec::with_capacity(chars.len());
                    for c in chars {
                        let heap_id = self.heap.allocate(HeapData::Str(Str::new(c.to_string())))?;
                        items.push(Value::Ref(heap_id));
                    }
                    items
                }
                _ => {
                    let type_ = iterable.py_type(Some(self.heap));
                    iterable.drop_with_heap(self.heap);
                    list_ref.drop_with_heap(self.heap);
                    return Err(ExcType::type_error_not_iterable(type_));
                }
            },
            Value::InternString(id) => {
                let s = self.interns.get_str(*id);
                let chars: Vec<char> = s.chars().collect();
                let mut items = Vec::with_capacity(chars.len());
                for c in chars {
                    let heap_id = self.heap.allocate(HeapData::Str(Str::new(c.to_string())))?;
                    items.push(Value::Ref(heap_id));
                }
                items
            }
            _ => {
                let type_ = iterable.py_type(Some(self.heap));
                iterable.drop_with_heap(self.heap);
                list_ref.drop_with_heap(self.heap);
                return Err(ExcType::type_error_not_iterable(type_));
            }
        };

        // Phase 2: Increment refcounts now that the borrow has ended
        for item in &copied_items {
            if let Value::Ref(id) = item {
                self.heap.inc_ref(*id);
            }
        }

        // Extend the list
        if let Value::Ref(id) = &list_ref {
            if let HeapData::List(list) = self.heap.get_mut(*id) {
                list.as_vec_mut().extend(copied_items);
            }
        }

        iterable.drop_with_heap(self.heap);
        self.push(list_ref);
        Ok(())
    }

    /// Converts a list to a tuple.
    ///
    /// Stack: [list] -> [tuple]
    fn list_to_tuple(&mut self) -> Result<(), RunError> {
        let list_ref = self.pop();

        // Phase 1: Copy items without refcount changes
        let copied_items: Vec<Value> = if let Value::Ref(id) = &list_ref {
            if let HeapData::List(list) = self.heap.get(*id) {
                list.as_vec().iter().map(Value::copy_for_extend).collect()
            } else {
                return Err(RunError::internal("ListToTuple: expected list"));
            }
        } else {
            return Err(RunError::internal("ListToTuple: expected list ref"));
        };

        // Phase 2: Increment refcounts now that the borrow has ended
        for item in &copied_items {
            if let Value::Ref(id) = item {
                self.heap.inc_ref(*id);
            }
        }

        list_ref.drop_with_heap(self.heap);

        let tuple = Tuple::new(copied_items);
        let heap_id = self.heap.allocate(HeapData::Tuple(tuple))?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Merges a mapping into a dict for **kwargs unpacking.
    ///
    /// Stack: [dict, mapping] -> [dict]
    /// Validates that mapping is a dict and that keys are strings.
    fn dict_merge(&mut self, func_name_id: u16) -> Result<(), RunError> {
        let mapping = self.pop();
        let dict_ref = self.pop();

        // Get function name for error messages
        let func_name = if func_name_id == 0xFFFF {
            "<unknown>".to_string()
        } else {
            self.interns.get_str(StringId::from_index(func_name_id)).to_string()
        };

        // Two-phase approach: copy items first, then inc refcounts
        // Phase 1: Copy key-value pairs without refcount changes
        // Check that mapping is a dict (Ref pointing to Dict)
        let copied_items: Vec<(Value, Value)> = if let Value::Ref(id) = &mapping {
            if let HeapData::Dict(dict) = self.heap.get(*id) {
                dict.iter()
                    .map(|(k, v)| (Value::copy_for_extend(k), Value::copy_for_extend(v)))
                    .collect()
            } else {
                let type_name = mapping.py_type(Some(self.heap)).to_string();
                mapping.drop_with_heap(self.heap);
                dict_ref.drop_with_heap(self.heap);
                return Err(ExcType::type_error_kwargs_not_mapping(&func_name, &type_name));
            }
        } else {
            let type_name = mapping.py_type(Some(self.heap)).to_string();
            mapping.drop_with_heap(self.heap);
            dict_ref.drop_with_heap(self.heap);
            return Err(ExcType::type_error_kwargs_not_mapping(&func_name, &type_name));
        };

        // Phase 2: Increment refcounts now that the borrow has ended
        for (key, value) in &copied_items {
            if let Value::Ref(id) = key {
                self.heap.inc_ref(*id);
            }
            if let Value::Ref(id) = value {
                self.heap.inc_ref(*id);
            }
        }

        // Merge into the dict, validating string keys
        let dict_id = if let Value::Ref(id) = &dict_ref {
            *id
        } else {
            mapping.drop_with_heap(self.heap);
            dict_ref.drop_with_heap(self.heap);
            return Err(RunError::internal("DictMerge: expected dict ref"));
        };

        for (key, value) in copied_items {
            // Validate key is a string (InternString or heap-allocated Str)
            let is_string = match &key {
                Value::InternString(_) => true,
                Value::Ref(id) => matches!(self.heap.get(*id), HeapData::Str(_)),
                _ => false,
            };
            if !is_string {
                key.drop_with_heap(self.heap);
                value.drop_with_heap(self.heap);
                mapping.drop_with_heap(self.heap);
                dict_ref.drop_with_heap(self.heap);
                return Err(ExcType::type_error_kwargs_nonstring_key());
            }

            // Get the string key for error messages (needed before moving key into closure)
            let key_str = match &key {
                Value::InternString(id) => self.interns.get_str(*id).to_string(),
                Value::Ref(id) => {
                    if let HeapData::Str(s) = self.heap.get(*id) {
                        s.as_str().to_string()
                    } else {
                        "<unknown>".to_string()
                    }
                }
                _ => "<unknown>".to_string(),
            };

            // Use with_entry_mut to avoid borrow conflict: takes data out temporarily
            let result = self.heap.with_entry_mut(dict_id, |heap, data| {
                if let HeapData::Dict(dict) = data {
                    dict.set(key, value, heap, self.interns)
                } else {
                    Err(RunError::internal("DictMerge: entry is not a Dict"))
                }
            });

            // If set returned Some, the key already existed (duplicate kwarg)
            if let Some(old_value) = result? {
                old_value.drop_with_heap(self.heap);
                mapping.drop_with_heap(self.heap);
                dict_ref.drop_with_heap(self.heap);
                return Err(ExcType::type_error_multiple_values(&func_name, &key_str));
            }
        }

        mapping.drop_with_heap(self.heap);
        self.push(dict_ref);
        Ok(())
    }

    /// Formats a value for f-string interpolation.
    ///
    /// Flags encoding:
    /// - bits 0-1: conversion (0=none, 1=str, 2=repr, 3=ascii)
    /// - bit 2: has format spec on stack
    ///
    /// Python f-string formatting order:
    /// 1. Apply format spec to original value (type-specific formatting)
    /// 2. Apply conversion flag to the result
    ///
    /// However, conversion flags like !s, !r, !a are applied BEFORE formatting
    /// if the value would be repr'd. The key insight is:
    /// - No conversion: format the original value type
    /// - !s conversion: convert to str first, then format as string
    /// - !r conversion: convert to repr first, then format as string
    /// - !a conversion: convert to ascii repr first, then format as string
    fn format_value(&mut self, flags: u8) -> Result<(), RunError> {
        let conversion = flags & 0x03;
        let has_format_spec = (flags & 0x04) != 0;

        // Pop format spec if present (pushed before value, so popped after)
        let format_spec = if has_format_spec {
            let spec_value = self.pop();
            Some(spec_value)
        } else {
            None
        };

        let value = self.pop();

        // Format with spec applied to original value type, or convert and format as string
        let formatted = if let Some(spec_value) = format_spec {
            // Get the parsed format spec
            let spec = self.get_format_spec(&spec_value, &value)?;

            // Format based on value type and conversion flag
            let result = match conversion {
                // No conversion - format original value
                0 => format_with_spec(&value, &spec, self.heap, self.interns)?,
                // !s - convert to str, format as string
                1 => {
                    let s = value.py_str(self.heap, self.interns);
                    format_string(&s, &spec)?
                }
                // !r - convert to repr, format as string
                2 => {
                    let s = value.py_repr(self.heap, self.interns);
                    format_string(&s, &spec)?
                }
                // !a - convert to ascii, format as string
                3 => {
                    let s = self.py_ascii(&value);
                    format_string(&s, &spec)?
                }
                _ => format_with_spec(&value, &spec, self.heap, self.interns)?,
            };

            spec_value.drop_with_heap(self.heap);
            result
        } else {
            // No format spec - just convert based on conversion flag
            match conversion {
                0 => value.py_str(self.heap, self.interns).into_owned(),
                1 => value.py_str(self.heap, self.interns).into_owned(),
                2 => value.py_repr(self.heap, self.interns).into_owned(),
                3 => self.py_ascii(&value),
                _ => value.py_str(self.heap, self.interns).into_owned(),
            }
        };

        value.drop_with_heap(self.heap);

        let heap_id = self.heap.allocate(HeapData::Str(Str::new(formatted)))?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Gets a ParsedFormatSpec from a format spec value.
    ///
    /// The `value_for_error` parameter is used to include the value type in error messages.
    fn get_format_spec(&self, spec_value: &Value, value_for_error: &Value) -> Result<ParsedFormatSpec, RunError> {
        match spec_value {
            Value::Int(n) if *n < 0 => {
                // Decode the encoded format spec
                let encoded = ((-*n) - 1) as u64;
                Ok(decode_format_spec(encoded))
            }
            _ => {
                // Dynamic format spec - parse the string
                let spec_str = spec_value.py_str(self.heap, self.interns);
                let value_type = value_for_error.py_type(Some(self.heap));
                spec_str.parse::<ParsedFormatSpec>().map_err(|invalid| {
                    RunError::Exc(
                        SimpleException::new(
                            ExcType::ValueError,
                            Some(format!(
                                "Invalid format specifier '{invalid}' for object of type '{value_type}'"
                            )),
                        )
                        .into(),
                    )
                })
            }
        }
    }

    /// Applies ASCII conversion (escapes non-ASCII characters).
    fn py_ascii(&self, value: &Value) -> String {
        use std::fmt::Write;
        let repr = value.py_repr(self.heap, self.interns);
        let mut result = String::new();
        for c in repr.chars() {
            if c.is_ascii() {
                result.push(c);
            } else {
                // Escape non-ASCII characters
                let code = c as u32;
                if code <= 0xFF {
                    write!(result, "\\x{code:02x}")
                } else if code <= 0xFFFF {
                    write!(result, "\\u{code:04x}")
                } else {
                    write!(result, "\\U{code:08x}")
                }
                .expect("string write should be infallible");
            }
        }
        result
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

    /// Calls a function with unpacked args tuple and optional kwargs dict.
    ///
    /// This is used for `f(*args)` and `f(**kwargs)` style calls.
    fn call_function_ex(
        &mut self,
        callable: Value,
        args_tuple: Value,
        kwargs: Option<Value>,
    ) -> Result<CallResult, RunError> {
        // Two-phase approach for extracting positional args to avoid borrow conflicts
        // Phase 1: Copy items without refcount changes
        let copied_args: Vec<Value> = if let Value::Ref(id) = &args_tuple {
            if let HeapData::Tuple(tuple) = self.heap.get(*id) {
                tuple.as_vec().iter().map(Value::copy_for_extend).collect()
            } else {
                callable.drop_with_heap(self.heap);
                args_tuple.drop_with_heap(self.heap);
                if let Some(k) = kwargs {
                    k.drop_with_heap(self.heap);
                }
                return Err(RunError::internal("CallFunctionEx: expected tuple for args"));
            }
        } else {
            callable.drop_with_heap(self.heap);
            args_tuple.drop_with_heap(self.heap);
            if let Some(k) = kwargs {
                k.drop_with_heap(self.heap);
            }
            return Err(RunError::internal("CallFunctionEx: expected tuple ref for args"));
        };

        // Phase 2: Increment refcounts for positional args
        for arg in &copied_args {
            if let Value::Ref(id) = arg {
                self.heap.inc_ref(*id);
            }
        }

        // Build ArgValues from positional args and optional kwargs
        let args = if let Some(kwargs_ref) = kwargs {
            // Extract kwargs dict items with two-phase approach
            // Phase 1: Copy items
            let copied_kwargs: Vec<(Value, Value)> = if let Value::Ref(id) = &kwargs_ref {
                if let HeapData::Dict(dict) = self.heap.get(*id) {
                    dict.iter()
                        .map(|(k, v)| (Value::copy_for_extend(k), Value::copy_for_extend(v)))
                        .collect()
                } else {
                    callable.drop_with_heap(self.heap);
                    args_tuple.drop_with_heap(self.heap);
                    kwargs_ref.drop_with_heap(self.heap);
                    for arg in copied_args {
                        arg.drop_with_heap(self.heap);
                    }
                    return Err(RunError::internal("CallFunctionEx: expected dict for kwargs"));
                }
            } else {
                callable.drop_with_heap(self.heap);
                args_tuple.drop_with_heap(self.heap);
                kwargs_ref.drop_with_heap(self.heap);
                for arg in copied_args {
                    arg.drop_with_heap(self.heap);
                }
                return Err(RunError::internal("CallFunctionEx: expected dict ref for kwargs"));
            };

            // Phase 2: Increment refcounts for kwargs
            for (k, v) in &copied_kwargs {
                if let Value::Ref(id) = k {
                    self.heap.inc_ref(*id);
                }
                if let Value::Ref(id) = v {
                    self.heap.inc_ref(*id);
                }
            }

            // Clean up the kwargs dict ref (we cloned the contents)
            kwargs_ref.drop_with_heap(self.heap);

            let kwargs_values = if copied_kwargs.is_empty() {
                KwargsValues::Empty
            } else {
                let kwargs_dict = Dict::from_pairs(copied_kwargs, self.heap, self.interns)?;
                KwargsValues::Dict(kwargs_dict)
            };

            if copied_args.is_empty() && matches!(kwargs_values, KwargsValues::Empty) {
                ArgValues::Empty
            } else if copied_args.is_empty() {
                ArgValues::Kwargs(kwargs_values)
            } else {
                ArgValues::ArgsKargs {
                    args: copied_args,
                    kwargs: kwargs_values,
                }
            }
        } else {
            // No kwargs
            match copied_args.len() {
                0 => ArgValues::Empty,
                1 => ArgValues::One(copied_args.into_iter().next().unwrap()),
                2 => {
                    let mut iter = copied_args.into_iter();
                    ArgValues::Two(iter.next().unwrap(), iter.next().unwrap())
                }
                _ => ArgValues::ArgsKargs {
                    args: copied_args,
                    kwargs: KwargsValues::Empty,
                },
            }
        };

        // Clean up the args tuple ref (we cloned the contents)
        args_tuple.drop_with_heap(self.heap);

        // Now call the function with the built ArgValues
        self.call_function(callable, args)
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

    /// Returns the current frame's name for traceback generation.
    ///
    /// Returns the function name for user-defined functions, or `<module>` for
    /// module-level code.
    fn current_frame_name(&self) -> StringId {
        let frame = self.current_frame();
        match frame.function_id {
            Some(func_id) => self.interns.get_function(func_id).name.name_id,
            None => MODULE_STRING_ID,
        }
    }

    /// Creates a `RawStackFrame` for the current execution point.
    ///
    /// Used when raising exceptions to capture traceback information.
    fn make_stack_frame(&self) -> RawStackFrame {
        RawStackFrame::new(self.current_position(), self.current_frame_name(), None)
    }

    /// Attaches initial frame information to an error if it doesn't have any.
    ///
    /// Only sets the innermost frame if the exception doesn't already have one.
    /// Caller frames are added separately during exception propagation.
    ///
    /// Uses the `hide_caret` flag from `ExceptionRaise` to determine whether to show
    /// the caret marker in the traceback. This flag is set by error creators that know
    /// whether CPython would show a caret for this specific error type.
    fn attach_frame_to_error(&self, error: RunError) -> RunError {
        match error {
            RunError::Exc(mut exc) => {
                if exc.frame.is_none() {
                    let mut frame = self.make_stack_frame();
                    // Use the hide_caret flag from the error (set by error creators)
                    frame.hide_caret = exc.hide_caret;
                    exc.frame = Some(frame);
                }
                RunError::Exc(exc)
            }
            RunError::UncatchableExc(mut exc) => {
                if exc.frame.is_none() {
                    exc.frame = Some(self.make_stack_frame());
                }
                RunError::UncatchableExc(exc)
            }
            RunError::Internal(_) => error,
        }
    }

    // ========================================================================
    // Exception Handling
    // ========================================================================

    /// Creates a RunError from a Value that should be an exception.
    ///
    /// Takes ownership of the exception value and drops it properly.
    /// The `is_raise` flag indicates if this is from a `raise` statement (hide caret).
    fn make_exception(&mut self, exc_value: Value, is_raise: bool) -> RunError {
        let simple_exc = match &exc_value {
            // Exception instance on heap
            Value::Ref(heap_id) => {
                if let HeapData::Exception(exc) = self.heap.get(*heap_id) {
                    // Clone the exception
                    let exc_clone = exc.clone();
                    // Drop the value with proper heap cleanup
                    exc_value.drop_with_heap(self.heap);
                    exc_clone
                } else {
                    // Not an exception type
                    exc_value.drop_with_heap(self.heap);
                    SimpleException::new(
                        ExcType::TypeError,
                        Some("exceptions must derive from BaseException".to_string()),
                    )
                }
            }
            // Exception type (e.g., `raise ValueError` instead of `raise ValueError()`)
            // Instantiate with no message
            Value::Builtin(Builtins::ExcType(exc_type)) => SimpleException::new(*exc_type, None),
            // Invalid exception value
            _ => {
                exc_value.drop_with_heap(self.heap);
                SimpleException::new(
                    ExcType::TypeError,
                    Some("exceptions must derive from BaseException".to_string()),
                )
            }
        };

        // Create frame with appropriate hide_caret setting
        let frame = if is_raise {
            RawStackFrame::from_raise(self.current_position(), self.current_frame_name())
        } else {
            self.make_stack_frame()
        };

        RunError::Exc(ExceptionRaise {
            exc: simple_exc,
            frame: Some(frame),
            hide_caret: false,
        })
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
    fn handle_exception(&mut self, mut error: RunError) -> Option<RunError> {
        // Ensure exception has initial frame info
        error = self.attach_frame_to_error(error);

        // For uncatchable exceptions (ResourceError like RecursionError),
        // we still need to unwind the stack to collect all frames for the traceback
        if matches!(error, RunError::UncatchableExc(_) | RunError::Internal(_)) {
            return Some(self.unwind_for_traceback(error));
        }

        // Only catchable exceptions can be handled
        let exc_info = match &error {
            RunError::Exc(exc) => exc.clone(),
            RunError::UncatchableExc(_) | RunError::Internal(_) => unreachable!(),
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

                // Push exception onto the exception_stack for bare raise
                // This allows nested except handlers to restore outer exception context
                self.exception_stack.push(exc_value);

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

            // Get the call site position before popping frame
            // This is where the caller invoked the function that's failing
            let call_position = self.current_frame().call_position;

            // Pop this frame
            self.pop_frame();

            // Add caller frame info to traceback (if we have call position)
            if let Some(pos) = call_position {
                let frame_name = self.current_frame_name();
                match &mut error {
                    RunError::Exc(exc) => exc.add_caller_frame(pos, frame_name),
                    RunError::UncatchableExc(exc) => exc.add_caller_frame(pos, frame_name),
                    RunError::Internal(_) => {}
                }
            }

            // Update instruction_ip for the new frame
            self.instruction_ip = self
                .current_frame()
                .call_position
                .map_or(0, |p| p.start().line as usize);
        }
    }

    /// Unwinds the call stack to collect all frames for a traceback.
    ///
    /// Used for uncatchable exceptions (like RecursionError) that can't be handled
    /// but still need a complete traceback showing all active call frames.
    fn unwind_for_traceback(&mut self, mut error: RunError) -> RunError {
        // Pop frames and add caller frame info to the traceback
        while self.frames.len() > 1 {
            // Get the call site position before popping frame
            let call_position = self.current_frame().call_position;

            // Pop this frame (cleans up namespace, etc.)
            self.pop_frame();

            // Add caller frame info to traceback
            if let Some(pos) = call_position {
                let frame_name = self.current_frame_name();
                match &mut error {
                    RunError::Exc(exc) => exc.add_caller_frame(pos, frame_name),
                    RunError::UncatchableExc(exc) => exc.add_caller_frame(pos, frame_name),
                    RunError::Internal(_) => {}
                }
            }
        }
        error
    }

    /// Creates an exception Value from exception info.
    ///
    /// Allocates an Exception on the heap and returns a Value::Ref to it.
    fn create_exception_value(&mut self, exc: &ExceptionRaise) -> Result<Value, RunError> {
        let exception = exc.exc.clone();
        let heap_id = self.heap.allocate(HeapData::Exception(exception))?;
        Ok(Value::Ref(heap_id))
    }

    /// Checks if an exception matches an exception type for except clause matching.
    ///
    /// Validates that `exc_type` is a valid exception type (ExcType or tuple of ExcTypes).
    /// Returns `Ok(true)` if exception matches, `Ok(false)` if not, or `Err` if exc_type is invalid.
    fn check_exc_match(&self, exception: &Value, exc_type: &Value) -> Result<bool, RunError> {
        let exc_type_enum = exception.py_type(Some(self.heap));
        self.check_exc_match_inner(exc_type_enum, exc_type)
    }

    /// Inner recursive helper for check_exc_match that handles tuples.
    fn check_exc_match_inner(&self, exc_type_enum: Type, exc_type: &Value) -> Result<bool, RunError> {
        match exc_type {
            // Valid exception type
            Value::Builtin(Builtins::ExcType(handler_type)) => {
                // Check if exception is an instance of handler_type
                Ok(matches!(exc_type_enum, Type::Exception(et) if et.is_subclass_of(*handler_type)))
            }
            // Tuple of exception types
            Value::Ref(id) => {
                if let HeapData::Tuple(tuple) = self.heap.get(*id) {
                    for v in tuple.as_vec() {
                        if self.check_exc_match_inner(exc_type_enum, v)? {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                } else {
                    // Not a tuple - invalid exception type
                    Err(ExcType::except_invalid_type_error())
                }
            }
            // Any other type is invalid for except clause
            _ => Err(ExcType::except_invalid_type_error()),
        }
    }
}
