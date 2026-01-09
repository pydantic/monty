# Monty Bytecode VM Migration Plan

## Design Decisions (Confirmed)

| Decision           | Choice                   | Rationale                                      |
|--------------------|--------------------------|------------------------------------------------|
| Bytecode encoding  | Hybrid variable-width    | Better cache utilization, 2x smaller bytecode  |
| Compilation timing | Eager (at prepare phase) | Simpler, no runtime compilation overhead       |
| Async heap model   | Shared heap              | Objects can pass between tasks, simpler GC     |

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

```rust
/// Bytecode opcodes - each instruction is 1 byte + optional operands.
/// Operands use variable-width encoding for compactness.
#[repr(u8)]
pub enum Op {
    // === Stack Operations ===
    Pop,                    // Discard top of stack
    Dup,                    // Duplicate top of stack
    Rot2,                   // Swap top two items
    Rot3,                   // Rotate top three items

    // === Constants & Literals ===
    LoadConst(ConstId),     // Push constant from constant pool
    LoadNone,               // Push None (common, deserves own opcode)
    LoadTrue,               // Push True
    LoadFalse,              // Push False
    LoadInt(i64),           // Push small integer inline

    // === Variables ===
    LoadLocal(u16),         // Push local variable (namespace slot)
    StoreLocal(u16),        // Pop and store to local
    LoadGlobal(u16),        // Push from global namespace
    StoreGlobal(u16),       // Pop and store to global
    LoadCell(u16),          // Load from cell (closure capture)
    StoreCell(u16),         // Store to cell
    DeleteLocal(u16),       // Delete local variable

    // === Binary Operations ===
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

    // === Comparison Operations ===
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

    // === Unary Operations ===
    UnaryNot,
    UnaryNeg,
    UnaryPos,
    UnaryInvert,            // Bitwise ~

    // === In-place Operations ===
    InplaceAdd,
    InplaceSub,
    InplaceMul,
    // ... (all augmented assignment ops)

    // === Collection Building ===
    BuildList(u16),         // Pop n items, build list
    BuildTuple(u16),        // Pop n items, build tuple
    BuildDict(u16),         // Pop 2n items (key/value pairs), build dict
    BuildSet(u16),          // Pop n items, build set
    BuildFString(u16),      // Pop n parts, build f-string

    // === Subscript & Attribute ===
    BinarySubscr,           // a[b]: pop index, pop obj, push result
    StoreSubscr,            // a[b] = c: pop value, pop index, pop obj
    LoadAttr(StringId),     // obj.attr: pop obj, push attr
    StoreAttr(StringId),    // obj.attr = x: pop value, pop obj

    // === Function Calls ===
    CallFunction(u8),       // Call with n positional args
    CallFunctionKw(u8, u8), // Call with n pos args, m kw args (names on stack)
    CallMethod(StringId, u8), // Call method with n args
    CallExternal(ExtFunctionId, u8), // External call (triggers pause)

    // === Control Flow ===
    Jump(i16),              // Unconditional jump (relative offset)
    JumpIfTrue(i16),        // Jump if TOS is truthy (pop)
    JumpIfFalse(i16),       // Jump if TOS is falsy (pop)
    JumpIfTrueOrPop(i16),   // Short-circuit OR: jump if true, else pop
    JumpIfFalseOrPop(i16),  // Short-circuit AND: jump if false, else pop

    // === Iteration ===
    GetIter,                // Convert TOS to iterator
    ForIter(i16),           // Advance iterator or jump to end

    // === Function Definition ===
    MakeFunction(FunctionId), // Create function object
    MakeClosure(FunctionId, u8), // Create closure with n captured cells

    // === Exception Handling ===
    SetupTry(i16),          // Push exception handler (jump offset to handler)
    PopExceptHandler,       // Pop exception handler
    Raise,                  // Raise TOS as exception
    RaiseFrom,              // Raise TOS from TOS-1
    Reraise,                // Re-raise current exception

    // === Return ===
    ReturnValue,            // Return TOS from function

    // === Special ===
    Nop,                    // No operation (for patching)
    Yield,                  // Future: generator yield
    Await,                  // Future: async await
}
```

### 1.2 Constant Pool

```rust
/// Constants referenced by LoadConst - separate from interns for flexibility
pub struct ConstPool {
    values: Vec<Value>,  // Immediate values (ints, floats, None, etc.)
}
```

**Note:** Strings stay in `Interns` - `LoadConst` for strings uses `Value::InternString(StringId)`.

### 1.3 Code Object

```rust
/// Compiled bytecode for a function or module
pub struct Code {
    /// Raw bytecode instructions
    bytecode: Vec<u8>,

    /// Constant pool for this code object
    constants: ConstPool,

    /// Line number table for tracebacks: (bytecode_offset, line_number)
    line_table: Vec<(usize, u32)>,

    /// Exception handler table: (start, end, handler, stack_depth)
    exception_table: Vec<ExceptionEntry>,

    /// Number of local variables (namespace size)
    num_locals: u16,

    /// Stack size hint for pre-allocation
    stack_size: u16,
}
```

### 1.4 Bytecode Encoding (Hybrid Variable-Width)

Use **hybrid variable-width encoding** (CPython-style) for optimal cache utilization:

```
Encoding tiers:
1. No operand (1 byte):     [opcode]
2. u8 operand (2 bytes):    [opcode][u8]
3. u16 operand (3 bytes):   [opcode][u16 little-endian]
4. Extended (4+ bytes):     [EXTENDED_ARG][high byte][opcode][low bytes]
```

**Specialized single-byte opcodes for hot paths:**
```rust
// Instead of LOAD_LOCAL + operand for common slots:
Op::LoadLocal0,    // First local (often 'self' or first param)
Op::LoadLocal1,
Op::LoadLocal2,
Op::LoadLocal3,

// Common constants without operand:
Op::LoadNone,
Op::LoadTrue,
Op::LoadFalse,
Op::LoadIntZero,   // Push 0
Op::LoadIntOne,    // Push 1
```

**Example - `x = a + b` compiles to 6 bytes:**
```
[LOAD_LOCAL_0]              # 1 byte (specialized for slot 0)
[LOAD_LOCAL] [0x01]         # 2 bytes (slot 1)
[BINARY_ADD]                # 1 byte
[STORE_LOCAL] [0x02]        # 2 bytes (slot 2)
```

**Rationale:** Cache benefits outweigh decode overhead. Hot loops fit in L1 cache, and decode cost is ~1-2 cycles per instruction vs 10+ cycles for cache miss.

---

## Phase 2: VM Architecture

### 2.1 VM State

```rust
/// The bytecode virtual machine
pub struct VM<'a, T: ResourceTracker> {
    /// Operand stack - values being computed
    stack: Vec<Value>,

    /// Call stack - function frames
    frames: Vec<CallFrame>,

    /// Current instruction pointer (index into current frame's bytecode)
    ip: usize,

    /// Heap for reference-counted objects (existing)
    heap: &'a mut Heap<T>,

    /// Namespace stack (existing, modified)
    namespaces: &'a mut Namespaces,

    /// Interned strings/bytes (existing)
    interns: &'a Interns,

    /// Exception handler stack
    exception_handlers: Vec<ExceptionHandler>,

    /// Current exception being handled (if any)
    current_exception: Option<Value>,
}
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

```rust
impl<'a, T: ResourceTracker> VM<'a, T> {
    pub fn run(&mut self) -> VMResult {
        loop {
            // Fetch
            let op = self.fetch_op();

            // Decode & Execute
            match op {
                Op::LoadConst(idx) => {
                    let value = self.current_frame().code.constants.get(idx);
                    self.push(value.clone_with_heap(self.heap));
                }

                Op::BinaryAdd => {
                    let rhs = self.pop();
                    let lhs = self.pop();
                    let result = lhs.py_add(&rhs, self.heap, self.interns)?;
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    self.push(result.unwrap_or(Value::None)); // TODO: TypeError
                }

                Op::CallExternal(func_id, arg_count) => {
                    let args = self.pop_n(arg_count);
                    return VMResult::ExternalCall {
                        function_id: func_id,
                        args: ArgValues::from_vec(args),
                        // State is implicitly: self.ip, self.stack, self.frames
                    };
                }

                Op::ReturnValue => {
                    let value = self.pop();
                    if self.frames.len() == 1 {
                        return VMResult::Complete(value);
                    }
                    self.pop_frame();
                    self.push(value);
                }

                // ... other ops
            }
        }
    }

    /// Resume after external call returns
    pub fn resume(&mut self, result: Value) -> VMResult {
        self.push(result);
        self.run()
    }
}
```

**Performance optimizations in the loop:**
1. **Computed goto** (if using unsafe): Jump table instead of match
2. **Stack caching**: Keep TOS in register
3. **Opcode specialization**: Separate ops for common patterns
4. **Inline caching**: Cache method lookups (future JIT prep)

---

## Phase 3: Bytecode Compiler

### 3.1 Compiler Structure

```rust
/// Compiles prepared AST to bytecode
pub struct Compiler<'a> {
    /// Current code being built
    code: CodeBuilder,

    /// Loop stack for break/continue
    loop_stack: Vec<LoopInfo>,

    /// Try stack for exception handlers
    try_stack: Vec<TryInfo>,

    /// Interns reference
    interns: &'a Interns,
}

struct CodeBuilder {
    bytecode: Vec<u8>,
    constants: Vec<Value>,
    line_table: Vec<(usize, u32)>,
    exception_table: Vec<ExceptionEntry>,
    num_locals: u16,
    max_stack: u16,
    current_stack: u16,
}
```

### 3.2 Expression Compilation

```rust
impl Compiler<'_> {
    fn compile_expr(&mut self, expr: &ExprLoc) {
        match &expr.expr {
            Expr::Literal(lit) => self.compile_literal(lit),

            Expr::Name(ident) => {
                match ident.scope {
                    NameScope::Local => self.emit(Op::LoadLocal(ident.slot)),
                    NameScope::Global => self.emit(Op::LoadGlobal(ident.slot)),
                    NameScope::Cell => self.emit(Op::LoadCell(ident.slot)),
                }
            }

            Expr::Op { left, op, right } => {
                // Short-circuit AND/OR
                if *op == Operator::And {
                    self.compile_expr(left);
                    let jump = self.emit_jump(Op::JumpIfFalseOrPop(0));
                    self.compile_expr(right);
                    self.patch_jump(jump);
                } else if *op == Operator::Or {
                    self.compile_expr(left);
                    let jump = self.emit_jump(Op::JumpIfTrueOrPop(0));
                    self.compile_expr(right);
                    self.patch_jump(jump);
                } else {
                    self.compile_expr(left);
                    self.compile_expr(right);
                    self.emit(op_to_binary_op(*op));
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
                self.emit(Op::Pop);  // Discard result
            }

            Node::Assign { target, object } => {
                self.compile_expr(object);
                self.compile_store(target);
            }

            Node::If { test, body, or_else } => {
                self.compile_expr(test);
                let else_jump = self.emit_jump(Op::JumpIfFalse(0));
                self.compile_block(body);

                if !or_else.is_empty() {
                    let end_jump = self.emit_jump(Op::Jump(0));
                    self.patch_jump(else_jump);
                    self.compile_block(or_else);
                    self.patch_jump(end_jump);
                } else {
                    self.patch_jump(else_jump);
                }
            }

            Node::For { target, iter, body, or_else } => {
                self.compile_expr(iter);
                self.emit(Op::GetIter);

                let loop_start = self.current_offset();
                self.loop_stack.push(LoopInfo { start: loop_start, breaks: vec![] });

                let end_jump = self.emit_jump(Op::ForIter(0));
                self.compile_store(target);
                self.compile_block(body);
                self.emit_jump_to(Op::Jump(0), loop_start);

                self.patch_jump(end_jump);
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
        func_compiler.emit(Op::LoadNone);
        func_compiler.emit(Op::ReturnValue);

        func_compiler.code.build()
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

        // Create VM
        let mut vm = VM::new(&mut heap, &mut namespaces, &self.interns);
        vm.push_frame(module_code, GLOBAL_NS_IDX);

        // Run
        match vm.run() {
            VMResult::Complete(value) => ExecutorResult::Complete(value),
            VMResult::ExternalCall { function_id, args } => {
                ExecutorResult::ExternalCall(ExternalCall {
                    function_id,
                    args,
                    vm_state: vm.snapshot(),  // Serialize VM state
                })
            }
            VMResult::Error(exc) => ExecutorResult::Error(exc),
        }
    }
}
```

### 4.4 Snapshot/Resume (Simplified!)

```rust
/// VM state for pause/resume - much simpler than current approach!
#[derive(Serialize, Deserialize)]
pub struct VMSnapshot {
    /// Operand stack
    stack: Vec<Value>,

    /// Call frames (each contains ip, namespace_idx, cells)
    frames: Vec<SerializedFrame>,

    /// Exception handler stack
    exception_handlers: Vec<ExceptionHandler>,
}

impl VM<'_, T> {
    pub fn snapshot(&self) -> VMSnapshot {
        VMSnapshot {
            stack: self.stack.clone(),
            frames: self.frames.iter().map(|f| f.serialize()).collect(),
            exception_handlers: self.exception_handlers.clone(),
        }
    }

    pub fn restore(snapshot: VMSnapshot, heap: &mut Heap<T>, ...) -> Self {
        // Reconstruct VM from snapshot
    }
}
```

**Why this is simpler:**
- No position tracking (IP is the position)
- No re-evaluation of expressions (values are on the stack)
- No ext_return_values cache (just push result and continue)
- No ClauseState (control flow is encoded in bytecode jumps)

---

## Phase 5: Performance Optimizations

### 5.1 Opcode Specialization

Create specialized opcodes for common patterns:

```rust
// Instead of: LoadLocal(0), LoadLocal(1), BinaryAdd
Op::AddLocals(u16, u16),  // Add two locals directly

// Instead of: LoadConst(int), BinaryAdd
Op::AddInt(i16),  // Add small int to TOS

// Instead of: LoadLocal(0)
Op::LoadLocal0,   // Most common: first local (self, first param)
Op::LoadLocal1,
Op::LoadLocal2,
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

Keep top-of-stack in a register:

```rust
impl VM {
    fn run(&mut self) -> VMResult {
        let mut tos: Option<Value> = None;  // Cached TOS

        loop {
            match self.fetch_op() {
                Op::LoadLocal(slot) => {
                    if let Some(v) = tos.take() {
                        self.stack.push(v);
                    }
                    tos = Some(self.load_local(slot));
                }
                Op::BinaryAdd => {
                    let rhs = tos.take().unwrap();
                    let lhs = self.pop();
                    tos = Some(lhs.py_add(&rhs, ...)?);
                }
                // ...
            }
        }
    }
}
```

### 5.4 Bytecode Threading (Advanced)

For maximum performance, use **direct threading** with computed goto:

```rust
// Requires unsafe and platform-specific code
// Each opcode handler ends with: goto *handlers[*ip++]
```

This eliminates the match dispatch overhead entirely.

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
Op::Await,  // Pause current task, wait for TOS (another task or future)
Op::Yield,  // Generator yield (related but different)
```

### 7.5 Benefits of Bytecode for Async

1. **Easy task switching**: Just save/restore VMSnapshot
2. **Shared heap**: All tasks use same heap (single-threaded, no locks)
3. **Fair scheduling**: Can preempt after N instructions
4. **Deterministic**: Same bytecode = same behavior

---

## Migration Strategy

### Step 1: Define Core Types (1 week)
- [ ] `Op` enum with all opcodes
- [ ] `Code` struct
- [ ] `CodeBuilder` for emission
- [ ] Unit tests for encoding/decoding

### Step 2: Basic Compiler (2 weeks)
- [ ] Compile literals and simple expressions
- [ ] Compile variables (local/global)
- [ ] Compile binary/unary operators
- [ ] Compile if/else statements
- [ ] Compile for loops
- [ ] Run in parallel with tree-walker for testing

### Step 3: VM Core (2 weeks)
- [ ] `VM` struct with stack and frames
- [ ] Main dispatch loop
- [ ] All arithmetic/comparison ops
- [ ] Variable load/store
- [ ] Control flow (jumps)

### Step 4: Functions & Closures (1 week)
- [ ] Function calls
- [ ] Return values
- [ ] Closures with captured cells
- [ ] Default parameters

### Step 5: Exception Handling (1 week)
- [ ] Try/except/finally compilation
- [ ] Exception handler stack
- [ ] Raise/reraise

### Step 6: External Calls & Snapshots (1 week)
- [ ] `CallExternal` opcode
- [ ] VMSnapshot serialization
- [ ] Resume mechanism
- [ ] Integration with existing `RunProgress` API

### Step 7: Remove Old Code (1 week)
- [ ] Delete `evaluate.rs`
- [ ] Delete `run_frame.rs` tree-walker
- [ ] Delete `SnapshotTracker`, `ClauseState`
- [ ] Update all tests

### Step 8: Performance Tuning (ongoing)
- [ ] Profile hot paths
- [ ] Add specialized opcodes
- [ ] Implement stack caching
- [ ] Add inline caches

---

## Testing Strategy

### Unit Tests
- Opcode encoding/decoding
- Compiler output for each node type
- VM execution of each opcode

### Integration Tests
- Existing `test_cases/*.py` files should pass unchanged
- Compare bytecode output vs tree-walker output

### Performance Tests
- Benchmark suite comparing tree-walker vs bytecode
- Track regression in CI

### Fuzz Testing
- Random bytecode sequences (for VM robustness)
- Random AST → bytecode → execute

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
