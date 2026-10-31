# `typing` module

`typing` exists so type-annotated code can `import` it without
`ModuleNotFoundError`. **No runtime type checking happens.** Apart from
`Union` and `Optional` (see [Unions](#unions)) and `Literal` and `Annotated`
(see [`Literal` and `Annotated`](#literal-and-annotated)) the forms are inert
marker objects that cannot be subscripted: `List[int]` and `Callable[[int], str]`
raise `TypeError: 'typing._SpecialForm' object is not subscriptable`.
Annotations are unaffected, being stringized rather than evaluated (see below).

## Names defined

`Any`, `Optional`, `Union`, `List`, `Dict`, `Tuple`, `Set`, `FrozenSet`,
`Callable`, `Type`, `Sequence`, `Mapping`, `Iterable`, `Iterator`,
`Generator`, `ClassVar`, `Final`, `Literal`, `TypeVar`, `Generic`,
`Protocol`, `Annotated`, `Self`, `Never`, `NoReturn`, `TYPE_CHECKING`.

`TYPE_CHECKING` is `False`, as in CPython at runtime.

## Not implemented

- `get_args`, `get_origin`, `cast`, `assert_type`,
    `assert_never`, `overload`, `final`, `runtime_checkable`, `NewType`,
    `NamedTuple`, `TypedDict`, `dataclass_transform`, `ParamSpec`,
    `Concatenate`, `Unpack`, `TypeAlias`, `TypeAliasType`, `LiteralString`.
- Annotation introspection on **functions and modules**: `__annotations__` is
    not populated there. Class `__annotations__` **is** populated; see below.
    Function annotations are read only through
    [`get_type_hints`](#get_type_hints).

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
- Consequences: code that reads `__annotations__` expecting type *objects*
    sees strings; [`get_type_hints()`](#get_type_hints) evaluates them. CPython 3.14's `@dataclass` reads evaluated objects
    (`annotationlib.Format.FORWARDREF`), but keeps a string path for `ClassVar` /
    `InitVar` so PEP 563 code still works, which is what makes stringized
    annotations enough to build on.

If you need real type validation, do it on the *host* side around the
sandbox boundary.

## `get_type_hints`

`get_type_hints(obj, globalns=None, localns=None, include_extras=False, *, format=None)` evaluates the annotations Monty stores.
It follows CPython 3.14's algorithm: a plain annotation evaluates where it was written, a string (forward reference) in `globalns` / `localns`, and `list['Foo']` / `Optional['Foo']` arguments are resolved at any depth.
Divergences:

- **Function annotations see only the function's globals.**
    Enclosing-function locals and class-body names are not visible, so `def outer(): Local = int; def inner(x: Local): ...` and a method annotated with a class attribute raise `NameError` where CPython resolves them.
- **A class created by `exec()` uses the session globals**, not the `exec` globals dict.
    A function created by `exec()` uses its dict, as in CPython.
- **Unsubscriptable `typing` forms fail on evaluation.**
    `x: List[int]`, `ClassVar[int]` (common in dataclasses) and `Callable[[int], str]` raise `TypeError: 'typing._SpecialForm' object is not subscriptable` from `get_type_hints`, since they are inert markers (see above).
    `Annotated[...]` is resolved and, unless `include_extras=True`, replaced by its type at any depth, as in CPython.
- **`format=` other than `None` raises `NotImplementedError`.**
- **`globalns` / `localns` must be a dict or `None`.**
    Anything else raises `TypeError: get_type_hints() globalns must be a dict, not int`, where CPython fails later with an unrelated message.
- **A string stored in a class's `__annotations__` evaluates as annotation source, not as a forward reference.**
    Monty's stored annotations are already strings, so one assigned directly (`C.__annotations__['x'] = 'Missing'`) is indistinguishable from `x: Missing`: it evaluates in the class and session namespaces, so `get_type_hints(C, {'Missing': int})` raises `NameError` where CPython returns `{'x': int}`.
- **A self-referential string stays a `str`.**
    With `a = 'a'`, `def f(x: 'a')` gives `{'x': 'a'}` where CPython returns `ForwardRef('a', ...)`; there is no `ForwardRef`.
- **Annotations are re-evaluated on every call**, including any side effects, where CPython evaluates a non-string annotation once.
- **Modules record no annotations**, so `get_type_hints(module)` is `{}`.
- **Evaluation errors show a `<string>` frame** in the traceback, where CPython shows its `typing` / `annotationlib` frames.
- **Host functions and host classes** return the hints the host answers with, unevaluated in the sandbox; see [host-values.md](host-values.md#type-hints).

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

## `Literal` and `Annotated`

`Literal['a', 'b']` and `Annotated[int, 'meta']` build CPython's
`typing._LiteralGenericAlias` and `typing._AnnotatedAlias`, with `__origin__`,
`__args__`, `__parameters__`, `__name__` and (for `Annotated`) `__metadata__`.
Choices deduplicate by value and type, nested forms flatten, an `Annotated`
delegates other attributes and calls to its type, and both take part in `|`
unions. Divergences:

- **`Literal` and `Annotated` themselves are `typing._SpecialForm`** rather
    than CPython's `_TypedCacheSpecialForm`, and calling one raises
    `TypeError: 'typing._SpecialForm' object is not callable` where CPython
    says `Cannot instantiate typing.Literal` (`Literal[1]()` does match).
- **Argument reprs use Monty's type names**, as in a generic alias:
    `typing.Annotated[Foo, 1]` where CPython prints `typing.Annotated[__main__.Foo, 1]`.
- **String arguments stay strings.** `Annotated['Foo', 1]` keeps `'Foo'` where
    CPython wraps it as `ForwardRef('Foo', is_class=True)`; `get_type_hints`
    resolves it either way, and `Literal['Foo']` is a plain choice in both.
- **The annotated type is barely checked.** A tuple is refused with CPython's
    message and `None` becomes `NoneType`, but a bare special form is accepted
    (`Annotated[Optional, 1]`) where CPython raises
    `TypeError: Plain typing.Optional is not valid as type argument`.
- **Only the listed attributes exist.** `__module__`, `__qualname__` and `__doc__` raise `AttributeError` on
    both forms, where CPython gives `'typing'`, `'Literal'` and `None` for `Literal[1]` and `'typing'`,
    `'Annotated'` and the `Annotated` docstring for `Annotated[int, 1]`;
    an `Annotated` delegates non-dunder names to its type and nothing else.
- **Error messages name the qualified type.** `len(Literal[1])` raises
    `object of type 'typing._LiteralGenericAlias' has no len()` and
    `Literal[1].foo` raises `'typing._LiteralGenericAlias' object has no attribute 'foo'`,
    where CPython says `'_LiteralGenericAlias'` and a bare `foo`.
- **An unhashable argument names the alias.** `hash(Literal[[1]])` raises
    `TypeError: unhashable type: 'typing._LiteralGenericAlias'` where CPython
    says `cannot use 'tuple' as a set element (unhashable type: 'list')`, and
    `hash(Annotated[int, [1]])` names `'typing._AnnotatedAlias'` where CPython names `'list'`.
- **Not iterable.** CPython iterates an alias to yield its starred form
    (`*Literal[1]`); Monty raises `TypeError: 'typing._LiteralGenericAlias' object is not iterable`.
- **No attributes beyond those listed.** `__class__`, `__or__` and the other
    dunders CPython exposes raise `AttributeError`, and an `Annotated` relays
    no dunder to its type.
- **Neither crosses the host boundary.** Like a generic alias, one built in
    the sandbox reaches the host as its repr string, and `type(Literal[1])` has
    no `MontyObject` form.

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
