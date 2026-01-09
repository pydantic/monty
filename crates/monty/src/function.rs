use std::fmt::Write;

use crate::{
    args::ArgValues,
    evaluate::EvalResult,
    expressions::{ExprLoc, Identifier, Node},
    heap::{Heap, HeapId},
    intern::{FunctionId, Interns, StringId},
    io::PrintWriter,
    namespace::{NamespaceId, Namespaces},
    parse::CodeRange,
    resource::ResourceTracker,
    run_frame::{RunFrame, RunResult},
    signature::Signature,
    snapshot::{AbstractSnapshotTracker, FrameExit, FunctionFrame},
    value::Value,
};

/// Stores a function definition.
///
/// Contains everything needed to execute a user-defined function: the body AST,
/// initial namespace layout, and captured closure cells. Functions are stored
/// on the heap and referenced via HeapId.
///
/// # Namespace Layout
///
/// The namespace has a predictable layout that allows sequential construction:
/// ```text
/// [params...][cell_vars...][free_vars...][locals...]
/// ```
/// - Slots 0..signature.param_count(): function parameters (see `Signature` for layout)
/// - Slots after params: cell refs for variables captured by nested functions
/// - Slots after cell_vars: free_var refs (captured from enclosing scope)
/// - Remaining slots: local variables
///
/// # Closure Support
///
/// - `free_var_enclosing_slots`: Enclosing namespace slots for captured variables.
///   At definition time, cells are captured from these slots and stored in a Closure.
///   At call time, they're pushed sequentially after cell_vars.
/// - `cell_var_count`: Number of cells to create for variables captured by nested functions.
///   At call time, cells are created and pushed sequentially after params.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Function {
    /// The function name (used for error messages and repr).
    pub name: Identifier,
    /// The function signature.
    pub signature: Signature,
    /// The prepared function body AST nodes.
    pub body: Vec<Node>,
    /// Size of the initial namespace (number of local variable slots).
    pub namespace_size: usize,
    /// Enclosing namespace slots for variables captured from enclosing scopes.
    ///
    /// At definition time: look up cell HeapId from enclosing namespace at each slot.
    /// At call time: captured cells are pushed sequentially (our slots are implicit).
    pub free_var_enclosing_slots: Vec<NamespaceId>,
    /// Number of cell variables (captured by nested functions).
    ///
    /// At call time, this many cells are created and pushed right after params.
    /// Their slots are implicitly params.len()..params.len()+cell_var_count.
    pub cell_var_count: usize,
    /// Maps cell variable indices to their corresponding parameter indices, if any.
    ///
    /// When a parameter is also captured by nested functions (cell variable), its value
    /// must be copied into the cell after binding. Each entry corresponds to a cell
    /// (index 0..cell_var_count), and contains `Some(param_index)` if that cell is for
    /// a parameter, or `None` otherwise.
    pub cell_param_indices: Vec<Option<usize>>,
    /// Prepared default value expressions, evaluated at function definition time.
    ///
    /// Layout: `[pos_defaults...][arg_defaults...][kwarg_defaults...]`
    /// Each group contains only the parameters that have defaults, in declaration order.
    /// The counts in `signature` indicate how many defaults exist for each group.
    pub default_exprs: Vec<ExprLoc>,
}

impl Function {
    /// Create a new function definition.
    ///
    /// # Arguments
    /// * `name` - The function name identifier
    /// * `signature` - The function signature with parameter names and defaults
    /// * `body` - The prepared function body AST
    /// * `namespace_size` - Number of local variable slots needed
    /// * `free_var_enclosing_slots` - Enclosing namespace slots for captured variables
    /// * `cell_var_count` - Number of cells to create for variables captured by nested functions
    /// * `cell_param_indices` - Maps cell indices to parameter indices for captured parameters
    /// * `default_exprs` - Prepared default value expressions for parameters
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: Identifier,
        signature: Signature,
        body: Vec<Node>,
        namespace_size: usize,
        free_var_enclosing_slots: Vec<NamespaceId>,
        cell_var_count: usize,
        cell_param_indices: Vec<Option<usize>>,
        default_exprs: Vec<ExprLoc>,
    ) -> Self {
        Self {
            name,
            signature,
            body,
            namespace_size,
            free_var_enclosing_slots,
            cell_var_count,
            cell_param_indices,
            default_exprs,
        }
    }

    /// Returns true if this function has any default parameter values.
    #[must_use]
    pub fn has_defaults(&self) -> bool {
        !self.default_exprs.is_empty()
    }

    /// Returns true if this function has any free variables (is a closure).
    #[must_use]
    pub fn is_closure(&self) -> bool {
        !self.free_var_enclosing_slots.is_empty()
    }

    /// Returns true if this function is equal to another function.
    ///
    /// We assume functions are equal if they have the same name and position.
    pub fn py_eq(&self, other: &Self) -> bool {
        self.name.py_eq(&other.name)
    }

    /// Calls this function with the given arguments.
    ///
    /// This method is used for non-closure functions. For closures (functions with
    /// captured variables), use `call_with_cells` instead.
    ///
    /// Returns `EvalResult::Value` on normal completion, or `EvalResult::ExternalCall`
    /// if execution is suspended waiting for an external function call.
    ///
    /// # Arguments
    /// * `function_id` - The ID of this function (for tracking in call stack)
    /// * `namespaces` - The namespace storage for managing all namespaces
    /// * `heap` - The heap for allocating objects
    /// * `args` - The arguments to pass to the function
    /// * `defaults` - Evaluated default values for optional parameters
    /// * `interns` - String storage for looking up interned names in error messages
    /// * `print` - The print for print output
    /// * `snapshot_tracker` - Tracker for recording execution position for resumption
    /// * `call_position` - Source position where this function is being called from
    #[allow(clippy::too_many_arguments)]
    pub fn call(
        &self,
        function_id: FunctionId,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        args: ArgValues,
        defaults: &[Value],
        interns: &Interns,
        print: &mut impl PrintWriter,
        snapshot_tracker: &mut impl AbstractSnapshotTracker,
        call_position: CodeRange,
    ) -> RunResult<EvalResult<Value>> {
        // Create a new local namespace for this function call (with memory and recursion tracking)
        // For resource errors (recursion, memory), we don't attach a frame here - the caller
        // will add the call site frame as the error propagates up, which is what we want.
        let local_idx = namespaces.new_namespace(self.namespace_size, heap)?;
        let namespace = namespaces.get_mut(local_idx).mut_vec();

        // 1. Bind arguments to parameters
        self.signature
            .bind(args, defaults, heap, interns, self.name, namespace)?;

        // 2. Push cell_var refs (slots param_count..param_count+cell_var_count)
        // These are cells for variables that nested functions capture from us.
        // For parameters that are also cell variables, we need to copy the param value into the cell.
        for (cell_idx, opt_param_idx) in self.cell_param_indices.iter().enumerate() {
            let initial_value = if let Some(param_idx) = opt_param_idx {
                // This cell is for a captured parameter - copy the parameter value
                namespace[*param_idx].clone_with_heap(heap)
            } else {
                // Regular cell variable - starts undefined
                Value::Undefined
            };
            let cell_id = heap.alloc_cell(initial_value);
            namespace.push(Value::Ref(cell_id));
            debug_assert_eq!(namespace.len(), self.signature.param_count() + cell_idx + 1);
        }

        // 3. No free_vars for non-closure functions (call_with_cells handles those)

        // 4. Fill remaining slots with Undefined for local variables
        namespace.resize_with(self.namespace_size, || Value::Undefined);

        // Track position depth before executing so we can extract function positions later
        let initial_depth = snapshot_tracker.depth();

        // Execute the function body in a new frame
        let mut frame = RunFrame::function_frame(local_idx, self.name.name_id, interns, snapshot_tracker, print);

        let result = match frame.execute(namespaces, heap, &self.body) {
            Ok(r) => r,
            Err(e) => {
                // Clean up namespace on error before propagating
                namespaces.drop_with_heap(local_idx, heap);
                return Err(e);
            }
        };

        // Handle the frame exit result
        Ok(handle_frame_exit(
            result,
            function_id,
            local_idx,
            self.name.name_id,
            0, // No captured cells for non-closure functions
            initial_depth,
            snapshot_tracker,
            namespaces,
            heap,
            call_position,
        ))
    }

    /// Calls this function as a closure with captured cells.
    ///
    /// Returns `EvalResult::Value` on normal completion, or `EvalResult::ExternalCall`
    /// if execution is suspended waiting for an external function call.
    ///
    /// # Arguments
    /// * `function_id` - The ID of this function (for tracking in call stack)
    /// * `namespaces` - The namespace manager for all namespaces
    /// * `heap` - The heap for allocating objects
    /// * `args` - The arguments to pass to the function
    /// * `captured_cells` - Cell HeapIds captured from the enclosing scope
    /// * `defaults` - Evaluated default values for optional parameters
    /// * `interns` - String storage for looking up interned names in error messages
    /// * `print` - The print for print output
    /// * `snapshot_tracker` - Tracker for recording execution position for resumption
    /// * `call_position` - Source position where this function is being called from
    ///
    /// This method is called when invoking a `Value::Closure`. The captured_cells
    /// are pushed sequentially after cell_vars in the namespace.
    #[allow(clippy::too_many_arguments)]
    pub fn call_with_cells(
        &self,
        function_id: FunctionId,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        args: ArgValues,
        captured_cells: &[HeapId],
        defaults: &[Value],
        interns: &Interns,
        print: &mut impl PrintWriter,
        snapshot_tracker: &mut impl AbstractSnapshotTracker,
        call_position: CodeRange,
    ) -> RunResult<EvalResult<Value>> {
        // Create a new local namespace for this function call (with memory and recursion tracking)
        // For resource errors (recursion, memory), we don't attach a frame here - the caller
        // will add the call site frame as the error propagates up, which is what we want.
        let local_idx = namespaces.new_namespace(self.namespace_size, heap)?;
        let namespace = namespaces.get_mut(local_idx).mut_vec();

        // 1. Bind arguments to parameters
        self.signature
            .bind(args, defaults, heap, interns, self.name, namespace)?;

        // 2. Push cell_var refs (slots param_count..param_count+cell_var_count)
        // A closure can also have cell_vars if it has nested functions.
        // For parameters that are also cell variables, we need to copy the param value into the cell.
        for (cell_idx, opt_param_idx) in self.cell_param_indices.iter().enumerate() {
            let initial_value = if let Some(param_idx) = opt_param_idx {
                // This cell is for a captured parameter - copy the parameter value
                namespace[*param_idx].clone_with_heap(heap)
            } else {
                // Regular cell variable - starts undefined
                Value::Undefined
            };
            let cell_id = heap.alloc_cell(initial_value);
            namespace.push(Value::Ref(cell_id));
            debug_assert_eq!(namespace.len(), self.signature.param_count() + cell_idx + 1);
        }

        // 3. Push free_var refs (captured cells from enclosing scope)
        // Order of captured_cells matches free_var_enclosing_slots
        for &cell_id in captured_cells {
            heap.inc_ref(cell_id);
            namespace.push(Value::Ref(cell_id));
        }

        // 4. Fill remaining slots with Undefined for local variables
        namespace.resize_with(self.namespace_size, || Value::Undefined);

        // Track position depth before executing so we can extract function positions later
        let initial_depth = snapshot_tracker.depth();

        // Execute the function body in a new frame
        let mut frame = RunFrame::function_frame(local_idx, self.name.name_id, interns, snapshot_tracker, print);

        let result = match frame.execute(namespaces, heap, &self.body) {
            Ok(r) => r,
            Err(e) => {
                // Clean up namespace on error before propagating
                namespaces.drop_with_heap(local_idx, heap);
                return Err(e);
            }
        };

        // Handle the frame exit result
        Ok(handle_frame_exit(
            result,
            function_id,
            local_idx,
            self.name.name_id,
            captured_cells.len(),
            initial_depth,
            snapshot_tracker,
            namespaces,
            heap,
            call_position,
        ))
    }

    /// Writes the Python repr() string for this function to a formatter.
    pub fn py_repr_fmt<W: Write>(
        &self,
        f: &mut W,
        interns: &Interns,
        // TODO use actual heap_id
        heap_id: usize,
    ) -> std::fmt::Result {
        write!(
            f,
            "<function '{}' at 0x{:x}>",
            interns.get_str(self.name.name_id),
            heap_id
        )
    }
}

/// Handles the result of executing a function frame.
///
/// On normal return, cleans up the namespace and returns the value.
/// On external call, preserves the namespace and pushes a FunctionFrame onto the call stack.
#[allow(clippy::too_many_arguments)]
fn handle_frame_exit(
    result: Option<FrameExit>,
    function_id: FunctionId,
    namespace_idx: NamespaceId,
    name_id: StringId,
    captured_cell_count: usize,
    initial_depth: usize,
    snapshot_tracker: &mut impl AbstractSnapshotTracker,
    namespaces: &mut Namespaces,
    heap: &mut Heap<impl ResourceTracker>,
    call_position: CodeRange,
) -> EvalResult<Value> {
    match result {
        Some(FrameExit::Return(value)) => {
            // Normal return - clean up the namespace
            namespaces.drop_with_heap(namespace_idx, heap);
            EvalResult::Value(value)
        }
        Some(FrameExit::ExternalCall(mut ext_call)) => {
            // Extract this function's positions from the shared tracker
            let saved_positions = snapshot_tracker.extract_after(initial_depth);

            // External call - preserve namespace and push this frame onto call stack
            ext_call.push_frame(FunctionFrame {
                function_id,
                namespace_idx,
                name_id,
                captured_cell_count,
                saved_positions,
                call_position,
            });
            EvalResult::ExternalCall(ext_call)
        }
        None => {
            // Implicit return None - clean up the namespace
            namespaces.drop_with_heap(namespace_idx, heap);
            EvalResult::Value(Value::None)
        }
    }
}
