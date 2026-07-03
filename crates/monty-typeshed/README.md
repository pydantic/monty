# Vendored types for a very minimal subset of the CPython stdlib

Copied originally from <https://github.com/astral-sh/ruff/tree/main/crates/ty_vendored>.
Monty vendors only the stdlib modules it supports, and custom stubs in
`custom/` strip or replace upstream typeshed files where Monty's runtime
surface is intentionally smaller than CPython's.

The `vendor/typeshed` directory is updated by calling `make update-typeshed` which calls the `update.py` script in this directory.

See <https://github.com/pydantic/monty> for more information on the project.

THEREFORE FILES IN THE `vendor/typeshed` DIRECTORY SHOULD NOT BE EDITED MANUALLY.
