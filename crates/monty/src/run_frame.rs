use crate::{
    args::ArgValues,
    evaluate::{EvalResult, EvaluateExpr},
    exception_private::{exc_err_static, exc_fmt, ExcType, ExceptionRaise, RawStackFrame, RunError, SimpleException},
    expressions::{ExprLoc, Identifier, NameScope, Node},
    for_iterator::ForIterator,
    heap::{Heap, HeapData},
    intern::{FunctionId, Interns, StringId, MODULE_STRING_ID},
    io::PrintWriter,
    namespace::{NamespaceId, Namespaces, GLOBAL_NS_IDX},
    operators::Operator,
    parse::{CodeRange, ExceptHandler, Try},
    resource::ResourceTracker,
    snapshot::{AbstractSnapshotTracker, ClauseState, ExternalCall, FrameExit, TryClauseState, TryPhase},
    types::PyTrait,
    value::{Attr, Value},
};

/// Result type for runtime operations.
pub type RunResult<T> = Result<T, RunError>;

/// Represents an execution frame with an index into Namespaces.
///
/// At module level, `local_idx == GLOBAL_NS_IDX` (same namespace).
/// In functions, `local_idx` points to the function's local namespace.
/// Global variables always use `GLOBAL_NS_IDX` (0) directly.
///
/// # Closure Support
///
/// Cell variables (for closures) are stored directly in the namespace as
/// `Value::Ref(cell_id)` pointing to a `HeapData::Cell`. Both captured cells
/// (from enclosing scopes) and owned cells (for variables captured by nested
/// functions) are injected into the namespace at function call time.
///
/// When accessing a variable with `NameScope::Cell`, we look up the namespace
/// slot to get the `Value::Ref(cell_id)`, then read/write through that cell.
#[derive(Debug)]
pub struct RunFrame<'i, P: AbstractSnapshotTracker, W: PrintWriter> {
    /// Index of this frame's local namespace in Namespaces.
    local_idx: NamespaceId,
    /// The name of the current frame (function name or "<module>").
    /// Uses string id to lookup
    name: StringId,
    /// reference to interns
    interns: &'i Interns,
    /// reference to position tracker
    snapshot_tracker: &'i mut P,
    /// Writer for print output
    print: &'i mut W,
    /// Current exception context for bare `raise` statements.
    ///
    /// Set when entering an except handler, cleared when exiting.
    /// Used by bare `raise` to re-raise the current exception.
    current_exception: Option<SimpleException>,
}

/// Extracts a value from `EvalResult`, returning early with `FrameExit::ExternalCall` if
/// an external call is pending.
///
/// Similar to `return_ext_call!` from evaluate.rs, but returns `Ok(Some(FrameExit::ExternalCall(...)))`
/// which is the appropriate return type for `execute_node` and related methods.
macro_rules! frame_ext_call {
    ($expr:expr) => {
        match $expr {
            EvalResult::Value(value) => value,
            EvalResult::ExternalCall(ext_call) => return Ok(Some(FrameExit::ExternalCall(ext_call))),
        }
    };
}

impl<'i, P: AbstractSnapshotTracker, W: PrintWriter> RunFrame<'i, P, W> {
    /// Creates a new frame for module-level execution.
    ///
    /// At module level, `local_idx` is `GLOBAL_NS_IDX` (0).
    pub fn module_frame(interns: &'i Interns, snapshot_tracker: &'i mut P, print: &'i mut W) -> Self {
        Self {
            local_idx: GLOBAL_NS_IDX,
            name: MODULE_STRING_ID,
            interns,
            snapshot_tracker,
            print,
            current_exception: None,
        }
    }

    /// Creates a new frame for function execution.
    ///
    /// The function's local namespace is at `local_idx`. Global variables
    /// always use `GLOBAL_NS_IDX` directly.
    ///
    /// Cell variables (for closures) are already injected into the namespace
    /// by Function::call or Function::call_with_cells before this frame is created.
    ///
    /// # Arguments
    /// * `local_idx` - Index of the function's local namespace in Namespaces
    /// * `name` - The function name StringId (for error messages)
    /// * `snapshot_tracker` - Tracker for the current position in the code
    /// * `print` - Writer for print output
    pub fn function_frame(
        local_idx: NamespaceId,
        name: StringId,
        interns: &'i Interns,
        snapshot_tracker: &'i mut P,
        print: &'i mut W,
    ) -> Self {
        Self {
            local_idx,
            name,
            interns,
            snapshot_tracker,
            print,
            current_exception: None,
        }
    }

    /// Executes all nodes in sequence, returning when a frame exit (return/yield) occurs.
    ///
    /// This will use `PositionTracker` to manage where in the block to resume execution.
    ///
    /// # Arguments
    /// * `namespaces` - The namespace stack
    /// * `heap` - The heap for allocations
    /// * `nodes` - The AST nodes to execute
    pub fn execute(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        nodes: &[Node],
    ) -> RunResult<Option<FrameExit>> {
        // The first position must be an Index - it tells us where to start in this block
        let position = self.snapshot_tracker.next();
        let start_index = position.index;
        let mut clause_state = position.clause_state;

        // execute from start_index
        for (i, node) in nodes.iter().enumerate().skip(start_index) {
            // External calls are returned as Ok(Some(FrameExit::ExternalCall(...))) from execute_node
            let exit_frame = self.execute_node(namespaces, heap, node, clause_state)?;
            if let Some(exit) = exit_frame {
                // Only record position for external calls (suspensions).
                // Returns complete the frame - there's no position to resume from.
                if matches!(exit, FrameExit::ExternalCall(_)) {
                    // Set the index of the node to execute on resume
                    // we will have called set_skip() already if we need to skip the current node
                    self.snapshot_tracker.record(i);
                }
                return Ok(Some(exit));
            }
            clause_state = None;

            // if enabled, clear cached return values after executing each node
            // This ensures cached values don't persist across statements or loop iterations
            if P::clear_return_values() {
                namespaces.clear_statement_cache(heap);
            }
        }
        Ok(None)
    }

    /// Executes a single node, returning exit info with positions if execution should stop.
    ///
    /// Returns `Some(exit)` if the node caused a yield/return, where:
    /// - `exit` is the FrameExit (Yield or Return)
    /// - `positions` is the position stack within this node (empty for simple yields/returns)
    fn execute_node(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        node: &Node,
        clause_state: Option<ClauseState>,
    ) -> RunResult<Option<FrameExit>> {
        // Check time limit at statement boundaries
        heap.tracker_mut().check_time().map_err(|e| {
            let frame = node.position().map(|pos| self.stack_frame(pos));
            RunError::UncatchableExc(e.into_exception(frame))
        })?;

        // Trigger garbage collection if scheduler says it's time.
        // GC runs at statement boundaries because:
        // 1. This is a natural pause point where we have access to GC roots
        // 2. The namespace state is stable (not mid-expression evaluation)
        // Note: GC won't run during long-running single expressions (e.g., large list
        // comprehensions). This is acceptable because most Python code is structured
        // as multiple statements, and resource limits (time, memory) still apply.
        if heap.tracker().should_gc() {
            heap.collect_garbage(|| namespaces.iter_heap_ids());
        }

        match node {
            Node::Expr(expr) => {
                match EvaluateExpr::new(
                    namespaces,
                    self.local_idx,
                    heap,
                    self.interns,
                    self.print,
                    self.snapshot_tracker,
                )
                .evaluate_discard(expr)
                {
                    Ok(EvalResult::Value(())) => {}
                    Ok(EvalResult::ExternalCall(ext_call)) => return Ok(Some(FrameExit::ExternalCall(ext_call))),
                    Err(mut e) => {
                        add_frame_info(self.name, expr.position, &mut e);
                        return Err(e);
                    }
                }
            }
            Node::Return(expr) => {
                return self.execute_expr(namespaces, heap, expr).map(|result| match result {
                    EvalResult::Value(value) => Some(FrameExit::Return(value)),
                    EvalResult::ExternalCall(ext_call) => Some(FrameExit::ExternalCall(ext_call)),
                });
            }
            Node::ReturnNone => return Ok(Some(FrameExit::Return(Value::None))),
            Node::Raise(exc) => {
                if let Some(exit) = self.raise(namespaces, heap, exc.as_ref())? {
                    return Ok(Some(exit));
                }
            }
            Node::Assert { test, msg } => {
                if let Some(exit) = self.assert_(namespaces, heap, test, msg.as_ref())? {
                    return Ok(Some(exit));
                }
            }
            Node::Assign { target, object } => {
                if let Some(exit) = self.assign(namespaces, heap, target, object)? {
                    return Ok(Some(exit));
                }
            }
            Node::OpAssign { target, op, object } => {
                if let Some(exit) = self.op_assign(namespaces, heap, target, op, object)? {
                    return Ok(Some(exit));
                }
            }
            Node::SubscriptAssign { target, index, value } => {
                if let Some(exit) = self.subscript_assign(namespaces, heap, target, index, value)? {
                    return Ok(Some(exit));
                }
            }
            Node::AttrAssign {
                object,
                attr,
                target_position,
                value,
            } => {
                if let Some(exit) = self.attr_assign(namespaces, heap, object, attr, *target_position, value)? {
                    return Ok(Some(exit));
                }
            }
            Node::For {
                target,
                iter,
                body,
                or_else,
            } => {
                if let Some(exit_frame) = self.for_(namespaces, heap, clause_state, target, iter, body, or_else)? {
                    return Ok(Some(exit_frame));
                }
            }
            Node::If { test, body, or_else } => {
                if let Some(exit_frame) = self.if_(namespaces, heap, clause_state, test, body, or_else)? {
                    return Ok(Some(exit_frame));
                }
            }
            Node::FunctionDef(function_id) => self.define_function(namespaces, heap, *function_id)?,
            Node::Try(try_) => {
                if let Some(exit_frame) = self.try_(namespaces, heap, clause_state, try_)? {
                    return Ok(Some(exit_frame));
                }
            }
        }
        Ok(None)
    }

    /// Evaluates an expression and returns a Value.
    fn execute_expr(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        expr: &ExprLoc,
    ) -> RunResult<EvalResult<Value>> {
        match EvaluateExpr::new(
            namespaces,
            self.local_idx,
            heap,
            self.interns,
            self.print,
            self.snapshot_tracker,
        )
        .evaluate_use(expr)
        {
            Ok(value) => Ok(value),
            Err(mut e) => {
                add_frame_info(self.name, expr.position, &mut e);
                Err(e)
            }
        }
    }

    fn execute_expr_bool(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        expr: &ExprLoc,
    ) -> RunResult<EvalResult<bool>> {
        match EvaluateExpr::new(
            namespaces,
            self.local_idx,
            heap,
            self.interns,
            self.print,
            self.snapshot_tracker,
        )
        .evaluate_bool(expr)
        {
            Ok(value) => Ok(value),
            Err(mut e) => {
                add_frame_info(self.name, expr.position, &mut e);
                Err(e)
            }
        }
    }

    /// Executes a raise statement.
    ///
    /// Handles:
    /// * Exception instance (heap-allocated HeapData::Exception) - raise directly
    /// * Exception type (Value::Callable with ExcType) - instantiate then raise
    /// * Anything else - TypeError
    fn raise(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        op_exc_expr: Option<&ExprLoc>,
    ) -> RunResult<Option<FrameExit>> {
        if let Some(exc_expr) = op_exc_expr {
            let value = frame_ext_call!(self.execute_expr(namespaces, heap, exc_expr)?);
            // Check if value is a heap-allocated exception
            if let Value::Ref(id) = &value {
                if let HeapData::Exception(exc) = heap.get(*id) {
                    let exc = exc.clone();
                    value.drop_with_heap(heap);
                    // Use raise_frame so traceback won't show caret for raise statement
                    return Err(exc.with_frame(self.raise_frame(exc_expr.position)).into());
                }
            }
            if let Value::Builtin(builtin) = &value {
                // Callable is inline - call it to get the exception
                let builtin = *builtin;
                let result = builtin.call(heap, ArgValues::Empty, self.interns, self.print)?;
                // Check if result is a heap-allocated exception
                if let Value::Ref(id) = &result {
                    if let HeapData::Exception(exc) = heap.get(*id) {
                        let exc = exc.clone();
                        result.drop_with_heap(heap);
                        // Use raise_frame so traceback won't show caret for raise statement
                        return Err(exc.with_frame(self.raise_frame(exc_expr.position)).into());
                    }
                }
            }
            value.drop_with_heap(heap);
            exc_err_static!(ExcType::TypeError; "exceptions must derive from BaseException")
        } else {
            // Bare raise - re-raise the current exception
            if let Some(ref exc) = self.current_exception {
                // Re-raise the current exception (from except handler context)
                Err(exc.clone().into())
            } else {
                // No current exception - this is a RuntimeError in Python
                exc_err_static!(ExcType::RuntimeError; "No active exception to reraise")
            }
        }
    }

    /// Executes an assert statement by evaluating the test expression and raising
    /// `AssertionError` if the test is falsy.
    ///
    /// If a message expression is provided, it is evaluated and used as the exception message.
    fn assert_(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        test: &ExprLoc,
        msg: Option<&ExprLoc>,
    ) -> RunResult<Option<FrameExit>> {
        let ok = frame_ext_call!(self.execute_expr_bool(namespaces, heap, test)?);
        if !ok {
            let msg = if let Some(msg_expr) = msg {
                let msg_value = frame_ext_call!(self.execute_expr(namespaces, heap, msg_expr)?);
                Some(msg_value.py_str(heap, self.interns).to_string())
            } else {
                None
            };
            return Err(SimpleException::new(ExcType::AssertionError, msg)
                .with_frame(self.stack_frame(test.position))
                .into());
        }
        Ok(None)
    }

    fn assign(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        target: &Identifier,
        expr: &ExprLoc,
    ) -> RunResult<Option<FrameExit>> {
        let new_value = frame_ext_call!(self.execute_expr(namespaces, heap, expr)?);

        // Determine which namespace to use
        let ns_idx = match target.scope {
            NameScope::Global => GLOBAL_NS_IDX,
            _ => self.local_idx, // Local and Cell both use local namespace
        };

        if target.scope == NameScope::Cell {
            // Cell assignment - look up cell HeapId from namespace slot, then write through it
            let namespace = namespaces.get_mut(ns_idx);
            let Value::Ref(cell_id) = namespace.get(target.namespace_id()) else {
                panic!("Cell variable slot doesn't contain a cell reference - prepare-time bug")
            };
            heap.set_cell_value(*cell_id, new_value);
        } else {
            // Direct assignment to namespace slot (Local or Global)
            let namespace = namespaces.get_mut(ns_idx);
            let old_value = std::mem::replace(namespace.get_mut(target.namespace_id()), new_value);
            old_value.drop_with_heap(heap);
        }
        Ok(None)
    }

    fn op_assign(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        target: &Identifier,
        op: &Operator,
        expr: &ExprLoc,
    ) -> RunResult<Option<FrameExit>> {
        let rhs = frame_ext_call!(self.execute_expr(namespaces, heap, expr)?);
        // Capture rhs type before it's consumed
        let rhs_type = rhs.py_type(Some(heap));

        // Cell variables need special handling - read through cell, modify, write back
        let err_target_type = if target.scope == NameScope::Cell {
            let namespace = namespaces.get_mut(self.local_idx);
            let Value::Ref(cell_id) = namespace.get(target.namespace_id()) else {
                panic!("Cell variable slot doesn't contain a cell reference - prepare-time bug")
            };
            let mut cell_value = heap.get_cell_value(*cell_id);
            // Capture type before potential drop
            let cell_value_type = cell_value.py_type(Some(heap));
            let result: RunResult<Option<Value>> = match op {
                // In-place add has special optimization for mutable types
                Operator::Add => {
                    let ok = cell_value.py_iadd(rhs, heap, None, self.interns)?;
                    if ok {
                        Ok(Some(cell_value))
                    } else {
                        Ok(None)
                    }
                }
                // For other operators, use binary op + replace
                Operator::Mult => {
                    let new_val = cell_value.py_mult(&rhs, heap, self.interns)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::Div => {
                    let new_val = cell_value.py_div(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::FloorDiv => {
                    let new_val = cell_value.py_floordiv(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::Pow => {
                    let new_val = cell_value.py_pow(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::Sub => {
                    let new_val = cell_value.py_sub(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::Mod => {
                    let new_val = cell_value.py_mod(&rhs);
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                _ => return Err(RunError::internal("assign operator not yet implemented")),
            };
            match result? {
                Some(new_value) => {
                    heap.set_cell_value(*cell_id, new_value);
                    None
                }
                None => Some(cell_value_type),
            }
        } else {
            // Direct access for Local/Global scopes
            let target_val = namespaces.get_var_mut(self.local_idx, target, self.interns)?;
            let target_type = target_val.py_type(Some(heap));
            let result: RunResult<Option<()>> = match op {
                // In-place add has special optimization for mutable types
                Operator::Add => {
                    let ok = target_val.py_iadd(rhs, heap, None, self.interns)?;
                    if ok {
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                // For other operators, use binary op + replace
                Operator::Mult => {
                    let new_val = target_val.py_mult(&rhs, heap, self.interns)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::Div => {
                    let new_val = target_val.py_div(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::FloorDiv => {
                    let new_val = target_val.py_floordiv(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::Pow => {
                    let new_val = target_val.py_pow(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::Sub => {
                    let new_val = target_val.py_sub(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::Mod => {
                    let new_val = target_val.py_mod(&rhs);
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                _ => return Err(RunError::internal("assign operator not yet implemented")),
            };
            match result? {
                Some(()) => None,
                None => Some(target_type),
            }
        };

        if let Some(target_type) = err_target_type {
            let e = SimpleException::augmented_assign_type_error(op, target_type, rhs_type);
            Err(e.with_frame(self.stack_frame(expr.position)).into())
        } else {
            Ok(None)
        }
    }

    fn subscript_assign(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        target: &Identifier,
        index: &ExprLoc,
        value: &ExprLoc,
    ) -> RunResult<Option<FrameExit>> {
        let key = frame_ext_call!(self.execute_expr(namespaces, heap, index)?);
        let val = frame_ext_call!(self.execute_expr(namespaces, heap, value)?);
        let target_val = namespaces.get_var_mut(self.local_idx, target, self.interns)?;
        if let Value::Ref(id) = target_val {
            let id = *id;
            heap.with_entry_mut(id, |heap, data| data.py_setitem(key, val, heap, self.interns))?;
            Ok(None)
        } else {
            let e = exc_fmt!(ExcType::TypeError; "'{}' object does not support item assignment", target_val.py_type(Some(heap)));
            Err(e.with_frame(self.stack_frame(index.position)).into())
        }
    }

    /// Assigns a value to an attribute on an object: `object.attr = value`.
    ///
    /// Currently only supports mutable dataclass instances. Returns an error for:
    /// - Non-heap values (they don't have attributes)
    /// - Immutable dataclasses (frozen=True)
    /// - Other heap types that don't support attribute assignment
    ///
    /// Supports chained attribute access like `a.b.c = value`.
    fn attr_assign(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        object_expr: &ExprLoc,
        attr: &Attr,
        target_position: CodeRange,
        value_expr: &ExprLoc,
    ) -> RunResult<Option<FrameExit>> {
        // Evaluate the value first
        let val = frame_ext_call!(self.execute_expr(namespaces, heap, value_expr)?);

        // Evaluate the object expression to get the object
        let object_val = frame_ext_call!(self.execute_expr(namespaces, heap, object_expr)?);

        let frame = self.stack_frame(target_position);
        let attr_str = attr.as_str(self.interns);

        if let Value::Ref(id) = &object_val {
            let id = *id;
            let result = heap.with_entry_mut(id, |heap, data| -> RunResult<()> {
                match data {
                    HeapData::Dataclass(dc) => {
                        if dc.is_frozen() {
                            // Drop the value we were going to assign
                            val.drop_with_heap(heap);
                            Err(ExcType::frozen_instance_error(attr_str))
                        } else {
                            // Convert attr to Value - uses InternString for interned attrs (no heap alloc)
                            let key = attr.to_value(heap)?;

                            // Set the attr - key ownership transferred to Dict
                            // If the key already exists, the duplicate key is dropped inside set_attr
                            let old_val = dc.set_attr(key, val, heap, self.interns)?;
                            if let Some(old) = old_val {
                                old.drop_with_heap(heap);
                            }
                            Ok(())
                        }
                    }
                    other => {
                        // Drop the value we were going to assign
                        val.drop_with_heap(heap);
                        let ty = other.py_type(Some(heap));
                        Err(ExcType::attribute_error_no_setattr(ty, attr_str))
                    }
                }
            });
            // Drop the object value we evaluated (it's a clone with incremented refcount)
            object_val.drop_with_heap(heap);
            result.map_err(|e| e.set_frame(frame))?;
            Ok(None)
        } else {
            // Drop the value and object
            val.drop_with_heap(heap);
            let ty = object_val.py_type(Some(heap));
            object_val.drop_with_heap(heap);
            Err(ExcType::attribute_error_no_setattr(ty, attr_str).set_frame(frame))
        }
    }

    /// Executes a for loop, propagating any `FrameExit` (yield/return) from the body.
    ///
    /// Returns `Some(FrameExit)` if a yield or explicit return occurred in the body,
    /// `None` if the loop completed normally.
    ///
    /// Supports iteration over: Range, List, Tuple, Dict (keys), Str (chars), Bytes (ints).
    /// Uses `ForIterator` for unified iteration with index-based state for resumption.
    #[allow(clippy::too_many_arguments)]
    fn for_(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        clause_state: Option<ClauseState>,
        target: &Identifier,
        iter: &ExprLoc,
        body: &[Node],
        _or_else: &[Node],
    ) -> RunResult<Option<FrameExit>> {
        // Get the iterator from the snapshot state if it
        let mut for_iter = if let Some(ClauseState::For(for_iter)) = clause_state {
            for_iter
        } else {
            let iter_value = frame_ext_call!(self.execute_expr(namespaces, heap, iter)?);
            // Create ForIterator from value
            let for_iter = ForIterator::new(iter_value, heap, self.interns)?;

            // Same as below, clear ext_return_values after evaluating the loop value but before entering the body.
            // This ensures that when we resume with ClauseState::For (which skips re-evaluating
            // the condition), there are no stale return values from the condition evaluation.
            if P::clear_return_values() {
                namespaces.clear_ext_return_values(heap);
            }
            for_iter
        };

        let namespace_id = target.namespace_id();
        loop {
            let value = match for_iter.for_next(heap, self.interns) {
                Ok(Some(v)) => v,
                Ok(None) => break, // Iteration complete
                Err(e) => {
                    for_iter.drop_with_heap(heap);
                    // Add frame info for errors from for_next (e.g., set/dict mutation during iteration)
                    return Err(e.set_frame(self.stack_frame(iter.position)));
                }
            };

            // For loop target is always local scope - must drop old value properly
            let namespace = namespaces.get_mut(self.local_idx);
            let old_value = std::mem::replace(namespace.get_mut(namespace_id), value);
            old_value.drop_with_heap(heap);

            match self.execute(namespaces, heap, body) {
                Ok(Some(exit)) => {
                    // Decrement iterator so on resume for_next() returns the same value.
                    // The loop variable is already set, but we need the iterator at the
                    // correct position for potential re-iteration after the body completes.
                    for_iter.decr();
                    self.snapshot_tracker.set_clause_state(ClauseState::For(for_iter));
                    return Ok(Some(exit));
                }
                Ok(None) => {
                    // for_next() already advanced, continue to next iteration
                }
                Err(e) => {
                    for_iter.drop_with_heap(heap);
                    return Err(e);
                }
            }
        }

        // Drop the original iterable value after loop completes
        for_iter.drop_with_heap(heap);
        Ok(None)
    }

    /// Executes an if statement.
    ///
    /// Evaluates the test condition and executes the appropriate branch.
    /// Tracks return value consumption for proper resumption with external calls.
    fn if_(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        clause_state: Option<ClauseState>,
        test: &ExprLoc,
        body: &[Node],
        or_else: &[Node],
    ) -> RunResult<Option<FrameExit>> {
        let is_true = if let Some(ClauseState::If(resume_test)) = clause_state {
            resume_test
        } else {
            let test = frame_ext_call!(self.execute_expr_bool(namespaces, heap, test)?);
            // Clear ext_return_values after evaluating the condition but before entering the body.
            // This ensures that when we resume with ClauseState::If (which skips re-evaluating
            // the condition), there are no stale return values from the condition evaluation.
            // Only clear when actually evaluating a real condition (not using ClauseState::If).
            if P::clear_return_values() {
                namespaces.clear_ext_return_values(heap);
            }
            test
        };
        if is_true {
            if let Some(frame_exit) = self.execute(namespaces, heap, body)? {
                self.snapshot_tracker.set_clause_state(ClauseState::If(true));
                return Ok(Some(frame_exit));
            }
        } else if let Some(frame_exit) = self.execute(namespaces, heap, or_else)? {
            self.snapshot_tracker.set_clause_state(ClauseState::If(false));
            return Ok(Some(frame_exit));
        }
        Ok(None)
    }

    /// Defines a function (or closure) by storing it in the namespace.
    ///
    /// If the function has free_var_enclosing_slots (captures variables from enclosing scope),
    /// this captures the cells from the enclosing namespace and stores a Closure.
    /// If the function has default values, they are evaluated at definition time and stored.
    /// Otherwise, it stores a simple Function reference.
    ///
    /// # Cell Sharing
    ///
    /// Closures share cells with their enclosing scope. The cell HeapIds are
    /// looked up from the enclosing namespace slots specified in free_var_enclosing_slots.
    /// This ensures modifications through `nonlocal` are visible to both scopes.
    fn define_function(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        function_id: FunctionId,
    ) -> RunResult<()> {
        let function = self.interns.get_function(function_id);

        // Evaluate default expressions at definition time
        // These are evaluated in the enclosing scope (not the function's own scope)
        let defaults = if function.has_defaults() {
            let mut defaults = Vec::with_capacity(function.default_exprs.len());
            for expr in &function.default_exprs {
                match self.execute_expr(namespaces, heap, expr) {
                    Ok(EvalResult::Value(value)) => defaults.push(value),
                    Ok(EvalResult::ExternalCall(_)) => {
                        // External calls in default expressions are not supported
                        for value in defaults.drain(..) {
                            value.drop_with_heap(heap);
                        }
                        return Err(ExcType::not_implemented(
                            "external function calls in default parameter expressions",
                        )
                        .into());
                    }
                    Err(err) => {
                        for value in defaults.drain(..) {
                            value.drop_with_heap(heap);
                        }
                        return Err(err);
                    }
                }
            }
            defaults
        } else {
            Vec::new()
        };

        let new_value = if function.is_closure() {
            // This function captures variables from enclosing scopes.
            // Look up the cell HeapIds from the enclosing namespace.
            let enclosing_namespace = namespaces.get(self.local_idx);
            let mut captured_cells = Vec::with_capacity(function.free_var_enclosing_slots.len());

            for &enclosing_slot in &function.free_var_enclosing_slots {
                // The enclosing namespace slot contains Value::Ref(cell_id)
                let Value::Ref(cell_id) = enclosing_namespace.get(enclosing_slot) else {
                    panic!("Expected cell in enclosing namespace slot {enclosing_slot:?} - prepare-time bug")
                };

                // Increment the cell's refcount since this closure now holds a reference
                heap.inc_ref(*cell_id);
                captured_cells.push(*cell_id);
            }

            Value::Ref(heap.allocate(HeapData::Closure(function_id, captured_cells, defaults))?)
        } else if !defaults.is_empty() {
            // Non-closure function with defaults needs heap allocation
            Value::Ref(heap.allocate(HeapData::FunctionDefaults(function_id, defaults))?)
        } else {
            // Simple function without captures or defaults
            Value::Function(function_id)
        };

        let namespace = namespaces.get_mut(self.local_idx);
        let old_value = std::mem::replace(namespace.get_mut(function.name.namespace_id()), new_value);
        // Drop the old value properly (dec_ref for Refs, no-op for others)
        old_value.drop_with_heap(heap);
        Ok(())
    }

    /// Executes a try/except/else/finally block.
    ///
    /// The execution flow is:
    /// 1. Execute try body
    /// 2. If exception raised: find matching handler, execute it (or mark for re-raise after finally)
    /// 3. If no exception: execute else block
    /// 4. Always execute finally block
    /// 5. If there was an unhandled exception, re-raise it after finally
    /// 6. If there was a return in try/except/else, return after finally
    ///
    /// External calls can occur at any point, requiring resume via TryClauseState.
    fn try_(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        clause_state: Option<ClauseState>,
        Try {
            body,
            handlers,
            or_else,
            finally,
        }: &Try<Node>,
    ) -> RunResult<Option<FrameExit>> {
        // Track if any exception was raised (even if caught) - else block should not run
        let mut exception_occurred = false;

        // Determine which phase to start in (for resumption) and extract pending state
        let (start_phase, handler_index, mut pending_exception, mut pending_return, mut handler_enclosing_exception) =
            match clause_state {
                Some(ClauseState::Try(state)) => (
                    state.phase,
                    state.handler_index,
                    state.pending_exception,
                    state.pending_return,
                    state.enclosing_exception,
                ),
                _ => (TryPhase::TryBody, None, None, None, None),
            };

        // Phase 1: Try body (only if starting from TryBody phase)
        if start_phase == TryPhase::TryBody {
            match self.execute(namespaces, heap, body) {
                Ok(None) => {
                    // No exception and no return - proceed to else block
                }
                Ok(Some(FrameExit::Return(value))) => {
                    // Return in try body - save for after finally
                    pending_return = Some(value);
                }
                Ok(Some(FrameExit::ExternalCall(ext_call))) => {
                    // External call - save state and return
                    self.snapshot_tracker.set_clause_state(ClauseState::Try(TryClauseState {
                        phase: TryPhase::TryBody,
                        handler_index: None,
                        pending_exception: None,
                        pending_return: None,
                        enclosing_exception: None,
                    }));
                    return Ok(Some(FrameExit::ExternalCall(ext_call)));
                }
                Err(RunError::Exc(exc)) => {
                    // An exception occurred - else block should not run
                    exception_occurred = true;
                    // Catchable exception - try to find a matching handler
                    match self.find_matching_handler(namespaces, heap, &exc, handlers) {
                        Ok(Some((idx, handler))) => {
                            // Found matching handler - execute it
                            match self.execute_handler(namespaces, heap, handler, &exc, &mut pending_return)? {
                                HandlerOutcome::Completed => {}
                                HandlerOutcome::ExternalCall {
                                    call,
                                    enclosing_exception,
                                } => {
                                    // Save state for resumption
                                    self.snapshot_tracker.set_clause_state(ClauseState::Try(TryClauseState {
                                        phase: TryPhase::ExceptHandler,
                                        handler_index: Some(idx),
                                        pending_exception: None,
                                        pending_return: None,
                                        enclosing_exception,
                                    }));
                                    return Ok(Some(FrameExit::ExternalCall(call)));
                                }
                            }
                        }
                        Ok(None) => {
                            // No matching handler - save exception for re-raise after finally
                            pending_exception = Some(exc);
                        }
                        Err(e) => {
                            // Error during handler matching - run finally then propagate
                            return self.execute_finally_then_error(namespaces, heap, finally, e);
                        }
                    }
                }
                Err(e @ (RunError::UncatchableExc(_) | RunError::Internal(_))) => {
                    // Uncatchable exceptions still run finally, then propagate
                    return self.execute_finally_then_error(namespaces, heap, finally, e);
                }
            }
        }

        // Phase 2: Else block (only if no exception occurred and not resuming past it)
        // The else block runs ONLY if no exception was raised in the try body
        if !exception_occurred
            && pending_return.is_none()
            && (start_phase == TryPhase::Else || start_phase == TryPhase::TryBody)
        {
            match self.execute(namespaces, heap, or_else) {
                Ok(None) => {}
                Ok(Some(FrameExit::Return(value))) => {
                    pending_return = Some(value);
                }
                Ok(Some(FrameExit::ExternalCall(ext_call))) => {
                    self.snapshot_tracker.set_clause_state(ClauseState::Try(TryClauseState {
                        phase: TryPhase::Else,
                        handler_index: None,
                        pending_exception: None,
                        pending_return: None,
                        enclosing_exception: None,
                    }));
                    return Ok(Some(FrameExit::ExternalCall(ext_call)));
                }
                Err(e) => {
                    // Exception in else block - run finally then propagate
                    return self.execute_finally_then_error(namespaces, heap, finally, e);
                }
            }
        }

        // Phase 2.5: Resume from ExceptHandler if needed
        if start_phase == TryPhase::ExceptHandler {
            if let Some(idx) = handler_index {
                if let Some(exit) = self.resume_except_handler(
                    namespaces,
                    heap,
                    &handlers[idx as usize],
                    idx,
                    finally,
                    HandlerResumeState {
                        pending_return: &mut pending_return,
                        enclosing_exception: &mut handler_enclosing_exception,
                    },
                )? {
                    return Ok(Some(exit));
                }
            }
        }

        // Phase 3: Finally block (always runs)
        match self.execute(namespaces, heap, finally) {
            Ok(None) => {
                // Finally completed normally - handle pending actions
                if let Some(exc) = pending_exception {
                    return Err(RunError::Exc(exc));
                }
                if let Some(value) = pending_return {
                    return Ok(Some(FrameExit::Return(value)));
                }
                Ok(None)
            }
            Ok(Some(FrameExit::Return(value))) => {
                // Return in finally overrides pending return/exception
                if let Some(old_return) = pending_return {
                    old_return.drop_with_heap(heap);
                }
                Ok(Some(FrameExit::Return(value)))
            }
            Ok(Some(FrameExit::ExternalCall(ext_call))) => {
                // External call in finally - save pending state for resumption
                self.snapshot_tracker.set_clause_state(ClauseState::Try(TryClauseState {
                    phase: TryPhase::Finally,
                    handler_index: None,
                    pending_exception: pending_exception.take(),
                    pending_return: pending_return.take(),
                    enclosing_exception: None,
                }));
                Ok(Some(FrameExit::ExternalCall(ext_call)))
            }
            Err(e) => {
                // Exception in finally overrides pending exception
                if let Some(old_return) = pending_return {
                    old_return.drop_with_heap(heap);
                }
                Err(e)
            }
        }
    }

    /// Finds a matching exception handler for the given exception.
    ///
    /// Iterates through handlers in order, returning the first one that matches.
    /// A bare `except:` matches any exception.
    ///
    /// Note: External calls during exception type evaluation are not supported and will
    /// return an error. This is a limitation of the current design.
    fn find_matching_handler<'a>(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        exc: &ExceptionRaise,
        handlers: &'a [ExceptHandler<Node>],
    ) -> RunResult<Option<(u16, &'a ExceptHandler<Node>)>> {
        for (idx, handler) in handlers.iter().enumerate() {
            match &handler.exc_type {
                None => {
                    // Bare except - catches everything
                    let idx = idx.try_into().expect("Failed to convert handler index to u16");
                    return Ok(Some((idx, handler)));
                }
                Some(type_expr) => {
                    // Evaluate the exception type expression
                    let eval_result = self.execute_expr(namespaces, heap, type_expr)?;
                    let type_value = match eval_result {
                        EvalResult::Value(v) => v,
                        EvalResult::ExternalCall(_) => {
                            // External calls in exception type expressions are not supported
                            return Err(RunError::internal(
                                "external function calls in except clause type expressions are not supported",
                            ));
                        }
                    };
                    let match_result = Self::matches_exception_type(&exc.exc, &type_value, heap);
                    type_value.drop_with_heap(heap);
                    // Propagate TypeError if the handler type is invalid
                    let idx = idx.try_into().expect("Failed to convert handler index to u16");
                    if match_result? {
                        return Ok(Some((idx, handler)));
                    }
                }
            }
        }
        Ok(None)
    }

    /// Checks if an exception matches a handler type.
    ///
    /// Supports:
    /// - Single exception type: `except ValueError:`
    /// - Tuple of types: `except (ValueError, TypeError):`
    /// - Exception hierarchy: `except LookupError:` catches `KeyError` and `IndexError`
    ///
    /// Returns `Err(TypeError)` if the handler type is not a valid exception class.
    fn matches_exception_type(
        exc: &SimpleException,
        handler_type: &Value,
        heap: &Heap<impl ResourceTracker>,
    ) -> RunResult<bool> {
        match handler_type {
            // Exception type (e.g., ValueError, LookupError, Exception)
            Value::Builtin(builtin) => {
                if let Some(handler_exc_type) = builtin.as_exc_type() {
                    Ok(exc.exc_type().is_subclass_of(handler_exc_type))
                } else {
                    // Builtin but not an exception type (e.g., int, str, list)
                    exc_err_static!(ExcType::TypeError; "catching classes that do not inherit from BaseException is not allowed")
                }
            }
            // Tuple of types (e.g., (ValueError, TypeError))
            Value::Ref(id) => {
                if let HeapData::Tuple(tuple) = heap.get(*id) {
                    for v in tuple.as_vec() {
                        if Self::matches_exception_type(exc, v, heap)? {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                } else {
                    // Not a tuple - invalid
                    exc_err_static!(ExcType::TypeError; "catching classes that do not inherit from BaseException is not allowed")
                }
            }
            // Invalid type (e.g., integer, string, etc.)
            _ => {
                exc_err_static!(ExcType::TypeError; "catching classes that do not inherit from BaseException is not allowed")
            }
        }
    }

    /// Executes an exception handler body.
    ///
    /// Binds the exception to the handler's variable (if specified),
    /// executes the handler body, then clears the variable.
    /// Also sets `current_exception` for bare `raise` support.
    fn execute_handler(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        handler: &ExceptHandler<Node>,
        exc: &ExceptionRaise,
        pending_return: &mut Option<Value>,
    ) -> RunResult<HandlerOutcome> {
        // Set current exception for bare raise support
        let old_current_exception = self.current_exception.take();
        self.current_exception = Some(exc.exc.clone());

        // Bind exception to variable if specified
        if let Some(ref name) = handler.name {
            let heap_id = heap.allocate(HeapData::Exception(exc.exc.clone()))?;
            let exc_value = Value::Ref(heap_id);
            let ns_idx = match name.scope {
                NameScope::Global => GLOBAL_NS_IDX,
                _ => self.local_idx,
            };
            let namespace = namespaces.get_mut(ns_idx);
            let old_value = std::mem::replace(namespace.get_mut(name.namespace_id()), exc_value);
            old_value.drop_with_heap(heap);
        }

        // Execute handler body
        match self.execute(namespaces, heap, &handler.body) {
            Ok(Some(FrameExit::ExternalCall(ext_call))) => Ok(HandlerOutcome::ExternalCall {
                call: ext_call,
                enclosing_exception: old_current_exception,
            }),
            Ok(Some(FrameExit::Return(value))) => {
                *pending_return = Some(value);
                self.clear_handler_state(namespaces, heap, handler, old_current_exception);
                Ok(HandlerOutcome::Completed)
            }
            Ok(None) => {
                self.clear_handler_state(namespaces, heap, handler, old_current_exception);
                Ok(HandlerOutcome::Completed)
            }
            Err(e) => {
                self.clear_handler_state(namespaces, heap, handler, old_current_exception);
                Err(e)
            }
        }
    }

    /// Resumes execution inside an except handler after an external call suspension.
    ///
    /// Restores the handler's binding/current exception, resumes execution, and either
    /// propagates another suspension or finishes the handler (updating pending returns).
    fn resume_except_handler(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        handler: &ExceptHandler<Node>,
        handler_index: u16,
        finally: &[Node],
        state: HandlerResumeState<'_>,
    ) -> RunResult<Option<FrameExit>> {
        let HandlerResumeState {
            pending_return,
            enclosing_exception,
        } = state;

        // Restore current_exception from the bound variable for bare raise support.
        if let Some(name) = &handler.name {
            let ns_idx = match name.scope {
                NameScope::Global => GLOBAL_NS_IDX,
                _ => self.local_idx,
            };
            let namespace = namespaces.get(ns_idx);
            if let Value::Ref(id) = namespace.get(name.namespace_id()) {
                if let HeapData::Exception(exc) = heap.get(*id) {
                    self.current_exception = Some(exc.clone());
                }
            }
        }

        match self.execute(namespaces, heap, &handler.body) {
            Ok(Some(FrameExit::ExternalCall(ext_call))) => {
                self.snapshot_tracker.set_clause_state(ClauseState::Try(TryClauseState {
                    phase: TryPhase::ExceptHandler,
                    handler_index: Some(handler_index),
                    pending_exception: None,
                    pending_return: None,
                    enclosing_exception: enclosing_exception.clone(),
                }));
                Ok(Some(FrameExit::ExternalCall(ext_call)))
            }
            Ok(Some(FrameExit::Return(value))) => {
                *pending_return = Some(value);
                self.clear_handler_state(namespaces, heap, handler, enclosing_exception.take());
                Ok(None)
            }
            Ok(None) => {
                self.clear_handler_state(namespaces, heap, handler, enclosing_exception.take());
                Ok(None)
            }
            Err(e) => {
                self.clear_handler_state(namespaces, heap, handler, enclosing_exception.take());
                self.execute_finally_then_error(namespaces, heap, finally, e)
            }
        }
    }

    /// Clears the handler's exception binding and restores the caller's exception context.
    fn clear_handler_state(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        handler: &ExceptHandler<Node>,
        restore_exception: Option<SimpleException>,
    ) {
        if let Some(ref name) = handler.name {
            let ns_idx = match name.scope {
                NameScope::Global => GLOBAL_NS_IDX,
                _ => self.local_idx,
            };
            let namespace = namespaces.get_mut(ns_idx);
            let old_value = std::mem::replace(namespace.get_mut(name.namespace_id()), Value::Undefined);
            old_value.drop_with_heap(heap);
        }

        self.current_exception = restore_exception;
    }

    /// Executes the finally block, then propagates the original error.
    ///
    /// If finally also raises an exception, that exception takes precedence.
    /// If finally makes an external call, the pending error is saved in state
    /// and will be propagated after finally completes on resumption.
    fn execute_finally_then_error(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        finally: &[Node],
        original_error: RunError,
    ) -> RunResult<Option<FrameExit>> {
        match self.execute(namespaces, heap, finally) {
            Ok(None) => {
                // Finally completed without overriding control flow - propagate original error
                Err(original_error)
            }
            Ok(Some(FrameExit::Return(value))) => match original_error {
                // Python semantics dictate that a return inside finally suppresses
                // any pending catchable exception.
                RunError::Exc(_) => Ok(Some(FrameExit::Return(value))),
                // Internal/unreachable errors and uncatchable exceptions must still propagate.
                other => {
                    value.drop_with_heap(heap);
                    Err(other)
                }
            },
            Ok(Some(FrameExit::ExternalCall(ext_call))) => {
                // External call in finally with pending error - save state for resumption
                // Only catchable exceptions can be stored; others propagate immediately
                let RunError::Exc(pending_exc) = original_error else {
                    return Err(original_error);
                };
                self.snapshot_tracker.set_clause_state(ClauseState::Try(TryClauseState {
                    phase: TryPhase::Finally,
                    handler_index: None,
                    pending_exception: Some(pending_exc),
                    pending_return: None,
                    enclosing_exception: None,
                }));
                Ok(Some(FrameExit::ExternalCall(ext_call)))
            }
            Err(e) => {
                // Finally raised an exception - it takes precedence
                Err(e)
            }
        }
    }

    /// Create frame without parent - the parent chain is built up by add_frame_info()
    /// as the error propagates through the call stack
    fn stack_frame(&self, position: CodeRange) -> RawStackFrame {
        RawStackFrame::new(position, self.name, None)
    }

    /// Creates a stack frame for a raise statement (no caret shown in traceback).
    fn raise_frame(&self, position: CodeRange) -> RawStackFrame {
        RawStackFrame::from_raise(position, self.name)
    }
}

/// Result of executing an exception handler \(either directly or via resumption\).
#[derive(Debug)]
enum HandlerOutcome {
    /// Handler finished executing (possibly storing a pending return).
    Completed,
    /// Handler suspended due to an external call. Carries the enclosing exception
    /// so nested bare raises keep working after resumption.
    ExternalCall {
        call: ExternalCall,
        enclosing_exception: Option<SimpleException>,
    },
}

/// Mutable references that need to persist across except-handler resumption.
struct HandlerResumeState<'a> {
    pending_return: &'a mut Option<Value>,
    enclosing_exception: &'a mut Option<SimpleException>,
}

/// Adds the caller's frame to an error as it propagates up the call stack.
///
/// This builds the traceback chain by appending each caller's frame information
/// to the exception, so the full call stack is visible when the error is displayed.
///
/// Note: AttributeError gets special handling - CPython doesn't show carets for it,
/// so we suppress carets by using the `add_caller_frame_no_caret` method.
fn add_frame_info(name: StringId, position: CodeRange, error: &mut RunError) {
    match error {
        RunError::Exc(exc) | RunError::UncatchableExc(exc) => {
            // CPython doesn't show carets for AttributeError on attribute access
            if exc.exc.exc_type() == ExcType::AttributeError {
                exc.add_caller_frame_no_caret(position, name);
            } else {
                exc.add_caller_frame(position, name);
            }
        }
        RunError::Internal(_) => {}
    }
}
