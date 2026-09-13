# `typing` module

`typing` exists so type-annotated code can `import` it without
`ModuleNotFoundError`. **No runtime type checking happens.** The forms are
inert marker objects and none of them can be subscripted: `Optional[str]`,
`List[int]` and `Callable[[int], str]` raise `TypeError: 'typing._SpecialForm' object is not subscriptable`, and
`Union[int, str]` raises `TypeError: 'type' object is not subscriptable`.
The builtin generics (`list[int]`, `dict[str, int]`, `tuple[int, ...]`) do
work; see [Runtime generic aliases](#runtime-generic-aliases). Annotations are
unaffected, being stringized rather than evaluated (see below).

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
- The blocker is that Monty has no union types: `int | None` raises
    `TypeError: unsupported operand type(s) for |: 'type' and 'NoneType'`, so evaluated annotations
    would fail on one of the most common forms. `|` unions are the remaining
    prerequisite for matching PEP 649, now that `list[int]` evaluates (see
    below).
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
- **`typing` forms stay unsubscriptable.** `typing.List[int]` and
    `typing.Optional[int]` raise as described above; only the builtin types
    build aliases.
- **No unions.** `list[int] | None` raises `TypeError`, as `int | None` does.
- **Not iterable.** CPython iterates an alias to yield its starred form
    (`*tuple[int, ...]`); Monty raises `TypeError: 'types.GenericAlias' object is not iterable`.
- **Argument reprs use Monty's type names.** A user class prints its bare name
    (`list[Foo]` where CPython prints `list[__main__.Foo]`, see
    [classes.md](classes.md)), and `collections.Counter[str]` prints as
    `Counter[str]`.
- **An unhashable argument names the alias.** `hash(list[[1]])` raises
    `TypeError: unhashable type: 'types.GenericAlias'` where CPython names the
    argument (`'list'`), as with a tuple holding a list.
- **A cycle through the arguments prints as `...`.** CPython's alias repr has
    no recursion guard and raises `RecursionError` on `l = []; l.append(list[l]); repr(l)`;
    Monty prints `[list[[...]]]`.
