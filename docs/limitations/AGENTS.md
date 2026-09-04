# Writing limitations pages

This file is for contributors and agents editing this directory.
It is excluded from the docs build (`exclude_docs` in `mkdocs.yml`, `exclude` in the unified-docs sync), so nothing
here is published.

## What goes here

Every pull request that adds, changes, or removes user-visible behavior MUST add (or update) a page here describing
how the feature diverges from CPython, and what subset of the CPython surface Monty implements.
Docstrings and inline comments do not count: the divergence has to be written down here.

One file per feature, named after the builtin, module, or construct it covers (`open.md`, `asyncio.md`, `re.md`).
Add a section to an existing file when the feature is already documented; create a new file only when there is no
fit.
A new file also needs an entry in the Limitations section of the `mkdocs.yml` nav, which orders these pages on the
site.
Keep that section flat: unified-docs turns nested nav groups into subdirectories, which would break the relative
links between pages.

Structure each file around what a Python user would actually try:

- Arguments/options that are rejected or ignored.
- Methods/attributes that raise `AttributeError`.
- Behaviour that differs from CPython even when the API exists.
- Error types / messages that differ from CPython.

Avoid implementation detail unless it explains a user-visible quirk.
Only divergences belong here; behaviour that matches CPython is not recorded.

## Links and snippets

Link between pages with relative markdown links (`see [classes.md](classes.md)`) rather than naming the file in
prose, so the reference works on the site as well as in the repository.
`mkdocs build --strict` fails on a link to a page that does not exist.

Python snippets on these pages are sandbox-side code, so they are marked ```` ```python test="skip" ````;
`make test-docs` would otherwise run them under CPython.
Skipped snippets are still formatted and linted, so they need imports and definitions for every name they use.

## Relationship to the rest of the docs

`index.md` is the one page here most users read: the shape of the subset, linking to the other pages for the
detail.
The pages outside this directory are conceptual and how-to material.
They link here rather than restating divergences, and these pages never duplicate them.
A divergence belongs here, not in a concept page.

When a change alters the shape of the subset — a stdlib module becomes importable, a parse-time rejection lands or
is lifted — update `index.md` and the root `README.md` bullets as well as the page for the feature.
See "Documentation surfaces that must stay in sync" in the repository's `CLAUDE.md`.
