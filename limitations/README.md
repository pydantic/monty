# Limitations

This directory is the single source of truth for how Monty diverges from CPython.
Module-level docstrings and inline comments are not
sufficient on their own — divergences live here so users and contributors
can find them in one place.

Every pull request that adds, changes, or removes user-visible behavior
MUST land (or update) a markdown document here describing how the feature
diverges from CPython and what subset of the CPython surface area Monty
actually implements.

One file per feature, named after the builtin / module / construct it
covers (e.g. `open.md`, `asyncio.md`, `re.md`). Add new sections to an
existing file when the feature is already documented; only create a new
file when there is no fit.

Keep entries concise but comprehensive — list every known divergence,
including ones that "feel obvious". A divergence that is not written down
is one that future readers (and future Claude) will assume does not exist.
Reviewers should reject PRs that change behavior without updating this
directory.

Structure each file around what a Python user would actually try:

- Arguments/options that are rejected or ignored.
- Methods/attributes that raise `AttributeError`.
- Behaviour that differs from CPython even when the API exists.
- Error types / messages that differ from CPython.

Avoid implementation detail unless it explains a user-visible quirk.
