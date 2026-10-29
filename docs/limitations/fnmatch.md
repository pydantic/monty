# `fnmatch` module

`fnmatch`, `fnmatchcase`, `filter` and `translate` are available, with CPython's signatures and errors.

- **`fnmatch()` is case-sensitive.** The sandbox's `os.path` is `posixpath`, whose `normcase` leaves names
    unchanged, so `fnmatch()` behaves as `fnmatchcase()` on every host, where CPython on Windows folds case.
- **Patterns are matched directly, not through `re`.** `translate()` returns the regex text CPython 3.14 writes
    (`(?s:...)\z`, with `(?>.*?x)` atomic groups), but matching interprets the pattern itself, so a pattern never
    compiles to a `re.Pattern` and the module has no `_compile_pattern` cache.
- **The functions carry no attributes and the module has no `__all__`.** `fnmatch.fnmatch.__name__`, `__module__`,
    `__qualname__` and `__doc__` raise `AttributeError`, as on every built-in Monty module function.
