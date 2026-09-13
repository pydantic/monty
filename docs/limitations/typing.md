# `typing` module

`typing` exists so type-annotated code can `import` it without
`ModuleNotFoundError`. **No runtime type checking happens.** Apart from
`Union` and `Optional` (see [Unions](#unions)) the forms are inert marker
objects that cannot be subscripted: `List[int]` and `Callable[[int], str]`
raise `TypeError: 'typing._SpecialForm' object is not subscriptable`. The
builtin generics (`list[int]`, `dict[str, int]`, `tuple[int, ...]`) and `|`
unions (`int | None`) do work; see
[Runtime generic aliases](#runtime-generic-aliases) and [Unions](#unions).
Annotations are unaffected, being stringized rather than evaluated (see below).

## Names defined

`Any`, `Optional`, `Union`, `List`, `Dict`, `Tuple`, `Set`, `FrozenSet`,
`Callable`, `Type`, `Sequence`, `Mapping`, `Iterable`, `Iterator`,
`Generator`, `ClassVar`, `Final`, `Literal`, `TypeVar`, `Generic`,
`Protocol`, `Annotated`, `Self`, `Never`, `NoReturn`, `TYPE_CHECKING`.

`TYPE_CHECKING` is `False`, as in CPython at runtime.

## Not implemented

- `get_type_hints`, `get_args`, `get_origin`, `cast`, `assert_type`,
    `assert_never`, `overload`, `final`, `runtime_checkable`, `NewType`,
    `NamedTuple`, `TypedDict`, `dataclass_transform`, `ParamSpec`,
    `Concatenate`, `Unpack`, `TypeAlias`, `TypeAliasType`, `LiteralString`.
- Annotation introspection on **functions and modules**: `__annotations__` is
    not populated there. Class `__annotations__` **is** populated; see below.

## Class annotations are stringized

A class body's annotations **are** recorded, in order, on the class's
`__annotations__` dict, but in **stringized** form, unconditionally. The values
are the annotation expression rendered back to source, never evaluated. As in
CPython's PEP 563 stringizer the expression is *unparsed* rather than sliced out
of the file, so original spacing, line breaks and quote style are normalized
away (`x: dict[str,int]` gives `'dict[str, int]'`):

```python test="skip"
class C:
    x: int
    y: list[int]


C.__annotations__  # {'x': 'int', 'y': 'list[int]'}  -- strings
```

This is a known temporary divergence; see `class__annotations.py`.

- **Divergence from CPython 3.14's default** (PEP 649), where these are the
    evaluated objects (`C.__annotations__['x'] is int`). CPython only agrees with
    Monty when the calling code uses `from __future__ import annotations`
    (PEP 563), which Monty's behaviour is otherwise equivalent to, except that
    Monty stringizes whether or not that import is present.
- Generic aliases and `|` unions now evaluate (see below), so the common
    annotation forms no longer block a PEP 649 migration; annotations are
    still stringized regardless.
- **Treat the values as provisional.** Code reading `__annotations__` sees
    strings today and would see type objects after a PEP 649 migration; the
    *keys* and their order are stable either way.
- Only **simple `name: T` targets** are recorded, as in CPython. A bare
    `obj.attr: T` contributes nothing to `__annotations__` on either, but CPython
    still *evaluates the target expression*: `undefined.attr: int` raises
    `NameError` there and is silently dropped by Monty. With a value
    (`obj.attr: T = v`) Monty raises `NotImplementedError`.
- Binding **`__annotations__` explicitly** in a class body that *also* has
    annotated names raises `NotImplementedError`. CPython instead stores the
    collected annotations into whatever the name holds, merging into an explicit
    `dict`, or raising `TypeError` if it holds something else. A class body that
    binds the name but annotates nothing is accepted, and its binding stands.
- **`from __future__ import annotations`** is accepted as a **no-op**, since it
    describes what Monty already does. See
    [language.md](language.md) for the other features.
- Consequences: `get_type_hints()` (which would evaluate the strings) is still
    not implemented, and code that reads `__annotations__` expecting type
    *objects* sees strings. CPython 3.14's `@dataclass` reads evaluated objects
    (`annotationlib.Format.FORWARDREF`), but keeps a string path for `ClassVar` /
    `InitVar` so PEP 563 code still works, which is what makes stringized
    annotations enough to build on.

If you need real type validation, do it on the *host* side around the
sandbox boundary.

## Runtime generic aliases

Subscripting a builtin type builds a `types.GenericAlias`, as in CPython:
`Record = tuple[int, int, int]` is a value with `__origin__`, `__args__` and
`__parameters__` (always `()`), reprs as `tuple[int, int, int]`, compares and
hashes by origin and arguments, calls through to its origin
(`list[int]([1, 2])` is `[1, 2]`), and resolves every other attribute on the
origin (`list[int].__name__` is `'list'`). `isinstance(x, list[int])` raises
`TypeError: isinstance() argument 2 cannot be a parameterized generic`, and
subscripting an alias again raises `TypeError: list[int] is not a generic class`.

The subscriptable types are `list`, `tuple`, `dict`, `set`, `frozenset`, `type`,
`collections.deque`, `collections.defaultdict`, `collections.Counter`,
`functools.partial`, `re.Pattern` and `re.Match`. `list.__class_getitem__(int)`
works for each, called directly on the type. Every other type raises
`TypeError: type 'int' is not subscriptable`, including the ones CPython
parameterizes that Monty lacks or models differently:

- `enumerate` (a builtin function in Monty, not a type).
- `collections.namedtuple` classes, which in CPython inherit
    `tuple.__class_getitem__`; `Point[int]` raises here.
- User classes: `__class_getitem__` is not looked up, so `Foo[int]` raises
    `TypeError: type 'Foo' is not subscriptable` whether or not the class
    defines it.

Divergences in the aliases themselves:

- **No `types` module.** `type(list[int])` reprs as `<class 'types.GenericAlias'>`,
    but `import types` still raises `ModuleNotFoundError`, so
    `isinstance(x, types.GenericAlias)` cannot be written; compare
    `type(x) is type(list[int])` instead.
- **Other `typing` forms stay unsubscriptable.** `typing.List[int]` raises
    as described above; only the builtin types build aliases, and only
    `typing.Union` / `typing.Optional` build unions (see [Unions](#unions)).
- **Not iterable.** CPython iterates an alias to yield its starred form
    (`*tuple[int, ...]`); Monty raises `TypeError: 'types.GenericAlias' object is not iterable`.
- **Argument reprs use Monty's type names.** A user class prints its bare name
    (`list[Foo]` where CPython prints `list[__main__.Foo]`, see
    [classes.md](classes.md)), and `collections.Counter[str]` prints as
    `Counter[str]`.
- **An unhashable argument names the alias.** `hash(list[[1]])` raises
    `TypeError: unhashable type: 'types.GenericAlias'` where CPython names the
    argument (`'list'`), as with a tuple holding a list.
- **A namedtuple subscript is one argument.** `list[Point(int, str)]` keeps
    the namedtuple as its single argument, where CPython's `PyTuple_Check`
    unpacks it into `list[int, str]`.
- **A cycle through the arguments prints as `...`.** CPython's alias repr has
    no recursion guard and raises `RecursionError` on `l = []; l.append(list[l]); repr(l)`;
    Monty prints `[list[[...]]]`.

## Unions

`int | None`, `str | list[int]` and the other `|` combinations of types build
a `typing.Union`, as in CPython 3.14 (where `types.UnionType` and
`typing.Union` are the same object): `typing.Union` is that type, so
`type(int | None) is typing.Union`. `typing.Union[int, str]` and
`typing.Optional[int]` build the same values. A union flattens nested unions,
drops duplicates and collapses to a lone member (`int | int is int`), reprs
as `int | None`, compares and hashes as an unordered set of members, and
works as the second argument of `isinstance`, including inside a tuple.
`__args__`, `__origin__` and `__parameters__` are set; every other attribute
raises `AttributeError`, and a union cannot be called, subscripted, iterated
or ordered, each with CPython's message.

`|` unions with a type on either side: builtin types, exception types, user
classes, `collections.namedtuple` classes, host classes, generic aliases,
`None` and the `typing` markers; anything else raises the usual
`unsupported operand type(s) for |` error. A union's own `|` accepts any
operand (`(int | str) | 1` is `int | str | 1`), as CPython 3.14's does.

Divergences:

- **Member reprs use Monty's type names**, as in a generic alias: `Foo | None`
    where CPython prints `__main__.Foo | None`.
- **An unhashable member names the union.** `hash(int | list[[1]])` raises
    `TypeError: unhashable type: 'typing.Union'` where CPython names the
    member.
- **`typing.Union` and `typing.Optional` are the only subscriptable forms.**
    `typing.List[int]` and the rest still raise.
- **Neither aliases nor unions cross the host boundary.** One built in the
    sandbox reaches the host as its repr string. Passed in from the host, a
    `list[int]` degrades to an external function (it is callable, so it is
    treated like any unmodeled class) and an `int | None` is rejected with
    `MontyConversionError`; neither has a `MontyObject` form. Their type
    objects (`types.GenericAlias`, `typing.Union`) round-trip by identity.
