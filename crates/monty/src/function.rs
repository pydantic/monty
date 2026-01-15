use std::fmt::Write;

use crate::{
    bytecode::Code,
    expressions::Identifier,
    intern::{Interns, StringId},
    namespace::NamespaceId,
    signature::Signature,
};

/// A compiled function ready for execution.
///
/// This is created during the compilation phase from a `PreparedFunctionDef`.
/// Contains everything needed to execute a user-defined function: compiled bytecode,
/// metadata, and closure information. Functions are stored on the heap and
/// referenced via HeapId.
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
    /// Number of default parameter values.
    ///
    /// At function definition time, this many default values are evaluated and stored
    /// in a separate defaults array. The signature indicates how these map to parameters.
    pub defaults_count: usize,
    /// Compiled bytecode for this function body.
    pub code: Code,
}

impl Function {
    /// Create a new compiled function.
    ///
    /// This is typically called by the bytecode compiler after compiling a `PreparedFunctionDef`.
    ///
    /// # Arguments
    /// * `name` - The function name identifier
    /// * `signature` - The function signature with parameter names and defaults
    /// * `namespace_size` - Number of local variable slots needed
    /// * `free_var_enclosing_slots` - Enclosing namespace slots for captured variables
    /// * `cell_var_count` - Number of cells to create for variables captured by nested functions
    /// * `cell_param_indices` - Maps cell indices to parameter indices for captured parameters
    /// * `defaults_count` - Number of default parameter values
    /// * `code` - The compiled bytecode for the function body
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        name: Identifier,
        signature: Signature,
        namespace_size: usize,
        free_var_enclosing_slots: Vec<NamespaceId>,
        cell_var_count: usize,
        cell_param_indices: Vec<Option<usize>>,
        defaults_count: usize,
        code: Code,
    ) -> Self {
        Self {
            name,
            signature,
            namespace_size,
            free_var_enclosing_slots,
            cell_var_count,
            cell_param_indices,
            defaults_count,
            code,
        }
    }

    /// Returns true if this function has any default parameter values.
    #[must_use]
    pub fn has_defaults(&self) -> bool {
        self.defaults_count > 0
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
