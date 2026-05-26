# Classes

Sandboxed Python code in Monty cannot define new classes. The `class`
statement is rejected at parse time (see [language.md](language.md)),
and `type()`, `@dataclass`, `collections.namedtuple`, and `typing.NamedTuple`
are all unavailable as class factories *inside* the sandbox.

The host can construct dataclass and namedtuple values (using the
`MontyObject` API) and pass them in. Sandboxed code can then read fields,
call methods, mutate (if not frozen), and round-trip them through the
host. Methods defined on a host-supplied dataclass DO work — see
`test_cases/dataclass__basic.py`.

## What does NOT exist for user code

- `class Foo: ...` — rejected at parse time.
- `class Foo(Bar): ...` — there is no inheritance, no MRO, no `super()`.
- Metaclasses, `__init_subclass__`, `__set_name__`.
- `__slots__`, descriptors (`__get__` / `__set__` / `__delete__`).
- Abstract base classes (`abc.ABC`, `@abstractmethod`).
- `@classmethod`, `@staticmethod`, `@property` decorators.
- Dunder protocols on user-side types: `__init__`, `__new__`, `__call__`,
  `__iter__`, `__next__`, `__getitem__`, `__setitem__`, `__contains__`,
  `__enter__`, `__exit__`, `__add__`, `__eq__`, `__hash__`, `__repr__`,
  `__str__`, `__bool__`, etc. None of these are dispatched for any value
  the sandbox itself constructs — they are only consulted on host-supplied
  dataclasses (and only for the methods the host attached).
- Multiple inheritance, mixins, diamond MRO.

## `FrozenInstanceError`

Raised when assigning to a field of a frozen host-supplied dataclass.
Subclass of `AttributeError` — `except AttributeError:` catches it, as in
CPython's `dataclasses` module.

## Practical consequence

Code that depends on user-defined dunders — custom iterators, custom
context managers, custom hashable wrappers, most ORM models, anything
using `__init_subclass__` for registration — will not run on Monty.
Define such types on the host side and pass them in.
