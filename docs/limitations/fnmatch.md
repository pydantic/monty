# `fnmatch` module

`fnmatch`, `fnmatchcase`, `filter` and `translate` are available, with CPython's signatures and errors.

- **`fnmatch()` and `filter()` are case-sensitive and never normalise separators.** The sandbox's `os.path` is
    `posixpath`, whose `normcase` leaves names unchanged, so `fnmatch()` behaves as `fnmatchcase()` and `'a/b'` does
    not match `'a\\b'` on every host, where CPython on Windows folds case and turns `/` into `\`. For the same
    reason a non-string name in `filter()` reaches the matcher and raises
    `TypeError: expected string or bytes-like object, got 'int'`, as CPython does on POSIX; Windows CPython raises
    `os.fspath`'s `expected str, bytes or os.PathLike object, not int` first.
- **The module has no `__all__`**, and its functions carry no attributes, as on every built-in module
    (see [modules.md](modules.md)).
