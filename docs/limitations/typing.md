# `typing` module

`typing` exists so type-annotated code can `import` it without
`ModuleNotFoundError`. **No runtime type checking happens.** Apart from
`Union` and `Optional` (see [Unions](#unions)) the forms are inert marker
objects that cannot be subscripted: `List[int]` and `Callable[[int], str]`
raise `TypeError: 'typing._SpecialForm' object is not subscriptable`.
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

Subscripting a builtin type builds a `types.GenericAlias`, but only for
`list`, `tuple`, `dict`, `set`, `frozenset`, `type`, `collections.deque`,
`collections.defaultdict`, `collections.Counter`, `functools.partial`,
`re.Pattern` and `re.Match`. Every other type raises
`TypeError: type 'int' is not subscriptable`, including ones CPython
parameterizes:

- `enumerate` (a builtin function in Monty, not a type).
- `collections.namedtuple` classes, which in CPython inherit
    `tuple.__class_getitem__`; `Point[int]` raises.
- User classes: `__class_getitem__` is not looked up, so `Foo[int]` raises
    whether or not the class defines it.

Divergences in the aliases themselves:

- **No `types` module.** `type(list[int])` reprs as `<class 'types.GenericAlias'>`,
    but `import types` raises `ModuleNotFoundError`, so
    `isinstance(x, types.GenericAlias)` cannot be written; compare
    `type(x) is type(list[int])` instead.
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
- **No `__class__`.** `list[int].__class__` raises `AttributeError`, as it
    does for every builtin value (see [builtins.md](builtins.md)); use
    `type(list[int])`.
- **No `__orig_class__` on call results.** CPython sets it on the result of
    calling an alias when the object accepts attributes, so
    `functools.partial[int](f).__orig_class__` is `functools.partial[int]`;
    Monty's partial objects take no attributes, so the lookup raises
    `AttributeError`. `partial` is the only subscriptable type whose CPython
    instances accept it.

## Unions

`int | None`, `typing.Union[int, str]` and `typing.Optional[int]` build a
`typing.Union`, as in CPython 3.14. Divergences:

- **Member reprs use Monty's type names**, as in a generic alias: `Foo | None`
    where CPython prints `__main__.Foo | None`.
- **An unhashable member names the union.** `hash(int | list[[1]])` raises
    `TypeError: unhashable type: 'typing.Union'` where CPython names the
    member.
- **No attributes beyond `__args__`, `__origin__` and `__parameters__`.**
    `__class__`, `__or__` and the other dunders CPython exposes raise
    `AttributeError`.
- **String members stay strings.** `Optional['Foo']` is `'Foo' | None` where
    CPython wraps the string as `ForwardRef('Foo')`; there is no `ForwardRef`.
- **Special forms are accepted as members.** `Optional[typing.Final]` builds
    `typing.Final | None` where CPython raises
    `TypeError: Plain typing.Final is not valid as type argument`.
- **Neither aliases nor unions cross the host boundary.** One built in the
    sandbox reaches the host as its repr string. Passed in from the host, a
    `list[int]` degrades to an external function (it is callable, so it is
    treated like any unmodeled class) and an `int | None` is rejected with
    `MontyConversionError`; neither has a `MontyObject` form. Their type
    objects round-trip: `types.GenericAlias` by identity, and Monty's
    `typing.Union` as the host's `types.UnionType`, which is `typing.Union`
    itself only from Python 3.14.
