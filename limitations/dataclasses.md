# `dataclasses` module

Native, in-sandbox `@dataclass`: sandboxed code can define its own dataclasses,
executed entirely inside the sandbox. Host-supplied dataclasses are a separate
mechanism, passed in and dispatching back to the host (see
`./classes.md`).

## Unsupported

Only the bare `@dataclass` decorator and `is_dataclass` exist. Everything below
**raises at decoration time** rather than producing a subtly wrong class.

Each raises `NotImplementedError`, marking a feature Monty has not built yet
rather than a mistake in the calling code. CPython accepts all of them, so the
exception type is a divergence in its own right: code catching `TypeError`
around a decoration will not catch these.

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
  raises `NameError`. There is no `MISSING` object, so the `Field` attributes
  whose value would be one raise `NotImplementedError: Field.default is not yet
  supported, dataclasses.MISSING is not implemented` (likewise
  `default_factory`, and `default` only for a field that has none).
  `Field.metadata` and `Field._field_type` raise the same way, for
  `types.MappingProxyType` and `dataclasses._FIELD`.
- **Module helpers** — `fields`, `asdict`, `astuple`, `replace`.

Mutable defaults are rejected as CPython rejects them
(`ValueError: mutable default <class 'list'> for field xs is not allowed: use
default_factory`), and so is a non-default field after a defaulted one
(`TypeError: non-default argument 'b' follows default argument 'a'`).

## Divergences from CPython

- **Annotations are stringized.** Fields come from the class's
  `__annotations__`, which Monty stores as never-evaluated source text (always
  PEP 563); see `./typing.md`. Field
  discovery and the generated methods are unaffected, the field *type* being
  inert metadata, but `C.__dataclass_fields__['x'].type` is the string `'int'`,
  not the `int` type object.
- **`__dataclass_fields__` holds only real fields.** CPython keeps `ClassVar`
  (and `InitVar`) entries in the mapping, marked `_FIELD_CLASSVAR`, and filters
  them in `fields()`. Monty has no field kinds, so the mapping *is* the field
  list and class variables never appear in it.
- **`Field` renders differently.** `repr(field)` follows CPython's layout but
  writes `MISSING` where CPython writes `<dataclasses._MISSING_TYPE object at
  0x..>`, and the stringized `type`. `repr(type(field))` is `<class 'Field'>`,
  not `<class 'dataclasses.Field'>` (`Field.__name__` matches either way, so
  attribute errors read the same).
- **Overwriting `__dataclass_fields__` un-marks the class.** Every dunder reads
  the mapping from the class namespace, so `C.__dataclass_fields__ = 5` makes
  `is_dataclass(C)` false and `C(...)` construct like a plain class. CPython
  keeps its generated methods and still calls `C` a dataclass.
- **`ClassVar` / `InitVar` detection is purely textual.** Monty matches the
  annotation text (bare, dotted, subscripted, or quoted) without checking that
  the name is actually imported, where CPython resolves a *string* annotation
  through the defining module's namespace. So `c: "ClassVar[int]"` without
  `ClassVar` in scope is excluded by Monty but is an ordinary field to CPython.
  Conversely any dotted spelling matches, so a same-named attribute on an
  unrelated module (`mymod.ClassVar`) is treated as `typing.ClassVar`.
- **A field holding a function or bound method reprs differently**, since
  Monty's own `repr` for those differs (see `./classes.md`). Only the
  text differs; the value and its equality match CPython.
- **A class-body `__setattr__` never runs for the synthesized `__init__`**,
  which writes fields straight into the instance `__dict__`. This is the
  never-dispatched attribute hook described in `./classes.md` rather than
  something dataclass-specific, so `@dataclass` does not reject it.
- **`@dataclass` on a non-class** (e.g. `dataclasses.dataclass(5)`) raises
  `TypeError: dataclass() should be called on a class, not '<type>'`. CPython
  instead raises an incidental `AttributeError` about `__module__` from its
  implementation. The `@deco` syntax only ever targets a class, so this affects
  only direct calls.

## Architectural gaps (cannot match)

- **No inheritance**, so field inheritance across base dataclasses is
  unsupported (Monty has no class inheritance at all).
- **`slots=True` / `weakref_slot=True`** — no `__slots__`, no weakrefs.
