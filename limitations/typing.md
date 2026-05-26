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
- Any introspection of annotations: `__annotations__` is not populated on
  functions or modules, so libraries that read it (Pydantic, attrs,
  inspect-based code) cannot run.

If you need real type validation, do it on the *host* side around the
sandbox boundary.
