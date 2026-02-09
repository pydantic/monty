use std::fmt::Write;

use crate::{
    bytecode::Code,
    expressions::Identifier,
    intern::{Interns, StringId},
    namespace::NamespaceId,
    signature::Signature,
    value::EitherStr,
};

/// A defined function once compiled and ready for execution.
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
pub(crate) struct Function {
    /// The function name (used for error messages and repr).
    pub name: Identifier,
    /// Qualified name (e.g., `Outer.<locals>.inner` or `Class.method`).
    pub qualname: EitherStr,
    /// Module name where this function was defined.
    pub module_name: StringId,
    /// Type parameters declared with PEP 695 syntax (`def f[T]`).
    ///
    /// Stored as interned names; runtime semantics are provided via `__type_params__`.
    pub type_params: Vec<StringId>,
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
    /// Namespace slot reserved for the `__class__` cell in class body functions.
    ///
    /// This enables zero-argument `super()` and `__class__` references in methods
    /// by providing a cell that is set to the final class object during creation.
    #[serde(default)]
    pub class_cell_slot: Option<NamespaceId>,
    /// Number of default parameter values.
    ///
    /// At function definition time, this many default values are evaluated and stored
    /// in a separate defaults array. The signature indicates how these map to parameters.
    pub defaults_count: usize,
    /// Whether this is an async function (`async def`).
    ///
    /// When true, calling this function creates a `Coroutine` object instead of
    /// immediately pushing a frame. The coroutine captures the bound arguments
    /// and starts execution only when awaited.
    pub is_async: bool,
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
    /// * `is_async` - Whether this is an async function
    /// * `code` - The compiled bytecode for the function body
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        name: Identifier,
        qualname: EitherStr,
        module_name: StringId,
        type_params: Vec<StringId>,
        signature: Signature,
        namespace_size: usize,
        free_var_enclosing_slots: Vec<NamespaceId>,
        cell_var_count: usize,
        cell_param_indices: Vec<Option<usize>>,
        defaults_count: usize,
        is_async: bool,
        code: Code,
    ) -> Self {
        Self {
            name,
            qualname,
            module_name,
            type_params,
            signature,
            namespace_size,
            free_var_enclosing_slots,
            cell_var_count,
            cell_param_indices,
            class_cell_slot: None,
            defaults_count,
            is_async,
            code,
        }
    }

    /// Creates a Function for a class body.
    ///
    /// Class body functions have no parameters, no defaults, and are not async.
    /// They may reserve a single `__class__` cell to support zero-arg `super()`.
    /// They are executed by the `BuildClass` opcode to populate the class namespace.
    pub fn new_class_body(
        name: Identifier,
        qualname: EitherStr,
        module_name: StringId,
        type_params: Vec<StringId>,
        namespace_size: usize,
        class_cell_slot: Option<NamespaceId>,
        code: Code,
    ) -> Self {
        let cell_var_count = usize::from(class_cell_slot.is_some());
        Self {
            name,
            qualname,
            module_name,
            type_params,
            signature: Signature::default(),
            namespace_size,
            free_var_enclosing_slots: Vec::new(),
            cell_var_count,
            cell_param_indices: vec![None; cell_var_count],
            class_cell_slot,
            defaults_count: 0,
            is_async: false,
            code,
        }
    }

    /// Writes the Python repr() string for this function to a formatter.
    pub fn py_repr_fmt<W: Write>(&self, f: &mut W, interns: &Interns, py_id: usize) -> std::fmt::Result {
        write!(
            f,
            "<function '{}' at 0x{:x}>",
            interns.get_str(self.name.name_id),
            py_id
        )
    }
}
