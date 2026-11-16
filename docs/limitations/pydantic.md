# `pydantic` module

Monty bundles a native subset of [pydantic](https://docs.pydantic.dev/) 2.13: `TypeAdapter`
validates sandbox values against type hints with pydantic's own error messages, and
`ValidationError` reports them as pydantic does.
Validation runs in Rust and never enters the sandbox except to call a dataclass's `default_factory`
or `__post_init__`.
The messages, `errors()` output and `str()` format are copied from pydantic-core, and the
`url` entries name pydantic 2.13.

## Names defined

`TypeAdapter`, `ValidationError`, `PydanticUserError`, `PydanticSchemaGenerationError`, `VERSION`, `__version__`.

## Not implemented

- `BaseModel`, `Field`, `ConfigDict`, `validate_call`, `pydantic.dataclasses`, `field_validator`,
    `model_validator`, `computed_field`, `TypeAdapter.json_schema()` and the rest of the package raise
    `AttributeError` or `ImportError`.
- `TypeAdapter.validate_strings()` and `TypeAdapter.json_schema()` raise `NotImplementedError`.
- `dump_python(..., include=, exclude=, fallback=, polymorphic_serialization=, context=)`, the same on
    `dump_json`, and `exclude_unset=True`, `exclude_defaults=True`, `exclude_computed_fields=True` or
    `serialize_as_any=True` raise `NotImplementedError` naming the argument; `mode`, `indent`, `ensure_ascii`
    and `exclude_none` are supported, `by_alias`, `round_trip` and `warnings` are type-checked as in pydantic and
    otherwise have no effect.
- `pydantic_core` is not importable, so `PydanticSerializationError` cannot be named; it is a `ValueError`.
- `TypeAdapter(type, config=...)` and `TypeAdapter(type, module=...)` raise `NotImplementedError`;
    `_parent_depth` is accepted and ignored.
- `validate_python(..., extra=, from_attributes=, context=, by_alias=, by_name=)`,
    `validate_json(..., extra=, context=, by_alias=, by_name=)` raise `NotImplementedError` naming the
    argument; `strict` is supported. `experimental_allow_partial=True`, `'on'` or `'trailing-strings'` raise
    `NotImplementedError` too; `False` and `'off'` are accepted.
- **Method signature errors do not count `self`.** `ta.validate_python(1, 2)` reports
    `takes 1 positional argument but 2 were given` and `validate_python(object=1)` an unexpected keyword
    argument, where pydantic's methods count the instance (`takes 2 positional arguments but 3 were given`)
    and name positional-only arguments passed as keywords. `TypeAdapter.__init__` counts it, as in pydantic.
- **`TypeAdapter[int]` raises `TypeError: type 'TypeAdapter' is not subscriptable`**; pydantic's
    `TypeAdapter` is `Generic`.
- **Methods cannot be read as bound methods.** `ta.validate_python` without a call, and
    `TypeAdapter.validate_python(ta, x)`, raise `AttributeError`.
- **`copy.copy` / `copy.deepcopy` of an adapter** build a new adapter that shares the hint's values
    (literal choices, defaults) rather than deep-copying them.
- `Annotated[T, ...]` validates as `T`: the metadata is ignored until `Field` exists.

## Type hints

`TypeAdapter` builds a validator for `int`, `float`, `str`, `bool`, `bytes`, `None`, `Any`, `object`,
`list[T]`, `set[T]`, `frozenset[T]`, `tuple[A, B]`, `tuple[T, ...]`, `dict[K, V]`, `X | Y`, `Optional[X]`,
`Literal[...]`, `datetime`, `date`, `time`, `timedelta`, `Decimal`, a stdlib `@dataclass` and a
`collections.namedtuple` class, and `typing.List`, `Set`, `FrozenSet`, `Tuple`, `Dict` and `Mapping` (bare
or parameterised, validated as the builtin; `Mapping[K, V]` builds a `dict`), at any nesting, including a dataclass whose fields name the class again.
The bare `list`, `set`, `frozenset`, `tuple` and `dict` take `Any` items.
Any other hint raises `PydanticSchemaGenerationError` with pydantic's message, and a generic with the wrong
number of arguments (`dict[int]`) the `TypeError` pydantic raises. Divergences:

- **A string hint is not evaluated.** `TypeAdapter('int')` and `TypeAdapter(list['Foo'])` raise
    `PydanticSchemaGenerationError` where pydantic resolves the name in the caller's module.
    A forward reference inside a dataclass field is resolved, through `typing.get_type_hints`.
- **`typing.Sequence`, `Iterable`, `Callable`, `Type[X]` / `type[X]`, and bare `type` and `complex`** raise
    `PydanticSchemaGenerationError` where pydantic builds a validator.
- **A plain subclass of a dataclass validates with the fields of the dataclass it inherits from**, as in
    pydantic, but a `@dataclass` that derives from another dataclass only has its own fields (see
    [dataclasses.md](dataclasses.md)).
- **A `TypeAdapter` used as a hint** raises `PydanticSchemaGenerationError`, as in pydantic.
- **The hint repr in the message uses Monty's names**: `<class 'Foo'>` where pydantic prints `<class '__main__.Foo'>`.
- **A dataclass's field hints are evaluated when the adapter is built**, through `typing.get_type_hints`,
    so its divergences apply (see [typing.md](typing.md#get_type_hints)); an undefined forward reference raises
    `NameError` from `TypeAdapter(...)` where pydantic defers the error to the first validation.
- **Classes are not memoised across adapters.** `TypeAdapter(Foo)` builds a fresh validator each time;
    pydantic caches the class's core schema.

## Validation

`validate_python(value, strict=None)` follows pydantic-core's lax and strict rules for every type above:
`'1'`, `1.0`, `b'1'`, `True` and `Decimal('1')` are accepted for `int`, `'yes'` / `'off'` / `0` / `1` for
`bool`, ISO 8601 text, timestamps and dates for `datetime`, and so on, with the same error types and
messages.
A dataclass instance passes through unchanged; a dict builds a new instance, calling `__post_init__`.
`validate_json(data, strict=None)` takes a `str` or `bytes` document (any other type is a `json_type`
error, a document that is not JSON a `json_invalid` one, including one nested more than 200 non-empty
containers deep) and applies pydantic-core's JSON rules: a string is accepted for every type that reads
text, a JSON number is a `float` in both modes while the other number and boolean conversions (`1` for
`bool`, `1.0` for `int`) are lax only, object keys are text whatever `strict` says, a key given twice
keeps its last value, and the document is streamed through the validators without building it first, so
an error names the same input pydantic would show.
Divergences:

- **Aware results carry `datetime.timezone` objects.** A parsed `'...Z'` or `'...+01:00'` gives
    `tzinfo=datetime.timezone.utc` or `timezone(timedelta(seconds=3600))`, where pydantic attaches its own
    `TzInfo` class; the offsets and comparisons agree, the reprs do not.
- **Sequences accepted for `list` are `list`, `tuple`, `set`, `frozenset` and `deque`**, plus a namedtuple
    for `tuple`; pydantic also accepts any other non-string iterable (a generator, a `dict_keys` view).
- **An input mapping must be a `dict`.** pydantic accepts any `Mapping` for `dict[K, V]` and dataclass fields.
- **`__post_init__` and `default_factory` cannot suspend.** They run through the synchronous call path, so an
    external or OS call inside one raises `NotImplementedError` (see [dataclasses.md](dataclasses.md)).
- **A `ValueError` raised in `__post_init__` propagates as itself**, where pydantic wraps it in a
    `ValidationError` of type `value_error`.
- **A dataclass field with `init=False` does not exist in Monty**, so every field is read from the input.
- **A `dict[K, V]` input that changes size during validation raises `RuntimeError`**, as a `for` loop over
    it would, where pydantic-core's iterator fails with a `PanicException`. Dataclass fields are validated in
    declaration order, as in pydantic, but each entry is read when the walk over the dict reaches it: a key a
    `default_factory` adds, changes or removes is seen as pydantic sees it, unless the dict lists that field
    before the factory's field, in which case the value read before the factory ran is validated.
- **A JSON object that repeats a dataclass field's key can validate more than one occurrence**: an
    occurrence read when the field was next in declaration order is validated at once, so its nested
    `__post_init__` can run even though a later occurrence replaces it; pydantic validates only the last.
- **Set results iterate in insertion order**, as Monty's sets do, where CPython's order depends on hashes.
- **The recursion guard is keyed on identity**: a list that contains itself reports `recursion_loop`,
    like pydantic, but a deeply nested input hits Monty's recursion limit (`RecursionError`) rather than
    pydantic's 255-level `recursion_loop` error.

## Serialisation

`dump_python(instance, mode='python' | 'json', exclude_none=)` and
`dump_json(instance, indent=, ensure_ascii=, exclude_none=)` follow pydantic-core's serializers for the types
above: Python mode copies containers and keeps every value, JSON mode writes temporal values as ISO text, a
`Decimal` as `str()`, `bytes` as UTF-8 text, sets and tuples as lists, dict keys as strings and a non-finite
float as `None` (`null` in `dump_json`), a dataclass is a dict of its fields in order, and a value of another
type than its hint falls back to its own type's form, as pydantic does after its warning.
Divergences:

- **No `PydanticSerializationUnexpectedValue` warning.** A value that does not match its hint is serialized
    silently; pydantic emits a `UserWarning`, and `warnings='error'` raises there but not in Monty.
- **Dataclasses are guarded more widely in Python mode.** `dump_python(mode='python')` guards every dataclass
    written through its declared type against cycles and nesting past 255 levels; pydantic guards only
    recursive types, so a non-recursive dataclass inside deep `Any` nesting can hit the limit a little sooner.
- **A frozen-dataclass dict key** raises `Unable to serialize unknown type` in `dump_json` and
    `dump_python(mode='json')`, where pydantic writes `str(key)`.
- **The unknown-type message uses Monty's class name**: `Unable to serialize unknown type: <class 'Foo'>`
    where pydantic prints `<class '__main__.Foo'>`.

## `ValidationError`

`str(exc)`, `repr(exc)`, `exc.title`, `exc.error_count()`, `exc.errors(include_url=, include_context=, include_input=)`,
`exc.json(indent=, include_url=, include_context=, include_input=)` and `ValidationError.from_exception_data(...)`
match pydantic, including the `For further information visit https://errors.pydantic.dev/2.13/v/...` lines.
Divergences:

- **Inputs in `errors()` are snapshots.** The exception stores a copy of each failing input: scalars,
    containers, temporal values and decimals come back as equal new objects, not the same ones, and an
    instance, function or other object with no snapshot form comes back as its `repr()` string; an exception
    input or `ctx` value comes back as an equal new exception.
    A `str` or `bytes` input of 1 MiB or more is checked against `max_memory` before it is copied, so it can
    raise `MemoryError` where pydantic keeps a reference.
    One exception's snapshots share a budget of about 32,000 nodes and 4 MiB of text; past it, the rest of each
    container and every later input is cut off as the string `'...'`.
- **`exc.args` is `()`**, as for a `ValidationError` pydantic raises; one built by `from_exception_data` also
    has `()`, where pydantic's is `(title, line_errors, input_type, hide_input)`.
- **An input's `__repr__` runs when the error is raised**, not when `str(exc)` is first read, because the
    exception stores a snapshot. A `__repr__` that raises gives `<unprintable X object>` in `str()`, `errors()`
    and `json()`; pydantic re-raises from `errors()` and writes `<Unserializable X object>` in `json()`.
- **The methods are call-only.** `exc.errors`, `exc.json`, `exc.error_count` and
    `ValidationError.from_exception_data` work only when called directly; reading one without calling it
    raises `AttributeError`, and `hasattr` returns `False` (see [builtins.md](builtins.md)).
- **An empty `ctx` is dropped**: `'ctx': {}` on a type that takes no context is left out of `errors()`, where
    pydantic reports `'ctx': {}`.
- **A non-`str` title raises `TypeError`** (`argument 'title': 'int' object is not an instance of 'str'`),
    where pydantic panics.
- **`ValidationError('msg')` builds a plain exception** with no line errors; its `errors()` raises
    `AttributeError`, where pydantic refuses the call with `TypeError`.
- **Unknown keys in a `from_exception_data` `ctx` are dropped**, where pydantic keeps them in `errors()`.
- **`exc.json()` writes a value with no JSON form** (an instance, a function, a custom exception subclass)
    as the `repr()` stored when the error was raised, where pydantic calls `str()` on it; the two differ only
    for a class that defines its own `__str__`.
- **`exc.json()` writes a float's shortest form**, as pydantic does, so `1e16` is `1e+16` either way; a
    non-finite float is `NaN` or `Infinity`, as in pydantic.
- **A `ValidationError` reaching the host keeps only its message.** The line errors stay in the worker:
    a Python host sees a `ValueError` carrying the rendered `str(exc)` (a `RuntimeError` for
    `PydanticUserError` and `PydanticSchemaGenerationError`), with no `errors()`, `title` or pydantic class,
    and a JavaScript host sees the dotted class name and the message.
    In the other direction, a pydantic `ValidationError` raised by a host function arrives as a plain
    `ValueError`, so `except ValidationError` in the sandbox does not catch it.
    An `ExternalExceptionData` naming `pydantic_core._pydantic_core.ValidationError` builds one with no line
    errors, like `ValidationError('msg')` above.
- **`PydanticUserError(message, *, code)`** stores `message` and `code` as `str()` of what was passed, so
    `PydanticUserError(1, code=None).message` is `'1'`. The positional-overflow error does not count `self`:
    `PydanticErrorMixin.__init__() takes 1 positional argument but 2 ...`, where pydantic says `takes 2 ... but 3`.
