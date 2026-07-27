# `dataclasses` module

Native, in-sandbox `@dataclass`: sandboxed code can define its own dataclasses,
executed entirely inside the sandbox (unlike host-supplied dataclasses, which
are passed in and dispatch back to the host — see [classes.md](classes.md)).

## Unsupported

Only the bare `@dataclass` decorator and `is_dataclass` exist. Everything below
**raises at decoration time** rather than producing a subtly wrong class, so a
class body Monty cannot honour never silently misbehaves:

- **The `@dataclass(...)` keyword form** — `frozen`, `eq=False`, `order`,
  `unsafe_hash`, `kw_only` and the hashing/ordering they imply. Any keyword
  raises `TypeError: dataclass() keyword options (eq, order, frozen,
  unsafe_hash, ...) are not yet supported`. A dataclass is therefore always
  `eq=True, frozen=False`: instances are unhashable and unordered.
- **`__post_init__`** — raises `TypeError: dataclass() does not yet support
  __post_init__ in a class body, which would be silently skipped`.
- **`InitVar[...]`** — raises `TypeError: dataclass() does not yet support
  InitVar (field <name>), which would become an ordinary field`. Detected
  textually, since annotations are never evaluated: the name need not be
  imported to be rejected.
- **`field()` / `default_factory` / `MISSING`** — `field(...)` in a class body
  raises `NameError`.
- **Module helpers** — `fields`, `asdict`, `astuple`, `replace`.

Mutable defaults are rejected as CPython rejects them
(`ValueError: mutable default <class 'list'> for field xs is not allowed: use
default_factory`), and so is a non-default field after a defaulted one
(`TypeError: non-default argument 'b' follows default argument 'a'`).

## Divergences from CPython

- **Annotations are stringized.** A dataclass derives its fields from the
  class's `__annotations__`, which Monty stores as **source-text strings**
  (always PEP 563), never evaluated — Monty cannot evaluate parameterized type
  expressions like `list[int]` at runtime. See
  [typing.md](typing.md#class-annotations-are-stringized). This matches CPython
  under `from __future__ import annotations`; field discovery and the generated
  methods are unaffected (the field *type* is inert metadata), but
  `fields(C)[i].type` will be a string, not a type object.
- **`ClassVar` / `InitVar` detection is purely textual.** Monty matches the
  annotation text (bare, dotted, subscripted, or quoted) without checking that
  the name is actually imported, where CPython resolves a *string* annotation
  through the defining module's namespace. So `c: "ClassVar[int]"` without
  `ClassVar` in scope is excluded by Monty but is an ordinary field to CPython.
  Conversely any dotted spelling matches, so a same-named attribute on an
  unrelated module (`mymod.ClassVar`) is treated as `typing.ClassVar`.
- **A mutable default is rejected by hashability, as in CPython**, including an
  instance of a class that sets `__hash__ = None` or defines `__eq__`. The
  divergences in how `__eq__`/`__hash__` themselves behave are in
  [classes.md](classes.md).
- **A function-valued default renders as `<bound method>` in `repr`.** The
  synthesized `__repr__`/`__eq__` read fields as `self.field` does, so a field
  left unset by a class-body `__init__` falls back to the class attribute and a
  function there binds as a method — matching CPython, except that Monty renders
  every bound method as the bare `<bound method>` (see
  [classes.md](classes.md)). Equality is unaffected: each read binds afresh, so
  two such instances are unequal in both.
- **A class-body `__setattr__` never runs for the synthesized `__init__`**,
  which writes fields straight into the instance `__dict__`. This is the
  general never-dispatched attribute hook documented in
  [classes.md](classes.md), not a dataclass-specific gap — a hand-written
  `__init__` on an ordinary class bypasses it identically — so `@dataclass`
  does not reject it.
- **`@dataclass` on a non-class** (e.g. `dataclasses.dataclass(5)`) raises
  `TypeError: dataclass() should be called on a class, not '<type>'`. CPython
  instead raises an incidental `AttributeError` about `__module__` from its
  implementation; Monty reports the misuse directly. (The `@deco` syntax only
  ever targets a class, so this affects only direct calls.)

## Architectural gaps (cannot match)

- **No inheritance**, so field inheritance across base dataclasses is
  unsupported (Monty has no class inheritance at all).
- **`slots=True` / `weakref_slot=True`** — no `__slots__`, no weakrefs.
