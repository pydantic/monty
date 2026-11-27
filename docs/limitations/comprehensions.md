# Comprehensions and generator expressions

Monty inlines list, set, and dict comprehensions into the surrounding code
object. The user-visible behaviour follows
[PEP 709](https://peps.python.org/pep-0709/): inlined comprehensions, no
synthetic frame in tracebacks, comprehension targets do not leak into the
enclosing scope.
For generator-specific restrictions, see [generators.md](generators.md).

## Divergences from CPython

- **A comprehension target must be a name.** CPython accepts
    `[i for obj.x in xs]` and `[i for d[k] in xs]`; Monty raises
    `SyntaxError: comprehension target must be a name, not an attribute` (or
    `not a subscript`). A comprehension's targets live in operand-stack slots,
    which a store to an object cannot reach. Attribute and subscript targets
    work in every other unpacking position.
- **`locals()` omits active loop targets** in both comprehensions and generator expressions.
    CPython includes them in the body.
- **Maximum number of `for` clauses.** Monty caps a single comprehension at
    255 `for` clauses; exceeding this raises
    `SyntaxError: comprehension has too many nested clauses (N); maximum is 255`.
    List, set and dict comprehensions can hit a tighter
    `SyntaxError: comprehension target + iterator count exceeds u8 depth operand` before that point.
    CPython has no equivalent compile-time limit. The cap bounds compiler
    recursion depth on attacker-controlled source.
