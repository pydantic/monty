# Limitations

How Monty diverges from CPython. Docstrings and inline comments do not count:
the divergence has to be written down here.

Every pull request that adds, changes, or removes user-visible behavior MUST
add (or update) a markdown document here describing how the feature diverges
from CPython, and what subset of the CPython surface Monty implements.

One file per feature, named after the builtin, module, or construct it covers
(`open.md`, `asyncio.md`, `re.md`). Add a section to an existing file when the
feature is already documented; create a new file only when there is no fit.

List every known divergence, including the ones that feel obvious. Reviewers
should reject PRs that change behavior without updating this directory.

Structure each file around what a Python user would actually try:

- Arguments/options that are rejected or ignored.
- Methods/attributes that raise `AttributeError`.
- Behaviour that differs from CPython even when the API exists.
- Error types / messages that differ from CPython.

Avoid implementation detail unless it explains a user-visible quirk.

## Relationship to `docs/`

`../docs/` is the docs site: conceptual and how-to material describing the *shape* of
what Monty implements. It links here rather than restating divergences, and this
directory never duplicates it. A divergence belongs here, not in a `docs/` page.

When a change alters the shape of the subset — a stdlib module becomes importable, a
parse-time rejection lands or is lifted — update `../docs/python-subset.md` and the
root `README.md` bullets as well as the file here. See "Documentation surfaces that must
stay in sync" in `../CLAUDE.md`.
