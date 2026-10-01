# `dataclasses` module

Native, in-sandbox `@dataclass`: sandboxed code can define its own dataclasses,
executed entirely inside the sandbox (unlike host-supplied class instances,
which enter via the `ClassInstance` wrapper and dispatch back to the host —
see [classes.md](classes.md)).

Host-supplied instances and this module barely interact:
`dataclasses.is_dataclass(x)` honours the flag the host sent, but `fields()`
and `asdict()` do not work on host instances (they are not native
dataclasses). Bare dataclasses are NOT accepted as inputs — the host must
wrap them in `ClassInstance` explicitly.

## Unsupported

`@dataclass`, `@dataclass(...)` with `eq` and/or `frozen`, `field()` with
`default`/`default_factory`, `MISSING`, `__post_init__` and `is_dataclass`
exist. Each unsupported feature listed below **raises where it is written** —
at the decoration, or at the `field()` call — rather than producing a subtly
wrong class, so a class body Monty cannot honour never silently misbehaves.

Each raises `NotImplementedError`, marking a feature Monty has not built yet
rather than a mistake in the calling code. CPython accepts all of them, so the
exception type is a divergence in its own right: code catching `TypeError`
around a decoration will not catch these.

- **Every `@dataclass(...)` option except `eq` and `frozen`** — `init`, `repr`,
    `order`, `unsafe_hash`, `match_args`, `kw_only`, `slots` and `weakref_slot`.
    Setting one away from its CPython default raises
    `NotImplementedError: dataclass() does not yet support the <name> option`;
    each is named individually rather than reported as an unknown keyword.
    Ordering dunders therefore do not exist, and hashing is whatever `eq`/`frozen`
    imply.
- **`InitVar[...]`** — raises
    `NotImplementedError: dataclass() does not yet support InitVar (field <name>), which would become an ordinary field`.
    Detected textually, since annotations are never evaluated: the name need not
    be imported to be rejected.
- **Every `field(...)` argument except `default` and `default_factory`** —
    `init`, `repr`, `hash`, `compare`, `metadata`, `kw_only` and `doc`. Setting one
    away from its CPython default raises
    `NotImplementedError: field() does not yet support the <name> argument`, at the `field()` call rather than at
    decoration. Nothing consults the three flags when the dunders are
    synthesized, so `init=False` would otherwise leave the field in `__init__`
    regardless, and Monty stores no per-field docstring for `doc` to fill. They
    therefore always read back as CPython's defaults (`f.init is True`,
    `f.kw_only is False`, `f.doc is None`).
- **`Field.metadata` and `Field._field_type`** — raise
    `NotImplementedError: Field.metadata is not yet supported, types.MappingProxyType is not implemented` (and likewise
    `dataclasses._FIELD`), the objects behind them being unimplemented.
- **Module helpers** — `fields`, `asdict`, `astuple` and `replace` do not exist: accessing them raises
    `AttributeError`, not `NotImplementedError`, since the module has no such attribute.

Mutable defaults are rejected as CPython rejects them
(`ValueError: mutable default <class 'list'> for field xs is not allowed: use default_factory`), and so is a non-default
field after a defaulted one
(`TypeError: non-default argument 'b' follows default argument 'a'`).

## Divergences from CPython

- **Annotations are stringized.** Fields come from the class's
    `__annotations__`, which Monty stores as never-evaluated source text (always
    PEP 563); see [typing.md](typing.md). Field
    discovery and the generated methods are unaffected, the field *type* being
    inert metadata, but `C.__dataclass_fields__['x'].type` is the string `'int'`,
    not the `int` type object.
- **`__dataclass_fields__` holds only real fields.** CPython keeps `ClassVar`
    (and `InitVar`) entries in the mapping, marked `_FIELD_CLASSVAR`, and filters
    them in `fields()`. Monty has no field kinds, so the mapping *is* the field
    list and class variables never appear in it.
- **`Field` renders differently.** `repr(field)` follows CPython's layout but
    writes the stringized `type`.
    `repr(type(field))` is `<class 'Field'>`,
    not `<class 'dataclasses.Field'>` (`Field.__name__` matches either way, so
    attribute errors read the same).
- **Error messages name `MISSING`'s type with its module.** `MISSING | int` and
    `MISSING.foo` say `'dataclasses._MISSING_TYPE'` where CPython says
    `'_MISSING_TYPE'`. `type(MISSING)` and `__name__` match.
- **`type(MISSING)()` raises** `TypeError: cannot create 'dataclasses._MISSING_TYPE' instances`, where CPython builds
    a second, distinct object.
- **`default_factory` and `__post_init__` cannot suspend.** Both run in a
    synchronous position the interpreter cannot preserve and resume, so calling an
    external function, an `os` function, or awaiting inside one raises
    `NotImplementedError: dataclass field default_factory: external function 'f' is not yet supported in this context`
    (and the `__post_init__` equivalent). Ordinary in-sandbox code in them runs normally.
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
    which writes fields straight into the instance `__dict__`. This is the
    never-dispatched attribute hook described in [classes.md](classes.md) rather than
    something dataclass-specific, so `@dataclass` does not reject it.
- **`@dataclass` on a non-class** (e.g. `dataclasses.dataclass(5)`) raises
    `TypeError: dataclass() should be called on a class, not '<type>'`. CPython
    instead raises an incidental `AttributeError` about `__module__` from its
    implementation. The `@deco` syntax only ever targets a class, so this affects
    only direct calls.
- **`dataclass(...)` returns a native callable, not a Python function.** CPython
    builds a closure, which Monty cannot: a native function has nowhere to keep
    the bound options but its own value. Applying it to a class is identical, and
    it reprs as `<function dataclass at 0x..>`, but `type()` says
    `builtin_function_or_method` where CPython says `function`, and CPython's repr
    names the closure (`dataclass.<locals>.wrap`). Having nowhere to live but the
    value, the options *are* the value: `dataclass(frozen=True) is dataclass(frozen=True)` is `True`, where each CPython
    call builds a fresh
    closure. Fixable only if Monty gains closures over native functions; nothing
    else depends on that, so it is not planned.
- **`del obj.field` on a frozen instance never raises `cannot delete field`**,
    because Monty's parser has no `del` statement at all. (Assignment matches
    CPython, message included, and `dataclasses.FrozenInstanceError` is
    importable.)
- **`__dataclass_params__` reads back normalised.** `C.__dataclass_params__`
    exists, reprs like CPython's and answers all ten flags, but each is the `bool`
    Monty acted on: `@dataclass(frozen=1)` reports `frozen=True` where CPython
    echoes the `1` you passed.
- **The class's metadata is read at use time, not built in at decoration.**
    CPython's `@dataclass` generates `__init__`, `__eq__`, `__hash__`, `__repr__`
    and `__setattr__` with the decoration's choices built in, and leaves
    `__dataclass_fields__` and `__dataclass_params__` behind as records nothing
    reads again. Monty generates no methods. It acts on those two namespace entries
    and on `__post_init__` each time an instance is built, compared, hashed,
    printed or assigned to. Changing any of them after decoration therefore changes
    the class in Monty and nothing in CPython:
    - **Rebinding `__dataclass_params__`** switches the `eq` and `frozen` in force.
        `C.__dataclass_params__ = Frozen.__dataclass_params__` freezes `C` and makes
        it hashable; borrowing an `eq=False` class's params makes `C(1) == C(1)`
        false. Binding anything that is not a params object (`None`) puts `C` on
        the defaults, `eq=True, frozen=False`, unfreezing a frozen class.
    - **Overwriting `__dataclass_fields__`** with a non-dict un-marks the class:
        `is_dataclass(C)` is false and `C(...)` constructs like a plain class.
        Rebinding it to another dict changes the fields, defaults and factories
        the next construction uses. Every default and factory is read out before
        the first factory runs, so a factory that rebinds it mid-construction
        changes nothing, as in CPython.
    - **A `__post_init__` added to a class that had none when decorated** runs on
        the next construction; CPython's generated `__init__` never calls it.
        Replacing a hook the class already had matches CPython, which also looks
        `self.__post_init__` up when it calls it.
    - **Re-decorating rebuilds the class.** `C = dataclass(frozen=True)(C)` gives
        Monty a fully frozen class that constructs normally. CPython keeps the
        `__init__` its first decoration generated, which writes fields through the
        new frozen `__setattr__` and so raises `FrozenInstanceError` on construction.

## Architectural gaps (cannot match)

- **No inheritance**, so field inheritance across base dataclasses is
    unsupported (Monty has no class inheritance at all).
- **`slots=True` / `weakref_slot=True`** — no `__slots__`, no weakrefs.
