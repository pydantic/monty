"""Wrapper controlling how a host class instance is exposed to the Monty sandbox.

Wrap any object in `ClassInstance` and pass it as an input (or return it from an
external function) to send it into the sandbox. The wrapper is a *policy*: it
decides which attributes cross eagerly, which may be fetched lazily, and which
methods sandbox code may call. The sandbox routes lazy attribute lookups and
method calls back to the wrapped instance by a session-local uuid, and when
sandbox code returns the instance, the host receives the original object back.

`ClassType` is the class-level sibling: wrap a *class* to pass it into the
sandbox. Its shared policies expose the class object itself (class
constants via `eager_attrs`/`lazy_attrs`, classmethods via
`allowed_methods`), `init=True` lets sandbox code instantiate it, and the
`instance_*` policies are applied to each constructed instance. An instance's
class branch carries its `ClassType`'s id and eager class attrs, so `type(x)`
inside the sandbox is the same object as a `ClassType` passed as a value.

Subclass and override `convert_value` (or `call_method` / `lookup_lazy_attrs`)
to transform values crossing the boundary.
"""

from __future__ import annotations

from collections.abc import Coroutine, Iterable
from dataclasses import KW_ONLY, dataclass, field, fields, is_dataclass
from inspect import iscoroutine
from types import FunctionType
from typing import Any, Literal, TypeAlias, cast
from uuid import UUID, uuid4

__all__ = 'ClassInstance', 'ClassType'

Policy: TypeAlias = Iterable[str] | Literal['all'] | None
"""An exposure policy as accepted by the wrappers: `None` exposes nothing,
`'all'` everything the wrapper deems public, an iterable of names exactly
those. Normalised at construction, see `BaseWrapper.__post_init__`."""


@dataclass
class BaseWrapper:
    """Base type for instance and type wrappers."""

    value: Any
    """The instance to send."""
    _: KW_ONLY

    eager_attrs: Policy = None
    """Attributes sent into the sandbox up front; `'all'` sends dataclass
    fields (or `__dict__` entries and `__slots__`) whose names don't start
    with `_`."""

    lazy_attrs: Policy = None
    """Attributes the sandbox may fetch on demand via name lookup."""

    allowed_methods: Policy = None
    """Methods the sandbox may call; `'all'` means functions defined on the
    class only (no nested classes or callables stored as attributes)."""

    def __post_init__(self) -> None:
        """Normalises the policies: `None` and `'all'` stay, any other iterable
        of names becomes a tuple (`eager_attrs`, order is the send order) or a
        frozenset. A `str` other than `'all'` raises `TypeError` rather than
        being treated as an iterable of characters."""
        self.eager_attrs = _normalize_policy('eager_attrs', self.eager_attrs, tuple)
        self.lazy_attrs = _normalize_policy('lazy_attrs', self.lazy_attrs, frozenset)
        self.allowed_methods = _normalize_policy('allowed_methods', self.allowed_methods, frozenset)

    def get_eager_attrs(self) -> dict[str, Any]:
        """The attributes to send into the sandbox with the instance."""
        if self.eager_attrs is None:
            return {}

        eager_attrs: Iterable[tuple[str, Any]]
        if self.eager_attrs == 'all':
            if self.is_dataclass():
                eager_attrs = [(f.name, getattr(self.value, f.name)) for f in fields(self.value)]
            else:
                eager_attrs = [
                    *getattr(self.value, '__dict__', {}).items(),
                    *(
                        (name, getattr(self.value, name))
                        for name in _slot_names(self.value_type)
                        if hasattr(self.value, name)
                    ),
                ]
            eager_attrs = [(name, value) for name, value in eager_attrs if not name.startswith('_')]
        else:
            eager_attrs = [(name, getattr(self.value, name)) for name in self.eager_attrs]
        return {name: self.convert_value(name, value) for name, value in eager_attrs}

    def lookup_lazy_attrs(self, name: str) -> Any:
        """Resolves a lazy attribute lookup from the sandbox.

        Raises `AttributeError` when `name` is not exposed by `lazy_attrs`; the
        sandbox then raises the same `AttributeError`. Any other exception (from
        a property, `convert_value`, or a value that cannot be converted) is
        raised inside the sandbox, bypassing `hasattr` / `getattr` defaults.
        """
        attr_value = self.get_attr(name, self.lazy_attrs)
        return self.convert_value(name, attr_value)

    def call_method(self, name: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        """Calls a method on the wrapped instance for the sandbox.

        Raises `AttributeError` when `name` is not exposed by `allowed_methods`
        or fails `method_allowed`. `__call__` is always rejected on instances —
        only `ClassType` accepts it (as construction) — so even
        `allowed_methods='all'` cannot invoke the instance itself. The return
        value passes through `convert_value` before crossing back; a coroutine
        result defers conversion until awaited.
        """
        if name == '__call__' or not self.method_allowed(name):
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

    def method_allowed(self, name: str) -> bool:
        """Extra gate on `call_method` beyond the `allowed_methods` policy.

        Under `'all'` only functions defined on the class qualify, so a nested
        class or a callable stored as an attribute is not exposed; an explicit
        set calls whatever `getattr` returns, since the host named it.
        """
        return self.allowed_methods != 'all' or _method_kind(self.value_type, name) is not None

    def convert_value(self, /, name: str, value: Any) -> Any:
        """Hook to transform attribute values and method return values before
        they cross into the sandbox; the default passes them through unchanged.
        Derived class instances are deliberately not auto-wrapped (that would
        silently widen exposure), so override this to wrap them with policies
        chosen per value, e.g. `return ClassInstance(value, eager_attrs='all')`.
        """
        return value

    def get_attr(self, name: str, policy: Policy) -> Any:
        """Raw attribute access guarded by a normalised exposure policy (no conversion)."""
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

    @property
    def value_type(self) -> type[Any]:
        """The class of the wrapped value (the class itself for `ClassType`)."""
        return cast(type[Any], type(self.value))


@dataclass
class ClassInstance(BaseWrapper):
    """Policy wrapper exposing a host class instance to the Monty sandbox.

    Example:
    ```python
    session.feed_run(
        'assert user.greeting() == "hi Samuel"',
        inputs={'user': ClassInstance(user, eager_attrs='all', allowed_methods={'greeting'})},
    )
    ```
    """

    _: KW_ONLY

    id: UUID = field(default_factory=uuid4)
    """Unique id for the value."""

    class_type: ClassType | None = None
    """The `ClassType` wrapper for the value's type.

    Defaults to `ClassType(type(value))`; pass one to carry a pinned `id` or
    eager class attrs with the instance. Its eager class attrs are sent on
    every crossing of the instance, so `type(x)` in the sandbox sees them.
    """

    def __post_init__(self) -> None:
        super().__post_init__()
        if self.class_type is None:
            self.class_type = ClassType(self.value_type)
        else:
            # validate
            if self.class_type.value is not type(self.value):
                raise ValueError(
                    f'class_type {self.class_type.value.__name__} does not match value {type(self.value).__name__}'
                )

    def convert_value(self, /, name: str, value: Any) -> Any:
        """Defers to the class wrapper's hook, so a `ClassType` subclass that
        redacts or wraps values covers the instances it constructs and any
        instance sent with it as `class_type`; override here to differ."""
        return cast(ClassType, self.class_type).convert_value(name, value)


type_id_cache: dict[str, UUID] = {}
"""Process-wide default class ids keyed by `module.qualname`, never evicted,
so instances keep a stable type identity across sessions. Pre-seed it (or
pass `ClassType(..., id=...)`) to pin ids when restoring a dump in a fresh
process. Distinct classes sharing a name share the default id, so sending
both into one session is rejected — pass an explicit `id` to one."""


@dataclass
class ClassType(BaseWrapper):
    """Policy wrapper exposing a host *class* to the Monty sandbox.

    `ClassInstance`'s sibling, applied to the class object itself: `eager_attrs`
    sends class constants with the type, `lazy_attrs` serves them on demand,
    and `allowed_methods` exposes classmethods/staticmethods.

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

    _: KW_ONLY

    id: UUID | None = None
    """Unique id for the type.

    If unset an ID will be reused or generated.
    """

    init: bool = False
    """Whether sandbox code may instantiate the class.

    Purely a host-side policy: it never crosses the wire, and `construct`
    checks it on every request.
    """

    instance_eager_attrs: Policy = None
    """Policy applied to constructed instances (see `ClassInstance`)."""

    instance_lazy_attrs: Policy = None
    """Policy applied to constructed instances (see `ClassInstance`)."""

    instance_allowed_methods: Policy = None
    """Policy applied to constructed instances (see `ClassInstance`)."""

    def __post_init__(self) -> None:
        super().__post_init__()
        self.instance_eager_attrs = _normalize_policy('instance_eager_attrs', self.instance_eager_attrs, tuple)
        self.instance_lazy_attrs = _normalize_policy('instance_lazy_attrs', self.instance_lazy_attrs, frozenset)
        self.instance_allowed_methods = _normalize_policy(
            'instance_allowed_methods', self.instance_allowed_methods, frozenset
        )
        if self.id is None:
            name = f'{self.value.__module__}.{self.value.__qualname__}'
            if cached_id := type_id_cache.get(name):
                self.id = cached_id
            else:
                self.id = type_id_cache[name] = uuid4()

    def get_eager_attrs(self) -> dict[str, Any]:
        """Class-object variant of eager attrs: `'all'` sends public
        non-callable entries of the class `__dict__` (class constants),
        skipping methods and descriptors; an explicit list reads exactly
        those names. Called when the class crosses as a value and for every
        crossing of one of its instances."""
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

    def method_allowed(self, name: str) -> bool:
        """Only classmethods and staticmethods are callable on the class, under
        every policy: an instance method reached through the class would take
        an arbitrary sandbox value as `self`."""
        return _method_kind(self.value_type, name) in ('classmethod', 'staticmethod')

    def construct(self, args: tuple[Any, ...], kwargs: dict[str, Any]) -> ClassInstance:
        """Constructs an instance for the sandbox, re-checking the `init` policy.

        Returns the instance wrapped with the `instance_*` policies and this
        wrapper's `convert_value`, so it registers and crosses back like any
        host-sent `ClassInstance`.
        """
        if not self.init:
            raise TypeError(f'cannot instantiate host class {self.value.__name__!r}')
        instance = self.value(*args, **kwargs)
        return self.instance_wrapper(instance)

    def instance_wrapper(self, instance: Any) -> ClassInstance:
        """Wraps a constructed instance with the `instance_*` policies.

        The instance carries this wrapper as its `class_type`, so its class
        keeps this wrapper's `id` (an explicit one included) and eager class
        attrs. A constructor returning an instance of another class (a
        `__new__` override) gets that class's default `ClassType` instead,
        since `ClassInstance` rejects a mismatched `class_type`.

        Override to customize how constructed instances are exposed.
        """
        return ClassInstance(
            instance,
            eager_attrs=self.instance_eager_attrs,
            lazy_attrs=self.instance_lazy_attrs,
            allowed_methods=self.instance_allowed_methods,
            class_type=self if type(instance) is self.value else None,
        )

    def attr_error(self, name: str) -> AttributeError:
        return AttributeError(f'type object {self.value.__name__!r} has no attribute {name!r}')

    @property
    def value_type(self) -> type[Any]:
        return self.value


def _normalize_policy(field_name: str, policy: Policy, collect: type[tuple[str, ...]] | type[frozenset[str]]) -> Policy:
    """Validates a policy and collects an iterable of names with `collect`.

    A bare `str` other than `'all'` is almost certainly a typo for a set of
    names — and would otherwise expose every substring — so it is rejected.
    """
    if policy is None or policy == 'all':
        return policy
    elif isinstance(policy, str):
        raise TypeError(f"{field_name} must be 'all', None or a set of names, got {policy!r}")
    return collect(policy)


def _method_kind(cls: type[Any], name: str) -> Literal['function', 'classmethod', 'staticmethod'] | None:
    """Classifies how `cls` defines `name`, from the raw `__dict__` entry of
    the first class in the MRO that has it; `None` for anything that is not a
    function defined on the class (a nested class, a constant, a builtin
    slot wrapper, an absent name)."""
    for klass in cls.__mro__:
        if name in vars(klass):
            raw = vars(klass)[name]
            if isinstance(raw, classmethod):
                return 'classmethod'
            elif isinstance(raw, staticmethod):
                return 'staticmethod'
            elif isinstance(raw, FunctionType):
                return 'function'
            else:
                return None
    return None


def _slot_names(cls: type[Any]) -> list[str]:
    """Names of the `__slots__` declared along `cls`'s MRO; `eager_attrs='all'`
    reads those set on the instance alongside `__dict__`, which a slotted
    class may not have at all."""
    names: list[str] = []
    for klass in cls.__mro__:
        slots = vars(klass).get('__slots__', ())
        names.extend([slots] if isinstance(slots, str) else slots)
    return names


def _is_class_machinery(value: object) -> bool:
    """Whether a class `__dict__` entry is a method or descriptor rather than
    a class constant — excluded from `eager_attrs='all'` on a `ClassType`.

    The `__get__` check catches every descriptor (`classmethod`,
    `staticmethod`, `property`, `functools.cached_property`, ...), so `'all'`
    only ever sends plain class constants.
    """
    return callable(value) or hasattr(type(value), '__get__')
