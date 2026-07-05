This directory contains custom type stubs where types available in Monty differ from those in the standard library.

These files are copied into `vendor/typeshed/stdlib` by `update.py`, overriding
or supplementing upstream typeshed files so type checking reflects Monty's
actual runtime surface.
