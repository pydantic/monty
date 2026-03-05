//! Generic runtime observer events for host-side instrumentation.
//!
//! This module defines Track A observer primitives with no policy semantics.
//! Observer callbacks are optional and disabled by default.

use std::{
    fmt,
    sync::{Arc, Mutex},
};

use crate::runtime_id::RuntimeValueId;

/// Event class for external-call request points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExternalCallKind {
    /// A host external function call.
    Function,
    /// A host OS call.
    Os,
    /// A host dataclass method call.
    Method,
}

/// Event class for external-call return points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalCallReturnKind {
    /// Host returned a concrete value.
    Return,
    /// Host returned an exception.
    Error,
    /// Host left a future unresolved.
    Future,
}

/// Value creation event payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueCreatedEvent {
    /// Stable runtime ID of the created value.
    pub value_id: RuntimeValueId,
}

/// Compact input set for operation-result events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpInputIds {
    /// No tracked inputs.
    None,
    /// One tracked input.
    One(RuntimeValueId),
    /// Two tracked inputs.
    Two(RuntimeValueId, RuntimeValueId),
}

impl OpInputIds {
    /// Creates an empty input set.
    #[must_use]
    pub fn none() -> Self {
        Self::None
    }
}

/// Operation-result event payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpResultEvent {
    /// Runtime ID of the produced value.
    pub output_id: RuntimeValueId,
    /// Runtime IDs of contributing inputs.
    pub inputs: OpInputIds,
}

/// External-call request event payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalCallRequestedEvent<'a> {
    /// Call identifier visible to the host.
    pub call_id: u32,
    /// External-call class.
    pub kind: ExternalCallKind,
    /// Runtime IDs for positional args.
    pub arg_runtime_ids: &'a [RuntimeValueId],
    /// Runtime IDs for keyword `(key, value)` pairs.
    pub kwarg_runtime_ids: &'a [(RuntimeValueId, RuntimeValueId)],
}

/// External-call return event payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalCallReturnedEvent {
    /// Call identifier visible to the host.
    pub call_id: u32,
    /// External-return class.
    pub kind: ExternalCallReturnKind,
}

/// Control-condition event payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlConditionEvent {
    /// Runtime ID for the condition value.
    pub condition_id: RuntimeValueId,
    /// True when the branch/jump was taken.
    pub branch_taken: bool,
}

/// Canonical runtime observer event set for Track A.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeObserverEvent<'a> {
    /// A value identity was created at runtime.
    ValueCreated(ValueCreatedEvent),
    /// An operation produced an output from input IDs.
    OpResult(OpResultEvent),
    /// Execution requested host interaction.
    ExternalCallRequested(ExternalCallRequestedEvent<'a>),
    /// Execution consumed a host interaction outcome.
    ExternalCallReturned(ExternalCallReturnedEvent),
    /// Execution evaluated a control-flow condition.
    ControlCondition(ControlConditionEvent),
}

/// Runtime observer callback interface.
pub trait RuntimeObserver: Send {
    /// Called for each emitted runtime event.
    fn on_event(&mut self, event: RuntimeObserverEvent<'_>);
}

/// No-op observer used by default.
#[derive(Default)]
pub struct NoopRuntimeObserver;

impl RuntimeObserver for NoopRuntimeObserver {
    fn on_event(&mut self, _: RuntimeObserverEvent<'_>) {}
}

type SharedRuntimeObserver = Arc<Mutex<dyn RuntimeObserver>>;

/// Cloneable observer handle used by run/repl/snapshot state.
#[derive(Clone, Default)]
pub struct RuntimeObserverHandle {
    inner: Option<SharedRuntimeObserver>,
}

impl fmt::Debug for RuntimeObserverHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeObserverHandle")
            .field("enabled", &self.inner.is_some())
            .finish()
    }
}

impl RuntimeObserverHandle {
    /// Creates a disabled observer handle.
    #[must_use]
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// Wraps an observer implementation in a shared handle.
    #[must_use]
    pub fn new(observer: impl RuntimeObserver + 'static) -> Self {
        Self {
            inner: Some(Arc::new(Mutex::new(observer))),
        }
    }

    /// Wraps an existing shared observer.
    #[must_use]
    pub fn from_shared(observer: Arc<Mutex<dyn RuntimeObserver>>) -> Self {
        Self { inner: Some(observer) }
    }

    /// Returns true when an observer is installed.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Emits an event to the installed observer.
    pub fn emit(&self, event: RuntimeObserverEvent<'_>) {
        let Some(observer) = &self.inner else {
            return;
        };

        match observer.lock() {
            Ok(mut guard) => guard.on_event(event),
            Err(poisoned) => poisoned.into_inner().on_event(event),
        }
    }
}
