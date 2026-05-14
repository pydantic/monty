//! Builder for emitting bytecode during compilation.
//!
//! `CodeBuilder` provides methods for emitting opcodes and operands, handling
//! forward jumps with patching, and tracking source locations for tracebacks.

use std::collections::HashSet;

use super::{
    code::{Code, ConstPool, ExceptionEntry, LocationEntry},
    op::Opcode,
};
use crate::{intern::StringId, parse::CodeRange, value::Value};

/// State of the abstract operand-stack tracker as the builder emits bytecode.
///
/// The compiler tracks "what the operand stack looks like at the point we're
/// about to emit the next opcode" so that it can record correct stack depths
/// in `ExceptionEntry` and check the merge invariant at jump patches. Control
/// flow makes that tracker into a small state machine:
///
/// * `Live(depth)` — the most recently emitted opcode falls through to the
///   next byte. `depth` is the operand-stack height there. `adjust_stack`
///   updates `depth`; `set_stack_depth` overrides it absolutely.
/// * `Dead` — the most recently emitted opcode unconditionally diverts
///   control flow (`Jump`, `ReturnValue`, `Raise`, `Reraise`), so subsequent
///   bytes are unreachable via fall-through. The depth has no meaningful
///   value until something re-establishes it: a `patch_jump` whose label
///   carries the jump-taken target depth, or an explicit `set_stack_depth`
///   for code reached via the exception table (handler entries).
///
/// Modeling this explicitly removes the need for compilers to manually save
/// and restore `dead_code_depth` around terminating statements — emit-of-
/// terminator transitions to `Dead`, and `patch_jump` transitions back to
/// `Live` from the label's recorded target depth. Stack-effect adjustments
/// in `Dead` are no-ops, so dead code can compile freely without poisoning
/// the tracker.
#[derive(Debug, Clone, Copy)]
enum TrackerState {
    Live(u16),
    Dead,
}

impl Default for TrackerState {
    fn default() -> Self {
        Self::Live(0)
    }
}

/// Operand bundle paired with an opcode at emit time.
///
/// Every linear (non-jump) emit funnels through `emit_with_operand(op, operand)`,
/// which writes the bytes for `operand` and consults `stack_effect_for(op, &operand)`
/// for the stack adjustment. Carrying the operand through this enum makes
/// stack-effect computation a single, exhaustive match — variable-effect
/// opcodes without an explicit arm panic instead of silently drifting.
#[derive(Debug)]
enum Operand<'a> {
    /// No operand bytes (e.g. `Pop`, `BinaryAdd`).
    None,
    /// Single u8 operand (e.g. `LoadLocal`, `CallFunction`).
    U8(u8),
    /// Single i8 operand.
    I8(i8),
    /// Single u16 operand, little-endian (e.g. `LoadConst`, `BuildList`).
    U16(u16),
    /// Two u8 operands (e.g. `UnpackEx`, `CallBuiltinFunction`).
    U8U8(u8, u8),
    /// u8 then u16 little-endian (e.g. `LoadLocalCallable`).
    U8U16(u8, u16),
    /// u16 little-endian then u8 (e.g. `MakeFunction`, `CallAttr`).
    U16U8(u16, u8),
    /// Two u16 little-endian (e.g. `LoadLocalCallableW`, `LoadGlobalCallable`).
    U16U16(u16, u16),
    /// u16 then two u8s (e.g. `MakeClosure`).
    U16U8U8(u16, u8, u8),
    /// `CallFunctionKw` shape: pos_count (u8), kw_count (u8), kw_count * name_id (u16 each).
    CallKw { pos_count: u8, kwname_ids: &'a [u16] },
    /// `CallAttrKw` shape: attr_name_id (u16), pos_count (u8), kw_count (u8), kw_count * name_id (u16 each).
    CallAttrKw {
        attr_name_id: u16,
        pos_count: u8,
        kwname_ids: &'a [u16],
    },
}

/// Builder for emitting bytecode during compilation.
///
/// Handles encoding opcodes and operands into raw bytes, managing forward jumps
/// that need patching, and tracking source locations for traceback generation.
///
/// # Usage
///
/// ```ignore
/// let mut builder = CodeBuilder::new();
/// builder.set_location(some_range, None);
/// builder.emit(Opcode::LoadNone);
/// builder.emit_u8(Opcode::LoadLocal, 0);
/// let jump = builder.emit_jump(Opcode::JumpIfFalse);
/// // ... emit more code ...
/// builder.patch_jump(jump);
/// let code = builder.build(num_locals);
/// ```
#[derive(Debug, Default)]
pub struct CodeBuilder {
    /// The bytecode being built.
    bytecode: Vec<u8>,

    /// Constants collected during compilation.
    constants: Vec<Value>,

    /// Source location entries for traceback generation.
    location_table: Vec<LocationEntry>,

    /// Exception handler entries.
    exception_table: Vec<ExceptionEntry>,

    /// Current source location (set before emitting instructions).
    current_location: Option<CodeRange>,

    /// Current focus location within the source range.
    current_focus: Option<CodeRange>,

    /// Current operand-stack tracker state — see `TrackerState`.
    tracker: TrackerState,

    /// Maximum stack depth seen during compilation.
    max_stack_depth: u16,

    /// Local variable names indexed by slot number.
    ///
    /// Populated during compilation to enable proper NameError messages
    /// when accessing undefined local variables.
    local_names: Vec<Option<StringId>>,

    /// Local variable slots that are assigned somewhere in this function.
    ///
    /// Used to determine whether to raise `UnboundLocalError` or `NameError`
    /// when loading an undefined local variable.
    assigned_locals: HashSet<u16>,
}

impl CodeBuilder {
    /// Creates a new empty CodeBuilder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the current source location for subsequent instructions.
    ///
    /// This location will be recorded in the location table when the next
    /// instruction is emitted. Call this before emitting instructions that
    /// correspond to source code.
    pub fn set_location(&mut self, range: CodeRange, focus: Option<CodeRange>) {
        self.current_location = Some(range);
        self.current_focus = focus;
    }

    /// Emits a no-operand instruction and updates stack depth tracking.
    pub fn emit(&mut self, op: Opcode) {
        self.emit_with_operand(op, &Operand::None);
    }

    /// Emits an instruction with a u8 operand.
    pub fn emit_u8(&mut self, op: Opcode, operand: u8) {
        self.emit_with_operand(op, &Operand::U8(operand));
    }

    /// Emits an instruction with an i8 operand.
    pub fn emit_i8(&mut self, op: Opcode, operand: i8) {
        self.emit_with_operand(op, &Operand::I8(operand));
    }

    /// Emits an instruction with two u8 operands.
    ///
    /// Used for `UnpackEx`: before_count + after_count.
    pub fn emit_u8_u8(&mut self, op: Opcode, operand1: u8, operand2: u8) {
        self.emit_with_operand(op, &Operand::U8U8(operand1, operand2));
    }

    /// Emits an instruction with a u16 operand (little-endian).
    pub fn emit_u16(&mut self, op: Opcode, operand: u16) {
        self.emit_with_operand(op, &Operand::U16(operand));
    }

    /// Emits an instruction with a u16 operand followed by a u8 operand.
    ///
    /// Used for `MakeFunction`, `CallAttr`, `CallAttrExtended`.
    pub fn emit_u16_u8(&mut self, op: Opcode, operand1: u16, operand2: u8) {
        self.emit_with_operand(op, &Operand::U16U8(operand1, operand2));
    }

    /// Emits an instruction with a u16 operand followed by two u8 operands.
    ///
    /// Used for `MakeClosure`: func_id + defaults_count + cell_count.
    pub fn emit_u16_u8_u8(&mut self, op: Opcode, operand1: u16, operand2: u8, operand3: u8) {
        self.emit_with_operand(op, &Operand::U16U8U8(operand1, operand2, operand3));
    }

    /// Emits `CallBuiltinFunction`: builtin_id (u8) + arg_count (u8).
    ///
    /// The builtin_id is the `#[repr(u8)]` discriminant of `BuiltinsFunctions`.
    /// This is an optimization that avoids constant pool lookup and stack manipulation.
    pub fn emit_call_builtin_function(&mut self, builtin_id: u8, arg_count: u8) {
        self.emit_with_operand(Opcode::CallBuiltinFunction, &Operand::U8U8(builtin_id, arg_count));
    }

    /// Emits `CallBuiltinType`: type_id (u8) + arg_count (u8).
    ///
    /// The type_id is the `#[repr(u8)]` discriminant of `BuiltinsTypes`.
    /// This is an optimization for type constructors like `list()`, `int()`, `str()`.
    pub fn emit_call_builtin_type(&mut self, type_id: u8, arg_count: u8) {
        self.emit_with_operand(Opcode::CallBuiltinType, &Operand::U8U8(type_id, arg_count));
    }

    /// Emits `CallFunctionKw` with inline keyword names.
    ///
    /// Operands: pos_count (u8) + kw_count (u8) + kw_count * name_id (u16 each).
    /// `kwname_ids` is the StringId for each keyword argument, in the order
    /// the values were pushed onto the operand stack.
    pub fn emit_call_function_kw(&mut self, pos_count: u8, kwname_ids: &[u16]) {
        self.emit_with_operand(Opcode::CallFunctionKw, &Operand::CallKw { pos_count, kwname_ids });
    }

    /// Emits `CallAttrKw` with inline keyword names.
    ///
    /// Operands: attr_name_id (u16) + pos_count (u8) + kw_count (u8) + kw_count * name_id (u16 each).
    pub fn emit_call_attr_kw(&mut self, attr_name_id: u16, pos_count: u8, kwname_ids: &[u16]) {
        self.emit_with_operand(
            Opcode::CallAttrKw,
            &Operand::CallAttrKw {
                attr_name_id,
                pos_count,
                kwname_ids,
            },
        );
    }

    /// Emits a forward jump instruction, returning a label to patch later.
    ///
    /// The jump offset is initially set to 0 and must be patched with
    /// `patch_jump()` once the target location is known.
    ///
    /// The returned label carries the bytecode offset to patch and the stack
    /// depth that the *jump-taken* path leaves on the stack at the patch
    /// target. `patch_jump` uses that depth to enforce the merge invariant
    /// (every branch arriving at the patch point agrees on stack depth) and
    /// to transition the tracker out of `Dead` when fall-through is
    /// unreachable. Forward jumps differ in how they affect the two paths:
    ///
    /// | Opcode                                 | fall-through effect | jump-taken target depth |
    /// |----------------------------------------|---------------------|-------------------------|
    /// | `Jump`                                 | n/a (dead)          | unchanged from pre-emit |
    /// | `JumpIfTrue` / `JumpIfFalse`           | `-1` (cond popped)  | pre-emit `- 1`          |
    /// | `JumpIfTrueOrPop` / `JumpIfFalseOrPop` | `-1` (cond popped)  | pre-emit (cond kept)    |
    /// | `ForIter`                              | `+1` (value pushed) | pre-emit `- 1` (iter popped) |
    ///
    /// After `Jump` the tracker becomes `Dead` (it's unconditional). All
    /// other jumps continue to fall through.
    ///
    /// # Panics
    ///
    /// Panics if called from the `Dead` state — emitting a jump in dead code
    /// produces unreachable bytecode and obscures the merge invariant.
    /// Callers must guard with `is_dead()` if they may reach this from a
    /// terminating block (see e.g. `compile_if`'s if-else bridge).
    #[must_use]
    pub fn emit_jump(&mut self, op: Opcode) -> JumpLabel {
        let TrackerState::Live(pre_depth) = self.tracker else {
            panic!("emit_jump({op:?}) called from Dead state — guard with is_dead() at the call site")
        };

        self.record_location();
        let offset = self.bytecode.len();
        self.bytecode.push(op as u8);
        // Placeholder for i16 offset (will be patched)
        self.bytecode.extend_from_slice(&0i16.to_le_bytes());

        let (fallthrough_effect, target_depth): (i16, u16) = match op {
            Opcode::Jump => (0, pre_depth),
            Opcode::JumpIfTrue | Opcode::JumpIfFalse => (-1, pre_depth.saturating_sub(1)),
            Opcode::JumpIfTrueOrPop | Opcode::JumpIfFalseOrPop => (-1, pre_depth),
            Opcode::ForIter => (1, pre_depth.saturating_sub(1)),
            _ => panic!("emit_jump called with non-jump opcode {op:?}"),
        };
        self.adjust_stack(fallthrough_effect);

        if op == Opcode::Jump {
            self.mark_dead();
        }

        JumpLabel { offset, target_depth }
    }

    /// Patches a forward jump to point to the current bytecode location.
    ///
    /// The offset is calculated relative to the position after the jump
    /// instruction's operand (i.e., where execution would continue if
    /// the jump is not taken).
    ///
    /// Tracker state transitions: if the tracker is `Dead` (because the
    /// last live emission was a terminator), `patch_jump` re-establishes
    /// `Live(label.target_depth)` from the label. If the tracker is already
    /// `Live`, it asserts agreement — the merge invariant.
    ///
    /// # Panics
    ///
    /// - In debug builds, panics if the tracker is `Live` and disagrees with
    ///   `label.target_depth` — this means two reachable paths arrive at the
    ///   patch point with different stack heights.
    /// - Always panics if the jump offset exceeds i16 range (-32768..32767),
    ///   which indicates the function is too large. This is a compile-time
    ///   error rather than silent truncation.
    pub fn patch_jump(&mut self, label: JumpLabel) {
        let target = self.bytecode.len();
        // Offset is relative to position after the jump instruction (opcode + i16 = 3 bytes)
        let target_i64 = i64::try_from(target).expect("bytecode target exceeds i64");
        let label_i64 = i64::try_from(label.offset).expect("bytecode label exceeds i64");
        let raw_offset = target_i64 - label_i64 - 3;
        let offset =
            i16::try_from(raw_offset).expect("jump offset exceeds i16 range (-32768..32767); function too large");
        let bytes = offset.to_le_bytes();
        self.bytecode[label.offset + 1] = bytes[0];
        self.bytecode[label.offset + 2] = bytes[1];

        match self.tracker {
            TrackerState::Live(d) => debug_assert_eq!(
                d, label.target_depth,
                "stack-depth mismatch at jump merge: builder tracker is {d} but jump label expects {}; \
                 branches reaching this merge point disagree on stack state",
                label.target_depth,
            ),
            TrackerState::Dead => self.set_stack_depth(label.target_depth),
        }
    }

    /// Emits a backward jump to a known target offset.
    ///
    /// Unlike forward jumps, backward jumps have a known target at emit time,
    /// so no patching is needed. Only fixed-effect opcodes are supported here
    /// (in practice, just `Jump` and the conditional jumps used for
    /// comprehension filters); branch-dependent jumps like `ForIter` or the
    /// `OrPop` variants would need per-branch target depths that this
    /// label-less path can't carry — panic if misused.
    ///
    /// After `Jump` (unconditional), the tracker transitions to `Dead`.
    /// Panics if called from `Dead` — emitting a backward jump in dead code
    /// produces unreachable bytecode and is always a compiler bug.
    pub fn emit_jump_to(&mut self, op: Opcode, target: usize) {
        assert!(
            !self.is_dead(),
            "emit_jump_to({op:?}) called from Dead state — guard with is_dead() at the call site"
        );
        self.record_location();
        let current = self.bytecode.len();
        // Offset is relative to position after this instruction (current + 3)
        let target_i64 = i64::try_from(target).expect("bytecode target exceeds i64");
        let current_i64 = i64::try_from(current).expect("bytecode offset exceeds i64");
        let raw_offset = target_i64 - (current_i64 + 3);
        let offset =
            i16::try_from(raw_offset).expect("jump offset exceeds i16 range (-32768..32767); function too large");
        self.bytecode.push(op as u8);
        self.bytecode.extend_from_slice(&offset.to_le_bytes());
        let effect = op.stack_effect().unwrap_or_else(|| {
            panic!("variable-effect opcode {op:?} emitted via emit_jump_to — only fixed-effect jumps are supported on the backward-jump path")
        });
        self.adjust_stack(effect);
        if op == Opcode::Jump {
            self.mark_dead();
        }
    }

    /// Returns the current bytecode offset.
    ///
    /// Use this to record loop start positions for backward jumps.
    #[must_use]
    pub fn current_offset(&self) -> usize {
        self.bytecode.len()
    }

    /// Emits `LoadLocal`, using specialized opcodes for slots 0-3.
    ///
    /// Slots 0-3 use zero-operand opcodes (`LoadLocal0`, etc.) for efficiency.
    /// Slots 4-255 use `LoadLocal` with a u8 operand.
    /// Slots 256+ use `LoadLocalW` with a u16 operand.
    /// Registers a local variable name for a given slot.
    ///
    /// This is called during compilation when we encounter a variable access.
    /// The name is used to generate proper NameError messages.
    pub fn register_local_name(&mut self, slot: u16, name: StringId) {
        let slot_idx = slot as usize;
        // Extend the vector if needed
        if slot_idx >= self.local_names.len() {
            self.local_names.resize(slot_idx + 1, None);
        }
        // Only set if not already set (first occurrence determines the name)
        if self.local_names[slot_idx].is_none() {
            self.local_names[slot_idx] = Some(name);
        }
    }

    /// Registers a local variable slot as "assigned" (vs undefined reference).
    ///
    /// Called during compilation for variables that are assigned somewhere in the function.
    /// Used at runtime to determine whether to raise `UnboundLocalError` (assigned local
    /// accessed before assignment) or `NameError` (name doesn't exist anywhere).
    pub fn register_assigned_local(&mut self, slot: u16) {
        self.assigned_locals.insert(slot);
    }

    /// Emits a `LoadLocal` instruction, using specialized variants for common slots.
    pub fn emit_load_local(&mut self, slot: u16) {
        match slot {
            0 => self.emit(Opcode::LoadLocal0),
            1 => self.emit(Opcode::LoadLocal1),
            2 => self.emit(Opcode::LoadLocal2),
            3 => self.emit(Opcode::LoadLocal3),
            _ => {
                if let Ok(s) = u8::try_from(slot) {
                    self.emit_u8(Opcode::LoadLocal, s);
                } else {
                    self.emit_u16(Opcode::LoadLocalW, slot);
                }
            }
        }
    }

    /// Emits a `LoadLocalCallable` instruction for call-context loads.
    ///
    /// Unlike `emit_load_local`, this does NOT use specialized 0-3 variants since
    /// external function calls are rare enough that the optimization isn't worth
    /// the extra opcode slots. The `name_id` is encoded directly in the operand
    /// to avoid needing to look up the name from the code's local_names array.
    pub fn emit_load_local_callable(&mut self, slot: u16, name_id: StringId) {
        let name_id_u16 = u16::try_from(name_id.index()).expect("name_id exceeds u16");
        if let Ok(s) = u8::try_from(slot) {
            self.emit_with_operand(Opcode::LoadLocalCallable, &Operand::U8U16(s, name_id_u16));
        } else {
            self.emit_with_operand(Opcode::LoadLocalCallableW, &Operand::U16U16(slot, name_id_u16));
        }
    }

    /// Emits a `LoadGlobalCallable` instruction for call-context loads.
    ///
    /// The `name_id` is encoded directly in the operand to avoid the ambiguity
    /// of looking up global names from a function's local_names array (global slots
    /// and local slots use different namespaces).
    pub fn emit_load_global_callable(&mut self, slot: u16, name_id: StringId) {
        let name_id_u16 = u16::try_from(name_id.index()).expect("name_id exceeds u16");
        self.emit_with_operand(Opcode::LoadGlobalCallable, &Operand::U16U16(slot, name_id_u16));
    }

    /// Emits `StoreLocal`, using wide variant for slots > 255.
    pub fn emit_store_local(&mut self, slot: u16) {
        if let Ok(s) = u8::try_from(slot) {
            self.emit_u8(Opcode::StoreLocal, s);
        } else {
            self.emit_u16(Opcode::StoreLocalW, slot);
        }
    }

    /// Adds a constant to the pool, returning its index.
    ///
    /// # Panics
    ///
    /// Panics if the constant pool exceeds 65535 entries. This is a compile-time
    /// error indicating the function has too many constants.
    #[must_use]
    pub fn add_const(&mut self, value: Value) -> u16 {
        let idx = self.constants.len();
        let idx_u16 = u16::try_from(idx).expect("constant pool exceeds u16 range (65535); too many constants");
        self.constants.push(value);
        idx_u16
    }

    /// Adds an exception handler entry.
    ///
    /// Entries should be added in innermost-first order for nested try blocks.
    pub fn add_exception_entry(&mut self, entry: ExceptionEntry) {
        self.exception_table.push(entry);
    }

    /// Returns the current tracked stack depth.
    ///
    /// # Panics
    ///
    /// Panics if the tracker is in the `Dead` state. Callers that capture
    /// depth (e.g. `compile_for`'s `loop_exit_depth`) only ever do so from
    /// reachable code, so being `Dead` here indicates a compiler bug.
    #[must_use]
    pub fn stack_depth(&self) -> u16 {
        match self.tracker {
            TrackerState::Live(d) => d,
            TrackerState::Dead => panic!(
                "stack_depth() called while tracker is in Dead state — \
                 callers should only read depth from reachable code"
            ),
        }
    }

    /// Reports whether the tracker is in the dead-code state.
    ///
    /// Used by compile_block to stop emitting after a terminator and by emit
    /// helpers to decide whether to bother computing live target depths.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        matches!(self.tracker, TrackerState::Dead)
    }

    /// Builds the final Code object.
    ///
    /// Consumes the builder and returns a Code object containing the
    /// compiled bytecode and all metadata.
    #[must_use]
    pub fn build(self, num_locals: u16) -> Code {
        // Convert local_names from Vec<Option<StringId>> to Vec<StringId>,
        // using StringId::default() for slots with no recorded name
        let local_names: Vec<StringId> = self.local_names.into_iter().map(Option::unwrap_or_default).collect();

        Code::new(
            self.bytecode,
            ConstPool::from_vec(self.constants),
            self.location_table,
            self.exception_table,
            num_locals,
            self.max_stack_depth,
            local_names,
            self.assigned_locals,
        )
    }

    /// Records the current location in the location table if set.
    fn record_location(&mut self) {
        if let Some(range) = self.current_location {
            let offset = u32::try_from(self.bytecode.len()).expect("bytecode length exceeds u32");
            self.location_table
                .push(LocationEntry::new(offset, range, self.current_focus));
        }
    }

    /// Sets the current stack depth to an absolute value, transitioning to
    /// `Live` regardless of prior state.
    ///
    /// Use this for points reached via the exception table (handler entries,
    /// finally cleanup) where the depth comes from outside the fall-through
    /// graph. For ordinary forward-jump merges, `patch_jump` is enough — it
    /// transitions `Dead → Live` automatically using the label's recorded
    /// target depth.
    pub fn set_stack_depth(&mut self, depth: u16) {
        self.tracker = TrackerState::Live(depth);
        self.max_stack_depth = self.max_stack_depth.max(depth);
    }

    /// Adjusts the stack depth by the given delta.
    ///
    /// Positive values indicate pushes, negative values indicate pops.
    /// In the `Dead` state this is a no-op: dead code can be emitted freely
    /// without poisoning the tracker, since the depth there is meaningless
    /// until re-established by a patch or `set_stack_depth`. Updates
    /// `max_stack_depth` if the new live depth exceeds it.
    fn adjust_stack(&mut self, delta: i16) {
        let TrackerState::Live(depth) = self.tracker else {
            return;
        };
        let new_depth = i32::from(depth) + i32::from(delta);
        // Stack depth shouldn't go negative (indicates compiler bug)
        debug_assert!(new_depth >= 0, "Stack depth went negative: {new_depth}");
        // Safe cast: new_depth is non-negative and stack won't exceed u16::MAX in practice
        let new_depth = u16::try_from(new_depth.max(0)).unwrap_or(u16::MAX);
        self.tracker = TrackerState::Live(new_depth);
        self.max_stack_depth = self.max_stack_depth.max(new_depth);
    }

    /// Transitions the tracker to the `Dead` state.
    ///
    /// Called internally after emitting unconditional terminators (`Jump`,
    /// `ReturnValue`, `Raise`, `Reraise`). Subsequent emits will not affect
    /// `current_stack_depth` until `patch_jump` or `set_stack_depth`
    /// re-establishes a live arrival depth.
    fn mark_dead(&mut self) {
        self.tracker = TrackerState::Dead;
    }

    /// Single-source-of-truth emit path: records location, writes opcode +
    /// operand bytes, applies the stack-effect computed by `stack_effect_for`,
    /// and transitions the tracker to `Dead` for unconditional terminators
    /// (`ReturnValue`, `Raise`, `Reraise`).
    ///
    /// `emit_jump` and `emit_jump_to` do *not* go through this path — they
    /// have branch-dependent effects and target-depth bookkeeping that don't
    /// fit the linear-emit shape.
    fn emit_with_operand(&mut self, op: Opcode, operand: &Operand<'_>) {
        self.record_location();
        self.bytecode.push(op as u8);
        match operand {
            Operand::None => {}
            Operand::U8(b) => self.bytecode.push(*b),
            Operand::I8(b) => self.bytecode.push(b.to_ne_bytes()[0]),
            Operand::U16(w) => self.bytecode.extend_from_slice(&w.to_le_bytes()),
            Operand::U8U8(a, b) => {
                self.bytecode.push(*a);
                self.bytecode.push(*b);
            }
            Operand::U8U16(a, w) => {
                self.bytecode.push(*a);
                self.bytecode.extend_from_slice(&w.to_le_bytes());
            }
            Operand::U16U8(w, b) => {
                self.bytecode.extend_from_slice(&w.to_le_bytes());
                self.bytecode.push(*b);
            }
            Operand::U16U16(w1, w2) => {
                self.bytecode.extend_from_slice(&w1.to_le_bytes());
                self.bytecode.extend_from_slice(&w2.to_le_bytes());
            }
            Operand::U16U8U8(w, b1, b2) => {
                self.bytecode.extend_from_slice(&w.to_le_bytes());
                self.bytecode.push(*b1);
                self.bytecode.push(*b2);
            }
            Operand::CallKw { pos_count, kwname_ids } => {
                self.bytecode.push(*pos_count);
                self.bytecode
                    .push(u8::try_from(kwname_ids.len()).expect("keyword count exceeds u8"));
                for &name_id in *kwname_ids {
                    self.bytecode.extend_from_slice(&name_id.to_le_bytes());
                }
            }
            Operand::CallAttrKw {
                attr_name_id,
                pos_count,
                kwname_ids,
            } => {
                self.bytecode.extend_from_slice(&attr_name_id.to_le_bytes());
                self.bytecode.push(*pos_count);
                self.bytecode
                    .push(u8::try_from(kwname_ids.len()).expect("keyword count exceeds u8"));
                for &name_id in *kwname_ids {
                    self.bytecode.extend_from_slice(&name_id.to_le_bytes());
                }
            }
        }
        self.adjust_stack(stack_effect_for(op, operand));
        if matches!(op, Opcode::ReturnValue | Opcode::Raise | Opcode::Reraise) {
            self.mark_dead();
        }
    }
}

/// Computes the operand-stack effect of `op` paired with `operand`.
///
/// All variable-effect opcodes get an explicit match arm. Fixed-effect opcodes
/// fall through to `op.stack_effect()`, which is `Some(_)` for them. A
/// variable-effect opcode that reaches the fallback (because its operand
/// shape doesn't match any enumerated arm) panics — preventing silent drift
/// when a new variable-effect opcode is added.
///
/// `MakeFunction`/`MakeClosure` have explicit arms even though `stack_effect()`
/// returns `Some(1)` for them — that fixed value is only correct with zero
/// defaults, so we override with `1 - defaults_count` for the general case.
fn stack_effect_for(op: Opcode, operand: &Operand<'_>) -> i16 {
    match (op, operand) {
        // === U8 operand, variable effect ===
        (Opcode::CallFunction, &Operand::U8(arg_count)) => -i16::from(arg_count),
        (Opcode::CallFunctionExtended, &Operand::U8(flags)) => -(1 + i16::from(flags & 0x01)),
        (Opcode::FormatValue, &Operand::U8(flags)) => {
            if flags & 0x04 != 0 {
                -1
            } else {
                0
            }
        }
        (Opcode::UnpackSequence, &Operand::U8(n)) => i16::from(n) - 1,

        // === U16 operand, variable effect ===
        (Opcode::BuildList | Opcode::BuildTuple | Opcode::BuildSet | Opcode::BuildFString, &Operand::U16(n)) => {
            1 - n.cast_signed()
        }
        (Opcode::BuildDict, &Operand::U16(n)) => 1 - 2 * n.cast_signed(),

        // === U8U8 operand, variable effect ===
        // UnpackEx: pops 1, pushes (before + 1 + after) → before + after.
        (Opcode::UnpackEx, &Operand::U8U8(before, after)) => i16::from(before) + i16::from(after),
        // Builtin calls: no callable on stack, pops args, pushes result → 1 - arg_count.
        (Opcode::CallBuiltinFunction | Opcode::CallBuiltinType, &Operand::U8U8(_, arg_count)) => {
            1 - i16::from(arg_count)
        }

        // === U16U8 operand, variable effect ===
        (Opcode::MakeFunction, &Operand::U16U8(_, defaults)) => 1 - i16::from(defaults),
        (Opcode::CallAttr, &Operand::U16U8(_, arg_count)) => -i16::from(arg_count),
        (Opcode::CallAttrExtended, &Operand::U16U8(_, flags)) => -(1 + i16::from(flags & 0x01)),

        // === U16U8U8 operand, variable effect ===
        (Opcode::MakeClosure, &Operand::U16U8U8(_, defaults, _)) => 1 - i16::from(defaults),

        // === Variable-length kw operands ===
        // pops callable + pos_args + kw_args, pushes result → -(pos_count + kw_count).
        (Opcode::CallFunctionKw, Operand::CallKw { pos_count, kwname_ids }) => {
            let kw_count = i16::try_from(kwname_ids.len()).expect("keyword count exceeds i16");
            -(i16::from(*pos_count) + kw_count)
        }
        (
            Opcode::CallAttrKw,
            Operand::CallAttrKw {
                pos_count, kwname_ids, ..
            },
        ) => {
            let kw_count = i16::try_from(kwname_ids.len()).expect("keyword count exceeds i16");
            -(i16::from(*pos_count) + kw_count)
        }

        // Fixed-effect fallback. Variable-effect opcodes (where stack_effect()
        // returns None) without an arm above hit the panic — keeps the tracker
        // honest if a new variable-effect opcode is added.
        (op, _) => op.stack_effect().unwrap_or_else(|| {
            panic!("stack_effect_for: variable-effect opcode {op:?} has no handler for operand variant {operand:?}")
        }),
    }
}

/// Label for a forward jump that needs patching.
///
/// Carries the bytecode offset where the jump instruction was emitted, plus
/// the stack depth that the jump-taken path leaves on the stack at the patch
/// target. `emit_jump` only runs in the live tracker state (panicking
/// otherwise), so this depth is always known when the label is constructed.
/// `patch_jump` uses it to enforce the merge invariant and to transition the
/// tracker back to `Live` when fall-through to the patch point is
/// unreachable.
#[derive(Debug, Clone, Copy)]
pub struct JumpLabel {
    offset: usize,
    target_depth: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emit_basic() {
        let mut builder = CodeBuilder::new();
        builder.emit(Opcode::LoadNone);
        builder.emit(Opcode::Pop);

        let code = builder.build(0);
        assert_eq!(code.bytecode(), &[Opcode::LoadNone as u8, Opcode::Pop as u8]);
    }

    #[test]
    fn test_emit_u8_operand() {
        let mut builder = CodeBuilder::new();
        builder.emit_u8(Opcode::LoadLocal, 42);

        let code = builder.build(0);
        assert_eq!(code.bytecode(), &[Opcode::LoadLocal as u8, 42]);
    }

    #[test]
    fn test_emit_u16_operand() {
        let mut builder = CodeBuilder::new();
        builder.emit_u16(Opcode::LoadConst, 0x1234);

        let code = builder.build(0);
        assert_eq!(code.bytecode(), &[Opcode::LoadConst as u8, 0x34, 0x12]);
    }

    #[test]
    fn test_forward_jump() {
        let mut builder = CodeBuilder::new();
        let jump = builder.emit_jump(Opcode::Jump);
        // The two LoadNones below are dead code (unconditional Jump above);
        // adjust_stack is a no-op in the Dead state, so they don't poison
        // tracking. patch_jump transitions the tracker back to Live using
        // the label's recorded target depth (0 here, unchanged from before
        // the Jump).
        builder.emit(Opcode::LoadNone); // 1 byte, skipped by jump
        builder.emit(Opcode::LoadNone); // 1 byte, skipped by jump
        builder.patch_jump(jump);
        builder.emit(Opcode::LoadNone); // Return value
        builder.emit(Opcode::ReturnValue);

        let code = builder.build(0);
        // Jump at offset 0, target at offset 5 (after 2x LoadNone)
        // Offset = 5 - 0 - 3 = 2
        assert_eq!(
            code.bytecode(),
            &[
                Opcode::Jump as u8,
                2,
                0, // i16 little-endian = 2
                Opcode::LoadNone as u8,
                Opcode::LoadNone as u8,
                Opcode::LoadNone as u8,
                Opcode::ReturnValue as u8,
            ]
        );
    }

    #[test]
    fn test_backward_jump() {
        let mut builder = CodeBuilder::new();
        let loop_start = builder.current_offset();
        builder.emit(Opcode::LoadNone); // offset 0, 1 byte
        builder.emit(Opcode::Pop); // offset 1, 1 byte
        builder.emit_jump_to(Opcode::Jump, loop_start); // offset 2, target 0

        let code = builder.build(0);
        // Jump at offset 2, target at offset 0
        // Offset = 0 - (2 + 3) = -5
        let expected_offset = (-5i16).to_le_bytes();
        assert_eq!(
            code.bytecode(),
            &[
                Opcode::LoadNone as u8,
                Opcode::Pop as u8,
                Opcode::Jump as u8,
                expected_offset[0],
                expected_offset[1],
            ]
        );
    }

    #[test]
    fn test_load_local_specialization() {
        let mut builder = CodeBuilder::new();
        builder.emit_load_local(0);
        builder.emit_load_local(1);
        builder.emit_load_local(2);
        builder.emit_load_local(3);
        builder.emit_load_local(4);
        builder.emit_load_local(256);

        let code = builder.build(0);
        assert_eq!(
            code.bytecode(),
            &[
                Opcode::LoadLocal0 as u8,
                Opcode::LoadLocal1 as u8,
                Opcode::LoadLocal2 as u8,
                Opcode::LoadLocal3 as u8,
                Opcode::LoadLocal as u8,
                4,
                Opcode::LoadLocalW as u8,
                0,
                1, // 256 in little-endian
            ]
        );
    }

    #[test]
    fn test_add_const() {
        let mut builder = CodeBuilder::new();
        let idx1 = builder.add_const(Value::Int(42));
        let idx2 = builder.add_const(Value::None);

        assert_eq!(idx1, 0);
        assert_eq!(idx2, 1);
    }
}
