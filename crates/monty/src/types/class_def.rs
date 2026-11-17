//! Native class definitions, implemented beside their constructors and members.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::DropWithContext,
    types::Type,
    value::{EitherStr, Value},
};

/// Metadata and class-level behaviour for a Rust-defined Python class.
/// Implement this on the payload type, or a marker when several classes share storage.
/// `native_types!` registers each implementation without changing its stored type identity.
pub(crate) trait ClassDef {
    /// CPython's `tp_name`, including its module qualifier where applicable.
    const NAME: &'static str;
    /// Name in the builtins namespace; module-only classes leave this absent.
    const BUILTIN_NAME: Option<&'static str> = None;
    /// Direct bases in declaration order; only `object` has none.
    const BASES: &'static [Type] = &[Type::Object];
    /// Written C3 order for multiple inheritance; otherwise walk the first base.
    const MRO: Option<&'static [Type]> = None;
    /// Whether CPython permits subclassing this class (`Py_TPFLAGS_BASETYPE`).
    const BASETYPE: bool = Self::EXC_TYPE.is_some();
    /// Whether subscripting the class constructs a `types.GenericAlias`.
    const CLASS_GETITEM: bool = false;
    /// Whether multiplication can fall back to sequence repetition.
    const SEQUENCE_REPEAT: bool = false;
    /// Whether instances are one of CPython's concrete iterator types.
    const ITERATOR: bool = false;
    /// Boundary exception type, also used by the default exception constructor.
    const EXC_TYPE: Option<ExcType> = None;

    /// Consumes the arguments, including on failure; may suspend in Python code.
    fn construct(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
        if let Some(exc) = Self::EXC_TYPE {
            exc.call(vm, args).map(CallResult::Value)
        } else {
            args.drop_with(vm);
            Err(ExcType::type_error_not_callable(Self::NAME))
        }
    }

    /// Returns an owned member, or `None` to continue lookup in the native MRO.
    fn class_attr(_attr: &EitherStr, _vm: &mut VM<'_>) -> Option<Value> {
        None
    }

    /// Returns untouched arguments in `Err` when ordinary attribute lookup should run.
    fn class_method(_attr: &EitherStr, args: ArgValues, _vm: &mut VM<'_>) -> Result<RunResult<CallResult>, ArgValues> {
        Err(args)
    }
}
