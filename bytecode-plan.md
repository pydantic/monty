# Monty Bytecode VM Migration Plan

<!-- NOTE: Do not use markdown tables in this document. They are hard to read and hard to maintain. Use bullet lists instead. -->

## Design Decisions (Confirmed)

- **Bytecode encoding**: Variable-width `Vec<u8>` — better cache utilization, 2x smaller bytecode
- **Compilation timing**: Eager (at prepare phase) — simpler, no runtime compilation overhead
- **Async heap model**: Shared heap — single heap for all tasks, RC + periodic mark-sweep GC for cycles (existing behavior)

---

## Executive Summary

Migrate Monty from a recursive tree-walking interpreter to a stack-based bytecode VM. This eliminates the complex snapshot/resume machinery while improving performance through better cache locality and reduced function call overhead.

**Key Goals:**
1. Simplify pause/resume for external calls (state = IP + stacks)
2. Improve performance (eliminate recursion, better cache locality)
3. Enable future JIT compilation
4. Enable future async support

**What We Keep:**
- `Value` enum (16-byte hybrid design)
- `Heap<T>` with reference counting and free list
- `Interns` for string/bytes interning
- `Function` struct (stores metadata, will also store bytecode)
- `Namespaces` stack (with modifications)

**What We Replace:**
- Recursive `evaluate_use()`/`execute_node()` → VM loop with instruction pointer
- `SnapshotTracker`/`ClauseState`/`FunctionFrame` → explicit `CallFrame` stack
- `ext_return_values` cache → direct stack manipulation on resume

---

## Phase 1: Define Bytecode Format

### 1.1 Opcode Enum

Bytecode is stored as raw `Vec<u8>` for cache efficiency. The `Opcode` enum is a pure
discriminant with no data - operands are fetched separately from the byte stream.

```rust
/// Opcode discriminant - just identifies the instruction type.
/// Operands (if any) follow in the bytecode stream and are fetched separately.
///
/// With `#[repr(u8)]`, each opcode is exactly 1 byte.
#[repr(u8)]
pub enum Opcode {
    // === Stack Operations (no operand) ===
    Pop,                    // Discard TOS
    Dup,                    // Duplicate TOS
    Rot2,                   // Swap top two: [a, b] → [b, a]
    Rot3,                   // Rotate top three: [a, b, c] → [c, a, b]

    // === Constants & Literals ===
    LoadConst,              // + u16 const_id: push constant from pool
    LoadNone,               // Push None
    LoadTrue,               // Push True
    LoadFalse,              // Push False
    LoadSmallInt,           // + i8: push small integer (-128 to 127)

    // === Variables ===
    // Specialized no-operand versions for common slots (hot path)
    LoadLocal0,             // Push local slot 0 (often 'self')
    LoadLocal1,             // Push local slot 1
    LoadLocal2,             // Push local slot 2
    LoadLocal3,             // Push local slot 3
    // General versions with operand
    LoadLocal,              // + u8 slot: push local variable
    LoadLocalW,             // + u16 slot: push local (wide, slot > 255)
    StoreLocal,             // + u8 slot: pop and store to local
    StoreLocalW,            // + u16 slot: store local (wide)
    LoadGlobal,             // + u16 slot: push from global namespace
    StoreGlobal,            // + u16 slot: store to global
    LoadCell,               // + u16 slot: load from closure cell
    StoreCell,              // + u16 slot: store to closure cell
    DeleteLocal,            // + u8 slot: delete local variable

    // === Binary Operations (no operand) ===
    BinaryAdd,
    BinarySub,
    BinaryMul,
    BinaryDiv,
    BinaryFloorDiv,
    BinaryMod,
    BinaryPow,
    BinaryAnd,              // Bitwise &
    BinaryOr,               // Bitwise |
    BinaryXor,              // Bitwise ^
    BinaryLShift,
    BinaryRShift,
    BinaryMatMul,

    // === Comparison Operations (no operand) ===
    CompareEq,
    CompareNe,
    CompareLt,
    CompareLe,
    CompareGt,
    CompareGe,
    CompareIs,
    CompareIsNot,
    CompareIn,
    CompareNotIn,

    // === Unary Operations (no operand) ===
    UnaryNot,
    UnaryNeg,
    UnaryPos,
    UnaryInvert,            // Bitwise ~

    // === In-place Operations (no operand) ===
    InplaceAdd,
    InplaceSub,
    InplaceMul,
    InplaceDiv,
    InplaceFloorDiv,
    InplaceMod,
    InplacePow,
    InplaceAnd,
    InplaceOr,
    InplaceXor,
    InplaceLShift,
    InplaceRShift,

    // === Collection Building ===
    BuildList,              // + u16 count: pop n items, build list
    BuildTuple,             // + u16 count: pop n items, build tuple
    BuildDict,              // + u16 count: pop 2n items (k/v pairs), build dict
    BuildSet,               // + u16 count: pop n items, build set
    BuildFString,           // + u16 count: pop n parts, concatenate

    // === Subscript & Attribute ===
    BinarySubscr,           // a[b]: pop index, pop obj, push result
    StoreSubscr,            // a[b] = c: pop value, pop index, pop obj
    DeleteSubscr,           // del a[b]: pop index, pop obj
    LoadAttr,               // + u16 name_id: pop obj, push obj.attr
    StoreAttr,              // + u16 name_id: pop value, pop obj, set obj.attr
    DeleteAttr,             // + u16 name_id: pop obj, delete obj.attr

    // === Function Calls ===
    CallFunction,           // + u8 arg_count: call TOS with n positional args
    CallFunctionKw,         // + u8 pos_count + u8 kw_count: call with pos and kw args
    CallMethod,             // + u16 name_id + u8 arg_count: call method
    CallExternal,           // + u16 func_id + u8 arg_count: external call (pauses VM)

    // === Control Flow ===
    Jump,                   // + i16 offset: unconditional relative jump
    JumpIfTrue,             // + i16 offset: jump if TOS truthy, always pop
    JumpIfFalse,            // + i16 offset: jump if TOS falsy, always pop
    JumpIfTrueOrPop,        // + i16 offset: jump if TOS truthy (keep), else pop
    JumpIfFalseOrPop,       // + i16 offset: jump if TOS falsy (keep), else pop

    // === Iteration ===
    GetIter,                // Convert TOS to iterator
    ForIter,                // + i16 offset: advance iterator or jump to end

    // === Function Definition ===
    MakeFunction,           // + u16 func_id: create function object
    MakeClosure,            // + u16 func_id + u8 cell_count: create closure

    // === Exception Handling ===
    // Note: No SetupTry/PopExceptHandler - we use static exception_table
    Raise,                  // Raise TOS as exception
    RaiseFrom,              // Raise TOS from TOS-1
    Reraise,                // Re-raise current exception (bare `raise`)
    ClearException,         // Clear current_exception when exiting except block

    // === Return ===
    ReturnValue,            // Return TOS from function

    // === Unpacking ===
    UnpackSequence,         // + u8 count: unpack TOS into n values
    UnpackEx,               // + u8 before + u8 after: unpack with *rest

    // === Special ===
    Nop,                    // No operation (for patching/alignment)
}
```

**Operand encoding:**
- No suffix, 0 bytes: `BinaryAdd`, `Pop`, `LoadNone`
- No suffix, 1 byte (u8/i8): `LoadLocal`, `StoreLocal`, `LoadSmallInt`
- `W` suffix, 2 bytes (u16/i16): `LoadLocalW`, `Jump`, `LoadConst`
- Compound (multiple operands): `CallFunctionKw` (u8 + u8), `MakeClosure` (u16 + u8)

### 1.2 Constant Pool

```rust
/// Constants referenced by LoadConst - separate from interns for flexibility
pub struct ConstPool {
    values: Vec<Value>,  // Immediate values (ints, floats, None, etc.)
}
```

**Note:** Strings stay in `Interns` - `LoadConst` for strings uses `Value::InternString(StringId)`.

### 1.3 Location Table Entry

```rust
/// Source location for a bytecode instruction, used for tracebacks.
///
/// Python 3.11+ tracebacks show carets under the relevant expression:
/// python
///
///    File "test.py", line 2, in foo
///      return a + b + c
///             ~~^~~
///
/// The `range` covers the full expression (`a + b`), while `focus` points
/// to the specific operator (`+`) that caused the error.
pub struct LocationEntry {
    /// Bytecode offset this entry applies to
    bytecode_offset: u32,

    /// Full source range of the expression (for the ~~~~ underline)
    range: CodeRange,

    /// Optional focus point within the range (for the ^ caret).
    /// If None, the entire range is underlined without a focus caret.
    focus: Option<CodeRange>,
}
```

**Traceback fidelity note:** Full Python 3.11-style focused traceback ranges (`~~^~~`) are
**not an immediate requirement**. Initial implementation should match current Monty behavior
(full expression range without focus). The `focus` field can be populated incrementally as
we improve error reporting. For now, set `focus: None` and use `range` for the full expression.

### 1.4 Exception Table Entry

``````rust
/// Entry in the exception table - maps a protected bytecode range to its handler.
///
/// Instead of maintaining a runtime stack of handlers (push/pop during execution),
/// we use a static table that's consulted when an exception is raised. This is
/// simpler and matches CPython 3.11+'s approach.
///
/// For nested try blocks, multiple entries may cover the same bytecode offset.
/// Entries are ordered innermost-first, so the VM uses the first matching entry.
///
/// Example: for `try: x = bar(); y = baz() except ValueError as e: print(e)`
/// ```text
/// 0:  LOAD_GLOBAL 'bar'
/// 4:  CALL_FUNCTION 0
/// 8:  STORE_LOCAL 'x'
/// ...
/// 24: JUMP 50              # skip handler if no exception
/// 30: <handler code>       # exception handler starts here
/// ```
/// Entry: `{ start: 0, end: 24, handler: 30, stack_depth: 0 }`
pub struct ExceptionEntry {
    /// Start of protected bytecode range (inclusive)
    start: u32,

    /// End of protected bytecode range (exclusive)
    end: u32,

    /// Bytecode offset of the exception handler
    handler: u32,

    /// Stack depth when entering the try block.
    /// Used to unwind the operand stack before jumping to handler.
    stack_depth: u16,
}
``````

When an exception is raised:
1. Search the exception table for an entry where `start <= ip < end`
2. Unwind the operand stack to `stack_depth`
3. Push the exception value onto the stack
4. Jump to `handler`

The handler code itself checks the exception type (e.g., `isinstance(exc, ValueError)`) and either handles it or re-raises.

**Exception handling notes:**
- **`current_exception`**: Set when entering an except handler (for bare `raise`), cleared by `ClearException` opcode when exiting the handler normally.
- **`except E as e` cleanup**: Python 3 deletes `e` at the end of the except block. The compiler emits `DeleteLocal` for the binding variable after the handler body.
- **Exception chaining (`__context__`/`__cause__`)**: **Not currently supported**. This is noted as future work - the current implementation does not set `__context__` when raising during exception handling, nor does it support `raise X from Y` semantics fully.
- **`finally` blocks**: Compiled as duplicated code paths (normal exit + exception exit) with appropriate cleanup.

### 1.5 Code Object

```rust
/// Compiled bytecode for a function or module
pub struct Code {
    /// Raw bytecode instructions
    bytecode: Vec<u8>,

    /// Constant pool for this code object
    constants: ConstPool,

    /// Source location table for tracebacks: maps bytecode offset to source location.
    /// Each entry contains the full CodeRange for the expression, plus an optional
    /// "focus" range for the ~^~ syntax in Python 3.11+ tracebacks.
    /// E.g., for `x + y` causing an error, the full range covers `x + y`,
    /// but the focus might just be `+` to show: `~~^~~`
    location_table: Vec<LocationEntry>,

    /// Exception handler table - see ExceptionEntry for details
    exception_table: Vec<ExceptionEntry>,

    /// Number of local variables (namespace size)
    num_locals: u16,

    /// Stack size hint for pre-allocation
    stack_size: u16,
}
```

### 1.6 Bytecode Encoding

Bytecode is stored as `Vec<u8>` with variable-width operands. This gives excellent
cache locality - hot loops fit entirely in L1 cache.

**Encoding rules:**
```
1. No operand (1 byte):     [opcode]
2. u8 operand (2 bytes):    [opcode][u8]
3. u16 operand (3 bytes):   [opcode][lo][hi]  (little-endian)
4. Compound operands:       [opcode][op1][op2]...
```

**Example - `x = a + b` compiles to 6 bytes:**
```
[LoadLocal0]                # 1 byte (specialized, no operand)
[LoadLocal] [0x01]          # 2 bytes (slot 1)
[BinaryAdd]                 # 1 byte
[StoreLocal] [0x02]         # 2 bytes (slot 2)
```

### 1.7 CodeBuilder (Bytecode Emission)

```rust
/// Builder for emitting bytecode during compilation.
/// Handles encoding opcodes and operands into raw bytes.
pub struct CodeBuilder {
    bytecode: Vec<u8>,
    constants: Vec<Value>,
    location_table: Vec<LocationEntry>,
    exception_table: Vec<ExceptionEntry>,

    /// Current source location (set before emitting instructions)
    current_location: Option<CodeRange>,
    current_focus: Option<CodeRange>,

    /// Track max stack depth for pre-allocation hint
    current_stack_depth: u16,
    max_stack_depth: u16,
}

impl CodeBuilder {
    /// Emit a no-operand instruction.
    pub fn emit(&mut self, op: Opcode) {
        self.record_location();
        self.bytecode.push(op as u8);
    }

    /// Emit instruction with u8 operand.
    pub fn emit_u8(&mut self, op: Opcode, operand: u8) {
        self.record_location();
        self.bytecode.push(op as u8);
        self.bytecode.push(operand);
    }

    /// Emit instruction with u16 operand (little-endian).
    pub fn emit_u16(&mut self, op: Opcode, operand: u16) {
        self.record_location();
        self.bytecode.push(op as u8);
        self.bytecode.extend_from_slice(&operand.to_le_bytes());
    }

    /// Emit instruction with i16 operand (for jumps).
    pub fn emit_i16(&mut self, op: Opcode, operand: i16) {
        self.record_location();
        self.bytecode.push(op as u8);
        self.bytecode.extend_from_slice(&operand.to_le_bytes());
    }

    /// Emit a forward jump, returning a label to patch later.
    pub fn emit_jump(&mut self, op: Opcode) -> JumpLabel {
        self.record_location();
        let label = JumpLabel(self.bytecode.len());
        self.bytecode.push(op as u8);
        self.bytecode.extend_from_slice(&0i16.to_le_bytes()); // placeholder
        label
    }

    /// Patch a forward jump to point to the current location.
    /// Panics if the jump offset exceeds i16 range (function too large).
    pub fn patch_jump(&mut self, label: JumpLabel) {
        let target = self.bytecode.len();
        let raw_offset = target as i64 - label.0 as i64 - 3; // -3 for opcode + i16
        let offset = i16::try_from(raw_offset)
            .expect("jump offset exceeds i16 range (-32768..32767); function too large");
        let bytes = offset.to_le_bytes();
        self.bytecode[label.0 + 1] = bytes[0];
        self.bytecode[label.0 + 2] = bytes[1];
    }

    /// Current bytecode offset (for jump targets).
    pub fn current_offset(&self) -> usize {
        self.bytecode.len()
    }

    /// Emit LoadLocal, using specialized opcodes for slots 0-3.
    pub fn emit_load_local(&mut self, slot: u16) {
        match slot {
            0 => self.emit(Opcode::LoadLocal0),
            1 => self.emit(Opcode::LoadLocal1),
            2 => self.emit(Opcode::LoadLocal2),
            3 => self.emit(Opcode::LoadLocal3),
            s if s <= 255 => self.emit_u8(Opcode::LoadLocal, s as u8),
            s => self.emit_u16(Opcode::LoadLocalW, s),
        }
    }

    /// Add a constant to the pool, returning its index.
    /// Panics if the constant pool exceeds u16 range (too many constants).
    pub fn add_const(&mut self, value: Value) -> u16 {
        let idx = self.constants.len();
        u16::try_from(idx)
            .expect("constant pool exceeds u16 range (65535); too many constants");
        self.constants.push(value);
        idx as u16
    }

    /// Build the final Code object.
    pub fn build(self, num_locals: u16) -> Code {
        Code {
            bytecode: self.bytecode,
            constants: ConstPool { values: self.constants },
            location_table: self.location_table,
            exception_table: self.exception_table,
            num_locals,
            stack_size: self.max_stack_depth,
        }
    }

    fn record_location(&mut self) {
        if let Some(range) = self.current_location {
            self.location_table.push(LocationEntry {
                bytecode_offset: self.bytecode.len() as u32,
                range,
                focus: self.current_focus,
            });
        }
    }
}

/// Label for forward jump patching.
pub struct JumpLabel(usize);
```

**Operand overflow handling:** All operand emissions must panic on overflow rather than
silently truncating. This catches pathological cases (giant functions with >32KB bytecode,
huge constant pools with >65535 entries) at compile time with clear error messages.

### 1.8 Opcode Decoding (VM Fetch Helpers)

```rust
impl VM<'_, T> {
    /// Fetch next byte from current frame's bytecode and advance frame's IP.
    fn fetch_byte(&mut self) -> u8 {
        let frame = self.frames.last_mut().expect("no active frame");
        let byte = frame.code.bytecode[frame.ip];
        frame.ip += 1;
        byte
    }

    /// Fetch opcode using safe conversion.
    fn fetch_opcode(&mut self) -> Opcode {
        let byte = self.fetch_byte();
        Opcode::try_from(byte).expect("invalid opcode in bytecode")
    }

    /// Fetch u8 operand.
    fn fetch_u8(&mut self) -> u8 {
        self.fetch_byte()
    }

    /// Fetch i8 operand.
    fn fetch_i8(&mut self) -> i8 {
        self.fetch_byte() as i8
    }

    /// Fetch u16 operand (little-endian).
    fn fetch_u16(&mut self) -> u16 {
        let lo = self.fetch_byte();
        let hi = self.fetch_byte();
        u16::from_le_bytes([lo, hi])
    }

    /// Fetch i16 operand (little-endian).
    fn fetch_i16(&mut self) -> i16 {
        let lo = self.fetch_byte();
        let hi = self.fetch_byte();
        i16::from_le_bytes([lo, hi])
    }

    /// Apply relative jump offset to current frame's IP.
    /// Offset is relative to position AFTER the jump instruction's operand.
    fn jump_relative(&mut self, offset: i16) {
        let frame = self.current_frame_mut();
        frame.ip = (frame.ip as isize + offset as isize) as usize;
    }
}

/// Safe conversion from byte to Opcode.
impl TryFrom<u8> for Opcode {
    type Error = &'static str;

    fn try_from(byte: u8) -> Result<Self, Self::Error> {
        // Opcode is #[repr(u8)] so we can check the range
        if byte <= Opcode::Nop as u8 {
            // Use a match for safety (no transmute)
            // In practice, generate this with a macro or build script
            Ok(OPCODE_TABLE[byte as usize])
        } else {
            Err("opcode byte out of range")
        }
    }
}

// Lookup table generated at compile time from Opcode variants
static OPCODE_TABLE: [Opcode; 256] = /* generated */;
```

**Rationale:** Variable-width encoding gives ~2x smaller bytecode than fixed-width.
Decode cost is trivial (~1-2 cycles) compared to cache miss cost (~10+ cycles).

---

## Phase 2: VM Architecture

### 2.1 VM State

```rust
/// The bytecode virtual machine.
///
/// Note: The instruction pointer (IP) lives in each `CallFrame`, not here.
/// This avoids sync bugs on call/return - each frame owns its position.
pub struct VM<'a, T: ResourceTracker> {
    /// Operand stack - values being computed
    stack: Vec<Value>,

    /// Call stack - function frames (each frame has its own IP)
    frames: Vec<CallFrame>,

    /// Heap for reference-counted objects (existing)
    heap: &'a mut Heap<T>,

    /// Namespace stack (existing, modified)
    namespaces: &'a mut Namespaces,

    /// Interned strings/bytes (existing)
    interns: &'a Interns,

    /// Current exception being handled (if any).
    /// Used by bare `raise` to re-raise the current exception.
    /// Set when entering an except handler, cleared when exiting.
    current_exception: Option<Value>,
}
// Note: No runtime exception_handlers stack needed - we use the static
// exception_table in Code to find handlers when an exception is raised.
```

### 2.2 Call Frame

```rust
/// A single function activation record
pub struct CallFrame {
    /// Bytecode being executed
    code: &Code,

    /// Instruction pointer within this frame's bytecode
    ip: usize,

    /// Base index into operand stack for this frame's locals
    stack_base: usize,

    /// Namespace index for this frame's locals
    namespace_idx: NamespaceId,

    /// Function ID (for tracebacks)
    function_id: Option<FunctionId>,

    /// Captured cells for closures
    cells: Vec<HeapId>,

    /// Call site position (for tracebacks)
    call_position: CodeRange,
}
```

### 2.3 Main Execution Loop

The VM fetches opcodes and operands from raw bytes, decoding on-the-fly:

```rust
impl<'a, T: ResourceTracker> VM<'a, T> {
    pub fn run(&mut self) -> VMResult {
        loop {
            // Fetch opcode byte and decode
            let opcode = self.fetch_opcode();

            // Execute - operands fetched inline as needed
            match opcode {
                // === No-operand instructions ===
                Opcode::LoadNone => {
                    self.push(Value::None);
                }

                Opcode::LoadTrue => {
                    self.push(Value::Bool(true));
                }

                // IMPORTANT: Always drop operands BEFORE propagating errors with `?`
                // to avoid reference count leaks. Pattern: compute, drop, then propagate.
                Opcode::BinaryAdd => {
                    let rhs = self.pop();
                    let lhs = self.pop();
                    let result = lhs.py_add(&rhs, self.heap, self.interns);
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    self.push(result?.unwrap_or(Value::None)); // TODO: TypeError
                }

                Opcode::ReturnValue => {
                    let value = self.pop();
                    if self.frames.len() == 1 {
                        return VMResult::Complete(value);
                    }
                    self.pop_frame();
                    self.push(value);
                }

                // === Specialized no-operand (hot path) ===
                Opcode::LoadLocal0 => {
                    let value = self.get_local(0);
                    self.push(value.clone_with_heap(self.heap));
                }

                Opcode::LoadLocal1 => {
                    let value = self.get_local(1);
                    self.push(value.clone_with_heap(self.heap));
                }

                // === Instructions with u8 operand ===
                Opcode::LoadLocal => {
                    let slot = self.fetch_u8() as u16;
                    let value = self.get_local(slot);
                    self.push(value.clone_with_heap(self.heap));
                }

                Opcode::StoreLocal => {
                    let slot = self.fetch_u8() as u16;
                    let value = self.pop();
                    self.set_local(slot, value);
                }

                Opcode::LoadSmallInt => {
                    let n = self.fetch_i8();
                    self.push(Value::Int(n as i64));
                }

                Opcode::CallFunction => {
                    let arg_count = self.fetch_u8();
                    self.call_function(arg_count)?;
                }

                // === Instructions with u16 operand ===
                Opcode::LoadLocalW => {
                    let slot = self.fetch_u16();
                    let value = self.get_local(slot);
                    self.push(value.clone_with_heap(self.heap));
                }

                Opcode::LoadConst => {
                    let idx = self.fetch_u16();
                    let value = self.current_code().constants.get(idx);
                    self.push(value.clone_with_heap(self.heap));
                }

                Opcode::LoadAttr => {
                    let name_id = self.fetch_u16();
                    let obj = self.pop();
                    let attr = self.get_attr(&obj, name_id);
                    obj.drop_with_heap(self.heap);
                    self.push(attr?);
                }

                // === Instructions with i16 operand (jumps) ===
                // Note: IP lives only in CallFrame (section 2.2), accessed via helpers
                Opcode::Jump => {
                    let offset = self.fetch_i16();
                    self.jump_relative(offset);
                }

                Opcode::JumpIfFalse => {
                    let offset = self.fetch_i16();
                    let cond = self.pop();
                    if !cond.is_truthy() {
                        self.jump_relative(offset);
                    }
                    cond.drop_with_heap(self.heap);
                }

                // === Compound operands ===
                Opcode::CallExternal => {
                    let func_id = self.fetch_u16();
                    let arg_count = self.fetch_u8();
                    let args = self.pop_n(arg_count as usize);
                    return VMResult::ExternalCall {
                        function_id: func_id,
                        args: ArgValues::from_vec(args),
                    };
                }

                Opcode::MakeClosure => {
                    let func_id = self.fetch_u16();
                    let cell_count = self.fetch_u8();
                    self.make_closure(func_id, cell_count)?;
                }

                // ... remaining opcodes
                _ => todo!("opcode {:?}", opcode),
            }
        }
    }

    /// Resume after external call completes - push result and continue.
    pub fn resume(&mut self, result: Value) -> VMResult {
        self.push(result);
        self.run()
    }
}
```

**Key points:**
- Operands are fetched **inline** after matching the opcode
- Jump offsets are relative to the IP **after** fetching the operand
- Specialized opcodes (`LoadLocal0`, etc.) avoid operand fetch entirely

### 2.4 Reference Count Safety in Error Handling

**CRITICAL:** When an operation can fail (returns `Result`), operands must be dropped BEFORE propagating the error with `?`. Otherwise, reference counts leak.

**The Problem:**
`Value` does not implement `Drop` to decrement reference counts—it requires `drop_with_heap(&mut Heap)`. When `?` propagates an error, Rust drops the `Value` structs but does NOT decrement heap refcounts, causing permanent memory leaks.

**Wrong (leaks on error):**
```rust
Opcode::BinaryAdd => {
    let rhs = self.pop();
    let lhs = self.pop();
    let result = lhs.py_add(&rhs, self.heap)?; // If error, lhs/rhs leak!
    lhs.drop_with_heap(self.heap);
    rhs.drop_with_heap(self.heap);
    self.push(result);
}
```

**Correct (drop before propagating):**
```rust
Opcode::BinaryAdd => {
    let rhs = self.pop();
    let lhs = self.pop();
    let result = lhs.py_add(&rhs, self.heap); // Don't use ? yet
    lhs.drop_with_heap(self.heap);            // Always drop operands
    rhs.drop_with_heap(self.heap);
    self.push(result?);                       // Now propagate error if any
}
```

**Pattern for all fallible operations:**
1. Pop operands
2. Call the operation (store Result, don't use `?`)
3. Drop all operands unconditionally with `drop_with_heap()`
4. Propagate error with `?` (or match on result)
5. Push result on success

**Helper methods** (e.g., `call_function`, `make_closure`) that pop values internally must follow the same pattern—they are responsible for dropping any values they pop before returning an error.

---

## Phase 3: Bytecode Compiler

### 3.1 Compiler Structure

```rust
/// Compiles prepared AST to bytecode.
/// Uses CodeBuilder (defined in section 1.7) for bytecode emission.
pub struct Compiler<'a> {
    /// Current code being built (see section 1.7 for CodeBuilder definition)
    code: CodeBuilder,

    /// Loop stack for break/continue (stores jump labels to patch)
    loop_stack: Vec<LoopInfo>,

    /// Try stack for exception handlers (tracks protected ranges)
    try_stack: Vec<TryInfo>,

    /// Interns reference for string/function lookups
    interns: &'a Interns,
}
```

### 3.2 Expression Compilation

```rust
impl Compiler<'_> {
    fn compile_expr(&mut self, expr: &ExprLoc) {
        // Set source location for traceback info
        self.code.set_location(expr.range, None);

        match &expr.expr {
            Expr::Literal(lit) => self.compile_literal(lit),

            Expr::Name(ident) => {
                match ident.scope {
                    NameScope::Local => self.code.emit_load_local(ident.slot),
                    NameScope::Global => self.code.emit_u16(Opcode::LoadGlobal, ident.slot),
                    NameScope::Cell => self.code.emit_u16(Opcode::LoadCell, ident.slot),
                }
            }

            Expr::Op { left, op, right } => {
                // Short-circuit AND/OR
                if *op == Operator::And {
                    self.compile_expr(left);
                    let jump = self.code.emit_jump(Opcode::JumpIfFalseOrPop);
                    self.compile_expr(right);
                    self.code.patch_jump(jump);
                } else if *op == Operator::Or {
                    self.compile_expr(left);
                    let jump = self.code.emit_jump(Opcode::JumpIfTrueOrPop);
                    self.compile_expr(right);
                    self.code.patch_jump(jump);
                } else {
                    self.compile_expr(left);
                    self.compile_expr(right);
                    self.code.emit(op_to_binary_opcode(*op));
                }
            }

            Expr::Call { callable, args } => {
                self.compile_call(callable, args);
            }

            // ... other expressions
        }
    }
}
```

### 3.3 Statement Compilation

```rust
impl Compiler<'_> {
    fn compile_stmt(&mut self, node: &Node) {
        match node {
            Node::Expr(expr) => {
                self.compile_expr(expr);
                self.code.emit(Opcode::Pop);  // Discard result
            }

            Node::Assign { target, object } => {
                self.compile_expr(object);
                self.compile_store(target);
            }

            Node::If { test, body, or_else } => {
                self.compile_expr(test);
                let else_jump = self.code.emit_jump(Opcode::JumpIfFalse);
                self.compile_block(body);

                if !or_else.is_empty() {
                    let end_jump = self.code.emit_jump(Opcode::Jump);
                    self.code.patch_jump(else_jump);
                    self.compile_block(or_else);
                    self.code.patch_jump(end_jump);
                } else {
                    self.code.patch_jump(else_jump);
                }
            }

            Node::For { target, iter, body, or_else } => {
                self.compile_expr(iter);
                self.code.emit(Opcode::GetIter);

                let loop_start = self.code.current_offset();
                self.loop_stack.push(LoopInfo { start: loop_start, breaks: vec![] });

                let end_jump = self.code.emit_jump(Opcode::ForIter);
                self.compile_store(target);
                self.compile_block(body);
                self.code.emit_jump_to(Opcode::Jump, loop_start);

                self.code.patch_jump(end_jump);
                // Handle or_else and break patches...
            }

            Node::Try(try_block) => {
                self.compile_try(try_block);
            }

            // ... other statements
        }
    }
}
```

### 3.4 Function Compilation

```rust
impl Compiler<'_> {
    fn compile_function(&mut self, func: &Function) -> Code {
        let mut func_compiler = Compiler::new(self.interns);

        // Compile function body
        for node in &func.body {
            func_compiler.compile_stmt(node);
        }

        // Implicit return None if no explicit return
        func_compiler.code.emit(Opcode::LoadNone);
        func_compiler.code.emit(Opcode::ReturnValue);

        func_compiler.code.build(func.namespace_size as u16)
    }
}
```

---

## Phase 4: Integration

### 4.1 Modified Function Struct

```rust
pub struct Function {
    // Existing fields...
    pub name: Identifier,
    pub signature: Signature,
    pub namespace_size: usize,
    pub free_var_enclosing_slots: Vec<NamespaceId>,
    pub cell_var_count: usize,
    pub default_exprs: Vec<ExprLoc>,

    // NEW: Compiled bytecode (replaces body: Vec<Node>)
    pub code: Code,
}
```

### 4.2 Modified Interns

```rust
pub struct Interns {
    strings: Vec<String>,
    bytes: Vec<Vec<u8>>,
    functions: Vec<Function>,  // Functions now contain Code
    external_functions: Vec<String>,
}
```

### 4.3 Compilation Timing (Eager)

Bytecode compilation happens during the **prepare phase**, before execution:

```rust
impl Executor {
    /// Called during prepare phase - compiles all functions upfront
    pub fn prepare(parsed: ParseResult) -> Self {
        let mut prepared = prepare_nodes(parsed);

        // Compile module-level code
        let module_code = Compiler::compile_module(&prepared.nodes);

        // Compile all functions eagerly
        for func in &mut prepared.functions {
            func.code = Compiler::compile_function(func);
        }

        Executor {
            module_code,
            functions: prepared.functions,
            // ...
        }
    }
}
```

**Rationale:** Eager compilation is simpler (no runtime compilation state), catches syntax/semantic errors early, and avoids compilation latency during execution.

### 4.4 Execution Entry Point

```rust
/// Result of execution - includes heap/namespaces for external call case.
pub enum ExecutorResult<T: ResourceTracker> {
    Complete(Value),
    Error(Exception),
    /// Paused at external call - includes full state for serialization
    ExternalCall {
        call: ExternalCall,
        /// Heap to serialize alongside VMSnapshot (owns all heap objects)
        heap: Heap<T>,
        /// Namespaces to serialize (local variable storage)
        namespaces: Namespaces,
    },
}

impl Executor {
    pub fn run_with_tracker<T: ResourceTracker>(
        &self,
        inputs: Vec<MontyObject>,
        tracker: T,
        print: &mut impl PrintWriter,
    ) -> ExecutorResult<T> {
        let mut heap = Heap::new(256, tracker);
        let mut namespaces = Namespaces::new(self.namespace_size);

        // Use pre-compiled module bytecode (eager compilation)
        let module_code = &self.module_code;

        // Create VM (borrows heap/namespaces)
        let mut vm = VM::new(&mut heap, &mut namespaces, &self.interns);
        vm.push_frame(module_code, GLOBAL_NS_IDX);

        // Run
        match vm.run() {
            VMResult::Complete(value) => ExecutorResult::Complete(value),
            VMResult::ExternalCall { function_id, args } => {
                // into_snapshot() consumes VM, transferring Value ownership
                let vm_state = vm.into_snapshot();
                ExecutorResult::ExternalCall {
                    call: ExternalCall { function_id, args, vm_state },
                    heap,        // Return heap for serialization
                    namespaces,  // Return namespaces for serialization
                }
            }
            VMResult::Error(exc) => ExecutorResult::Error(exc),
        }
    }
}
```

**Note:** The `ExternalCall` case returns ownership of `heap` and `namespaces` alongside the `VMSnapshot`. All three must be serialized together - `HeapId` values in the snapshot are indices into that specific heap instance.

### 4.5 Snapshot/Resume (Simplified!)

```rust
/// Serializable representation of a call frame.
///
/// Cannot store `&Code` (a reference) - instead stores `FunctionId` to look up
/// the pre-compiled Code object on resume. Module-level code uses `None`.
#[derive(Serialize, Deserialize)]
pub struct SerializedFrame {
    /// Which function's code this frame executes (None = module-level)
    function_id: Option<FunctionId>,

    /// Instruction pointer within this frame's bytecode
    ip: usize,

    /// Base index into operand stack for this frame's locals
    stack_base: usize,

    /// Namespace index for this frame's locals
    namespace_idx: NamespaceId,

    /// Captured cells for closures (HeapIds remain valid after heap deserialization)
    cells: Vec<HeapId>,

    /// Call site position (for tracebacks)
    call_position: CodeRange,
}

impl CallFrame {
    /// Convert to serializable form.
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

/// VM state for pause/resume - much simpler than current approach!
///
/// **Ownership:** This struct OWNS the values (refcounts were incremented).
/// Must be used with the serialized Heap - HeapId values are indices into that heap.
#[derive(Serialize, Deserialize)]
pub struct VMSnapshot {
    /// Operand stack (may contain Value::Ref(HeapId) pointing to heap)
    stack: Vec<Value>,

    /// Call frames (serializable form - stores FunctionId, not &Code)
    frames: Vec<SerializedFrame>,

    /// Current exception being handled (if any)
    current_exception: Option<Value>,
}

impl<'a, T: ResourceTracker> VM<'a, T> {
    /// Consume the VM and create a snapshot for pause/resume.
    ///
    /// **Ownership transfer:** This method takes `self` by value, consuming the VM.
    /// The snapshot owns all Values (refcounts already correct from the live VM).
    /// The heap must be serialized alongside this snapshot.
    ///
    /// This is NOT a clone - it's a transfer. After calling this, the original VM
    /// is gone and only the snapshot (+ serialized heap) represents the state.
    pub fn into_snapshot(self) -> VMSnapshot {
        VMSnapshot {
            // Move values directly - no clone, no refcount increment needed
            // (the VM owned them, now the snapshot owns them)
            stack: self.stack,
            frames: self.frames.into_iter().map(|f| f.serialize()).collect(),
            current_exception: self.current_exception,
        }
    }

    /// Reconstruct VM from snapshot.
    ///
    /// The heap and namespaces must already be deserialized. FunctionId values
    /// in frames are used to look up pre-compiled Code objects from the Executor.
    pub fn restore(
        snapshot: VMSnapshot,
        heap: &'a mut Heap<T>,
        namespaces: &'a mut Namespaces,
        interns: &'a Interns,
        executor: &Executor,  // To look up Code by FunctionId
    ) -> Self {
        VM {
            stack: snapshot.stack,
            frames: snapshot.frames.into_iter()
                .map(|sf| CallFrame {
                    code: executor.get_code(sf.function_id),
                    ip: sf.ip,
                    stack_base: sf.stack_base,
                    namespace_idx: sf.namespace_idx,
                    function_id: sf.function_id,
                    cells: sf.cells,
                    call_position: sf.call_position,
                })
                .collect(),
            heap,
            namespaces,
            interns,
            current_exception: snapshot.current_exception,
        }
    }
}
```

**Ownership semantics (CRITICAL):**
- `into_snapshot(self)` **consumes** the VM - it does NOT clone
- Values move from VM to snapshot; refcounts stay the same (no increment)
- After snapshotting, only the snapshot owns the Values
- This avoids the "who decrements refcounts?" ambiguity of cloning

**Heap serialization (matching current approach):**
- **Full heap is serialized** alongside VMSnapshot - not just the VM state
- `HeapId` is just a `usize` that serializes naturally as the slot index
- `Value::Ref(HeapId)` serializes as the numeric ID; heap entries are in the serialized `Heap<T>`
- Reference counts are preserved exactly in the serialized heap
- On resume, deserialized heap + VMSnapshot reconstruct the full state

**Frame serialization:**
- `CallFrame` stores `&Code` (a reference) which cannot be serialized
- `SerializedFrame` stores `Option<FunctionId>` instead
- On restore, `FunctionId` is used to look up the pre-compiled `Code` from `Executor`
- Module-level code uses `function_id: None`; `Executor` provides `module_code` for that case

**Namespace/local state:**
- Namespaces are serialized alongside heap (existing pattern)
- Local variables are `Value` entries that may point to heap via `HeapId`

**Argument ownership across external call boundary:**
- Args are converted to `MontyObject` (self-contained) before crossing boundary
- Original `Value` refs are dropped via `drop_with_heap()`
- Return value converted back via `to_value()` and pushed to stack

**Why bytecode makes this simpler:**
- No position tracking (IP in each frame is the position)
- No re-evaluation of expressions (values are on the stack)
- No ext_return_values cache (just push result and continue)
- No ClauseState (control flow is encoded in bytecode jumps)

---

## Phase 5: Performance Optimizations

### 5.1 Opcode Specialization

Create specialized opcodes for common patterns (already included in section 1.1):

```rust
// Instead of: LoadLocal + u8(0), LoadLocal + u8(1), BinaryAdd
// Use zero-operand specialized opcodes:
Opcode::LoadLocal0,   // Most common: first local (self, first param)
Opcode::LoadLocal1,
Opcode::LoadLocal2,
Opcode::LoadLocal3,

// Future specializations (not in initial implementation):
// Opcode::AddLocals,    // + u8 slot1 + u8 slot2: add two locals directly
// Opcode::AddSmallInt,  // + i8: add small int to TOS
```

### 5.2 Inline Caching (JIT Prep)

```rust
/// Inline cache entry for attribute/method lookups
pub struct InlineCache {
    /// Type ID of last successful lookup
    type_id: TypeId,
    /// Cached result (method pointer or attribute offset)
    cached: CachedLookup,
}

// In LoadAttr:
if let Some(cache) = self.get_inline_cache(ip) {
    if obj.type_id() == cache.type_id {
        return cache.cached.apply(obj);  // Fast path
    }
}
// Slow path: full lookup, then cache
```

### 5.3 Stack Caching

Keep top-of-stack in a register (future optimization):

```rust
impl VM<'_, T> {
    fn run(&mut self) -> VMResult {
        let mut tos: Option<Value> = None;  // Cached TOS

        loop {
            match self.fetch_opcode() {
                Opcode::LoadLocal => {
                    let slot = self.fetch_u8() as u16;
                    if let Some(v) = tos.take() {
                        self.stack.push(v);
                    }
                    tos = Some(self.get_local(slot).clone_with_heap(self.heap));
                }
                Opcode::BinaryAdd => {
                    let rhs = tos.take().unwrap();
                    let lhs = self.pop();
                    tos = Some(lhs.py_add(&rhs, self.heap, self.interns)?);
                }
                // ...
            }
        }
    }
}
```

---

## Phase 6: Future - JIT Compilation

### 6.1 JIT Architecture

```
Bytecode → Trace Recording → IR → Machine Code
              ↑                      ↓
              └──── Hot Loop ←───────┘
```

### 6.2 Trace Recording

```rust
/// Records a trace of executed bytecode for JIT compilation
pub struct TraceRecorder {
    trace: Vec<TraceOp>,
    loop_header: usize,
    iteration_count: usize,
}

// When a back-edge (loop) is hot:
if self.is_hot_loop(target) {
    self.start_recording();
}
```

### 6.3 JIT IR

```rust
/// JIT intermediate representation - SSA form
pub enum JitOp {
    LoadLocal { dst: VReg, slot: u16 },
    BinaryAdd { dst: VReg, lhs: VReg, rhs: VReg },
    Guard { vreg: VReg, expected_type: TypeId, deopt: Label },
    // ...
}
```

### 6.4 Type Specialization

```rust
// If we observe that a loop always adds integers:
// Original: BinaryAdd (generic)
// Specialized: IntAdd (no type checks)
// With guard: Guard(lhs, Int), Guard(rhs, Int), IntAdd
```

### 6.5 Deoptimization

When a guard fails, fall back to interpreter:
```rust
fn deoptimize(&mut self, state: &JitState) {
    // Reconstruct VM state from JIT registers
    self.ip = state.bytecode_ip;
    self.stack = state.reconstruct_stack();
    // Continue in interpreter
}
```

---

## Phase 7: Future - Async Support

### 7.1 Async Architecture (Shared Heap Model)

All tasks share a single heap, enabling object passing between tasks without copying:

```
┌─────────────────────────────────────────┐
│              Event Loop                  │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  │
│  │ Task 1  │  │ Task 2  │  │ Task 3  │  │
│  │ (VM 1)  │  │ (VM 2)  │  │ (VM 3)  │  │
│  └────┬────┘  └────┬────┘  └────┬────┘  │
│       │            │            │        │
│       ▼            ▼            ▼        │
│  ┌──────────────────────────────────┐   │
│  │    Shared Heap (single-threaded) │   │
│  │    - No locks needed             │   │
│  │    - Objects can be passed       │   │
│  │    - Unified reference counting  │   │
│  └──────────────────────────────────┘   │
└─────────────────────────────────────────┘
```

**Benefits of shared heap:**
- Objects can be passed between tasks (just pass `Value::Ref`)
- Single GC for all tasks (no cross-heap references)
- No serialization needed for inter-task communication
- Single-threaded = no locks, no data races

### 7.2 Task Representation

```rust
/// An async task - a suspended VM
pub struct Task {
    /// Task ID for scheduling
    id: TaskId,

    /// VM state (paused)
    vm_state: VMSnapshot,

    /// What this task is waiting for
    waiting_for: WaitReason,
}

pub enum WaitReason {
    /// Waiting for external call result
    ExternalCall(ExternalCall),

    /// Waiting for another task to complete
    TaskJoin(TaskId),

    /// Waiting for I/O (future)
    IO(IoHandle),
}
```

### 7.3 Event Loop

```rust
pub struct EventLoop {
    /// Ready queue - tasks that can run
    ready: VecDeque<Task>,

    /// Waiting tasks - keyed by what they're waiting for
    waiting: HashMap<WaitKey, Task>,

    /// Shared heap (all tasks share one heap)
    heap: Heap<TrackedResources>,

    /// Shared namespaces (global scope)
    global_namespace: Namespace,
}

impl EventLoop {
    pub fn run(&mut self) -> EventLoopResult {
        loop {
            // Run next ready task
            if let Some(mut task) = self.ready.pop_front() {
                let mut vm = VM::restore(task.vm_state, &mut self.heap, ...);

                match vm.run() {
                    VMResult::Complete(value) => {
                        // Wake tasks waiting on this one
                        self.complete_task(task.id, value);
                    }
                    VMResult::ExternalCall { func_id, args } => {
                        // Return to host for external handling
                        return EventLoopResult::ExternalCall {
                            task_id: task.id,
                            call: ExternalCall { func_id, args },
                        };
                    }
                    VMResult::Await(awaited_task_id) => {
                        // Park this task until awaited completes
                        task.waiting_for = WaitReason::TaskJoin(awaited_task_id);
                        self.waiting.insert(WaitKey::Task(awaited_task_id), task);
                    }
                }
            } else if self.waiting.is_empty() {
                return EventLoopResult::AllComplete;
            } else {
                return EventLoopResult::AllBlocked;
            }
        }
    }

    pub fn complete_external(&mut self, task_id: TaskId, result: Value) {
        if let Some(mut task) = self.waiting.remove(&WaitKey::External(task_id)) {
            task.vm_state.push(result);
            self.ready.push_back(task);
        }
    }
}
```

### 7.4 Async/Await Opcodes

```rust
// Already defined in section 1.1 as placeholders:
Opcode::Await,  // Pause current task, wait for TOS (another task or future)
// Note: Yield not currently in opcode list; add when implementing generators
```

### 7.5 Benefits of Bytecode for Async

1. **Easy task switching**: Just save/restore VMSnapshot
2. **Shared heap**: All tasks use same heap (single-threaded, no locks)
3. **Fair scheduling**: Can preempt after N instructions
4. **Deterministic**: Same bytecode = same behavior

---

## Migration Strategy

### Step 0: Update Documentation
- [x] Update `CLAUDE.md` to explain the bytecode VM architecture
- [x] Remove references to tree-walker (`evaluate.rs`, `run_frame.rs`, `SnapshotTracker`, `ClauseState`)
- [x] Document new code structure (`src/bytecode/` module)
- [x] Explain the VM execution model (operand stack, call frames, IP per frame)

### Step 1: Define Core Types
- [x] `Opcode` enum with all opcodes (`#[repr(u8)]`, no data variants)
- [x] `Code` struct (bytecode, constants, location_table, exception_table)
- [x] `CodeBuilder` for emission (emit, emit_u8, emit_u16, emit_jump, patch_jump)
- [x] `TryFrom<u8>` for safe opcode decoding

### Step 2: Basic Compiler
- [x] Compile literals and simple expressions
- [x] Compile variables (local/global/cell)
- [x] Compile binary/unary operators (including short-circuit AND/OR)
- [x] Compile if/else statements (and ternary expressions)
- [x] Compile for loops (while loops not yet in parser)
- [x] Compile collections (list, tuple, dict, set)
- [x] Compile subscript operations
- [x] Compile assert statements
- [ ] Run in parallel with tree-walker for testing (requires Step 3)

### Step 3: VM Core
- [x] `VM` struct with stack and frames
- [x] Main dispatch loop (fetch opcode, fetch operands, execute)
- [x] All arithmetic/comparison ops (bitwise ops use `todo!()`)
- [x] Variable load/store (local, global, cell)
- [x] Control flow (jumps, conditionals)
- [x] Collection building (list, tuple, dict, set)
- [x] Subscript operations (get, set; delete uses `todo!()`)
- [x] Sequence unpacking
- [x] Basic exception handling (raise, reraise)

Note: Some operations use `todo!()` pending implementation:
- Bitwise operations (BinaryAnd, BinaryOr, BinaryXor, etc.)
- Membership test (CompareIn, CompareNotIn) - needs py_contains on Value
- F-string compilation - needs changes to f-string representation
- Keyword arguments (CallFunctionKw) - needs implementation

### Step 3.5: Iteration & Print Support
- [x] `HeapData::Iterator` variant for storing `ForIterator` on heap
- [x] `GetIter` opcode - creates iterator and stores on heap
- [x] `ForIter` opcode - advances iterator or jumps to end when exhausted
- [x] `print_writer` parameter added to VM for print output
- [x] `CallFunction` opcode - calls builtin functions (print, len, etc.)
- [x] `Expr::Call` compilation - emits callable + args + CallFunction

### Step 4: Functions & Closures
- [x] Builtin function calls (print, len, range, etc.)
- [x] User-defined function calls
- [x] Return values
- [x] Closures with captured cells
- [x] Default parameters
- [x] Method calls (AttrCall compilation, CallMethod opcode)
- [x] Attribute access (LoadAttr, StoreAttr) for Dataclass objects

### Step 5: Exception Handling
- [x] Try/except/finally compilation
- [x] Exception table generation (static, no runtime stack)
- [x] Raise/reraise opcodes
- [x] Exception lookup and stack unwinding in VM

### Step 6: External Calls & Snapshots
- [x] External function calls via `CallFunction` with `Value::ExtFunction`
- [x] VMSnapshot serialization (alongside heap)
- [x] Resume mechanism (push result, continue)
- [x] Integration with existing `RunProgress` API

### Step 7: Remove Old Code
- [x] Delete `evaluate.rs`
- [x] Delete `run_frame.rs` tree-walker
- [x] Delete `SnapshotTracker`, `ClauseState`, `FunctionFrame`
- [x] Remove `#![allow(dead_code, unused_imports)]` from bytecode/mod.rs
- [x] Most tests pass (461 pass, 7 fail - see below)

### Remaining Work (Post-Migration Cleanup)
- [x] Implement bitwise operations (BinaryAnd, BinaryOr, BinaryXor, etc.)
- [x] Implement membership testing (CompareIn, CompareNotIn)
- [x] Implement py_setitem for Value (dict assignment)
- [ ] Fix traceback generation for bytecode exceptions (5 attr tests)
- [ ] Implement F-string compilation
- [ ] Implement keyword arguments (CallFunctionKw)
- [ ] Implement `**kwargs` unpacking support (builtin__print_kwargs.py)

### Current Test Status
- **461 tests pass, 7 tests fail**
- Failures are mostly traceback formatting issues (not core functionality)
- The bytecode VM is feature-complete for most Python operations

---

## Testing Strategy

### Verification
- All existing `test_cases/*.py` files must pass unchanged
- No new test files required for the migration
- Test via `make test-ref-count-panic`

### Validation Approach
1. Run existing test suite after each phase
2. Any behavioral difference from tree-walker is a bug
3. Performance comparison via existing benchmarks (optional)

### Notes
- Tests live in `tests/` and `test_cases/` per repo guidelines
- Do not add internal unit tests for opcodes/compiler - existing Python tests provide coverage
- If a test fails, the bytecode VM has a bug - fix the VM, not the test

---

## Files to Modify/Create

### New Files
- `src/bytecode/mod.rs` - module root
- `src/bytecode/op.rs` - opcode definitions
- `src/bytecode/code.rs` - Code struct
- `src/bytecode/compiler.rs` - AST → bytecode
- `src/bytecode/vm.rs` - execution engine
- `src/bytecode/snapshot.rs` - serialization

### Modified Files
- `src/function.rs` - add `Code` field
- `src/intern.rs` - store compiled functions
- `src/run.rs` - use VM instead of tree-walker
- `src/lib.rs` - export bytecode module

### Deleted Files (after migration)
- `src/evaluate.rs`
- `src/run_frame.rs`
- `src/snapshot.rs` (replaced by bytecode/snapshot.rs)

---

## Verification

After each phase, verify:

1. **Correctness**: All existing `test_cases/*.py` pass
2. **Performance**: No regression vs tree-walker (should improve)
3. **Snapshots**: External call pause/resume works
4. **Memory**: No leaks (ref counting still works)

Final verification:
```bash
make test-ref-count-panic  # All tests pass
cargo bench                 # Performance improved
```
