//! Generator objects and the VM operations that run them.
//!
//! A generator's frame runs on the ordinary frame stack, so host calls inside it
//! suspend and snapshot like any other call. Advancing one is a call: `py_next`
//! pushes the frame and returns `FramePushed`, `yield` returns to the caller as
//! `ReturnValue` would (honouring `should_return`), and `return` raises
//! `StopIteration(value)` in the caller. `for`, `next()` and synchronous
//! consumers therefore drive generators through their existing frame handling.

use std::{fmt::Write, mem};

use monty_macros::pyattrs;
use serde::{Deserialize, Serialize};

use super::{CallFrame, VM, exceptions::CaughtException, namespace::function_namespace};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, FrameExit},
    exception_private::{ExcType, ExcTypeExt, ExceptionValue, RunError, RunResult, SimpleException},
    hash::{HashValue, identity_hash},
    heap::{
        ContainsHeap, DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput,
        HeapReader,
    },
    intern::{FunctionId, Interns},
    py_attrs::PyAttrs,
    py_value::PyValue,
    types::{LazyHeapSet, PyTrait, Type, TypeRef, py_trait::PyObjectIdentity},
    value::{EitherStr, Value},
};

/// The bytes of a `Yield` instruction: the opcode and its mode operand.
const YIELD_INSTRUCTION_LEN: usize = 2;

/// A generator object: a call to a function containing `yield`, whose frame runs
/// only while the generator is advanced.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Generator {
    /// The generator function, for `repr` and to find the frame's code.
    #[serde(rename = "F")]
    function_id: FunctionId,
    /// Owned: the defining function's globals dict, `None` under slot globals.
    #[serde(rename = "G", default)]
    globals: Option<HeapId>,
    #[serde(rename = "S")]
    state: GeneratorState,
}

/// Where a generator's frame state lives: saved in the generator, or on the VM stacks while it runs.
#[derive(Debug, Serialize, Deserialize)]
enum GeneratorState {
    /// Arguments bound; the body has not started.
    #[serde(rename = "C")]
    Created(SavedFrame),
    /// Paused after a `yield`; resuming pushes the sent value as its result.
    #[serde(rename = "S")]
    Suspended(SavedFrame),
    /// The frame is on the VM frame stack, which owns its values. Keeps the
    /// buffers emptied by resuming, so the next yield reuses their allocations.
    #[serde(rename = "R")]
    Running {
        #[serde(skip)]
        stack: Vec<Value>,
        #[serde(skip)]
        exceptions: Vec<CaughtException>,
    },
    /// Returned or raised; advancing it again reports exhaustion.
    #[serde(rename = "D")]
    Completed,
}

/// A paused generator frame, detached from the VM's stacks.
#[derive(Debug, Serialize, Deserialize)]
struct SavedFrame {
    #[serde(rename = "P")]
    ip: usize,
    #[serde(rename = "L")]
    locals_count: u16,
    /// Owned: the frame's locals, then its operands, from its stack base.
    #[serde(rename = "S")]
    stack: Vec<Value>,
    /// Owned: exceptions the frame is handling, so a bare `raise` after a yield re-raises them.
    #[serde(rename = "E")]
    exceptions: Vec<CaughtException>,
}

impl Generator {
    /// Creates a generator from bound locals, taking ownership of them and of `globals`.
    pub(super) fn new(function_id: FunctionId, locals: Vec<Value>, globals: Option<HeapId>) -> Self {
        let locals_count = u16::try_from(locals.len()).expect("function namespace size exceeds u16");
        Self {
            function_id,
            globals,
            state: GeneratorState::Created(SavedFrame {
                ip: 0,
                locals_count,
                stack: locals,
                exceptions: Vec::new(),
            }),
        }
    }

    /// What the host sees when one crosses the boundary: the sandbox `repr` without its address.
    pub(crate) fn boundary_repr(&self, interns: &Interns) -> String {
        format!("<generator object {}>", self.name(interns))
    }

    /// The generator function's name, which both reprs show.
    fn name<'a>(&self, interns: &'a Interns) -> &'a str {
        interns.get_str(interns.get_function(self.function_id).name.name_id)
    }

    /// Visits every owned heap reference, for cycle collection.
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Some(globals) = self.globals {
            on_child(globals);
        }
        if let GeneratorState::Created(saved) | GeneratorState::Suspended(saved) = &self.state {
            let exceptions = saved.exceptions.iter().map(CaughtException::value);
            for value in saved.stack.iter().chain(exceptions) {
                if let Value::Ref(id) = value {
                    on_child(*id);
                }
            }
        }
    }
}

impl HeapItem for Generator {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(globals) = self.globals.take() {
            stack.push(globals);
        }
        if let GeneratorState::Created(saved) | GeneratorState::Suspended(saved) = &mut self.state {
            let exceptions = saved.exceptions.iter_mut().map(CaughtException::value_mut);
            for value in saved.stack.iter_mut().chain(exceptions) {
                value.py_dec_ref_ids(stack);
            }
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Generator> {
    fn py_type(&self, _: &VM<'h>) -> TypeRef {
        Type::WalkGenerator.into()
    }

    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_is_iterable(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_is_iterator(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        Ok(CallResult::Value(self.clone_value(vm.heap)))
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        vm.resume_generator(self.id(), Value::None)
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        let name = self.get(vm.heap).name(vm.interns);
        let identity = self.py_identity().encoded();
        Ok(write!(f, "<generator object {name} at 0x{identity:x}>")?)
    }
}

/// The `generator` type's methods.
pub(crate) struct PyGeneratorImpl;

#[pyattrs]
impl<'heap> PyAttrs<'heap> for PyGeneratorImpl {
    type Receiver = HeapObjectRead<'heap, Generator>;

    #[pymethod(Send)]
    fn send<'h>(this: &HeapObjectRead<'h, Generator>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let value = args.get_one_arg("generator.send", vm.heap)?;
        vm.resume_generator(this.id(), value)?
            .ok_or_else(ExcType::stop_iteration)
    }

    #[pymethod(DunderNext)]
    fn next<'h>(this: &HeapObjectRead<'h, Generator>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        args.check_zero_args("generator.__next__", vm.heap)?;
        vm.resume_generator(this.id(), Value::None)?
            .ok_or_else(ExcType::stop_iteration)
    }
}

/// A frame's part in running generators; `Plain` for ordinary frames.
///
/// An enum rather than an `Option` and a flag, which would pad every `CallFrame` by 8 bytes.
/// A delegate frame advances its caller's `yield from`: the caller yields what the
/// frame produces, and resumes when it is exhausted.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) enum GeneratorRole {
    #[default]
    #[serde(rename = "P")]
    Plain,
    /// A generator's own body; see [`RunningGenerator`].
    #[serde(rename = "G")]
    Generator(RunningGenerator),
    /// A `__next__` method or frozen helper advancing a delegate.
    #[serde(rename = "D")]
    Delegate,
    /// A generator's body that is also its caller's delegate.
    #[serde(rename = "B")]
    DelegateGenerator(RunningGenerator),
}

impl GeneratorRole {
    /// Whether this is an ordinary frame, which serialization can omit.
    pub(super) fn is_plain(&self) -> bool {
        matches!(self, Self::Plain)
    }

    /// Whether this frame is a generator's own body.
    pub(super) fn is_generator(&self) -> bool {
        matches!(self, Self::Generator(_) | Self::DelegateGenerator(_))
    }

    /// Whether this frame advances its caller's `yield from` delegate.
    fn is_delegate(&self) -> bool {
        matches!(self, Self::Delegate | Self::DelegateGenerator(_))
    }

    /// Marks a frame just pushed by `yield from` as advancing its delegate.
    fn mark_delegate(&mut self) {
        *self = match mem::take(self) {
            Self::Generator(running) => Self::DelegateGenerator(running),
            _ => Self::Delegate,
        };
    }

    /// Takes the running generator's reference, keeping whether the frame is a delegate.
    fn take_running(&mut self) -> Option<RunningGenerator> {
        match mem::take(self) {
            Self::Generator(running) => Some(running),
            Self::DelegateGenerator(running) => {
                *self = Self::Delegate;
                Some(running)
            }
            other => {
                *self = other;
                None
            }
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for GeneratorRole {
    fn drop_with(self, ctx: &mut C) {
        if let Self::Generator(running) | Self::DelegateGenerator(running) = self {
            running.drop_with(ctx);
        }
    }
}

/// Owned reference from a running generator's frame to its [`Generator`], which
/// stays `Running` until the frame yields or returns. Dropping it means the frame
/// was abandoned (an exception unwound it, or its task or snapshot was discarded),
/// so the generator completes.
#[derive(Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RunningGenerator(HeapId);

impl<C: ContainsHeap> DropWithContext<C> for RunningGenerator {
    fn drop_with(self, ctx: &mut C) {
        set_state(ctx.heap_mut(), self.0, GeneratorState::Completed);
        ctx.heap_mut().dec_ref(self.0);
    }
}

/// Replaces a generator's state through a short-lived heap reader.
fn set_state(heap: &mut Heap, id: HeapId, state: GeneratorState) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        if let HeapReadOutput::Generator(generator) = heap.read(id) {
            generator.get_mut(heap).state = state;
        }
    });
}

impl VM<'_> {
    /// Creates the generator a call to a generator function returns, owning `locals`.
    pub(super) fn create_generator(
        &mut self,
        function_id: FunctionId,
        locals: Vec<Value>,
        globals: Option<HeapId>,
    ) -> Value {
        if let Some(globals) = globals {
            self.heap.inc_ref(globals);
        }
        let generator = Generator::new(function_id, locals, globals);
        Value::allocate(generator, self.heap)
    }

    /// Resumes a generator with `sent` (`None` for `next()`), pushing its frame.
    ///
    /// Returns `None` when it is already exhausted. The frame's yield becomes the
    /// call's result and its return raises `StopIteration` in the caller.
    pub(crate) fn resume_generator(&mut self, id: HeapId, sent: Value) -> RunResult<Option<CallResult>> {
        let mut sent = DropGuard::new(sent, self);
        let (value, this) = sent.as_parts();
        let HeapReadOutput::Generator(generator) = this.heap.read(id) else {
            unreachable!("resume_generator requires a generator")
        };
        match &generator.get(this.heap).state {
            GeneratorState::Running { .. } => return Err(ExcType::value_error("generator already executing")),
            GeneratorState::Completed => return Ok(None),
            GeneratorState::Created(_) if !matches!(value, Value::None) => {
                return Err(ExcType::type_error(
                    "can't send non-None value to a just-started generator",
                ));
            }
            GeneratorState::Created(_) | GeneratorState::Suspended(_) => {}
        }
        // Reserve the frame's recursion level before taking any state, so a
        // `RecursionError` leaves the generator resumable.
        if !this.current_frame.is_parked {
            this.incr_recursion()?;
        }
        let (sent, this) = sent.into_parts();
        let running = GeneratorState::Running {
            stack: Vec::new(),
            exceptions: Vec::new(),
        };
        let generator = generator.get_mut(this.heap);
        let (started, mut saved) = match mem::replace(&mut generator.state, running) {
            GeneratorState::Created(saved) => (false, saved),
            GeneratorState::Suspended(saved) => (true, saved),
            _ => unreachable!("checked above"),
        };
        let (function_id, globals) = (generator.function_id, generator.globals);

        let stack_base = this.stack.len();
        let exception_stack_base = this.exception_stack.len();
        this.stack.append(&mut saved.stack);
        this.exception_stack.append(&mut saved.exceptions);
        if started {
            this.push(sent);
        } else {
            sent.drop_with(this);
        }
        // Keep the emptied buffers for the next yield.
        if let HeapReadOutput::Generator(generator) = this.heap.read(id)
            && let GeneratorState::Running { stack, exceptions } = &mut generator.get_mut(this.heap).state
        {
            *stack = saved.stack;
            *exceptions = saved.exceptions;
        }

        this.heap.inc_ref(id);
        let mut frame = CallFrame::new_function(
            &this.interns.get_function(function_id).code,
            stack_base,
            saved.locals_count,
            exception_stack_base,
            function_id,
            this.current_offset(),
            function_namespace(globals, &*this.heap),
        );
        frame.ip = saved.ip;
        frame.generator = GeneratorRole::Generator(RunningGenerator(id));
        this.push_admitted_frame(frame);
        Ok(Some(CallResult::FramePushed))
    }

    /// Executes `Yield` in `YIELD_VALUE` mode: pops the value and suspends.
    ///
    /// Returns the value when the generator's frame was run by a synchronous
    /// consumer, which must leave the run loop with it.
    pub(super) fn yield_value(&mut self) -> Option<FrameExit> {
        let value = self.pop();
        self.yield_to_caller(value)
    }

    /// Suspends the current generator frame and gives `value` to its caller.
    ///
    /// A delegate's caller is an outer generator paused at its `yield from`,
    /// which yields the same value in turn, so this repeats up the chain.
    fn yield_to_caller(&mut self, value: Value) -> Option<FrameExit> {
        loop {
            let (should_return, delegate) = self.suspend_generator_frame();
            if should_return {
                return Some(FrameExit::Return(value));
            }
            if !delegate {
                self.push(value);
                return None;
            }
        }
    }

    /// Moves the current generator frame's state into its generator and
    /// restores the caller, returning the frame's `should_return` and delegate flags.
    fn suspend_generator_frame(&mut self) -> (bool, bool) {
        let caller = self.suspended_frames.pop().expect("a generator frame has a caller");
        let mut frame = mem::replace(&mut self.current_frame, caller);
        let RunningGenerator(id) = frame.generator.take_running().expect("yield outside a generator frame");
        // Generator frames only ever carry their function's globals, which the generator owns.
        frame.namespace.take().drop_with(self.heap);

        // Scoped so the reader is released before the `dec_ref` below can free the generator.
        {
            let HeapReadOutput::Generator(generator) = self.heap.read(id) else {
                unreachable!("a running generator frame owns its generator")
            };
            let generator = generator.get_mut(self.heap);
            let GeneratorState::Running {
                mut stack,
                mut exceptions,
            } = mem::replace(&mut generator.state, GeneratorState::Completed)
            else {
                unreachable!("a generator frame's generator is running")
            };
            stack.extend(self.stack.drain(frame.stack_base()..));
            exceptions.extend(self.exception_stack.drain(frame.exception_stack_base()..));
            generator.state = GeneratorState::Suspended(SavedFrame {
                ip: frame.ip,
                locals_count: frame.locals_count,
                stack,
                exceptions,
            });
        }

        self.instruction_ip = self.current_frame.ip;
        if !self.current_frame.is_parked {
            self.decr_recursion();
        }
        // Possibly the last reference: `next(make_generator())`.
        self.heap.dec_ref(id);
        (frame.should_return, frame.generator.is_delegate())
    }

    /// Executes `ReturnValue` in a generator frame: completes the generator and
    /// raises `StopIteration(value)` in the caller, or resumes a delegating caller.
    ///
    /// Returns an exception the caller's frames did not handle.
    pub(super) fn return_from_generator(&mut self, value: Value) -> Option<RunError> {
        let RunningGenerator(id) = self
            .current_frame
            .generator
            .take_running()
            .expect("generator return outside a generator frame");
        let delegate = self.current_frame.generator.is_delegate();
        let call_offset = self.current_frame.call_offset;
        set_state(self.heap.heap_mut(), id, GeneratorState::Completed);
        let should_return = self.pop_frame();
        self.heap.dec_ref(id);

        if delegate {
            self.finish_yield_from(value);
            return None;
        }
        // Raise at the call that advanced the generator, as for any failing callee.
        if let Some(offset) = call_offset {
            self.instruction_ip = offset as usize;
        }
        let exception = ExceptionValue::stop_iteration(value, self);
        let exception = Value::allocate(exception, self.heap);
        let error = self.make_exception(&exception, false);
        if should_return {
            exception.drop_with(self);
            return Some(error);
        }
        self.handle_exception_with_value(error, Some(exception))
    }

    /// Executes `Yield` in `YIELD_FROM` mode on `[iterator, sent]`.
    ///
    /// Each execution advances the delegate once. Rewinding to this instruction
    /// before yielding makes the next resume advance it again; exhaustion
    /// replaces the iterator with its `StopIteration` value and moves on.
    pub(super) fn yield_from(&mut self) -> RunResult<Option<FrameExit>> {
        let sent = self.pop();
        let Value::Ref(id) = *self.peek() else {
            sent.drop_with(self);
            return Err(RunError::internal("yield from: expected iterator ref on stack"));
        };
        self.current_frame.ip = self.instruction_ip;
        let advanced = if matches!(sent, Value::None) {
            PyValue::from_heap(id, self.heap).py_next(self)
        } else if matches!(self.heap.read(id), HeapReadOutput::Generator(_)) {
            self.resume_generator(id, sent)
        } else {
            // As in CPython, other delegates receive a sent value through their own `send`.
            let send = EitherStr::from("send".to_owned());
            PyValue::from_heap(id, self.heap)
                .py_call_attr(self, &send, ArgValues::One(sent))
                .map(Some)
        };
        match advanced {
            Ok(Some(CallResult::Value(value))) => Ok(self.yield_to_caller(value)),
            Ok(Some(CallResult::FramePushed)) => {
                self.current_frame.generator.mark_delegate();
                Ok(None)
            }
            Ok(None) => {
                self.finish_yield_from(Value::None);
                Ok(None)
            }
            Err(error) if error.is_stop_iteration() => {
                self.finish_yield_from(Value::None);
                Ok(None)
            }
            Err(error) => Err(error),
            Ok(Some(other)) => {
                other.drop_with(self);
                Err(ExcType::not_implemented("yield from: this iterator cannot suspend to the host").into())
            }
        }
    }

    /// Ends a `yield from`: replaces the delegate with its result and steps past the
    /// `Yield` instruction, to which the delegating frame's `ip` was rewound.
    fn finish_yield_from(&mut self, result: Value) {
        self.pop().drop_with(self);
        self.push(result);
        self.current_frame.ip += YIELD_INSTRUCTION_LEN;
    }

    /// `ReturnValue` in a frame advancing a `yield from` delegate (a `__next__`
    /// method or a frozen helper): the caller yields the returned item.
    ///
    /// When `__next__` or `send` is a class, the frame is its `__init__`, and the
    /// instance `instantiate_class` left on the generator's stack is the item.
    pub(super) fn return_to_delegating_generator(&mut self, value: Value) -> RunResult<Option<FrameExit>> {
        let is_initializer = self.current_frame.is_initializer;
        let should_return = self.pop_frame();
        debug_assert!(!should_return, "a delegate frame is pushed by its generator's dispatch");
        if is_initializer && !matches!(value, Value::None) {
            let type_name = value.py_type_name(self);
            value.drop_with(self);
            self.pop().drop_with(self);
            Err(ExcType::type_error_init_return(type_name))
        } else {
            let item = if is_initializer { self.pop() } else { value };
            Ok(self.yield_to_caller(item))
        }
    }

    /// A `StopIteration` escaping a `yield from` delegate's frame ends the
    /// delegation: the delegating generator resumes with the exception's `value`.
    pub(super) fn end_yield_from_with(&mut self, stop: Value) {
        let result = match &stop {
            Value::Ref(id) => match self.heap.get(*id) {
                HeapData::Exception(exception) => exception.stop_iteration_value(self),
                _ => Value::None,
            },
            _ => Value::None,
        };
        stop.drop_with(self);
        let is_initializer = self.current_frame.is_initializer;
        self.pop_frame();
        if is_initializer {
            // The pending instance of a class-valued `__next__` sits above the delegate.
            self.pop().drop_with(self);
        }
        self.finish_yield_from(result);
    }
}

impl ExceptionValue {
    /// The `StopIteration` a generator's `return value` raises: no arguments for
    /// `None`, else the value itself, also as the message when it is a string.
    pub(crate) fn stop_iteration(value: Value, vm: &mut VM<'_>) -> Self {
        let message = match value.py_value(vm) {
            PyValue::Str(string) => Some(string.as_str(vm).to_owned()),
            _ => None,
        };
        let value = if matches!(value, Value::None) {
            None
        } else {
            Some(Box::new(value))
        };
        Self {
            exception: SimpleException::new(ExcType::StopIteration, message),
            value,
        }
    }
}
