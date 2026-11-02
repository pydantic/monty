# `pydantic` module

Monty bundles a native subset of [pydantic](https://docs.pydantic.dev/) 2.13: `TypeAdapter`
validates sandbox values against type hints with pydantic's own error messages, and
`ValidationError` reports them as pydantic does.
Validation runs in Rust and never enters the sandbox except to call a dataclass's `default_factory`
or `__post_init__`.
The messages, `errors()` output and `str()` format are copied from pydantic-core, and the
`url` entries name pydantic 2.13.

## Names defined

`TypeAdapter`, `Field`, `ConfigDict`, `ValidationError`, `PydanticUserError`, `PydanticSchemaGenerationError`,
`VERSION`, `__version__`.

## Not implemented

- `BaseModel`, `field_validator`, `model_validator`, `computed_field` and the rest of
    the package raise `AttributeError` or `ImportError`.
    `pydantic.dataclasses` defines `dataclass`, `is_pydantic_dataclass` and `Field`; `rebuild_dataclass` and the
    rest of that module raise `AttributeError`.
- `TypeAdapter.validate_strings()` raises `NotImplementedError`.
- `dump_python(..., include=, exclude=, fallback=, polymorphic_serialization=, context=)`, the same on
    `dump_json`, and `exclude_unset=True`, `exclude_defaults=True`, `exclude_computed_fields=True` or
    `serialize_as_any=True` raise `NotImplementedError` naming the argument; `mode`, `indent`, `ensure_ascii`
    and `exclude_none` are supported, `by_alias`, `round_trip` and `warnings` are accepted and have no effect.
- `pydantic_core` is not importable, so `PydanticSerializationError` cannot be named; it is a `ValueError`.
- `TypeAdapter(type, module=...)` raises `NotImplementedError`; `_parent_depth` is accepted and ignored.
- `validate_python(..., extra=, from_attributes=, context=, by_alias=, by_name=)`,
    `validate_json(..., extra=, context=, by_alias=, by_name=)` and `experimental_allow_partial=True` raise
    `NotImplementedError` naming the argument; `strict` is supported.
- `Field()` stores `default`, `default_factory`, `alias`, `validation_alias`, `serialization_alias`, `title`,
    `description`, `examples`, `exclude`, `frozen`, `json_schema_extra`, `strict`, `coerce_numbers_to_str`, `gt`,
    `ge`, `lt`, `le`, `multiple_of`, `min_length`, `max_length` and `pattern`; its other arguments raise
    `NotImplementedError` naming the argument. `validation_alias` takes only a `str`, never an `AliasPath` or
    `AliasChoices`. `frozen` and `exclude` are stored and `exclude` is honoured by `dump_*`; `frozen` does nothing,
    as in pydantic's own dataclasses.
- `ConfigDict` keys read: `strict`, `extra`, `populate_by_name`, `str_strip_whitespace`, `str_to_lower`,
    `str_to_upper`, `str_min_length`, `str_max_length`, `coerce_numbers_to_str` and `title`. pydantic's other keys
    raise `NotImplementedError` naming the key when a `TypeAdapter` or dataclass is built with them; a key pydantic
    does not know is ignored, as pydantic ignores it. A config reaches a plain type through
    `TypeAdapter(type, config=...)` and a dataclass through its `__pydantic_config__` attribute.
- `Field()` bounds (`gt`, `ge`, `lt`, `le`) on a `date`, `time`, `datetime` or `timedelta` raise
    `NotImplementedError` when the adapter is built.
- **Signature errors do not count `self`.** `TypeAdapter.validate_python(1, 2)` reports
    `takes 1 positional argument but 2 were given` and `validate_python(object=1)` an unexpected keyword
    argument, where pydantic's methods count the instance (`takes 2 positional arguments but 3 were given`)
    and name positional-only arguments passed as keywords.
- **`Annotated[T, ...]` reads only `Field(...)` metadata.** Other metadata objects (`annotated_types.Gt`,
    `pydantic.AfterValidator`, ...) are ignored.
- **A `Field()` argument of the wrong type raises when `Field()` is called**: `Field(alias=1)` and
    `Field(min_length=-1)` raise `TypeError` / `ValueError` at once, where pydantic stores the value and fails when the
    adapter is built. Unknown keywords join `json_schema_extra` without pydantic's deprecation warning.
- **Schema-building errors are plain exceptions.** A bound of the wrong kind (`Annotated[int, Field(gt=1.5)]`)
    raises `ValueError` and an invalid `pattern` raises `re.PatternError` from `TypeAdapter(...)`, where pydantic
    wraps them in `pydantic_core.SchemaError`.
- **A constraint the type cannot apply is dropped**: `Annotated[int, Field(min_length=1)]` validates as `int`, where
    pydantic raises `TypeError` while validating.
- **`FieldInfo.default` raises `AttributeError` when no default was given**, where pydantic returns
    `PydanticUndefined`; `FieldInfo.metadata` and the other attributes pydantic computes are not available.
- **`FieldInfo.default_factory` prints as the factory's display name** in `repr()`, as pydantic's does, but a
    function other than a class or builtin prints its `repr()`.

## `TypeAdapter.json_schema()`

The schema is generated from the validators Monty built, following pydantic's `GenerateJsonSchema` for the types
`TypeAdapter` accepts: the keywords, titles, `$defs` and `$ref` placement, key order and `by_alias` default match
pydantic for those types.

- `union_format='primitive_type_union'` and a `schema_generator` other than `None` raise `NotImplementedError`
    naming the argument; `by_alias`, `ref_template` and `mode` are supported.
- **Argument errors are raised for every type**: a non-bool `by_alias` (`TypeError`), an unknown `mode`
    (`AssertionError`) and a non-string `ref_template` (`AttributeError`) fail before the schema is built, where
    pydantic raises only when a field or `$ref` reads the argument, so `TypeAdapter(int).json_schema(by_alias='x')`
    succeeds there.
- **Two dataclasses sharing a name get `$defs` keys `Name` and `Name_2`** (then `Name_3`, ...), in order of first
    reference, where pydantic qualifies both with their module path (`__main____Name`); sandbox classes have no
    `__module__` or `__qualname__`.
- A dataclass field's `Annotated` metadata other than `Field(...)` is ignored, as in validation, so
    `annotated_types` constraints and `WithJsonSchema` do not reach the schema.
- A namedtuple's fields are untyped (see [collections.md](collections.md)), so its `prefixItems` carry only titles
    and defaults.

## `pydantic.dataclasses.dataclass`

The decorator applies the stdlib `@dataclass` (see [dataclasses.md](dataclasses.md)), stores `config` as
`__pydantic_config__` and a `TypeAdapter` of the class as `__pydantic_validator__`, and replaces `__init__` with one
that validates its arguments.

- `init=True` raises `AssertionError` as pydantic does; `order`, `unsafe_hash`, `kw_only` and `slots` set away from
    their defaults and `repr=False` raise `NotImplementedError`, as the stdlib decorator's do, where pydantic honours
    them; `validate_on_init` is accepted and ignored.
- `config` must be a `ConfigDict` (a dict) or `None`; pydantic also accepts a class with the keys as attributes.
- **`__pydantic_validator__` is a `TypeAdapter`**, not a `pydantic_core.SchemaValidator`; it has no `validate_assignment`.
- **The `input` of a `missing` or `multiple_argument_values` error is a dict** of the arguments given, keyed by
    field name for positional ones, where pydantic shows an `ArgsKwargs(...)` object.
- **Decorating a non-class raises `TypeError`** (`dataclass() should be called on a class`), where pydantic fails with
    an `AttributeError` about `__bases__`.
- Assignment after construction is not validated, as in pydantic without `validate_assignment`.

## `validate_call`

The wrapper validates the call's arguments against the function's annotations as pydantic-core's `arguments`
schema does, then calls the function; `validate_return=True` validates the result and `config` takes a `ConfigDict`.

- **Only a `def` or `lambda` can be wrapped.** A `functools.partial` raises the `PydanticUserError` pydantic raises for
    other objects, and Monty has no method decorators (see [classes.md](classes.md)), so a method cannot be decorated
    in a class body.
- **The `ValidationError` title is the function's `__name__`**, where pydantic uses `__qualname__`
    (`outer.<locals>.inner`).
- **The `input` of a `missing_argument`, `missing_positional_only_argument` or `missing_keyword_only_argument` error
    is a dict** of the arguments given, positional ones keyed by parameter name, where pydantic shows an
    `ArgsKwargs(...)` object.
- `validate_return=True` on an `async def` raises `NotImplementedError`; without it an async function is wrapped and
    its arguments validated.
- **The wrapper has `__name__`, `__qualname__` and `raw_function`** and nothing else of a function's: no
    `__wrapped__`, `__doc__` or `__defaults__`.

## Type hints

`TypeAdapter` builds a validator for `int`, `float`, `str`, `bool`, `bytes`, `None`, `Any`, `object`,
`list[T]`, `set[T]`, `frozenset[T]`, `tuple[A, B]`, `tuple[T, ...]`, `dict[K, V]`, `X | Y`, `Optional[X]`,
`Literal[...]`, `datetime`, `date`, `time`, `timedelta`, `Decimal`, a stdlib `@dataclass` and a
`collections.namedtuple` class, at any nesting, including a dataclass whose fields name the class again.
The bare `list`, `set`, `frozenset`, `tuple` and `dict` take `Any` items.
Any other hint raises `PydanticSchemaGenerationError` with pydantic's message, and a generic with the wrong
number of arguments (`dict[int]`) the `TypeError` pydantic raises. Divergences:

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
    it would, where pydantic-core's iterator fails with a `PanicException`. Dataclass arguments read from
    a dict that a `default_factory` mutates are validated in both: a key the factory adds or changes is read
    when the walk reaches it, and a removed one is missing.
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
- **A cycle raises in Python mode too.** `dump_python` of a container that holds itself raises
    `ValueError: Circular reference detected (id repeated)`; pydantic returns a nested copy. `dump_json`
    raises pydantic's `PydanticSerializationError` for it.
- **The unknown-type message uses Monty's class name**: `Unable to serialize unknown type: <class 'Foo'>`
    where pydantic prints `<class '__main__.Foo'>`.

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
