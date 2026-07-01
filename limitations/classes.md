# Classes

Sandboxed Python code in Monty can define simple classes. A `class`
statement with instance methods, `__init__`, `__repr__`/`__str__`, and
class variables works. The class body has a real scope (like CPython's
class-body code object), so class variables may be arbitrary expressions
and may reference earlier class variables:

```python
class Foo:
    count = 0

    def __init__(self, a: int) -> None:
        self.a = a

    def bar(self) -> int:
        return self.a * 2

    def __repr__(self) -> str:
        return f'Foo(a={self.a})'
```

See `test_cases/class__basic.py` and `test_cases/class__repr.py`.

The host can also construct dataclass and namedtuple values (using the
`MontyObject` API) and pass them in; those are a separate mechanism whose
methods dispatch back to the host (see `test_cases/dataclass__basic.py`).

## What works

- `class Foo: ...` with a body of instance methods and class variables
  (`name = <expr>` or `name: T = <expr>`, where `<expr>` is any expression,
  including calls, comprehensions, and references to earlier class variables).
- Class variable values run in the class-body scope, top-to-bottom; because
  that body is a real, suspendable frame, a class-variable value may call an
  external/OS function.
- `__init__` with arbitrary positional/keyword/default/`*args`/`**kwargs`
  parameters (methods are ordinary functions; `self` is the first parameter).
- Instance attribute get/set (`obj.x`, `obj.x = ...`), including attributes
  not declared in `__init__`.
- Instance methods, bound methods (`m = obj.method; m()`).
- Class variables, read via the class (`Foo.count`) or an instance
  (`obj.count`).
- `__repr__` and `__str__` dispatch (via `repr()`, `str()`, f-strings,
  `print`, and inside container reprs). `str()` falls back to `__repr__`.
- `type(obj)` returns the class object; `type(obj) is Foo` and
  `isinstance(obj, Foo)` work.
- `Foo.__name__`.

## Divergences from CPython

- **Default `repr`** (no user `__repr__`) is `<Foo object at 0x..>` using the
  **bare** class name, where CPython uses the qualified name
  `<module.Foo object at 0x..>`.
- **`__init__`/method argument-count errors** name the method without the
  class qualifier — e.g. `__init__() missing 1 required positional argument:
  'y'`, where CPython says `Foo.__init__() missing ...`.
- **`type(obj)`** returns the class object (so identity works), but its own
  `repr` is `<class 'Foo'>` with the bare name (CPython qualifies it).
- **`__repr__`/`__str__` cannot suspend**: they are run to completion
  synchronously, so a `__repr__`/`__str__` that calls an external/OS function
  raises rather than yielding to the host. `__init__` and regular methods
  *can* suspend on external/OS calls.
- **Equality and hashing are identity-only**: a user `__eq__`/`__hash__` is
  not dispatched. `a == b` is true only when `a is b`; instances hash by
  identity. Instances are always truthy (no `__bool__`/`__len__` dispatch).
- **Bound methods compare and hash by identity**: each `obj.method` access
  creates a fresh object, so `obj.method == obj.method` is `False` and two
  accesses hash differently. CPython compares/hashes bound methods by
  `(instance, func)`, making separate accesses equal.
- A user `__str__` returning a non-`str` raises `TypeError: __str__ returned
  non-string (type X)` (and likewise for `__repr__`); the wording may differ
  from CPython.
- **Comprehensions in the class body** can see class variables, because Monty
  inlines comprehensions into the enclosing scope. In CPython a comprehension
  has its own scope that skips the class scope, so only the *leftmost iterable*
  is evaluated in class scope and the body cannot see class variables
  (`[n + offset for n in nums]` referencing a class variable `offset` raises
  `NameError` in CPython but succeeds in Monty).
- **Same-name collision is rejected, not resolved.** When an enclosing-function
  local and a class variable share a name *and* a method captures the enclosing
  one, CPython keeps the two distinct (a class-dict entry vs. a closure cell).
  Monty maps one name to a single slot and so cannot represent both; it raises
  `NotImplementedError` at compile time ("class member 'x' that shadows a
  captured variable of the same name from an enclosing scope") rather than
  miscompiling. Distinct names work fine.

## What does NOT exist for user code

- `class Foo(Bar): ...` — no inheritance, no MRO, no `super()` (rejected at
  parse time: "class inheritance and metaclasses").
- Metaclasses, `__init_subclass__`, `__set_name__`, and any other
  metaclass-driven namespace customization.
- `__slots__`, descriptors (`__get__` / `__set__` / `__delete__`).
- Abstract base classes (`abc.ABC`, `@abstractmethod`).
- `@classmethod`, `@staticmethod`, `@property`, and any other class/method
  decorators (rejected at parse time).
- Dunder protocols other than `__init__`, `__repr__`, `__str__`: `__new__`,
  `__call__`, `__iter__`, `__next__`, `__getitem__`, `__setitem__`,
  `__contains__`, `__enter__`, `__exit__`, `__add__`, `__eq__`, `__hash__`,
  `__bool__`, etc. are not dispatched for user-defined instances.
- Class-body statements other than a `def`, a simple `name [: T] = <expr>`
  variable assignment, `pass`, or a docstring — e.g. `if`/`for`/`while` in the
  class body, or tuple/multiple assignment targets (rejected at parse time).
- Assignment expressions (`:=`) in class-variable values or method parameter
  defaults (rejected at parse time). In CPython the walrus target becomes a
  class member (`class C: x = (y := 5)` gives `C.y`); Monty's class-namespace
  assembly only records directly-assigned names, so the syntax is reserved
  rather than silently dropping the binding.
- `del obj.attr` (the `del` statement is unsupported generally).

## `FrozenInstanceError`

Raised when assigning to a field of a frozen host-supplied dataclass.
Subclass of `AttributeError` — `except AttributeError:` catches it, as in
CPython's `dataclasses` module. (User-defined classes in the sandbox are
never frozen.)
