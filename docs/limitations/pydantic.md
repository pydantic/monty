# `pydantic` module

Monty bundles a native subset of [pydantic](https://docs.pydantic.dev/) 2.13: `TypeAdapter`
validates sandbox values against type hints with pydantic's own error messages, and
`ValidationError` reports them as pydantic does.
Validation runs in Rust and never enters the sandbox except to call a dataclass's `default_factory`
or `__post_init__`.
The messages, `errors()` output and `str()` format are copied from pydantic-core, and the
`url` entries name pydantic 2.13.

## Names defined

`TypeAdapter`, `ValidationError`, `PydanticUserError`, `PydanticSchemaGenerationError`, `VERSION`.

## Not implemented

- `BaseModel`, `Field`, `ConfigDict`, `validate_call`, `pydantic.dataclasses`, `field_validator`,
    `model_validator`, `computed_field`, `TypeAdapter.json_schema()` and the rest of the package raise
    `AttributeError` or `ImportError`.
- `TypeAdapter.validate_strings()` raises `NotImplementedError`.
- `TypeAdapter(type, config=...)` and `TypeAdapter(type, module=...)` raise `NotImplementedError`;
    `_parent_depth` is accepted and ignored.
- `validate_python(..., extra=, from_attributes=, context=, by_alias=, by_name=)`,
    `validate_json(..., extra=, context=, by_alias=, by_name=)` and `experimental_allow_partial=True` raise
    `NotImplementedError` naming the argument; `strict` is supported.
- **Signature errors do not count `self`.** `TypeAdapter.validate_python(1, 2)` reports
    `takes 1 positional argument but 2 were given` and `validate_python(object=1)` an unexpected keyword
    argument, where pydantic's methods count the instance (`takes 2 positional arguments but 3 were given`)
    and name positional-only arguments passed as keywords.
- `Annotated[T, ...]` validates as `T`: the metadata is ignored until `Field` exists.

## Type hints

`TypeAdapter` builds a validator for `int`, `float`, `str`, `bool`, `bytes`, `None`, `Any`, `object`,
`list[T]`, `set[T]`, `frozenset[T]`, `tuple[A, B]`, `tuple[T, ...]`, `dict[K, V]`, `X | Y`, `Optional[X]`,
`Literal[...]`, `datetime`, `date`, `time`, `timedelta`, `Decimal`, a stdlib `@dataclass` and a
`collections.namedtuple` class, at any nesting, including a dataclass whose fields name the class again.
The bare `list`, `set`, `frozenset`, `tuple` and `dict` take `Any` items.
Anything else raises `PydanticSchemaGenerationError` with pydantic's message. Divergences:

- **A string hint is not evaluated.** `TypeAdapter('int')` and `TypeAdapter(list['Foo'])` raise
    `PydanticSchemaGenerationError` where pydantic resolves the name in the caller's module.
    A forward reference inside a dataclass field is resolved, through `typing.get_type_hints`.
- **`type[X]`** raises `PydanticSchemaGenerationError` where pydantic builds an `is-subclass` validator.
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
text, numbers and booleans convert in lax mode only, object keys are text whatever `strict` says, a key
given twice keeps its last value, and the document is streamed through the validators without building
it first, so an error names the same input pydantic would show.
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
- **Set results iterate in insertion order**, as Monty's sets do, where CPython's order depends on hashes.
- **The recursion guard is keyed on identity**: a list that contains itself reports `recursion_loop`,
    like pydantic, but a deeply nested input hits Monty's recursion limit (`RecursionError`) rather than
    pydantic's 255-level `recursion_loop` error.

## `ValidationError`

`str(exc)`, `repr(exc)`, `exc.title`, `exc.error_count()`, `exc.errors(include_url=, include_context=, include_input=)`,
`exc.json(indent=, include_url=, include_context=, include_input=)` and `ValidationError.from_exception_data(...)`
match pydantic, including the `For further information visit https://errors.pydantic.dev/2.13/v/...` lines.
Divergences:

- **Inputs in `errors()` are snapshots.** The exception stores a copy of each failing input: scalars,
    containers, temporal values and decimals come back as equal new objects, not the same ones, and an
    instance, function or other object with no snapshot form comes back as its `repr()` string.
    A snapshot is also capped at a few thousand nodes; the rest of a larger input degrades to its repr.
- **`exc.args` is `(str(exc),)`**, where pydantic's is `(title, line_errors, input_type, hide_input)`.
- **`ValidationError('msg')` builds a plain exception** with no line errors; its `errors()` raises
    `AttributeError`, where pydantic refuses the call with `TypeError`.
- **Unknown keys in a `from_exception_data` `ctx` are dropped**, where pydantic keeps them in `errors()`.
- **`exc.json()` writes a float's shortest form**, as pydantic does, so `1e16` is `1e+16` either way; a
    non-finite float is `NaN` or `Infinity`, as in pydantic.
- **`PydanticUserError` messages are plain.** `str(PydanticSchemaGenerationError('x'))` is `'x'`, where
    pydantic appends the documentation URL for the error code; the errors Monty raises itself include it.
