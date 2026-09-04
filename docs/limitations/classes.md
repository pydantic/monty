# Classes

Sandboxed Python code in Monty can define simple classes. A `class`
statement with instance methods, `__init__`, `__eq__`, `__repr__`/`__str__`,
and class variables works. The class body has a real scope (like CPython's
class-body code object), so class variables may be arbitrary expressions
and may reference earlier class variables:

```python test="skip"
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

The host can also send its own class instances in (wrapped in a
`ClassInstance` policy wrapper) and namedtuple values; those are a separate
mechanism whose method calls and lazy attribute lookups dispatch back to the
host, routed by the wrapper's uuid (see `test_cases/dataclass__basic.py`
and "Host class instances" below).

## Supported surface

Listed to bound what the divergences below apply to. Working,
CPython-matching features: instance methods, `__init__` (full parameter
shapes), instance and class attribute get/set (including `setattr(Foo, ...)`
and function-attributes-become-methods), bound methods, class variables
(arbitrary expressions, evaluated in a real suspendable class-body scope),
**class decorators** (`@deco class Foo`),
`__repr__`/`__str__`/`__enter__`/`__exit__`/`__eq__`/`__hash__` dispatch,
`obj.__class__`, `Foo.__name__`, `Foo.__doc__`/`obj.__doc__`,
`Foo.__annotations__` (ordered; values stringized and provisional, see
[typing.md](typing.md)), `type(obj)`/`isinstance(obj, Foo)`, and the 3-arg
`type()` constructor. The `__enter__`/`__exit__` divergences are in
[with.md](with.md).

## Dynamic class creation — `type(name, bases, dict)`

The 3-arg `type()` form creates classes at runtime with CPython's validation
order and error wording, but with these divergences:

- **`bases` must be the empty tuple `()`.** Any non-empty bases tuple, even
  `(object,)`, raises `TypeError: type() bases are not supported`, the
  runtime counterpart of the parse-time `class Foo(Bar)` rejection.
- **Keywords are always rejected.** CPython forwards extra keywords to
  `__init_subclass__`; Monty has no `__init_subclass__`, but the error
  message matches what `object.__init_subclass__` produces
  (`A.__init_subclass__() takes no keyword arguments`).
- Only `__doc__` is synthesized into the namespace when absent (as `None`,
  matching CPython). CPython also sets `__module__`, `__qualname__`,
  `__dict__`, `__weakref__`, etc.; those attributes raise `AttributeError`
  in Monty, as for compiled classes.
- **Non-string namespace keys raise `TypeError`**
  (`non-string key (int) in the namespace of class 'A'`). CPython accepts
  them with only a `RuntimeWarning`; Monty has no warnings machinery, so it
  raises rather than silently accepting.

## Divergences from CPython

- **Default `repr`** (no user `__repr__`) is `<Foo object at 0x..>` using the
  **bare** class name, where CPython uses the qualified name
  `<module.Foo object at 0x..>`.
- **`__init__`/method argument-count errors** name the method without the
  class qualifier, e.g. `__init__() missing 1 required positional argument:
  'y'`, where CPython says `Foo.__init__() missing ...`.
- **`type(obj)`** returns the class object (so identity works), but its own
  `repr` is `<class 'Foo'>` with the bare name; CPython qualifies it.
- **The class object is not itself a `type` instance.** The bare name `type`
  resolves to the builtin `type` *function*, not a type object, so
  `type(Foo) is type` is `False` (CPython: `True`) and `isinstance(Foo, type)`
  raises `TypeError: isinstance() arg 2 must be a type, a tuple of types, or a
  union` (CPython: `True`). There is no metaclass.
- **Bound methods report `function`, not `method`.** `type(obj.method)` is
  `<class 'function'>` where CPython says `<class 'method'>`; Monty has no
  dedicated `method` type.
- **Ordering comparisons on instances raise, but a user `__lt__`/`__gt__`/… is
  not dispatched.** `a < b` on instances of a class with no comparison dunders
  raises `TypeError: '<' not supported between instances of 'Foo' and 'Foo'`
  (matching CPython). A class that *defines* `__lt__` etc. still raises: those
  dunders are not dispatched (see the not-dispatched dunder list below).
- **`__repr__`/`__str__` cannot suspend**: they are run to completion
  synchronously, so a `__repr__`/`__str__` that calls an external/OS function
  raises rather than yielding to the host. `__init__` and regular methods
  *can* suspend on external/OS calls.
- **Only a plain-function `__init__` can suspend.** When `__init__` is bound to
  something else (a builtin, another class, a bound method, ...), it is called
  with CPython's descriptor-binding semantics (no `self` prepended unless it is
  a plain function) and CPython's `None`-return contract is enforced, but it
  runs to completion synchronously, so it cannot yield to the host, and an
  external-function `__init__` raises `NotImplementedError` rather than
  suspending.
- **`__eq__`/`__hash__`/`__index__` cannot suspend**: like `__repr__`/`__str__`
  they run to completion synchronously, so one that calls an external/OS
  function raises rather than yielding to the host. An exception raised by
  `__eq__` terminates the run instead of being catchable by a `try` around the
  comparison.
- **Ordering dunders are still not dispatched**; see the entry above.
  Instances are always truthy (no `__bool__`/`__len__` dispatch).
- **Bound methods compare and hash by identity**: each `obj.method` access
  creates a fresh object, so `obj.method == obj.method` is `False` and two
  accesses hash differently. CPython compares/hashes bound methods by
  `(instance, func)`, making separate accesses equal.
- **Bound-method `repr`** is the bare `<bound method>`; CPython renders
  `<bound method Foo.m of <__main__.Foo object at 0x..>>`.
- **Assigning `Foo.__name__`** stores an ordinary class member. Unlike CPython,
  where `type.__name__` is a metaclass descriptor whose setter renames the
  class, it does not rename the class, so `Foo.__name__` reads and `repr(Foo)`
  keep the original name while instances see the member.
- **Assigning `obj.__class__`** stores an ordinary instance attribute rather
  than reassigning the object's class. `obj.__class__ = X` then reads back `X`,
  but `type(obj)` and `isinstance` still report the original class, leaving an
  internally inconsistent object. CPython either reassigns the class (for a
  compatible class) or raises `TypeError: __class__ must be set to a class, not
  '...' object`.
- **Recursive/deep `__repr__`/`__str__` raises `RecursionError` earlier than
  CPython.** A `__repr__` (or `__str__`) that reprs `self`, or a deep chain of
  instances whose reprs nest (e.g. a long linked list), re-enters the
  interpreter on the native Rust call stack once per nesting level, unlike
  ordinary Python-level recursion, which lives on a heap-allocated frame stack
  and is bounded at 1000 by the normal recursion limit. A native stack overflow
  would abort the process, which is fatal for the in-process/wasm API sharing
  the host process, so this native re-entry is capped independently at a much
  lower, fixed depth, raising a catchable `RecursionError` once exceeded. So
  infinite `__repr__` recursion raises `RecursionError` (matching CPython's
  outcome, though not its exact depth), but a deep-but-finite chain that
  CPython's default 1000-frame limit would still render may raise
  `RecursionError` in Monty. The same cap applies to synchronous callback
  evaluation such as `map()`, `filter()`, `sorted()`/`list.sort(key=...)`,
  `min()`/`max(key=...)`, and exotic `__init__` recursion (see the "Recursion"
  section of [resource_limits.md](resource_limits.md)).
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

## Crossing the host boundary (`pydantic_monty` / `@pydantic/monty`)

A sandbox-defined class **instance** crosses out structurally: the host
receives a read-only `MontyClassProxy` with `.name`, `.is_dataclass`, `.id`
and `.attributes` (the instance `__dict__`, converted; the JS package spells
these `.name` / `.isDataclass` / `.id` / `.attributes`). The host cannot call
methods on it — the methods are defined only inside the sandbox, and the proxy holds
no live object. The instance and its class carry worker-generated uuids (stored
on the heap objects, so stable across crossings and dump/restore). Passing the
proxy back into the sandbox (as an input or an external-function result) hands
over the **original object** — `back is foo` and `isinstance(back, Foo)` hold —
with these divergences:

- The proxy's `attributes` are not applied on the way back: editing them
  host-side does not change the sandbox object (only a still-live sandbox
  object is resolved, and it keeps its own state).
- A proxy whose sandbox object has since been freed raises
  `RuntimeError: invalid input type: sandbox instance of 'Foo' (id ...) no
  longer exists` rather than materializing a host-backed copy.
- A proxy of a **host-sent** instance (produced after a restore) has no
  sandbox object to resolve to: passing it back re-enters as a host-backed
  copy built from its `attributes`, not the host's original object.

```python test="skip"
from pydantic_monty import Monty

with Monty() as pool, pool.checkout() as session:
    result = session.feed_run(
        'class A:\n    def __init__(self):\n        self.x = 1\nA()'
    )
    # result is MontyClassProxy(name='A', attributes={'x': 1})
```

A sandbox-defined class **object** (`A` itself) still has no structural host
representation and converts to its type text (e.g. `"<class 'A'>"`). A user
`__repr__` is NOT consulted when an instance crosses the boundary — the host
gets the structured proxy, not the repr string.

## Host class instances (`ClassInstance` wrapper)

Host objects enter the sandbox only when explicitly wrapped in the host
package's `ClassInstance` policy wrapper (passing a bare dataclass or class
instance as an input raises `MontyConversionError` in Python, `TypeError` in
JS). Inside the sandbox they are proxies
whose eager attrs were copied at send time; everything else routes back to the
host by the wrapper's `id` uuid (never `id()` or any other address-derived
value). Divergences from real CPython objects:

- **`type(x)` returns a lightweight stand-in for the real class**, since the
  class itself stays on the host. The sandbox keeps one such object per host
  class id: `type(a) is type(b)` holds for instances of one class,
  `x.__class__` returns it, and a `ClassType` passed as a value with the same
  id resolves to it too (`type(p) is Point`); equality and hashing go by
  class id alone. It names the real class (`type(x).__name__` is `'Point'`,
  repr is `<class 'Point'>` — without CPython's module qualification like
  `<class 'mymod.Point'>`), and error messages name the real class too
  (`unhashable type: 'Point'`, `'Point' object is not subscriptable`) — always
  bare, so where CPython's message is module-qualified (`'mymod.Point' object
  does not support the context manager protocol (missed __exit__ method)`)
  Monty says `'Point'`. But it is not the class: calling it suspends
  a `__call__` request to the host, which only succeeds when the host granted
  `init` on a `ClassType` wrapper (see below); and — like Monty class objects
  generally — it exposes `__name__` plus any eager class attrs the host sent
  (`__module__`, `__qualname__`, `__doc__`, `__mro__`, `__bases__`, ... raise
  `AttributeError`, and so does `__class__`, which CPython answers with
  `type`). Returned to the host, it resolves back to the real class object
  when the class is registered in the session.
- **`isinstance(x, Point)` matches by exact class id only**: the host never
  sends bases, so an instance of a subclass is not an instance of `Point` in
  the sandbox, and `issubclass` does not exist.
- **`repr()` shows all eager attrs in order** (`Point(x=1, y=2)`). After
  sandbox code sets a new attribute, that attribute appears in the repr too —
  CPython's dataclass repr shows declared fields only.
- **Lazy attribute lookups consult the host for `obj.attr`, `getattr()` and
  `hasattr()`** — but not from inside a synchronous nested call the
  interpreter makes itself (a `__repr__`, `__eq__` or sort key invoked from
  Rust): there the lookup cannot suspend, so the attribute reads as absent
  (`hasattr` → `False`, `getattr` raises/returns the default). Underscore-
  prefixed names never consult the host (dunder probes stay local).
- **Lazy attribute reads run host code** (a `@property`, a JS getter, the
  wrapper's `convert_value`), and only a host `AttributeError` reads as
  "absent". Any other host exception is raised inside the sandbox where the
  read happened, and `hasattr()` / `getattr(obj, name, default)` do not
  swallow it (CPython treats a raising property the same way). A value the
  wire cannot carry raises `TypeError: Cannot convert X to Monty value ...`
  in the sandbox, where CPython would return it.
- **Lazy lookups are not cached**: every access is a fresh host round trip,
  and host-side mutations between accesses are visible. Eager attrs are a
  snapshot — host-side mutations after send are NOT visible, and sandbox
  `setattr` does not affect the host object.
- **`allowed_methods='all'` exposes only functions defined on the class**
  (its MRO is searched): a nested class, a callable stored as an attribute,
  or any other non-function class attribute raises `AttributeError` when
  called, where CPython would call it. An explicit set of names calls
  whatever `getattr` returns. In JS, `'all'` requires a function found on a
  prototype below `Object.prototype` / `Function.prototype`, so `toString()`,
  `hasOwnProperty()`, `call()`, `bind()` and the like are absent; and
  `constructor`, `__proto__`, `prototype`, `arguments` and `caller` are
  refused under every policy, an explicit list included.
- **A method read as a value is not a bound method**: with the name only in
  `allowed_methods`, `m = x.greeting` raises `AttributeError` and
  `hasattr(x, 'greeting')` is `False` — only the call `x.greeting(...)`
  reaches the host. If the name is also in `lazy_attrs`, the read crosses as
  a host function proxy that is resolved by name through `external_lookup`
  when called, not bound to the instance.
- **Equality uses the eager attrs only** (same class + equal attrs);
  methods like a custom `__eq__` are not consulted. **Host instances are
  always unhashable** — matching CPython's rule for a class defining
  `__eq__` without `__hash__` — so a frozen dataclass that hashes in
  CPython raises `TypeError: unhashable type: '...'` in the sandbox.
- **Frozen dataclasses are not frozen in the sandbox**: there is no frozen
  policy on the wire, so in-sandbox `setattr` succeeds on the sandbox copy
  of any host instance (the host object is never touched).
- **`dataclasses.fields()` / `asdict()` do not work on host instances**;
  `dataclasses.is_dataclass(x)` returns the flag the host sent.
- Returning a host-sent instance gives the host the **original object**
  (identity preserved), discarding any sandbox-side attr mutations. Sending
  the same object twice yields equal (same class uuid + attrs) sandbox
  values, but each send allocates its own proxy, so `a is b` is `False`.
- **Instance ids are per wrapper; class ids are per process** (host
  classes) — Python keys class ids by `module.qualname` in
  `pydantic_monty.class_instance.type_id_cache`, JS by class object — so
  instances of the same host class compare equal by type across sessions in
  one process. In a fresh process the ids differ unless pinned explicitly
  (`ClassType(..., id=...)`, or pre-seeding the cache) — required when
  restoring a dump there. Because the Python key is the name, two distinct
  class objects sharing a `module.qualname` (a class redefined in a notebook
  cell, or built by a factory function) get the same default id, and sending
  both into one session raises `ValueError` rather than silently aliasing
  them — give one an explicit `id`. JS validates an explicit `id` as a
  canonical uuid and stores it lowercased, so `wrapper.id` may differ in case
  from the value passed. Sandbox-defined uuids live in the heap and survive
  dump/restore.
- **Inheritance is not modelled**: a host class's bases are not sent, so
  base-class attributes and methods are not consulted, and `__bases__`
  raises `AttributeError`.
- **The host keeps every wrapper it sends until the session ends**: each
  wrapper sent (nested ones included), each `init=True` construction and each
  `convert_value` wrap adds an entry to the host-side instance store that
  `max_memory` does not count; re-sending a wrapper with the same id
  overwrites its entry rather than adding one. See the class-instance store
  note in [pool-architecture.md](pool-architecture.md).

## Host classes (`ClassType` wrapper)

A host may pass a bare *class* into the sandbox with the `ClassType` policy
wrapper — `ClassInstance`'s sibling, applied to the class object itself:
`eager_attrs` sends class constants with the type, `lazy_attrs` serves them
on demand, and `allowed_methods` exposes classmethods/staticmethods (calls
and lazy lookups route to the host by the class uuid, exactly like instance
routing). With `init=True` (`pydantic_monty.ClassType(Point, init=True)`; JS
`new ClassType(Point, { init: true })`) sandbox code can also call the
class; the construction crosses as a `__call__` method call, runs
**host-side**, and the constructed instance crosses back wrapped with the
wrapper's `instance_*` policies (`instance_eager_attrs`, `instance_lazy_attrs`,
`instance_allowed_methods`; JS `instanceEagerAttrs`, ...). `init` is purely
host-side policy — it never crosses the wire, and the wrapper checks it on
every construction request. Divergences:

- Missing/denied class attributes raise CPython's type-object wording
  (`AttributeError: type object 'Point' has no attribute 'x'`). Like
  instance attrs, only `Type.attr` syntax consults the host, underscore
  names stay local, and lazy class lookups are not cached.
- **`allowed_methods` on a `ClassType` exposes classmethods and
  staticmethods only**, under `'all'` and an explicit set alike: calling an
  instance method through the class (`Person.greet(other)`) raises
  `AttributeError: type object 'Person' has no attribute 'greet'`, where
  CPython would pass `other` as `self`. In JS, `'all'` exposes the class's
  own static functions, none inherited from a base class.
- With `init` absent or false, calling the class raises
  `TypeError: cannot instantiate host class 'Point'` (CPython would
  construct). That includes `type(x)()` on a plain `ClassInstance`: sending
  an instance registers a default `ClassType` for its class, with `init`
  false.
- Constructor exceptions propagate into the sandbox like external-function
  errors.
- After a session restore the class registration is gone: construction and
  classmethod calls raise `RuntimeError`, lazy class attrs raise
  `AttributeError`, and a host class crossing back to the host (as a value,
  or as `type(x)`) is a read-only `MontyClassTypeProxy` (`name`, `id`,
  `is_dataclass`, `attributes`) rather than the original class; passing the
  proxy back in re-enters as the same sandbox type object. In JS an
  unregistered class comes back as a plain `{__monty_type__: 'Type', ...}`
  marker; a registered one resolves to the class object.
- JS constructors have no keyword arguments; kwargs arrive as a trailing
  options-bag argument, as with wrapped method calls, and a `__proto__`
  keyword is dropped from that bag.
- Eager class attrs are a snapshot at send time, and (like eager instance
  attrs) host-side mutations after send are not visible. They are re-sent on
  every crossing of the class and of each of its instances (the cost scales
  with the number of eager class attrs): a non-empty set replaces the
  sandbox copy, an empty set leaves it alone, so re-sending cannot clear it;
  a re-send also overwrites the class name and `is_dataclass` flag.
- Dumps written before the shared-type-object layout (dump format version 8)
  are rejected on load.

## What does NOT exist for user code

- `class Foo(Bar): ...` — no inheritance, no MRO, no `super()` (rejected at
  parse time: "class inheritance and metaclasses"; the runtime equivalent
  `type('Foo', (Bar,), {})` raises `TypeError`, see above).
- Metaclasses, `__init_subclass__`, `__set_name__`, and any other
  metaclass-driven namespace customization.
- `__slots__`, descriptors (`__get__` / `__set__` / `__delete__`).
- Abstract base classes (`abc.ABC`, `@abstractmethod`).
- Method decorators — `@classmethod`, `@staticmethod`, `@property`, and any
  decorator on a `def` inside a class body (rejected at parse time). Decorators
  on classes and on non-method functions are supported.
- **Classes are barely introspectable**: `__dict__`, `__bases__` and `dir()`
  are all unavailable (`cls.__name__` and `cls.__annotations__` work, the
  latter with stringized values, see [typing.md](typing.md)). A class decorator
  can therefore discover fields and nothing else.
- **Tracebacks from decorator application point at the whole `class` statement**
  (a span from the first decorator through the body, with the body elided as
  `...<N lines>...`), where CPython pins the individual decorator that raised.
  Every decorator in a stack reports that same location; only the callee frame
  identifies which one raised.
- Dunder protocols other than `__init__`, `__repr__`, `__str__`,
  `__enter__`, `__exit__`, `__iter__`, `__next__`, `__contains__`, `__eq__`,
  `__hash__`, and `__index__`: `__new__`, `__call__`, `__getitem__`,
  `__setitem__`, `__add__`, `__ne__`, `__bool__`, etc. are not dispatched for
  user-defined instances. `__ne__` is always the negation of `__eq__`, as
  CPython derives it by default, so a custom `__ne__` is ignored.
- **`__index__` is dispatched for indexing, but not for arithmetic
  operators.** A class defining it works as a subscript *read* (`seq[obj]`), as
  a slice bound (`seq[obj:]`, `slice(obj)`), and as an integer argument
  (`range(obj)`, `'x'.center(obj)`, `s.find(sub, obj)`). It is **not**
  consulted by sequence repetition, so `'ab' * obj` and `[0] * obj` raise
  `TypeError: unsupported operand type(s) for *` where CPython repeats — each
  numeric operator carries its own coercion, which does not route through the
  shared index path.
- **Subscript assignment does not dispatch `__index__`.** `lst[obj] = x` raises
  `TypeError: list indices must be integers or slices, not Foo` where CPython
  coerces and assigns; only the read side takes the index path.
- **`slice()` stores coerced bounds, not the objects passed.** CPython's
  `slice()` keeps its arguments untouched and only calls `__index__` when the
  slice is *used*, so `slice(obj).start` is `obj`; Monty coerces during
  construction, so it is the resulting `int`. A bound whose `__index__` raises
  therefore raises at `slice(...)` rather than at use, and one that is neither
  `None`, an `int`, nor `__index__`-able is rejected up front instead of on
  first use.
- **Slice bounds are stored saturated to `i64`.** Because bounds are coerced at
  construction (above), one beyond `i64` is clamped to `i64::MIN`/`i64::MAX`
  rather than kept exact: `slice(10**30).stop` is `9223372036854775807`, where
  CPython reports `10**30`. *Slicing* with such a bound still matches CPython —
  it clamps to the sequence either way, so `[1, 2, 3][10**30:]` is `[]` — the
  divergence is only visible by reading the attribute back. This applies to
  bounds written as literals and to those returned by `__index__` alike. Plain
  indexing is unaffected: `[1, 2, 3][10**30]` raises `IndexError` as CPython
  does.
- `__iter__` / `__next__` / `__contains__` **are** dispatched, but like
  `__repr__`/`__str__` they run synchronously, so one that calls an external or
  OS function cannot suspend and raises `NotImplementedError`. The error is
  raised at the offending call inside the callee, so a `try`/`except` there
  catches it like any other exception. Two related protocols are still not
  dispatched, so a class relying on either is not
  iterable:
  - the legacy `__getitem__`-only fallback: CPython iterates a class defining
    `__getitem__` but not `__iter__` from index 0 until `IndexError`, while
    Monty reports it as not iterable. (`monty -t` accepts `iter(obj)` for
    such a class, so this fails only at runtime, see [iter.md](iter.md).)
  - `__reversed__`, so `reversed(obj)` on any user instance raises
    `TypeError: '{cls}' object is not reversible`. That matches CPython for a
    class defining neither `__reversed__` nor `__len__` + `__getitem__`, and
    diverges for one that does.
- `__next__` is looked up on the class only, never the instance `__dict__`, and
  a `StopIteration` raised anywhere inside it ends the iteration, including one
  that propagates out of a nested call, where CPython's PEP 479 protections
  apply only to generators, which Monty does not have.
- **A `__contains__` returning a user instance is always `True`.** The result is
  coerced by Monty's truthiness, which reports every instance as truthy (see
  above), where CPython's `PyObject_IsTrue` consults the returned object's
  `__bool__`/`__len__`. Every other return type coerces as CPython does.
- Attribute-access hooks are **never** dispatched: `__getattr__`,
  `__getattribute__`, `__setattr__`, `__delattr__`, and `__del__`. A missing
  attribute always raises the default `AttributeError` even when the class
  defines `__getattr__`, and attribute writes always go straight to the
  instance `__dict__`. `object.__setattr__` exists (see below) and, since
  there are no hooks to skip, differs from a plain `obj.x = v` only on a
  `@dataclass(frozen=True)` instance, which it writes to and `obj.x = v`
  refuses — the same escape hatch CPython's generated `__init__` uses. On a
  class object it does not write at all, where `Foo.x = v` sets a class member.
- Introspection attributes other than `__name__`, `__doc__`, `__annotations__`
  and `obj.__class__`: `Foo.__dict__`, `obj.__dict__`, `Foo.__bases__`,
  `Foo.__mro__`, `Foo.__qualname__`, `Foo.__module__`, and explicit
  `obj.__repr__()` / `obj.__str__()` calls when the class defines none, all
  raise `AttributeError`.
- Class-body statements other than a `def`, a simple `name [: T] = <expr>`
  variable assignment, `pass`, `...`, or a docstring, e.g. `if`/`for`/`while`
  in the class body, or tuple/multiple assignment targets (rejected at parse
  time).
- Assignment expressions (`:=`) that bind in the class-body scope: in
  class-variable values, method parameter defaults, and lambda parameter
  defaults (rejected at parse time). In CPython the walrus target becomes a
  class member (`class C: x = (y := 5)` gives `C.y`); Monty's class-namespace
  assembly only records directly-assigned names, so the syntax is reserved
  rather than silently dropping the binding. A walrus inside a lambda *body*
  (`f = lambda: (z := 1)`) binds in the lambda's own scope and works. A walrus
  in a comprehension in the class body is also rejected (CPython rejects that
  too, but as a `SyntaxError` with different wording). A walrus in an
  *annotation* (`x: (y := int) = 5`) runs in Monty, since annotation
  expressions are captured as source text (stringized) and never evaluated, so
  the walrus never binds; CPython raises `SyntaxError`. This follows from
  annotations never being evaluated, so it would change if they ever are (see
  [typing.md](typing.md)).
- `del obj.attr` (the `del` statement is unsupported generally).

## `object`

The name resolves, but it is a carrier for `object.__setattr__` rather than a
type: Monty has no inheritance, so there is no base class for it to be.
`isinstance(x, object)` is `True` for every value, as in CPython.

- **`object()` cannot be constructed** — raises `TypeError: cannot create
  'object' instances`, where CPython returns a featureless instance.
- **`class Foo(object):` is still rejected**, like any base list (see above),
  so the idiom carries no more weight than `class Foo:`.
- **Only `__setattr__` and `__name__` resolve.** Every other member CPython's
  `object` carries — `__doc__`, `__init__`, `__eq__`, `__getattribute__`,
  `__class__`, `__mro__`, `__bases__`, `__qualname__`, `__module__`,
  `__dict__` — raises `AttributeError`, with Monty's generic `'type' object
  has no attribute 'x'` where CPython says `type object 'object' has no
  attribute 'x'`.
- **`object.__setattr__` accepts only instances of sandbox-defined classes.**
  Anything else raises CPython's `AttributeError: '<type>' object has no
  attribute '<name>' and no __dict__ for setting new attributes` — including
  a class object, where CPython instead raises `TypeError: can't apply this
  __setattr__ to type object`.
- **It reprs as `<built-in function object.__setattr__>`**, where CPython says
  `<slot wrapper '__setattr__' of 'object' objects>`.

## `FrozenInstanceError`

Raised when assigning to a field of a dataclass declared in the sandbox with
`@dataclass(frozen=True)` (see [dataclasses.md](dataclasses.md)); host-supplied instances
are never frozen in the sandbox (see "Host class instances" above). Subclass
of `AttributeError`, so `except AttributeError:` catches it, as in CPython's
`dataclasses` module. A plain `class` is never frozen, and
`object.__setattr__` writes past the check either way.
