# Vendored types for a very minimal subset of the CPython stdlib

Copied originally from <https://github.com/astral-sh/ruff/tree/main/crates/ty_vendored> but only parts of
<https://github.com/python/typeshed/blob/main/stdlib/builtins.pyi> are kept, since those are the
only functions supported from the stdlib.

the `vendor/typeshed` directory is updated by calling `make update-typeshed` which calls the `update.py` script in this directory.

THEREFORE FILES IN THE `vendor/typeshed` DIRECTORY SHOULD NOT BE EDITED MANUALLY.
