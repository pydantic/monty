# `annotationlib` module

`annotationlib` provides `Format`, `ForwardRef` and `get_annotations()`, the pieces
[`typing.get_type_hints()`](typing.md#get_type_hints) is built on.
`typing.ForwardRef` is the same type.

## Not implemented

- `call_annotate_function`, `call_evaluate_function` and `get_annotate_from_class_namespace`:
    there are no `__annotate__` functions, since annotations are stored stringized (see [typing.md](typing.md)).
- `annotations_to_string` and `type_repr`.

## `Format`

`Format.VALUE`, `Format.VALUE_WITH_FAKE_GLOBALS`, `Format.FORWARDREF` and `Format.STRING` are the ints
`1` to `4`, so `format=Format.STRING` and `format=4` agree, as in CPython.
Divergences:

- **`Format` is a plain class, not an `enum.IntEnum`.**
    `Format.STRING` is the `int` `4`: `repr(Format.STRING)` is `4` rather than `<Format.STRING: 4>`,
    `Format.STRING.name` and `.value` raise `AttributeError`, `Format(4)` raises `TypeError`, and `repr(Format)` is
    `<class 'Format'>` rather than `<enum 'Format'>`.

## `ForwardRef`

`ForwardRef(arg, *, module=None, owner=None, is_argument=True, is_class=False)` keeps an annotation as source
text, with `__forward_arg__`, `__forward_module__`, `__forward_is_argument__`, `__forward_is_class__`, `__owner__`
and `__resolved_str__`, equality and hashing, `|`, and
`evaluate(*, globals=None, locals=None, type_params=None, owner=None, format=Format.VALUE)`.
As in CPython, a reference made while evaluating an annotation remembers the globals it was evaluated against and
equals no constructed reference, and `evaluate()` without `globals` or an `owner` sees builtins only.
Divergences:

- **`type_params` is accepted and ignored**: Monty has no type parameters.
- **`__globals__`, `__cell__`, `__extra_names__`, `__code__` and `__ast_node__` raise `AttributeError`.**
- **`__forward_arg__` is the resolved text** of a reference an evaluation built, and a class is written by its bare
    name; see [typing.md](typing.md#get_type_hints).
- **A class owner supplies the session globals** to `evaluate()`, where CPython reads the class's module from
    `sys.modules`.
- **`NotImplementedError` for an unsupported `format` carries the text**, so `repr(exc)` is
    `NotImplementedError('2')` where CPython's is `NotImplementedError(2)`; `str(exc)` agrees.
- **Arity errors do not count `self`**: `ForwardRef('a', 'b')` raises
    `TypeError: ForwardRef.__init__() takes 1 positional argument but 2 were given` where CPython counts 2 and 3.
- **`copy.deepcopy()` shares a reference** rather than rebuilding it.

## `get_annotations`

`get_annotations(obj, *, globals=None, locals=None, eval_str=False, format=Format.VALUE)` returns `obj`'s
annotations evaluated where they were written, or as stored under `from __future__ import annotations` or in
`STRING` format; a string stays a string unless `eval_str=True`.
It shares the [`get_type_hints`](typing.md#get_type_hints) divergences on function scopes, `exec()` classes,
re-evaluation, modules, tracebacks and host objects, and adds:

- **`globals` / `locals` must be a dict or `None`.**
    Anything else raises `TypeError: get_annotations() globals must be a dict, not int`, where CPython fails later
    inside `eval()`.
- **Host functions and classes** answer with the host's `typing.get_type_hints()`, so their forward references come
    back resolved and `None` as `NoneType`; `eval_str` has no effect on them.
