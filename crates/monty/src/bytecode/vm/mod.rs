//! Bytecode virtual machine for executing compiled Python code.
//!
//! The VM uses a stack-based execution model with an operand stack for computation
//! and a call stack for function frames. Each frame owns its instruction pointer (IP).

mod binary;
mod call;
mod collections;
mod compare;
mod exceptions;
mod format;

use std::cmp::Ordering;

use call::CallResult;

use crate::{
    args::ArgValues,
    bytecode::{code::Code, op::Opcode},
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    for_iterator::ForIterator,
    heap::{Heap, HeapData, HeapId},
    intern::{ExtFunctionId, FunctionId, Interns, StringId},
    io::PrintWriter,
    namespace::{GLOBAL_NS_IDX, NamespaceId, Namespaces},
    parse::CodeRange,
    resource::ResourceTracker,
    types::{LongInt, PyTrait},
    value::{BitwiseOp, Value},
};

/// Tries an operation and handles exceptions, reloading cached frame state.
///
/// Use this in the main run loop where `cached_frame`
/// are used. After catching an exception, reloads the cache since the handler
/// may be in a different frame.
macro_rules! try_catch_sync {
    ($self:expr, $cached_frame:ident, $expr:expr) => {
        if let Err(e) = $expr {
            if let Some(result) = $self.handle_exception(e) {
                return Err(result);
            }
            // Exception was caught - handler may be in different frame, reload cache
            reload_cache!($self, $cached_frame);
        }
    };
}

/// Handles an exception and reloads cached frame state if caught.
///
/// Use this in the main run loop where `cached_frame`
/// are used. After catching an exception, reloads the cache since the handler
/// may be in a different frame.
///
/// Wrapped in a block to allow use in match arm expressions.
macro_rules! catch_sync {
    ($self:expr, $cached_frame:ident, $err:expr) => {{
        if let Some(result) = $self.handle_exception($err) {
            return Err(result);
        }
        // Exception was caught - handler may be in different frame, reload cache
        reload_cache!($self, $cached_frame);
    }};
}

/// Fetches a byte from bytecode using cached code/ip, advancing ip.
///
/// Used in the run loop for fast operand fetching without frame access.
macro_rules! fetch_byte {
    ($cached_frame:expr) => {{
        let byte = $cached_frame.code.bytecode()[$cached_frame.ip];
        $cached_frame.ip += 1;
        byte
    }};
}

/// Fetches a u8 operand using cached code/ip.
macro_rules! fetch_u8 {
    ($cached_frame:expr) => {
        fetch_byte!($cached_frame)
    };
}

/// Fetches an i8 operand using cached code/ip.
macro_rules! fetch_i8 {
    ($cached_frame:expr) => {{ i8::from_ne_bytes([fetch_byte!($cached_frame)]) }};
}

/// Fetches a u16 operand (little-endian) using cached code/ip.
macro_rules! fetch_u16 {
    ($cached_frame:expr) => {{
        let lo = $cached_frame.code.bytecode()[$cached_frame.ip];
        let hi = $cached_frame.code.bytecode()[$cached_frame.ip + 1];
        $cached_frame.ip += 2;
        u16::from_le_bytes([lo, hi])
    }};
}

/// Fetches an i16 operand (little-endian) using cached code/ip.
macro_rules! fetch_i16 {
    ($cached_frame:expr) => {{
        let lo = $cached_frame.code.bytecode()[$cached_frame.ip];
        let hi = $cached_frame.code.bytecode()[$cached_frame.ip + 1];
        $cached_frame.ip += 2;
        i16::from_le_bytes([lo, hi])
    }};
}

/// Reloads cached frame state from the current frame.
///
/// Call this after any operation that modifies the frame stack (calls, returns,
/// exception handling).
macro_rules! reload_cache {
    ($self:expr, $cached_frame:ident) => {{
        $cached_frame = $self.new_cached_frame();
    }};
}

/// Applies a relative jump offset to the cached IP.
///
/// Uses checked arithmetic to safely compute the new IP, panicking if the
/// jump would result in a negative or overflowing instruction pointer.
macro_rules! jump_relative {
    ($ip:expr, $offset:expr) => {{
        let ip_i64 = i64::try_from($ip).expect("instruction pointer exceeds i64");
        let new_ip = ip_i64 + i64::from($offset);
        $ip = usize::try_from(new_ip).expect("jump resulted in negative or overflowing IP");
    }};
}

/// Result of VM execution.
pub enum FrameExit {
    /// Execution completed successfully with a return value.
    Return(Value),

    /// Execution paused for an external function call.
    ///
    /// The caller should execute the external function and call `resume()`
    /// with the result.
    ExternalCall {
        /// ID of the external function to call.
        ext_function_id: ExtFunctionId,
        /// Arguments for the external function (includes both positional and keyword args).
        args: ArgValues,
    },
}

/// A single function activation record.
///
/// Each frame represents one level in the call stack and owns its own
/// instruction pointer. This design avoids sync bugs on call/return.
#[derive(Debug)]
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

/// Cached state of the VM derived from the current frame as an optimization
#[derive(Debug, Copy, Clone)]
pub struct CachedFrame<'code> {
    /// Bytecode being executed.
    code: &'code Code,

    /// Instruction pointer within this frame's bytecode.
    ip: usize,

    /// Namespace index for this frame's locals.
    namespace_idx: NamespaceId,
}

impl<'code> From<&CallFrame<'code>> for CachedFrame<'code> {
    fn from(frame: &CallFrame<'code>) -> Self {
        Self {
            code: frame.code,
            ip: frame.ip,
            namespace_idx: frame.namespace_idx,
        }
    }
}

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
    pub fn run_module(&mut self, code: &'a Code) -> Result<FrameExit, RunError> {
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
    ///
    /// Uses locally cached `code` and `ip` variables to avoid repeated
    /// `frames.last_mut().expect()` calls during operand fetching. The cache
    /// is reloaded after any operation that modifies the frame stack.
    pub fn run(&mut self) -> Result<FrameExit, RunError> {
        // Cache frame state locally to avoid repeated frames.last_mut() calls.
        // The Code reference has lifetime 'a (lives in Interns), independent of frame borrow.
        let mut cached_frame: CachedFrame<'a> = self.new_cached_frame();

        loop {
            // Check time limit and trigger GC if needed at each instruction.
            // For NoLimitTracker, these are inlined no-ops that compile away.
            self.heap.tracker_mut().check_time()?;

            if self.heap.should_gc() {
                // Sync IP before GC for safety
                self.current_frame_mut().ip = cached_frame.ip;
                self.run_gc();
            }

            // Track instruction IP for exception table lookup
            self.instruction_ip = cached_frame.ip;

            // Fetch opcode using cached values (no frame access)
            let opcode = {
                let byte = cached_frame.code.bytecode()[cached_frame.ip];
                cached_frame.ip += 1;
                Opcode::try_from(byte).expect("invalid opcode in bytecode")
            };

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
                    let idx = fetch_u16!(cached_frame);
                    // Copy without incrementing refcount first (avoids borrow conflict)
                    let value = cached_frame.code.constants().get(idx).copy_for_extend();
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
                    let n = fetch_i8!(cached_frame);
                    self.push(Value::Int(i64::from(n)));
                }
                // Variables - Specialized Local Loads (no operand)
                Opcode::LoadLocal0 => try_catch_sync!(self, cached_frame, self.load_local(&cached_frame, 0)),
                Opcode::LoadLocal1 => try_catch_sync!(self, cached_frame, self.load_local(&cached_frame, 1)),
                Opcode::LoadLocal2 => try_catch_sync!(self, cached_frame, self.load_local(&cached_frame, 2)),
                Opcode::LoadLocal3 => try_catch_sync!(self, cached_frame, self.load_local(&cached_frame, 3)),
                // Variables - General Local Operations
                Opcode::LoadLocal => {
                    let slot = u16::from(fetch_u8!(cached_frame));
                    try_catch_sync!(self, cached_frame, self.load_local(&cached_frame, slot));
                }
                Opcode::LoadLocalW => {
                    let slot = fetch_u16!(cached_frame);
                    try_catch_sync!(self, cached_frame, self.load_local(&cached_frame, slot));
                }
                Opcode::StoreLocal => {
                    let slot = u16::from(fetch_u8!(cached_frame));
                    self.store_local(&cached_frame, slot);
                }
                Opcode::StoreLocalW => {
                    let slot = fetch_u16!(cached_frame);
                    self.store_local(&cached_frame, slot);
                }
                Opcode::DeleteLocal => {
                    let slot = u16::from(fetch_u8!(cached_frame));
                    self.delete_local(&cached_frame, slot);
                }
                // Variables - Global Operations
                Opcode::LoadGlobal => {
                    let slot = fetch_u16!(cached_frame);
                    try_catch_sync!(self, cached_frame, self.load_global(slot));
                }
                Opcode::StoreGlobal => {
                    let slot = fetch_u16!(cached_frame);
                    self.store_global(slot);
                }
                // Variables - Cell Operations (closures)
                Opcode::LoadCell => {
                    let slot = fetch_u16!(cached_frame);
                    try_catch_sync!(self, cached_frame, self.load_cell(slot));
                }
                Opcode::StoreCell => {
                    let slot = fetch_u16!(cached_frame);
                    self.store_cell(slot);
                }
                // Binary Operations - route through exception handling for tracebacks
                Opcode::BinaryAdd => try_catch_sync!(self, cached_frame, self.binary_add()),
                Opcode::BinarySub => try_catch_sync!(self, cached_frame, self.binary_sub()),
                Opcode::BinaryMul => try_catch_sync!(self, cached_frame, self.binary_mult()),
                Opcode::BinaryDiv => try_catch_sync!(self, cached_frame, self.binary_div()),
                Opcode::BinaryFloorDiv => try_catch_sync!(self, cached_frame, self.binary_floordiv()),
                Opcode::BinaryMod => try_catch_sync!(self, cached_frame, self.binary_mod()),
                Opcode::BinaryPow => try_catch_sync!(self, cached_frame, self.binary_pow()),
                // Bitwise operations - only work on integers
                Opcode::BinaryAnd => try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::And)),
                Opcode::BinaryOr => try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::Or)),
                Opcode::BinaryXor => try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::Xor)),
                Opcode::BinaryLShift => {
                    try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::LShift));
                }
                Opcode::BinaryRShift => {
                    try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::RShift));
                }
                Opcode::BinaryMatMul => todo!("BinaryMatMul not implemented"),
                // Comparison Operations
                Opcode::CompareEq => self.compare_eq(),
                Opcode::CompareNe => self.compare_ne(),
                Opcode::CompareLt => self.compare_ord(Ordering::is_lt),
                Opcode::CompareLe => self.compare_ord(Ordering::is_le),
                Opcode::CompareGt => self.compare_ord(Ordering::is_gt),
                Opcode::CompareGe => self.compare_ord(Ordering::is_ge),
                Opcode::CompareIs => self.compare_is(false),
                Opcode::CompareIsNot => self.compare_is(true),
                Opcode::CompareIn => try_catch_sync!(self, cached_frame, self.compare_in(false)),
                Opcode::CompareNotIn => try_catch_sync!(self, cached_frame, self.compare_in(true)),
                Opcode::CompareModEq => {
                    let const_idx = fetch_u16!(cached_frame);
                    let k = cached_frame.code.constants().get(const_idx);
                    try_catch_sync!(self, cached_frame, self.compare_mod_eq(k));
                }
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
                    let value_type = value.py_type(self.heap);
                    let result: Result<Option<Value>, crate::resource::ResourceError> = match &value {
                        Value::Int(n) => {
                            // Use checked_neg to handle i64::MIN overflow
                            if let Some(negated) = n.checked_neg() {
                                Ok(Some(Value::Int(negated)))
                            } else {
                                // i64::MIN negated overflows to LongInt
                                let li = -LongInt::from(*n);
                                li.into_value(self.heap).map(Some)
                            }
                        }
                        Value::Float(f) => Ok(Some(Value::Float(-f))),
                        Value::Bool(b) => Ok(Some(Value::Int(if *b { -1 } else { 0 }))),
                        Value::Ref(id) => {
                            if let HeapData::LongInt(li) = self.heap.get(*id) {
                                let negated = -LongInt::new(li.inner().clone());
                                negated.into_value(self.heap).map(Some)
                            } else {
                                Ok(None) // Not a LongInt - unsupported type
                            }
                        }
                        _ => Ok(None), // Unsupported type
                    };
                    value.drop_with_heap(self.heap);
                    match result {
                        Ok(Some(v)) => self.push(v),
                        Ok(None) => {
                            catch_sync!(self, cached_frame, ExcType::unary_type_error("-", value_type));
                        }
                        Err(e) => {
                            catch_sync!(self, cached_frame, RunError::from(e));
                        }
                    }
                }
                Opcode::UnaryPos => {
                    // Unary plus - typically a no-op for numbers
                    let value = self.pop();
                    let value_type = value.py_type(self.heap);
                    let result = match &value {
                        Value::Int(_) | Value::Float(_) | Value::Bool(_) => Some(value.clone_immediate()),
                        _ => None,
                    };
                    value.drop_with_heap(self.heap);
                    if let Some(v) = result {
                        self.push(v);
                    } else {
                        catch_sync!(self, cached_frame, ExcType::unary_type_error("+", value_type));
                    }
                }
                Opcode::UnaryInvert => {
                    // Bitwise NOT
                    let value = self.pop();
                    let value_type = value.py_type(self.heap);
                    let result: Result<Option<Value>, crate::resource::ResourceError> = match &value {
                        Value::Int(n) => Ok(Some(Value::Int(!n))),
                        Value::Bool(b) => Ok(Some(Value::Int(!i64::from(*b)))),
                        Value::Ref(id) => {
                            if let HeapData::LongInt(li) = self.heap.get(*id) {
                                // LongInt bitwise NOT: ~x = -(x + 1)
                                let inverted = -(li.inner() + 1i32);
                                LongInt::new(inverted).into_value(self.heap).map(Some)
                            } else {
                                Ok(None) // Not a LongInt - unsupported type
                            }
                        }
                        _ => Ok(None), // Unsupported type
                    };
                    value.drop_with_heap(self.heap);
                    match result {
                        Ok(Some(v)) => self.push(v),
                        Ok(None) => {
                            catch_sync!(self, cached_frame, ExcType::unary_type_error("~", value_type));
                        }
                        Err(e) => {
                            catch_sync!(self, cached_frame, RunError::from(e));
                        }
                    }
                }
                // In-place Operations - route through exception handling
                Opcode::InplaceAdd => try_catch_sync!(self, cached_frame, self.inplace_add()),
                // Other in-place ops use the same logic as binary ops for now
                Opcode::InplaceSub => try_catch_sync!(self, cached_frame, self.binary_sub()),
                Opcode::InplaceMul => try_catch_sync!(self, cached_frame, self.binary_mult()),
                Opcode::InplaceDiv => try_catch_sync!(self, cached_frame, self.binary_div()),
                Opcode::InplaceFloorDiv => try_catch_sync!(self, cached_frame, self.binary_floordiv()),
                Opcode::InplaceMod => try_catch_sync!(self, cached_frame, self.binary_mod()),
                Opcode::InplacePow => try_catch_sync!(self, cached_frame, self.binary_pow()),
                Opcode::InplaceAnd => {
                    try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::And));
                }
                Opcode::InplaceOr => try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::Or)),
                Opcode::InplaceXor => {
                    try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::Xor));
                }
                Opcode::InplaceLShift => {
                    try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::LShift));
                }
                Opcode::InplaceRShift => {
                    try_catch_sync!(self, cached_frame, self.binary_bitwise(BitwiseOp::RShift));
                }
                // Collection Building - route through exception handling
                Opcode::BuildList => {
                    let count = usize::from(fetch_u16!(cached_frame));
                    try_catch_sync!(self, cached_frame, self.build_list(count));
                }
                Opcode::BuildTuple => {
                    let count = usize::from(fetch_u16!(cached_frame));
                    try_catch_sync!(self, cached_frame, self.build_tuple(count));
                }
                Opcode::BuildDict => {
                    let count = usize::from(fetch_u16!(cached_frame));
                    try_catch_sync!(self, cached_frame, self.build_dict(count));
                }
                Opcode::BuildSet => {
                    let count = usize::from(fetch_u16!(cached_frame));
                    try_catch_sync!(self, cached_frame, self.build_set(count));
                }
                Opcode::FormatValue => {
                    let flags = fetch_u8!(cached_frame);
                    try_catch_sync!(self, cached_frame, self.format_value(flags));
                }
                Opcode::BuildFString => {
                    let count = usize::from(fetch_u16!(cached_frame));
                    try_catch_sync!(self, cached_frame, self.build_fstring(count));
                }
                Opcode::ListExtend => {
                    try_catch_sync!(self, cached_frame, self.list_extend());
                }
                Opcode::ListToTuple => {
                    try_catch_sync!(self, cached_frame, self.list_to_tuple());
                }
                Opcode::DictMerge => {
                    let func_name_id = fetch_u16!(cached_frame);
                    try_catch_sync!(self, cached_frame, self.dict_merge(func_name_id));
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
                        Err(e) => catch_sync!(self, cached_frame, e),
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
                    let name_idx = fetch_u16!(cached_frame);
                    let name_id = StringId::from_index(name_idx);
                    try_catch_sync!(self, cached_frame, self.load_attr(name_id));
                }
                Opcode::StoreAttr => {
                    let name_idx = fetch_u16!(cached_frame);
                    let name_id = StringId::from_index(name_idx);
                    try_catch_sync!(self, cached_frame, self.store_attr(name_id));
                }
                Opcode::DeleteAttr => {
                    todo!("DeleteAttr not implemented")
                }
                // Control Flow - use cached_frame.ip directly for jumps
                Opcode::Jump => {
                    let offset = fetch_i16!(cached_frame);
                    jump_relative!(cached_frame.ip, offset);
                }
                Opcode::JumpIfTrue => {
                    let offset = fetch_i16!(cached_frame);
                    let cond = self.pop();
                    if cond.py_bool(self.heap, self.interns) {
                        jump_relative!(cached_frame.ip, offset);
                    }
                    cond.drop_with_heap(self.heap);
                }
                Opcode::JumpIfFalse => {
                    let offset = fetch_i16!(cached_frame);
                    let cond = self.pop();
                    if !cond.py_bool(self.heap, self.interns) {
                        jump_relative!(cached_frame.ip, offset);
                    }
                    cond.drop_with_heap(self.heap);
                }
                Opcode::JumpIfTrueOrPop => {
                    let offset = fetch_i16!(cached_frame);
                    if self.peek().py_bool(self.heap, self.interns) {
                        jump_relative!(cached_frame.ip, offset);
                    } else {
                        let value = self.pop();
                        value.drop_with_heap(self.heap);
                    }
                }
                Opcode::JumpIfFalseOrPop => {
                    let offset = fetch_i16!(cached_frame);
                    if self.peek().py_bool(self.heap, self.interns) {
                        let value = self.pop();
                        value.drop_with_heap(self.heap);
                    } else {
                        jump_relative!(cached_frame.ip, offset);
                    }
                }
                // Iteration - route through exception handling
                Opcode::GetIter => {
                    let value = self.pop();
                    // Create a ForIterator from the value and store on heap
                    match ForIterator::new(value, self.heap, self.interns) {
                        Ok(iter) => match self.heap.allocate(HeapData::Iterator(iter)) {
                            Ok(heap_id) => self.push(Value::Ref(heap_id)),
                            Err(e) => catch_sync!(self, cached_frame, e.into()),
                        },
                        Err(e) => catch_sync!(self, cached_frame, e),
                    }
                }
                Opcode::ForIter => {
                    let offset = fetch_i16!(cached_frame);
                    // Peek at the iterator on TOS and extract heap_id
                    let Value::Ref(heap_id) = *self.peek() else {
                        return Err(RunError::internal("ForIter: expected iterator ref on stack"));
                    };

                    // Use advance_iterator which avoids std::mem::replace overhead
                    // by using a two-phase approach: read state, get value, update index
                    match self.heap.advance_iterator(heap_id, self.interns) {
                        Ok(Some(value)) => self.push(value),
                        Ok(None) => {
                            // Iterator exhausted - pop it and jump to end
                            let iter = self.pop();
                            iter.drop_with_heap(self.heap);
                            jump_relative!(cached_frame.ip, offset);
                        }
                        Err(e) => {
                            // Error during iteration (e.g., dict size changed)
                            let iter = self.pop();
                            iter.drop_with_heap(self.heap);
                            catch_sync!(self, cached_frame, e);
                        }
                    }
                }
                // Function Calls - sync IP before call, reload cache after frame changes
                Opcode::CallFunction => {
                    let arg_count = usize::from(fetch_u8!(cached_frame));

                    // Sync IP before call (call_function may access frame for traceback)
                    self.current_frame_mut().ip = cached_frame.ip;

                    match self.exec_call_function(arg_count) {
                        Ok(CallResult::Push(result)) => self.push(result),
                        Ok(CallResult::FramePushed) => reload_cache!(self, cached_frame),
                        Ok(CallResult::External(ext_id, args)) => {
                            return Ok(FrameExit::ExternalCall {
                                ext_function_id: ext_id,
                                args,
                            });
                        }
                        Err(err) => catch_sync!(self, cached_frame, err),
                    }
                }
                Opcode::CallBuiltinFunction => {
                    // Fetch operands: builtin_id (u8) + arg_count (u8)
                    let builtin_id = fetch_u8!(cached_frame);
                    let arg_count = usize::from(fetch_u8!(cached_frame));

                    match self.exec_call_builtin_function(builtin_id, arg_count) {
                        Ok(result) => self.push(result),
                        // IP sync deferred to error path (no frame push possible)
                        Err(err) => catch_sync!(self, cached_frame, err),
                    }
                }
                Opcode::CallBuiltinType => {
                    // Fetch operands: type_id (u8) + arg_count (u8)
                    let type_id = fetch_u8!(cached_frame);
                    let arg_count = usize::from(fetch_u8!(cached_frame));

                    match self.exec_call_builtin_type(type_id, arg_count) {
                        Ok(result) => self.push(result),
                        // IP sync deferred to error path (no frame push possible)
                        Err(err) => catch_sync!(self, cached_frame, err),
                    }
                }
                Opcode::CallFunctionKw => {
                    // Fetch operands: pos_count, kw_count, then kw_count name indices
                    let pos_count = usize::from(fetch_u8!(cached_frame));
                    let kw_count = usize::from(fetch_u8!(cached_frame));

                    // Read keyword name StringIds
                    let mut kwname_ids = Vec::with_capacity(kw_count);
                    for _ in 0..kw_count {
                        kwname_ids.push(StringId::from_index(fetch_u16!(cached_frame)));
                    }

                    // Sync IP before call (call_function may access frame for traceback)
                    self.current_frame_mut().ip = cached_frame.ip;

                    match self.exec_call_function_kw(pos_count, kwname_ids) {
                        Ok(CallResult::Push(result)) => self.push(result),
                        Ok(CallResult::FramePushed) => reload_cache!(self, cached_frame),
                        Ok(CallResult::External(ext_id, args)) => {
                            return Ok(FrameExit::ExternalCall {
                                ext_function_id: ext_id,
                                args,
                            });
                        }
                        Err(err) => catch_sync!(self, cached_frame, err),
                    }
                }
                Opcode::CallMethod => {
                    // CallMethod: u16 name_id, u8 arg_count
                    // Stack: [obj, arg1, arg2, ..., argN] -> [result]
                    let name_idx = fetch_u16!(cached_frame);
                    let arg_count = usize::from(fetch_u8!(cached_frame));
                    let name_id = StringId::from_index(name_idx);

                    match self.exec_call_method(name_id, arg_count) {
                        Ok(result) => self.push(result),
                        // IP sync deferred to error path (no frame push possible)
                        Err(err) => catch_sync!(self, cached_frame, err),
                    }
                }
                Opcode::CallFunctionExtended => {
                    let flags = fetch_u8!(cached_frame);
                    let has_kwargs = (flags & 0x01) != 0;

                    // Sync IP before call
                    self.current_frame_mut().ip = cached_frame.ip;

                    match self.exec_call_function_extended(has_kwargs) {
                        Ok(CallResult::Push(result)) => self.push(result),
                        Ok(CallResult::FramePushed) => reload_cache!(self, cached_frame),
                        Ok(CallResult::External(ext_id, args)) => {
                            return Ok(FrameExit::ExternalCall {
                                ext_function_id: ext_id,
                                args,
                            });
                        }
                        Err(err) => catch_sync!(self, cached_frame, err),
                    }
                }
                // Function Definition
                Opcode::MakeFunction => {
                    let func_idx = fetch_u16!(cached_frame);
                    let defaults_count = usize::from(fetch_u8!(cached_frame));
                    let func_id = FunctionId::from_index(func_idx);

                    if defaults_count == 0 {
                        // No defaults - use inline Value::Function (no heap allocation)
                        self.push(Value::DefFunction(func_id));
                    } else {
                        // Pop default values from stack (drain maintains order: first pushed = first in vec)
                        let defaults = self.pop_n(defaults_count);

                        // Create FunctionDefaults on heap and push reference
                        let heap_id = self.heap.allocate(HeapData::FunctionDefaults(func_id, defaults))?;
                        self.push(Value::Ref(heap_id));
                    }
                }
                Opcode::MakeClosure => {
                    let func_idx = fetch_u16!(cached_frame);
                    let defaults_count = usize::from(fetch_u8!(cached_frame));
                    let cell_count = usize::from(fetch_u8!(cached_frame));
                    let func_id = FunctionId::from_index(func_idx);

                    // Pop cells from stack (pushed after defaults, so on top)
                    // Cells are Value::Ref pointing to HeapData::Cell
                    // We use individual pops which reverses order, so we need to reverse back
                    let mut cells = Vec::with_capacity(cell_count);
                    for _ in 0..cell_count {
                        // mut needed for dec_ref_forget when ref-count-panic feature is enabled
                        #[cfg_attr(not(feature = "ref-count-return"), expect(unused_mut))]
                        let mut cell_val = self.pop();
                        match &cell_val {
                            Value::Ref(heap_id) => {
                                // Keep the reference - the Closure will own the HeapId
                                cells.push(*heap_id);
                                // Mark the Value as dereferenced since Closure takes ownership
                                // of the reference count (we don't call drop_with_heap because
                                // we're not decrementing the refcount, just transferring it)
                                #[cfg(feature = "ref-count-panic")]
                                cell_val.dec_ref_forget();
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
                    catch_sync!(self, cached_frame, error);
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
                        SimpleException::new_msg(ExcType::RuntimeError, "No active exception to reraise").into()
                    };
                    catch_sync!(self, cached_frame, error);
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
                // Return - reload cache after popping frame
                Opcode::ReturnValue => {
                    let value = self.pop();
                    if self.frames.len() == 1 {
                        // Module-level return - we're done
                        return Ok(FrameExit::Return(value));
                    }
                    // Pop current frame and push return value
                    self.pop_frame();
                    self.push(value);
                    // Reload cache from parent frame
                    reload_cache!(self, cached_frame);
                }
                // Unpacking - route through exception handling
                Opcode::UnpackSequence => {
                    let count = usize::from(fetch_u8!(cached_frame));
                    try_catch_sync!(self, cached_frame, self.unpack_sequence(count));
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
    pub fn resume(&mut self, result: Value) -> Result<FrameExit, RunError> {
        self.push(result);
        self.run()
    }

    /// Resumes execution after an external call raised an exception.
    ///
    /// Uses the exception handling mechanism to try to catch the exception.
    /// If caught, continues execution at the handler. If not, propagates the error.
    pub fn resume_with_exception(&mut self, error: RunError) -> Result<FrameExit, RunError> {
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
                    Some(func_id) => &interns.get_function(func_id).code,
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
    // Stack Operations
    // ========================================================================

    /// Pushes a value onto the operand stack.
    #[inline]
    pub(super) fn push(&mut self, value: Value) {
        self.stack.push(value);
    }

    /// Pops a value from the operand stack.
    #[inline]
    pub(super) fn pop(&mut self) -> Value {
        self.stack.pop().expect("stack underflow")
    }

    /// Peeks at the top of the operand stack without removing it.
    #[inline]
    pub(super) fn peek(&self) -> &Value {
        self.stack.last().expect("stack underflow")
    }

    /// Pops n values from the stack in reverse order (first popped is last in vec).
    pub(super) fn pop_n(&mut self, n: usize) -> Vec<Value> {
        let start = self.stack.len() - n;
        self.stack.drain(start..).collect()
    }

    // ========================================================================
    // Frame Operations
    // ========================================================================

    /// Returns a reference to the current (topmost) call frame.
    #[inline]
    pub(super) fn current_frame(&self) -> &CallFrame<'a> {
        self.frames.last().expect("no active frame")
    }

    /// Creates a new cached frame from the current frame.
    #[inline]
    pub(super) fn new_cached_frame(&self) -> CachedFrame<'a> {
        self.current_frame().into()
    }

    /// Returns a mutable reference to the current call frame.
    #[inline]
    pub(super) fn current_frame_mut(&mut self) -> &mut CallFrame<'a> {
        self.frames.last_mut().expect("no active frame")
    }

    /// Pops the current frame from the call stack.
    ///
    /// Cleans up the frame's stack region and namespace (except for global namespace).
    pub(super) fn pop_frame(&mut self) {
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

    /// Runs garbage collection with proper GC roots.
    ///
    /// GC roots include values in namespaces, the operand stack, and exception stack.
    fn run_gc(&mut self) {
        // Collect roots from all reachable values
        let stack_roots = self.stack.iter().filter_map(Value::ref_id);
        let exc_roots = self.exception_stack.iter().filter_map(Value::ref_id);
        let ns_roots = self.namespaces.iter_heap_ids();

        // Collect all roots into a vec to avoid lifetime issues
        let roots: Vec<HeapId> = stack_roots.chain(exc_roots).chain(ns_roots).collect();

        self.heap.collect_garbage(roots);
    }

    /// Returns the current source position for traceback generation.
    ///
    /// Uses `instruction_ip` which is set at the start of each instruction in the run loop,
    /// ensuring accurate position tracking even when using cached IP for bytecode fetching.
    pub(super) fn current_position(&self) -> CodeRange {
        let frame = self.current_frame();
        // Use instruction_ip which points to the start of the current instruction
        // (set at the beginning of each loop iteration in run())
        frame
            .code
            .location_for_offset(self.instruction_ip)
            .map(crate::bytecode::code::LocationEntry::range)
            .unwrap_or_default()
    }

    // ========================================================================
    // Variable Operations
    // ========================================================================

    /// Loads a local variable and pushes it onto the stack.
    ///
    /// Returns a NameError if the variable is undefined (never assigned).
    fn load_local(&mut self, cached_frame: &CachedFrame<'a>, slot: u16) -> RunResult<()> {
        let namespace = self.namespaces.get(cached_frame.namespace_idx);
        // Copy without incrementing refcount first (avoids borrow conflict)
        let value = namespace.get(NamespaceId::new(usize::from(slot))).copy_for_extend();

        // Check for undefined value - raise NameError if so
        if matches!(value, Value::Undefined) {
            let name = cached_frame.code.local_name(slot);
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
    fn store_local(&mut self, cached_frame: &CachedFrame<'a>, slot: u16) {
        let value = self.pop();
        let namespace = self.namespaces.get_mut(cached_frame.namespace_idx);
        let ns_slot = NamespaceId::new(usize::from(slot));
        let old_value = std::mem::replace(namespace.get_mut(ns_slot), value);
        old_value.drop_with_heap(self.heap);
    }

    /// Deletes a local variable (sets it to Undefined).
    fn delete_local(&mut self, cached_frame: &CachedFrame<'a>, slot: u16) {
        let namespace = self.namespaces.get_mut(cached_frame.namespace_idx);
        let ns_slot = NamespaceId::new(usize::from(slot));
        let old_value = std::mem::replace(namespace.get_mut(ns_slot), Value::Undefined);
        old_value.drop_with_heap(self.heap);
    }

    /// Loads a global variable and pushes it onto the stack.
    ///
    /// Returns a NameError if the variable is undefined.
    fn load_global(&mut self, slot: u16) -> RunResult<()> {
        let namespace = self.namespaces.get(GLOBAL_NS_IDX);
        // Copy without incrementing refcount first (avoids borrow conflict)
        let value = namespace.get(NamespaceId::new(usize::from(slot))).copy_for_extend();

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
        let ns_slot = NamespaceId::new(usize::from(slot));
        let old_value = std::mem::replace(namespace.get_mut(ns_slot), value);
        old_value.drop_with_heap(self.heap);
    }

    /// Loads from a closure cell and pushes onto the stack.
    ///
    /// Returns a NameError if the cell value is undefined (free variable not bound).
    fn load_cell(&mut self, slot: u16) -> RunResult<()> {
        let cell_id = self.current_frame().cells[usize::from(slot)];
        // get_cell_value already clones with proper refcount via clone_with_heap
        let value = self.heap.get_cell_value(cell_id);

        // Check for undefined value - raise NameError for unbound free variable
        if matches!(value, Value::Undefined) {
            let name = self.current_frame().code.local_name(slot);
            return Err(self.free_var_error(name));
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
        let cell_id = self.current_frame().cells[usize::from(slot)];
        self.heap.set_cell_value(cell_id, value);
    }
}
