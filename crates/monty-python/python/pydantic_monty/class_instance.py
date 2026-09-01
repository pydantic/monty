"""Wrapper controlling how a host class instance is exposed to the Monty sandbox.

Wrap any object in `ClassInstance` and pass it as an input (or return it from an
external function) to send it into the sandbox. The wrapper is a *policy*: it
decides which attributes cross eagerly, which may be fetched lazily, and which
methods sandbox code may call. The sandbox routes lazy attribute lookups and
method calls back to the wrapped instance by a session-local uuid, and when
sandbox code returns the instance, the host receives the original object back.

`ClassType` is the class-level subclass: wrap a *class* to pass it into the
sandbox. The inherited policies expose the class object itself (class
constants via `eager_attrs`/`lazy_attrs`, classmethods via
`allowed_methods`), `init=True` lets sandbox code instantiate it, and the
`instance_*` policies are applied to each constructed instance.

Subclass and override `convert_value` (or `call_method` / `lookup_lazy_attrs`)
to transform values crossing the boundary.
"""

from __future__ import annotations

from collections.abc import Coroutine, Iterable, Sequence
from dataclasses import KW_ONLY, dataclass, field, fields, is_dataclass
from inspect import iscoroutine
from typing import Any, Literal
from uuid import UUID, uuid4

__all__ = 'ClassInstance', 'ClassType'


@dataclass
class ClassInstance:
    """Policy wrapper exposing a host class instance to the Monty sandbox.

    Example:
    ```python
    session.feed_run(
        'assert user.greeting() == "hi Samuel"',
        inputs={'user': ClassInstance(user, eager_attrs='all', allowed_methods={'greeting'})},
    )
    ```
    """

    value: Any
    """The instance to send."""
    _: KW_ONLY

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

    id: UUID = field(default_factory=uuid4)
    """The instance's sandbox identity: reuse one wrapper to re-send an object
    under the same id; reusing an id for a different object raises `ValueError`.
    On `ClassType`, the class's id, fixed by the class's first crossing."""

    def get_eager_attrs(self) -> dict[str, Any]:
        """The attributes to send into the sandbox with the instance."""
        if self.eager_attrs is None:
            return {}

        eager_attrs: Iterable[tuple[str, Any]]
        if self.eager_attrs == 'all':
            if self.is_dataclass():
                eager_attrs = [(f.name, getattr(self.value, f.name)) for f in fields(self.value)]
            else:
                eager_attrs = self.value.__dict__.items()
            eager_attrs = [(name, value) for name, value in eager_attrs if not name.startswith('_')]
        else:
            eager_attrs = [(name, getattr(self.value, name)) for name in self.eager_attrs]
        return {name: self.convert_value(name, value) for name, value in eager_attrs}

    def lookup_lazy_attrs(self, name: str) -> Any:
        """Resolves a lazy attribute lookup from the sandbox.

        Raises `AttributeError` when `name` is not exposed by `lazy_attrs`; the
        sandbox then raises the same `AttributeError`.
        """
        attr_value = self.get_attr(name, self.lazy_attrs)
        return self.convert_value(name, attr_value)

    def call_method(self, name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        """Calls a method on the wrapped instance for the sandbox.

        Raises `AttributeError` when `name` is not exposed by `allowed_methods`.
        `__call__` is always rejected on instances — only `ClassType` accepts it
        (as construction) — so even `allowed_methods='all'` cannot invoke the
        instance itself. The return value passes through `convert_value` before
        crossing back; a coroutine result defers conversion until awaited.
        """
        if name == '__call__':
            raise self.attr_error(name)
        method = self.get_attr(name, self.allowed_methods)
        result = method(*args, **kwargs)
        if iscoroutine(result):
            return self._convert_awaited(name, result)
        return self.convert_value(name, result)

    async def _convert_awaited(self, name: str, coro: Coroutine[Any, Any, Any]) -> Any:
        """Awaits an async method's result, then applies `convert_value` — so
        redaction hooks see the resolved value, never the coroutine object."""
        return self.convert_value(name, await coro)

    def convert_value(self, /, name: str, value: Any) -> Any:
        """Hook to transform attribute values and method return values before
        they are sent to the sandbox.

        The default passes values through unchanged, so a derived class
        instance fails conversion with the usual "wrap it in ClassInstance"
        error. Deliberately no automatic wrapping: each object's exposure
        must be an explicit host decision — a wrapper inheriting this
        wrapper's policies could silently widen access to an instance the
        host had locked down elsewhere. Override to wrap derived values with
        policies chosen per value, e.g.
        `return ClassInstance(value, eager_attrs='all')`.
        """
        return value

    def get_attr(self, name: str, policy: set[str] | Literal['all'] | None) -> Any:
        """Raw attribute access guarded by an exposure policy (no conversion)."""
        if policy != 'all' and (policy is None or name not in policy):
            raise self.attr_error(name)
        return getattr(self.value, name)

    def attr_error(self, name: str) -> AttributeError:
        """The error a denied or missing attribute raises; `ClassType`
        overrides it with CPython's type-object wording."""
        return AttributeError(f'{type(self.value).__name__!r} object has no attribute {name!r}')

    def is_dataclass(self) -> bool:
        """Whether the wrapped value is a dataclass."""
        return is_dataclass(self.value)

    def get_frozen(self) -> bool:
        """Whether the sandbox copy is frozen; auto-detects frozen dataclasses."""
        if self.frozen is not None:
            return self.frozen
        # instance lookup falls through to the class, where dataclasses store it
        params = getattr(self.value, '__dataclass_params__', None)
        return params.frozen if params is not None else False


@dataclass
class ClassType(ClassInstance):
    """Policy wrapper exposing a host *class* to the Monty sandbox.

    Inherits `ClassInstance`, applied to the class object itself: `eager_attrs`
    sends class constants with the type, `lazy_attrs` serves them on demand,
    and `allowed_methods` exposes classmethods/staticmethods. `frozen` is the
    exception — a type object rejects `setattr` regardless, so it (with the
    `instance_*` policies) governs constructed instances instead.

    With `init=True`, sandbox code may call the class; the construction
    arrives as a `__call__` method call, runs host-side, and the result
    crosses back wrapped in a `ClassInstance` carrying the `instance_*`
    policies.

    Example:
    ```python
    session.feed_run(
        'p = Point(1, 2)\nassert p.x == 1',
        inputs={'Point': ClassType(Point, init=True, instance_eager_attrs='all')},
    )
    ```
    """

    value: type[Any]
    """The type/class to send."""

    init: bool = False
    """Whether sandbox code may instantiate the class.

    Purely a host-side policy: it never crosses the wire, and `construct`
    checks it on every request.
    """

    instance_eager_attrs: Sequence[str] | Literal['all'] | None = None
    """Policy applied to constructed instances (see `ClassInstance`)."""

    instance_lazy_attrs: set[str] | Literal['all'] | None = None
    """Policy applied to constructed instances (see `ClassInstance`)."""

    instance_allowed_methods: set[str] | Literal['all'] | None = None
    """Policy applied to constructed instances (see `ClassInstance`)."""

    def get_eager_attrs(self) -> dict[str, Any]:
        """Class-object variant of eager attrs: `'all'` sends public
        non-callable entries of the class `__dict__` (class constants),
        skipping methods and descriptors; an explicit list reads exactly
        those names."""
        if self.eager_attrs is None:
            return {}
        if self.eager_attrs == 'all':
            eager_attrs = [
                (name, value)
                for name, value in vars(self.value).items()
                if not name.startswith('_') and not _is_class_machinery(value)
            ]
        else:
            eager_attrs = [(name, getattr(self.value, name)) for name in self.eager_attrs]
        return {name: self.convert_value(name, value) for name, value in eager_attrs}

    def call_method(self, name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        """Routes `__call__` (construction) to `construct`; every other name
        is a classmethod/staticmethod call gated by `allowed_methods`."""
        if name == '__call__':
            return self.construct(args, kwargs)
        else:
            return super().call_method(name, args, kwargs)

    def construct(self, args: tuple[Any, ...], kwargs: dict[str, Any]) -> ClassInstance:
        """Constructs an instance for the sandbox, re-checking the `init` policy.

        Returns the instance wrapped with the `instance_*` policies, so it
        registers and crosses back like any host-sent `ClassInstance`.
        """
        if not self.init:
            raise TypeError(f'cannot instantiate host class {self.value.__name__!r}')
        instance = self.value(*args, **kwargs)
        return self.instance_wrapper(instance)

    def instance_wrapper(self, instance: Any) -> ClassInstance:
        """Wraps a constructed instance with the `instance_*` policies (and
        the shared `frozen`). Override to customize how constructed instances
        are exposed."""
        return ClassInstance(
            instance,
            eager_attrs=self.instance_eager_attrs,
            lazy_attrs=self.instance_lazy_attrs,
            allowed_methods=self.instance_allowed_methods,
            frozen=self.frozen,
        )

    def attr_error(self, name: str) -> AttributeError:
        return AttributeError(f'type object {self.value.__name__!r} has no attribute {name!r}')


def _is_class_machinery(value: object) -> bool:
    """Whether a class `__dict__` entry is a method or descriptor rather than
    a class constant — excluded from `eager_attrs='all'` on a `ClassType`.

    The `__get__` check catches every descriptor (`classmethod`,
    `staticmethod`, `property`, `functools.cached_property`, ...), so `'all'`
    only ever sends plain class constants.
    """
    return callable(value) or hasattr(type(value), '__get__')
