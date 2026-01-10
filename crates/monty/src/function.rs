use std::fmt::Write;

use crate::{
    bytecode::Code,
    expressions::{ExprLoc, Identifier, Node},
    intern::{Interns, StringId},
    namespace::NamespaceId,
    signature::Signature,
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
    /// Compiled bytecode for this function.
    ///
    /// This is `None` until the function is compiled during the eager compilation phase
    /// in `Executor::new()`. After compilation, it contains the bytecode for the function body.
    pub code: Option<Code>,
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
            code: None,
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

    /// Returns the function name as a string ID.
    #[must_use]
    pub fn name_id(&self) -> StringId {
        self.name.name_id
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
