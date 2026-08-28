---
include:
  - "crates/**/*.rs"
---

Monty exists so LLMs can write ordinary Python; silent divergence from CPython
in a common idiom is the worst failure, because the model has no signal to write
something else. When a change alters a builtin, dunder, method, or exception's
result, type, message, or attributes, verify it matches CPython. A change that
diverges (or widens an existing divergence) needs a matching entry under
`limitations/`; a new or widened divergence with no `limitations/` entry is a
finding. Prefer flagging a plausibly-silent wrong result over a clean
`AttributeError`, which is recoverable. This is diff-level review only -- the
executable run-both-and-diff check against CPython is the `review-usability`
skill and is out of scope here; do not claim a divergence you cannot see in the
change.
