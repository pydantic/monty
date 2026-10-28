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

Only the features listed below are missing.
Each **raises where it is written** — at the decoration, or at the `field()` or `make_dataclass()` call — rather
than producing a subtly wrong class; `Field.metadata` raises when it is read.

Each raises `NotImplementedError`, marking a feature Monty has not built yet
rather than a mistake in the calling code. CPython accepts all of them, so the
exception type is a divergence in its own right: code catching `TypeError`
around a decoration will not catch these.

- **`@dataclass(slots=True)` and `@dataclass(weakref_slot=True)`** raise
    `NotImplementedError: dataclass() does not yet support the <name> option`: Monty has neither `__slots__` nor
    weakrefs.
    `weakref_slot=True` without `slots` reports this rather than CPython's
    `TypeError: weakref_slot is True but slots is False`.
- **`field(metadata=...)`** raises `NotImplementedError: field() does not yet support the metadata argument`, at
    the `field()` call rather than at decoration: Monty has no `types.MappingProxyType`.
- **`Field.metadata`** raises
    `NotImplementedError: Field.metadata is not yet supported, types.MappingProxyType is not implemented`.
- **`make_dataclass` field names built at runtime** raise
    `NotImplementedError: make_dataclass() does not yet support the field name 'x', which appears nowhere in the source code`.
    Fields are keyed by interned name and Monty cannot intern new text while running, so a name must be spelled
    somewhere in the program, as an identifier or a string literal.

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

- **`__dataclass_fields__` holds no `ClassVar` entries.** CPython keeps them in
    the mapping, marked `_FIELD_CLASSVAR`, and filters them in `fields()`; Monty
    drops them, so class variables never appear in it. `InitVar` entries are kept
    and marked `_FIELD_INITVAR`, as in CPython.
    A `ClassVar` bound to a `field()` therefore keeps the `Field` object as its class attribute, and none of the
    `ClassVar` field checks (`kw_only`, `default_factory`) run.

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

- **`default_factory`, `__post_init__` and the `asdict()` / `astuple()` factories cannot suspend.** All run in a
    synchronous position the interpreter cannot preserve and resume, so calling an
    external function, an `os` function, or awaiting inside one raises
    `NotImplementedError: external function 'f' cannot suspend in this synchronous context`.
    The traceback identifies the calling code.
    Ordinary in-sandbox code in them runs normally.

- **`ClassVar` / `InitVar` / `KW_ONLY` detection in a class body is purely textual.** Monty matches the
    annotation text (bare, dotted, subscripted, or quoted) without checking that
    the name is actually imported, where CPython resolves a *string* annotation
    through the defining module's namespace. So `c: "ClassVar[int]"` without
    `ClassVar` in scope is excluded by Monty but is an ordinary field to CPython.
    Conversely any dotted spelling matches, so a same-named attribute on an
    unrelated module (`mymod.ClassVar`) is treated as `typing.ClassVar`.

- **`KW_ONLY` and `InitVar` are placeholders.** A class body's annotations are never evaluated, so only
    `make_dataclass`, which is handed the objects themselves, recognises them by identity.
    Their reprs match CPython's, but `type()` of either reports
    `typing._SpecialForm` (CPython: `dataclasses._KW_ONLY_TYPE` and `type`), and `InitVar[int]` evaluated outside an
    annotation raises `TypeError: 'typing._SpecialForm' object is not subscriptable`.

- **A field holding a function or bound method reprs differently**, since
    Monty's own `repr` for those differs (see [classes.md](classes.md)). Only the
    text differs; the value and its equality match CPython.

- **The generated `__repr__` uses the class's `__name__`**, since Monty classes
    have no `__qualname__` (see [classes.md](classes.md)). A dataclass defined in
    a function reprs as `Local(x=1)` where CPython gives
    `make.<locals>.Local(x=1)`.

- **A class-body `__setattr__` never runs for the generated `__init__`** of a
    non-frozen dataclass, which writes fields straight into the instance
    `__dict__`. This is the never-dispatched attribute hook described in
    [classes.md](classes.md) rather than something dataclass-specific.

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
    because Monty's parser has no `del` statement at all. Calling the generated
    `C.__delattr__(obj, 'x')` directly raises it as CPython does, and assignment
    matches CPython, message included.

- **`__dataclass_params__` and `Field` flags read back normalised.** `C.__dataclass_params__`
    exists, reprs like CPython's and answers all ten flags, but each is the `bool`
    Monty acted on: `@dataclass(frozen=1)` reports `frozen=True` where CPython
    echoes the `1` you passed. `field(init=0)` likewise reports `init=False`.
    Truthiness is taken at the call, so a `__bool__` that raises does so there rather than when CPython's generated
    code first reads the flag.

- **The generated methods follow Monty's function conventions.** Each method
    `@dataclass` generates (`__init__`, `__repr__`, `__eq__`, `__hash__`, the
    ordering dunders, and `__setattr__`/`__delattr__` when frozen, as CPython's
    options and the class body decide) reports `function` from `type()`, reprs as
    `<function 'C.__init__' at 0x..>` and lacks `__name__`, as every Monty
    function does (see [language.md](language.md)).

- **Calling a generated method directly needs an instance of a sandbox class
    as `self`.** `C.__repr__(5)` raises
    `TypeError: C.__repr__() requires a class instance as self, not 'int'`, where
    CPython's generated code reads the fields off whatever it is given and fails
    on the first one (`AttributeError: 'int' object has no attribute 'x'`).
    Argument binding and its error messages match CPython.

- **A frozen `__setattr__` refuses every attribute on any instance.** CPython's
    refuses only when `self` is an instance of the decorated class or the name is
    a field, and otherwise defers to `super()`. So `F.__setattr__(other, 'w', 1)`,
    or `Q.__setattr__ = F.__setattr__` followed by `Q().w = 1`, raises
    `FrozenInstanceError` where CPython raises a `TypeError` from `super()`.

- **Ordering errors name the dataclasses, not the fields.** `order=True` compares the field tuples as Monty compares
    tuples, so a pair of fields that cannot be ordered raises
    `TypeError: '<' not supported between instances of 'C' and 'C'` where CPython names the fields' types
    (`'NoneType' and 'int'`); see [language.md](language.md).

- **Every ordering consults the generated `__lt__`.** The operators, `sorted()`, `min()` and `max()` compare through
    the field list of the class's generated `__lt__` whichever operator is asked, so rebinding `C.__le__` after
    decoration changes nothing, and replacing `C.__lt__` makes instances unorderable, even for `<=`, where CPython
    still calls the generated `__le__`.
    A hand-written `__lt__` is never dispatched (see [classes.md](classes.md)).

- **`asdict()` / `astuple()` rebuild only lists, tuples and dicts.** Any other value is deep-copied, so a dataclass
    inside a namedtuple, `defaultdict`, `Counter` or other container subclass is copied rather than converted, where
    CPython rebuilds those containers around converted members.
    A dict is snapshotted before its members are converted, so a conversion that resizes it does not raise
    `RuntimeError` as CPython's live iteration does.

- **An `InitVar` declared with `field(init=False)` is never passed to `__post_init__`.** CPython's generated
    `__init__` still names it in that call and raises `NameError`; Monty calls the hook without it.

- **`fields()` / `asdict()` / `astuple()` / `replace()` / `__replace__` read `__dataclass_fields__` from the class
    namespace.**
    CPython reads it with `getattr`, so it would also find the attribute on an instance's `__dict__`.

- **`__replace__` is a native function.** Every dataclass shares one, as CPython's share `dataclasses._replace`, and
    it reprs as `<function _replace at 0x..>` and binds to an instance like a method.
    But `type()` reports `builtin_function_or_method` and it has no `__name__`, like every Monty module function.

- **A dataclass restored from a 1.4 snapshot has no `__replace__`.** Those dumps hold classes as decorated then, so
    `copy.replace()` on their instances raises `TypeError: replace() does not support C objects`;
    `dataclasses.replace()` still works.
    Classes decorated after the restore, and those in 1.0 and 1.1 dumps (which are upgraded on load), have it.

- **`__doc__` is never filled in from the signature.** CPython sets an undocumented dataclass's `__doc__` to its
    `__init__` signature (`'C(x: int)'`, or `'Z()'` from `make_dataclass('Z', [])`); Monty leaves it `None`.
    A class with its own docstring keeps it, as in CPython.

- **`make_dataclass` sets `__module__` only when `module=` is given.** CPython names the calling module otherwise;
    Monty classes have no `__module__` (see [classes.md](classes.md)), and the class reprs as `<class 'C'>`.

- **`make_dataclass(namespace=...)` refuses a non-string key** with
    `TypeError: non-string key (int) in the namespace of class 'C'`, as `type()` does, where CPython only warns.

## Architectural gaps (cannot match)

- **No inheritance**, so field inheritance across base dataclasses is
    unsupported (Monty has no class inheritance at all).
    `make_dataclass(..., bases=...)` with any base raises `TypeError: make_dataclass() bases are not supported`, as
    `type()` refuses one.
- **`slots=True` / `weakref_slot=True`** — no `__slots__`, no weakrefs.
