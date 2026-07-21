# `typing` module

`typing` exists purely so type-annotated code can `import` it without
`ModuleNotFoundError`. **No runtime type checking happens.** The forms are
inert marker objects; subscripting them (`list[int]`, `Optional[str]`,
`Union[int, str]`) returns a placeholder value but does not validate
anything.

## Names defined

`Any`, `Optional`, `Union`, `List`, `Dict`, `Tuple`, `Set`, `FrozenSet`,
`Callable`, `Type`, `Sequence`, `Mapping`, `Iterable`, `Iterator`,
`Generator`, `ClassVar`, `Final`, `Literal`, `TypeVar`, `Generic`,
`Protocol`, `Annotated`, `Self`, `Never`, `NoReturn`, `TYPE_CHECKING`.

`TYPE_CHECKING` is `False` (as in CPython at runtime).

## Not implemented

- `get_type_hints`, `get_args`, `get_origin`, `cast`, `assert_type`,
  `assert_never`, `overload`, `final`, `runtime_checkable`, `NewType`,
  `NamedTuple`, `TypedDict`, `dataclass_transform`, `ParamSpec`,
  `Concatenate`, `Unpack`, `TypeAlias`, `TypeAliasType`, `LiteralString`.
- Annotation introspection on **functions and modules**: `__annotations__` is
  not populated there. (Class `__annotations__` **is** populated — see below.)

## Class annotations are stringized

A class body's annotations **are** recorded, in order, on the class's
`__annotations__` dict — but in **stringized** form, unconditionally. The values
are the annotation expression rendered back to source, never evaluated. As in
CPython's PEP 563 stringizer the expression is *unparsed* rather than sliced out
of the file, so original spacing, line breaks and quote style are normalized
away (`x: dict[str,int]` gives `'dict[str, int]'`):

```python
class C:
    x: int
    y: list[int]
C.__annotations__        # {'x': 'int', 'y': 'list[int]'}  -- strings
```

This is a known temporary divergence; see `class__annotations.py`.

- **Divergence from CPython 3.14's default** (PEP 649), where these are the
  evaluated objects (`C.__annotations__['x'] is int`). CPython only agrees with
  Monty when the calling code uses `from __future__ import annotations`
  (PEP 563), which Monty's behaviour is otherwise equivalent to — except that
  Monty stringizes whether or not that import is present.
- The blocker is that Monty has no generic types: `list[int]` and
  `dict[str, int]` raise `TypeError: 'type' object is not subscriptable`, and
  `int | None` raises `TypeError: unsupported operand type(s) for |: 'type'
  and 'NoneType'`, so evaluated annotations would fail on the most common
  forms. Runtime `types.GenericAlias` and `|` unions are the prerequisite for
  matching PEP 649.
- **Treat the values as provisional.** Code reading `__annotations__` sees
  strings today and would see type objects after a PEP 649 migration; the
  *keys* and their order are stable either way.
- Only **simple `name: T` targets** are recorded, as in CPython. A bare
  `obj.attr: T` contributes nothing to `__annotations__` on either, but CPython
  still *evaluates the target expression* — `undefined.attr: int` raises
  `NameError` there and is silently dropped by Monty. With a value
  (`obj.attr: T = v`) Monty raises `NotImplementedError`.
- Binding **`__annotations__` explicitly** in a class body that *also* has
  annotated names raises `NotImplementedError`. CPython instead stores the
  collected annotations into whatever the name holds — merging into an explicit
  `dict`, or raising `TypeError` if it holds something else. A class body that
  binds the name but annotates nothing is accepted, and its binding stands.
- **`from __future__ import annotations`** is accepted as a **no-op**, since it
  describes what Monty already does. See
  [language.md](language.md#__future__-imports) for the other features.
- Consequences: `get_type_hints()` (which would evaluate the strings) is still
  not implemented, and code that reads `__annotations__` expecting type
  *objects* sees strings. CPython 3.14's `@dataclass` reads evaluated objects
  (`annotationlib.Format.FORWARDREF`), but keeps a string path for `ClassVar` /
  `InitVar` so PEP 563 code still works — which is what makes stringized
  annotations enough to build on.

If you need real type validation, do it on the *host* side around the
sandbox boundary.
