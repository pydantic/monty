use std::{
    cell::OnceCell,
    fmt::{self, Write},
    sync::Arc,
};

use serde::{Deserialize, Deserializer, de::Error as _};

use crate::{args::Signature, bytecode::Code, expressions::Identifier, intern::Interns, namespace::NamespaceId};

/// How an exact positional call can bypass argument binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExactPositionalCall {
    /// Arguments can remain on the VM stack as synchronous frame locals.
    Sync(usize),
    /// Arguments can move directly from the VM stack into a coroutine namespace.
    Async(usize),
}

/// A defined function once compiled and ready for execution.
///
/// This is created during the compilation phase from a `PreparedFunctionDef`.
/// Contains everything needed to execute a user-defined function: compiled bytecode,
/// metadata, and closure information. Functions are stored on the heap and
/// referenced via HeapId.
///
/// # Namespace Layout
///
/// Parameters occupy slots `0..signature.param_count()` (see `Signature`).
/// Cell variables, captured free variables, and ordinary locals follow, but
/// their slots are **explicit** (carried in `cell_var_slots` / `free_var_slots`)
/// rather than positional: a transitively captured (pass-through) free variable
/// is discovered late during preparation and is assigned a slot in the locals
/// region, so the old contiguous `[params][cells][free][locals]` invariant no
/// longer holds. Each cell/free slot is therefore placed individually at frame
/// setup (see `install_closure_cells`).
///
/// # Closure Support
///
/// - `free_var_enclosing_slots[i]`: slot in the *enclosing* frame to read cell
///   `i` from when building a `Closure` at definition time.
/// - `free_var_slots[i]`: slot in *this* frame where that captured cell is
///   installed at call time (parallel to `free_var_enclosing_slots`).
/// - `cell_var_slots[i]`: slot in this frame for an owned cell (a local captured
///   by a nested function); a fresh cell is created there at call time.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Function {
    /// The function name (used for error messages and repr).
    pub name: Identifier,
    /// The function signature.
    pub signature: Signature,
    /// Size of the initial namespace (number of local variable slots).
    pub namespace_size: usize,
    /// Enclosing namespace slots for variables captured from enclosing scopes.
    ///
    /// At definition time the enclosing frame reads the cell `HeapId` at each
    /// slot to build a `Closure`. Parallel to [`Self::free_var_slots`].
    pub free_var_enclosing_slots: Vec<NamespaceId>,
    /// This frame's slots that receive the captured free-var cells, parallel to
    /// [`Self::free_var_enclosing_slots`]. Explicit (not positional) so
    /// late-allocated pass-through slots land correctly.
    pub free_var_slots: Vec<NamespaceId>,
    /// This frame's slots for owned cell variables (locals captured by nested
    /// functions); a fresh cell is created for each at call time. Parallel to
    /// [`Self::cell_param_indices`].
    pub cell_var_slots: Vec<NamespaceId>,
    /// Maps each cell variable (parallel to [`Self::cell_var_slots`]) to its
    /// parameter index when the cell is for a captured parameter, so the bound
    /// value can be copied in; `None` means the cell starts `Undefined`.
    pub cell_param_indices: Vec<Option<usize>>,
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
    /// Cached binder-free call plan, derived from the fields above and cached
    /// via [`Self::exact_positional_call`].
    ///
    /// Never serialized: a function loaded from a REPL dump starts with this
    /// empty and derives it fresh on first call, so staleness with respect to
    /// an older binary's derivation logic is structurally impossible rather
    /// than merely checked.
    #[serde(skip)]
    exact_positional_call: OnceCell<Option<ExactPositionalCall>>,
    /// Compiled bytecode for this function body. Wrapped in `Arc` to avoid deep clone.
    pub code: Arc<Code>,
}

/// Serialized fields for [`Function`], kept separate so untrusted dumps can be
/// validated before constructing runtime metadata.
#[derive(Deserialize)]
struct FunctionFields {
    name: Identifier,
    signature: Signature,
    namespace_size: usize,
    free_var_enclosing_slots: Vec<NamespaceId>,
    free_var_slots: Vec<NamespaceId>,
    cell_var_slots: Vec<NamespaceId>,
    cell_param_indices: Vec<Option<usize>>,
    defaults_count: usize,
    is_async: bool,
    code: Arc<Code>,
}

impl<'de> Deserialize<'de> for Function {
    /// Rejects forged function metadata that cannot satisfy the namespace layout.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = FunctionFields::deserialize(deserializer)?;
        if fields.signature.param_count() > fields.namespace_size {
            Err(D::Error::custom("function parameter count exceeds namespace size"))
        } else {
            Ok(Self {
                name: fields.name,
                signature: fields.signature,
                namespace_size: fields.namespace_size,
                free_var_enclosing_slots: fields.free_var_enclosing_slots,
                free_var_slots: fields.free_var_slots,
                cell_var_slots: fields.cell_var_slots,
                cell_param_indices: fields.cell_param_indices,
                defaults_count: fields.defaults_count,
                is_async: fields.is_async,
                exact_positional_call: OnceCell::new(),
                code: fields.code,
            })
        }
    }
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
    /// * `free_var_enclosing_slots` - Enclosing-frame slots for captured cells
    /// * `free_var_slots` - This frame's slots receiving the captured cells
    /// * `cell_var_slots` - This frame's slots for owned cells
    /// * `cell_param_indices` - Maps each owned cell to a parameter index, if any
    /// * `defaults_count` - Number of default parameter values
    /// * `is_async` - Whether this is an async function
    /// * `code` - The compiled bytecode for the function body
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        name: Identifier,
        signature: Signature,
        namespace_size: usize,
        free_var_enclosing_slots: Vec<NamespaceId>,
        free_var_slots: Vec<NamespaceId>,
        cell_var_slots: Vec<NamespaceId>,
        cell_param_indices: Vec<Option<usize>>,
        defaults_count: usize,
        is_async: bool,
        code: Code,
    ) -> Self {
        Self {
            name,
            signature,
            namespace_size,
            free_var_enclosing_slots,
            free_var_slots,
            cell_var_slots,
            cell_param_indices,
            defaults_count,
            is_async,
            exact_positional_call: OnceCell::new(),
            code: Arc::new(code),
        }
    }

    /// Returns the binder-free call plan for this function, deriving and
    /// caching it on first use.
    pub(crate) fn exact_positional_call(&self) -> Option<ExactPositionalCall> {
        *self
            .exact_positional_call
            .get_or_init(|| self.derive_exact_positional_call())
    }

    /// Derives the binder-free call plan from authoritative function metadata.
    fn derive_exact_positional_call(&self) -> Option<ExactPositionalCall> {
        if self.cell_var_slots.is_empty() && self.free_var_slots.is_empty() {
            self.signature.exact_positional_count().map(|count| {
                if self.is_async {
                    ExactPositionalCall::Async(count)
                } else {
                    ExactPositionalCall::Sync(count)
                }
            })
        } else {
            None
        }
    }

    /// Writes the Python repr() string for this function to a formatter.
    pub fn py_repr_fmt<W: Write>(&self, f: &mut W, interns: &Interns, py_id: impl fmt::LowerHex) -> fmt::Result {
        write!(
            f,
            "<function '{}' at 0x{:x}>",
            interns.get_str(self.name.name_id),
            py_id
        )
    }
}
