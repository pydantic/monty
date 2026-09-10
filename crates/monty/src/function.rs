use std::{
    cell::OnceCell,
    cmp::Ordering,
    fmt::{self, Write},
    sync::Arc,
};

use serde::{Deserialize, Deserializer, de::Error as _};

#[cfg(feature = "test-hooks")]
use crate::args::SignatureMetadataFault;
use crate::{args::Signature, bytecode::Code, expressions::Identifier, intern::Interns, namespace::NamespaceId};

/// How an exact positional call can bypass argument binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExactPositionalCall {
    /// Arguments can remain on the VM stack as synchronous frame locals.
    Sync(usize),
    /// Arguments can move directly from the VM stack into a coroutine namespace.
    Async(usize),
}

/// Metadata faults injected by dump boundary tests.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub enum FunctionMetadataFault {
    /// Makes the signature require more slots than its namespace.
    SignatureSlotsBeyondNamespace,
    /// Makes the namespace impossible to represent in a frame.
    NamespaceTooLarge,
    /// Breaks the parallel free-variable vectors.
    FreeVarLengthMismatch,
    /// Breaks the parallel owned-cell vectors.
    CellVarLengthMismatch,
    /// Points a free-variable slot beyond the namespace.
    FreeVarSlotOutOfRange,
    /// Points an owned-cell slot beyond the namespace.
    CellVarSlotOutOfRange,
    /// Points an owned cell at a nonexistent parameter slot.
    CellParamIndexOutOfRange,
    /// Makes positional-only defaults outnumber their parameters.
    PosDefaultsCountOutOfRange,
    /// Makes positional defaults outnumber their parameters.
    ArgDefaultsCountOutOfRange,
    /// Breaks the keyword-only parameter/default-map pairing.
    KwargDefaultMapLengthMismatch,
    /// Makes keyword-only default indices non-contiguous.
    KwargDefaultIndexGap,
    /// Makes the function default count disagree with its signature.
    DefaultsCountMismatch,
    /// Maps two captured free variables to the same local slot.
    DuplicateFreeVarSlot,
    /// Maps an owned and captured cell to the same local slot.
    CellFreeVarSlotOverlap,
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
/// - `free_var_enclosing_slots[i]`: legacy compiler record of the enclosing
///   slot for captured cell `i`; runtime closure creation uses bytecode.
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
    /// Legacy compiler record of enclosing slots; closure creation uses bytecode.
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
    /// Function name used by diagnostics and repr.
    name: Identifier,
    /// Parameter layout consumed by the binder.
    signature: Signature,
    /// Number of local slots reserved by each frame.
    namespace_size: usize,
    /// Legacy record of enclosing slots used to compile closure loads.
    free_var_enclosing_slots: Vec<NamespaceId>,
    /// Local slots receiving captured free-variable cells.
    free_var_slots: Vec<NamespaceId>,
    /// Local slots receiving freshly allocated owned cells.
    cell_var_slots: Vec<NamespaceId>,
    /// Parameter slots copied into owned cells, when applicable.
    cell_param_indices: Vec<Option<usize>>,
    /// Number of default values evaluated at function creation.
    defaults_count: usize,
    /// Whether calls create a coroutine rather than a frame.
    is_async: bool,
    /// Compiled function body.
    code: Arc<Code>,
}

impl FunctionFields {
    /// Validates compiler-established invariants before metadata reaches the VM.
    fn validate(&self) -> Result<(), &'static str> {
        self.signature.validate()?;

        if u16::try_from(self.namespace_size).is_err() {
            return Err("function namespace size exceeds frame limit");
        }

        let signature_slots = self.signature.total_slots();
        if signature_slots > self.namespace_size {
            return Err("function signature slots exceed namespace size");
        }
        if self.defaults_count != self.signature.total_defaults_count() {
            return Err("function default count does not match signature");
        }
        if self.free_var_enclosing_slots.len() != self.free_var_slots.len() {
            return Err("function free-variable metadata has different lengths");
        }
        if self.cell_var_slots.len() != self.cell_param_indices.len() {
            return Err("function cell-variable metadata has different lengths");
        }

        for slots in [&self.cell_var_slots, &self.free_var_slots] {
            let mut previous_slot = None;
            for slot in slots {
                let slot = slot.index();
                if !(signature_slots..self.namespace_size).contains(&slot) {
                    return Err("function closure slot is outside the locals region");
                }
                if previous_slot.is_some_and(|previous| previous >= slot) {
                    return Err("function closure slots are not strictly ordered");
                }
                previous_slot = Some(slot);
            }
        }

        let mut cell_slots = self.cell_var_slots.iter().peekable();
        let mut free_slots = self.free_var_slots.iter().peekable();
        while let (Some(cell_slot), Some(free_slot)) = (cell_slots.peek(), free_slots.peek()) {
            match cell_slot.cmp(free_slot) {
                Ordering::Less => {
                    cell_slots.next();
                }
                Ordering::Greater => {
                    free_slots.next();
                }
                Ordering::Equal => return Err("function closure slots overlap"),
            }
        }

        if self
            .cell_param_indices
            .iter()
            .flatten()
            .any(|&index| index >= signature_slots)
        {
            return Err("function cell parameter index is out of range");
        }

        Ok(())
    }
}

impl<'de> Deserialize<'de> for Function {
    /// Rejects metadata that violates compiler-established function invariants.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = FunctionFields::deserialize(deserializer)?;
        fields.validate().map_err(D::Error::custom)?;
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

    /// Injects a metadata fault for dump validation tests.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn corrupt_metadata_for_tests(&mut self, fault: FunctionMetadataFault) {
        match fault {
            FunctionMetadataFault::SignatureSlotsBeyondNamespace => {
                self.namespace_size = self.signature.total_slots() - 1;
            }
            FunctionMetadataFault::NamespaceTooLarge => {
                self.namespace_size = usize::from(u16::MAX) + 1;
            }
            FunctionMetadataFault::FreeVarLengthMismatch => {
                self.free_var_slots.pop().expect("test function has a free variable");
            }
            FunctionMetadataFault::CellVarLengthMismatch => {
                self.cell_param_indices.pop().expect("test function has an owned cell");
            }
            FunctionMetadataFault::FreeVarSlotOutOfRange => {
                let slot = NamespaceId::new(self.namespace_size).expect("test namespace fits in u16");
                *self
                    .free_var_slots
                    .first_mut()
                    .expect("test function has a free variable") = slot;
            }
            FunctionMetadataFault::CellVarSlotOutOfRange => {
                let slot = NamespaceId::new(self.namespace_size).expect("test namespace fits in u16");
                *self
                    .cell_var_slots
                    .first_mut()
                    .expect("test function has an owned cell") = slot;
            }
            FunctionMetadataFault::CellParamIndexOutOfRange => {
                *self
                    .cell_param_indices
                    .first_mut()
                    .expect("test function has an owned cell") = Some(self.signature.total_slots());
            }
            FunctionMetadataFault::PosDefaultsCountOutOfRange => self
                .signature
                .corrupt_metadata_for_tests(SignatureMetadataFault::PosDefaultsCountOutOfRange),
            FunctionMetadataFault::ArgDefaultsCountOutOfRange => self
                .signature
                .corrupt_metadata_for_tests(SignatureMetadataFault::ArgDefaultsCountOutOfRange),
            FunctionMetadataFault::KwargDefaultMapLengthMismatch => self
                .signature
                .corrupt_metadata_for_tests(SignatureMetadataFault::KwargDefaultMapLengthMismatch),
            FunctionMetadataFault::KwargDefaultIndexGap => self
                .signature
                .corrupt_metadata_for_tests(SignatureMetadataFault::KwargDefaultIndexGap),
            FunctionMetadataFault::DefaultsCountMismatch => {
                self.defaults_count += 1;
            }
            FunctionMetadataFault::DuplicateFreeVarSlot => {
                let slots = self.free_var_slots.as_mut_slice();
                slots[1] = slots[0];
            }
            FunctionMetadataFault::CellFreeVarSlotOverlap => {
                self.free_var_slots[0] = self.cell_var_slots[0];
            }
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
