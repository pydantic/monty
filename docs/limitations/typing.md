# `typing` module

`typing` exists so type-annotated code can `import` it without
`ModuleNotFoundError`. **No runtime type checking happens.** The forms are inert
marker objects: the generic ones can be subscripted, building an alias
(see [`typing` aliases](#typing-aliases)), `Union` and `Optional` build unions
(see [Unions](#unions)), and the rest raise CPython's `TypeError`.
Annotations are compiled and evaluated lazily, as in CPython 3.14 (see [Annotations](#annotations)).

## Names defined

`Any`, `Optional`, `Union`, `List`, `Dict`, `Tuple`, `Set`, `FrozenSet`,
`Callable`, `Type`, `Sequence`, `Mapping`, `Iterable`, `Iterator`,
`Generator`, `ClassVar`, `Final`, `Literal`, `TypeVar`, `Generic`,
`Protocol`, `Annotated`, `Self`, `Never`, `NoReturn`, `TYPE_CHECKING`,
`ForwardRef` (the [`annotationlib`](annotationlib.md) type) and
[`get_type_hints`](#get_type_hints).

`TYPE_CHECKING` is `False`, as in CPython at runtime.

## Not implemented

- `get_args`, `get_origin`, `cast`, `assert_type`,
    `assert_never`, `overload`, `final`, `runtime_checkable`, `NewType`,
    `NamedTuple`, `TypedDict`, `dataclass_transform`, `ParamSpec`,
    `Concatenate`, `Unpack`, `TypeAlias`, `TypeAliasType`, `LiteralString`,
    `evaluate_forward_ref`.
- Annotation introspection on **modules**: a module-level `x: int` is checked
    and dropped, so the module's `__annotations__` reads as `{}` (see
    [language.md](language.md#module-level-dunder-variables)). Functions and
    classes have theirs; see below.

## Annotations

Annotations follow CPython 3.14 (PEP 649): an annotated `def` or class body compiles its annotations into an
`__annotate__(format)` function, `__annotations__` is evaluated from it on first access, and under
`from __future__ import annotations` the stored values are strings instead (see
[language.md](language.md#__future__-imports)).
A class keeps the function in its namespace as `__annotate_func__` and the evaluated dict as `__annotations_cache__`,
the keys CPython uses; a namespace handed to `type()` may hold an `__annotate__` or an `__annotations__` of its own.
Divergences:

- **`f.__annotations__` is recomputed on every access.**
    A function has no `__dict__` to cache it in, so `f.__annotations__ is f.__annotations__` is `False`, a change to
    the dict is lost, an annotation with a side effect runs again, and `f.__annotations__ = {...}` raises
    `AttributeError`. A class evaluates once and caches, as in CPython.
- **`hasattr()` and `getattr()` with a default evaluate `__annotations__` synchronously**, so an
    `AttributeError` an annotation raises can answer `False` or the default as in CPython; an annotation that calls
    an external function raises `NotImplementedError` there, where CPython evaluates it, and a lazy host attribute
    reads as undefined (see [classes.md](classes.md#host-class-instances-classinstance-wrapper)).
    `obj.__annotations__` and `getattr()` without a default can suspend for the host.
- **`C.__annotate__` raises `AttributeError`.**
    The function only resolves class-body names when run under the class namespace, which `C.__annotations__` and
    [`get_annotations`](annotationlib.md#get_annotations) do, so it is not exposed on the class. `f.__annotate__` is
    exposed on functions.
- **Enclosing-function locals are not visible.**
    An annotation resolves names through the class namespace (a class body's own annotations) and the module globals,
    so `def outer(): T = int; def f(x: T): ...` raises `NameError` on `f.__annotations__` where CPython captures `T`,
    and a method's annotations do not see class-body names where CPython's do.
- **A class created by `exec()` evaluates in the session globals**, not the `exec` globals dict.
    A function created by `exec()` uses its dict, as in CPython.
- Only **simple `name: T` targets** are recorded, as in CPython. A bare
    `obj.attr: T` contributes nothing to `__annotations__` on either, but CPython
    still *evaluates the target expression*: `undefined.attr: int` raises
    `NameError` there and is silently dropped by Monty. With a value
    (`obj.attr: T = v`) Monty raises `NotImplementedError`.
- Binding **`__annotations__` explicitly** in a class body that *also* has annotated names raises
    `NotImplementedError` under `from __future__ import annotations`, where CPython merges the collected strings into
    it. Without the import the binding is an ordinary member that `C.__annotations__` returns, as in CPython.
- **A hand-written `__annotate__` closure runs with its real cells** in `FORWARDREF` and `STRING` format, where
    CPython substitutes a stringifier for an empty cell.

## `get_type_hints`

`get_type_hints(obj, globalns=None, localns=None, include_extras=False, *, format=None)` evaluates the annotations of a function or class.
It follows CPython 3.14's algorithm: the object's `__annotate__` runs (or its stored `__annotations__` dict is read), then a string (forward reference) evaluates in `globalns` / `localns`, `list['Foo']` / `Optional['Foo']` arguments are resolved at any depth, and `Annotated` metadata is stripped unless `include_extras`.
Under `from __future__ import annotations` every annotation is such a string, so it evaluates in `globalns` / `localns` only.
`format=` takes the [`annotationlib.Format`](annotationlib.md#format) members: `VALUE` (the default), `FORWARDREF`, which returns an unresolved name as an [`annotationlib.ForwardRef`](annotationlib.md#forwardref), and `STRING`, which returns the source text.
Divergences:

- **Name resolution follows [Annotations](#annotations)**: enclosing-function locals, and class-body names in a method's annotations, are not visible.
- **`globalns` / `localns` must be a dict or `None`.**
    Anything else raises `TypeError: get_type_hints() globalns must be a dict, not int`, where CPython fails later with an unrelated message.
- **A function's annotations are re-evaluated on every call**, including any side effects, where CPython evaluates them once (on `__annotations__`) and caches the result; a class's are evaluated once and cached.
    A builtin rebound after the first call (`exec('int = str')`) is therefore seen by the next call for a function.
- **Modules record no annotations**, so `get_type_hints(module)` is `{}`.
- **Evaluation errors show a `<string>` frame** in the traceback, where CPython shows its `typing` / `annotationlib` frames.
- **Host functions and host classes** return the hints the host answers with, unevaluated in the sandbox; see [host-values.md](host-values.md#type-hints).
    The host evaluates them in `VALUE` form whatever the `format`: `FORWARDREF` raises where CPython would return a `ForwardRef`, and `STRING` raises `NotImplementedError`.
- **`FORWARDREF` never asks the host for a name.**
    A name the host would supply through a name lookup is a `ForwardRef` in this format where CPython resolves it.
- **A `FORWARDREF` reference holds the resolved text.**
    CPython keeps the objects an unresolved expression used aside under placeholder names, so `x: int | Later` gives `ForwardRef('int | Later')` with a `__forward_arg__` of `'__annotationlib_name_1__ | Later'`; Monty writes `int | Later` into `__forward_arg__` and resolves `int` by name when the reference is evaluated.
    A class is written by its bare name: `Foo | Later` where CPython's `__resolved_str__` reads `__main__.Foo | Later`.
- **Comparison operators are not stringified.**
    `x: Later == 1` evaluates to `False` in `FORWARDREF` where CPython returns `ForwardRef('Later == 1')`.
- **`STRING` is the stringized source.**
    A starred argument stays as written (`x: Later(*[2])` gives `'Later(*[2])'`) where CPython reconstructs the call it made: `'Later(2)'`.

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

## `typing` aliases

Subscripting a generic `typing` form — `List`, `Dict`, `Tuple`, `Set`, `FrozenSet`, `Callable`, `Type`, `Sequence`,
`Mapping`, `Iterable`, `Iterator`, `Generator`, `ClassVar`, `Final`, `Literal` and `Annotated` — builds an alias
with CPython's repr (`typing.List[int]`), `__origin__`, `__args__`, `__parameters__` and, for `Annotated`,
`__metadata__`; equality and hashing follow the form and its arguments, a `None` argument becomes `NoneType` as in
CPython, and calling an alias other than `Annotated` (which calls its type) raises CPython's `TypeError`.
`Optional[X]` builds a union (see [Unions](#unions)); `Any`, `TypeVar`, `Self`, `Never`, `NoReturn`, `Generic` and
`Protocol` raise CPython's `TypeError` when subscripted.
Divergences:

- **The alias type is `types.GenericAlias`.**
    `type(List[int])` is the type of `list[int]`, where CPython has `typing._GenericAlias`; the two aliases still
    compare unequal (`List[int] != list[int]`).
- **`__origin__` of `Callable`, `Sequence`, `Mapping`, `Iterable`, `Iterator` and `Generator` is the `typing` form**,
    where CPython gives the `collections.abc` class, which Monty does not have.
- **Arguments are not validated** beyond `Annotated` needing two, `ClassVar` / `Final` one and `Callable` a
    parameter list and a result: `List[int, str]` and `Type[3]` are accepted as written where CPython raises.
- **Dunder attributes raise `AttributeError`**: `__name__` and `__qualname__`, where CPython gives the form's
    name (`'List'`), and `__module__`, `__doc__`, `__class__` and `__dict__`, where CPython gives the alias's own
    values. Anything else is looked up on the origin, as in CPython.

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
    CPython wraps the string as `ForwardRef('Foo')`.
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
