"""Wrapper controlling how a host class instance is exposed to the Monty sandbox.

Wrap any object in `ClassInstance` and pass it as an input (or return it from an
external function) to send it into the sandbox. The wrapper is a *policy*: it
decides which attributes cross eagerly, which may be fetched lazily, and which
methods sandbox code may call. The sandbox routes lazy attribute lookups and
method calls back to the wrapped instance by a session-local uuid, and when
sandbox code returns the instance, the host receives the original object back.

`ClassType` is the class-level sibling: wrap a *class* to pass it into the
sandbox, optionally letting sandbox code instantiate it (`init=True`); each
constructed instance is wrapped with the `ClassType`'s instance policies.

Subclass and override `convert_value` (or `call_method` / `lookup_lazy_attrs`)
to transform values crossing the boundary.
"""

from __future__ import annotations

from collections.abc import Iterable, Sequence
from dataclasses import dataclass, fields, is_dataclass
from typing import Any, Literal

__all__ = ('ClassInstance', 'ClassType')


@dataclass
class ClassInstance:
    """Policy wrapper exposing a host class instance to the Monty sandbox.

    Example:
    ```python
    session.feed_run(
        'assert user.greeting() == "hi Sam"',
        inputs={'user': ClassInstance(user, eager_attrs='all', allowed_methods={'greeting'})},
    )
    ```
    """

    class_instance: Any
    """The instance to send."""

    eager_attrs: Sequence[str] | Literal['all'] | None = None
    """Attributes sent into the sandbox up front; `'all'` sends dataclass
    fields (or `__dict__` entries) whose names don't start with `_`."""

    lazy_attrs: set[str] | Literal['all'] | None = None
    """Attributes the sandbox may fetch on demand via name lookup."""

    allowed_methods: set[str] | Literal['all'] | None = None
    """Methods the sandbox may call on the instance."""

    frozen: bool | None = None
    """Whether the sandbox rejects `setattr` with `FrozenInstanceError`.
    `None` auto-detects frozen dataclasses; any other object defaults to mutable."""

    init: bool = False
    """Whether sandbox code may instantiate the instance's class via `type(x)(...)`."""

    def get_eager_attrs(self) -> dict[str, Any]:
        """The attributes to send into the sandbox with the instance."""
        if self.eager_attrs is None:
            return {}

        eager_attrs: Iterable[tuple[str, Any]]
        if self.eager_attrs == 'all':
            if is_dataclass(self.class_instance):
                eager_attrs = [(f.name, getattr(self.class_instance, f.name)) for f in fields(self.class_instance)]
            else:
                eager_attrs = self.class_instance.__dict__.items()
            eager_attrs = [(name, value) for name, value in eager_attrs if not name.startswith('_')]
        else:
            eager_attrs = [(name, getattr(self.class_instance, name)) for name in self.eager_attrs]
        return {name: self.convert_value(name, value) for name, value in eager_attrs}

    def lookup_lazy_attrs(self, name: str) -> Any:
        """Resolves a lazy attribute lookup from the sandbox.

        Raises `AttributeError` when `name` is not exposed by `lazy_attrs`; the
        sandbox then raises the same `AttributeError`.
        """
        attr_value = self._get_attr(name, self.lazy_attrs)
        return self.convert_value(name, attr_value)

    def call_method(self, name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        """Calls a method on the wrapped instance for the sandbox.

        Raises `AttributeError` when `name` is not exposed by `allowed_methods`.
        The return value passes through `convert_value` before crossing back.
        """
        method = self._get_attr(name, self.allowed_methods)
        return self.convert_value(name, method(*args, **kwargs))

    def convert_value(self, /, name: str, value: Any) -> Any:
        """Hook to transform attribute values and method return values before
        they are sent to the sandbox.

        The default wraps dataclass instances in a child `ClassInstance`
        sharing this wrapper's policies — so methods (and attrs) yielding
        dataclasses work without ceremony — and passes everything else
        through unchanged. Override to customize.
        """
        if is_dataclass(value) and not isinstance(value, type):
            return self.child_wrapper(value)
        return value

    def child_wrapper(self, value: Any) -> ClassInstance:
        """Wraps a derived value (nested attr / method return) with the same
        exposure policies as this wrapper; `frozen` reverts to auto-detect."""
        return type(self)(
            value,
            eager_attrs=self.eager_attrs,
            lazy_attrs=self.lazy_attrs,
            allowed_methods=self.allowed_methods,
        )

    def _get_attr(self, name: str, policy: set[str] | Literal['all'] | None) -> Any:
        """Raw attribute access guarded by an exposure policy (no conversion)."""
        if policy != 'all' and (policy is None or name not in policy):
            raise AttributeError(f'{type(self.class_instance).__name__!r} object has no attribute {name!r}')
        return getattr(self.class_instance, name)

    def get_frozen(self) -> bool:
        """Whether the sandbox copy is frozen; auto-detects frozen dataclasses."""
        if self.frozen is not None:
            return self.frozen
        # instance lookup falls through to the class, where dataclasses store it
        params = getattr(self.class_instance, '__dataclass_params__', None)
        return params.frozen if params is not None else False

    def get_init(self) -> bool:
        """Whether the sandbox may instantiate the instance's class."""
        return self.init


@dataclass
class ClassType:
    """Policy wrapper exposing a host *class* to the Monty sandbox.

    With `init=True`, sandbox code may call the class to construct instances;
    the construction runs host-side and the result crosses back wrapped in a
    `ClassInstance` carrying this wrapper's instance policies.

    Example:
    ```python
    session.feed_run(
        'p = Point(1, 2)\nassert p.x == 1',
        inputs={'Point': ClassType(Point, init=True, eager_attrs='all')},
    )
    ```
    """

    class_type: type
    """The class to send."""

    init: bool = False
    """Whether sandbox code may instantiate the class. Checked here on every
    construction request — the wire flag alone is never trusted."""

    eager_attrs: Sequence[str] | Literal['all'] | None = None
    """Instance policy applied to constructed instances (see `ClassInstance`)."""

    lazy_attrs: set[str] | Literal['all'] | None = None
    """Instance policy applied to constructed instances (see `ClassInstance`)."""

    allowed_methods: set[str] | Literal['all'] | None = None
    """Instance policy applied to constructed instances (see `ClassInstance`)."""

    frozen: bool | None = None
    """Instance policy applied to constructed instances (see `ClassInstance`)."""

    def get_init(self) -> bool:
        """Whether the sandbox may instantiate the class."""
        return self.init

    def construct(self, args: tuple[Any, ...], kwargs: dict[str, Any]) -> ClassInstance:
        """Constructs an instance for the sandbox, re-checking the `init` policy.

        Returns the instance wrapped with this wrapper's instance policies, so
        it registers and crosses back like any host-sent `ClassInstance`.
        """
        if not self.init:
            raise TypeError(f'cannot instantiate host class {self.class_type.__name__!r}')
        instance = self.class_type(*args, **kwargs)
        return self.instance_wrapper(instance)

    def instance_wrapper(self, instance: Any) -> ClassInstance:
        """Wraps a constructed instance with this wrapper's instance policies.
        Override to customize how constructed instances are exposed."""
        return ClassInstance(
            instance,
            eager_attrs=self.eager_attrs,
            lazy_attrs=self.lazy_attrs,
            allowed_methods=self.allowed_methods,
            frozen=self.frozen,
        )
