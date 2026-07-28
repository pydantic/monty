# `dataclasses` module

Native, in-sandbox `@dataclass`: sandboxed code can define its own dataclasses,
executed entirely inside the sandbox (unlike host-supplied dataclasses, which
are passed in and dispatch back to the host — see [classes.md](classes.md)).

## Unsupported

Only the bare `@dataclass` decorator and `is_dataclass` exist. Everything below
**raises at decoration time** rather than producing a subtly wrong class, so a
class body Monty cannot honour never silently misbehaves.

Each raises `NotImplementedError` — a feature Monty has not built yet, not a
mistake in the calling code. CPython accepts all of them, so the exception type
is a divergence in its own right: code catching `TypeError` around a decoration
will not catch these.

- **The `@dataclass(...)` keyword form** — `frozen`, `eq=False`, `order`,
  `unsafe_hash`, `kw_only` and the hashing/ordering they imply. Any keyword
  raises `NotImplementedError: dataclass() keyword options (eq, order, frozen,
  unsafe_hash, ...) are not yet supported`. A dataclass is therefore always
  `eq=True, frozen=False`: instances are unhashable and unordered.
- **`__post_init__`** — raises `NotImplementedError: dataclass() does not yet
  support __post_init__ in a class body, which would be silently skipped`.
- **`InitVar[...]`** — raises `NotImplementedError: dataclass() does not yet
  support InitVar (field <name>), which would become an ordinary field`.
  Detected textually, since annotations are never evaluated: the name need not
  be imported to be rejected.
- **`field()` / `default_factory` / `MISSING`** — `field(...)` in a class body
  raises `NameError`.
- **Module helpers** — `fields`, `asdict`, `astuple`, `replace`.

Mutable defaults are rejected as CPython rejects them
(`ValueError: mutable default <class 'list'> for field xs is not allowed: use
default_factory`), and so is a non-default field after a defaulted one
(`TypeError: non-default argument 'b' follows default argument 'a'`).

## Divergences from CPython

- **Annotations are stringized.** Fields come from the class's
  `__annotations__`, which Monty stores as never-evaluated source text (always
  PEP 563) — see [typing.md](typing.md#class-annotations-are-stringized). Field
  discovery and the generated methods are unaffected, the field *type* being
  inert metadata, but `fields(C)[i].type` would be a string, not a type object.
- **`ClassVar` / `InitVar` detection is purely textual.** Monty matches the
  annotation text (bare, dotted, subscripted, or quoted) without checking that
  the name is actually imported, where CPython resolves a *string* annotation
  through the defining module's namespace. So `c: "ClassVar[int]"` without
  `ClassVar` in scope is excluded by Monty but is an ordinary field to CPython.
  Conversely any dotted spelling matches, so a same-named attribute on an
  unrelated module (`mymod.ClassVar`) is treated as `typing.ClassVar`.
- **A field holding a function or bound method reprs differently**, since
  Monty's own `repr` for those differs (see [classes.md](classes.md)). Only the
  text differs; the value and its equality match CPython.
- **A class-body `__setattr__` never runs for the synthesized `__init__`**,
  which writes fields straight into the instance `__dict__`. Not a
  dataclass-specific gap — the never-dispatched attribute hook of
  [classes.md](classes.md) — so `@dataclass` does not reject it.
- **`@dataclass` on a non-class** (e.g. `dataclasses.dataclass(5)`) raises
  `TypeError: dataclass() should be called on a class, not '<type>'`. CPython
  instead raises an incidental `AttributeError` about `__module__` from its
  implementation; Monty reports the misuse directly. (The `@deco` syntax only
  ever targets a class, so this affects only direct calls.)

## Architectural gaps (cannot match)

- **No inheritance**, so field inheritance across base dataclasses is
  unsupported (Monty has no class inheritance at all).
- **`slots=True` / `weakref_slot=True`** — no `__slots__`, no weakrefs.
