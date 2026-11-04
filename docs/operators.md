# Python operators

## Multiplication dispatch

For `a * b`, Monty tries the left operand's multiplication implementation, then the right operand's reflected
implementation (`__rmul__`) if the first is absent or returns `NotImplemented`.
User instances of the same class skip the reflected retry.
Exceptions propagate immediately.

Builtin sequence repetition is handled after numeric methods: for `sequence * obj`, a user-defined `obj.__rmul__`
runs before `obj.__index__` is used as a repetition count.
If `__rmul__` is absent or returns `NotImplemented`, repetition accepts an integer or calls `__index__`.
If neither is supported, multiplication raises `TypeError`; a declined `__rmul__` is not called again.
This applies to lists, deques, tuples, named tuples, strings and bytes.

List/deque `*=` uses the same ordering: a result from `__rmul__` replaces the assigned value, while native repetition
mutates the existing container so aliases observe the change.
Other types use binary multiplication as the augmented-assignment fallback.
For unsupported user-defined in-place dunders, see the
[class limitations](limitations/classes.md#what-does-not-exist-for-user-code).
