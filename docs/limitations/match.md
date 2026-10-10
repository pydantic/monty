# `match` statement (structural pattern matching)

Monty implements PEP 634 `match` statements: value, singleton, capture, wildcard,
sequence (with one `*rest`), mapping (with `**rest`), class, `as` and `|` patterns,
with guards.
Names are bound only after the whole pattern matches, before the guard runs, as in
CPython, so a case whose pattern fails part-way leaves no bindings behind and a case
whose guard fails leaves them all.
The compile-time `SyntaxError`s match CPython's wording and location: duplicate
captures, alternatives binding different names, an irrefutable pattern before the
last case, duplicate constant mapping keys, repeated class-pattern attributes and
two starred names.
`@dataclass` classes get `__match_args__` (the field names, in order) and
`collections.namedtuple` classes expose it too, so positional class patterns work on
both.

## Class patterns

- **`isinstance` has no inheritance.** A class pattern on a user-defined or host class
    matches instances of exactly that class; see [classes.md](classes.md).
- **Host-backed objects.** A class pattern reads attributes with the normal attribute
    lookup. On a host class instance an attribute the host has not already sent would
    need a round trip to the host, which cannot happen mid-pattern, so it raises
    `NotImplementedError: class pattern attribute 'x' requires a host lookup, which match statements do not support`
    instead of matching or failing.
- **Host classes only have the `__match_args__` the host sent** as a class
    attribute. Without one, a positional sub-pattern raises
    `TypeError: Cls() accepts 0 positional sub-patterns (1 given)` even when the
    host-side class is a dataclass; keyword sub-patterns (`case Cls(x=0)`) work.

## Parse-time checks

- **Duplicate mapping keys are only detected for literals**, optionally negated
    (`1`, `-1`, `'a'`, `None`, `b'x'`, `1.5`), compared by Python equality so `1`,
    `1.0` and `True` collide.
    CPython folds constant expressions first, so it also rejects `{1 + 0j: a, 1: b}`;
    Monty compiles it and the second key simply never matches a distinct entry.
- **A pattern after `**rest`** reports `SyntaxError: Pattern cannot follow a double star pattern`
    (Ruff's wording) where CPython says `invalid syntax`.
- **Bytecode operand limits.** A sequence or class pattern with more than 255
    sub-patterns, a mapping pattern with more than 65535 keys, or a pattern that
    captures more than 255 names raises `SyntaxError: too many sub-patterns in ... pattern`
    (or `too many names in pattern`) at compile time; CPython has no such limit.
