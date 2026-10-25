use std::{fmt::Write, sync::Arc};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::RunResult,
    hash::{HashValue, identity_hash},
    heap::HeapObjectRead,
    types::{LazyHeapSet, PyTrait, Type},
    value::{EitherStr, Value},
};

/// A host-provided callable identified by its external lookup name.
///
/// The name allocation is shared with the heap's weak external-function cache,
/// which preserves identity while any function with this name remains live.
/// Hints are leaf data (no heap references), so the entry stays GC-inert.
#[derive(Debug, serde::Deserialize)]
#[serde(from = "DumpedExtFunction")]
pub(crate) struct ExtFunction {
    name: Arc<str>,
    /// The host's type hints, shared by every crossing of this name.
    hints: ExtHints,
}

/// A host function's `(name, hint)` type hints, in the order the host sent them.
pub(crate) type ExtHints = Box<[(Box<str>, ExtHint)]>;

/// One host type hint: a builtin type, or the text of anything else.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum ExtHint {
    Type(Type),
    Text(Box<str>),
}

impl ExtFunction {
    /// Creates an external function with an owned, shareable lookup name and no hints.
    pub(crate) fn new(name: &str) -> Self {
        Self {
            name: Arc::from(name),
            hints: Box::default(),
        }
    }

    /// Returns the function name as text.
    pub(crate) fn as_str(&self) -> &str {
        &self.name
    }

    /// Returns the shared name used as the weak-cache key.
    pub(crate) fn cache_key(&self) -> Arc<str> {
        Arc::clone(&self.name)
    }

    /// Clones the function name for an external call suspension.
    pub(crate) fn clone_name(&self) -> EitherStr {
        self.name.to_string().into()
    }

    /// The host's `(name, hint)` type hints, in the order it sent them.
    pub(crate) fn hints(&self) -> &[(Box<str>, ExtHint)] {
        &self.hints
    }

    /// Replaces the hints; every live reference to this name observes the change.
    pub(crate) fn set_hints(&mut self, hints: ExtHints) {
        self.hints = hints;
    }
}

/// A hint-less function dumps as its bare name, the shape older dumps hold.
impl serde::Serialize for ExtFunction {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.hints.is_empty() {
            serializer.serialize_str(&self.name)
        } else {
            WithHints {
                name: Arc::clone(&self.name),
                hints: self.hints.clone(),
            }
            .serialize(serializer)
        }
    }
}

/// Either dumped shape of an [`ExtFunction`].
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum DumpedExtFunction {
    Name(Arc<str>),
    WithHints(WithHints),
}

/// The dumped shape of an [`ExtFunction`] with hints.
#[derive(serde::Serialize, serde::Deserialize)]
struct WithHints {
    #[serde(rename = "N")]
    name: Arc<str>,
    #[serde(rename = "H")]
    hints: ExtHints,
}

impl From<DumpedExtFunction> for ExtFunction {
    fn from(dumped: DumpedExtFunction) -> Self {
        match dumped {
            DumpedExtFunction::Name(name) => Self {
                name,
                hints: Box::default(),
            },
            DumpedExtFunction::WithHints(WithHints { name, hints }) => Self { name, hints },
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, ExtFunction> {
    /// Indistinguishable from a sandbox `def` to Python code, which is the
    /// point: a host function is passed around like any other function.
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::Function
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    /// Identity only. Two lookups of the same name share one heap entry (the
    /// weak external-function cache), so `is` already answers this.
    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_bool(&self, _: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        Ok(write!(f, "<function '{}' external>", self.get(vm.heap).as_str())?)
    }

    /// Suspends: only the host can run this, so the VM yields the name and the
    /// arguments and waits to be resumed with the result.
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        Ok(CallResult::External(self.get(vm.heap).clone_name(), args))
    }
}
