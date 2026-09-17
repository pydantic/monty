---
name: review-usability
description: Check whether the common Python code an LLM would plausibly write still works on this branch, testing real cases in ./playground against CPython. Use to find behaviour that diverges from CPython or trips up ordinary idiomatic code.
---

# Usability review

Monty exists so LLMs can write Python that calls tools. Real usage is therefore the
most common patterns, not exotic corners — and a divergence in a common pattern is the
worst kind of bug, because the model has no way to know it must write something else.

Think hard on this one.

```bash
git diff origin/main...HEAD
```

1. For each feature the branch touches, list the idioms a model reaches for first — the
    obvious method, argument form, combination with another builtin. Include ones the
    branch does *not* handle; that's where the gaps are.

1. Write real test files in `playground/` (see `python-playground`), named recognisably.

1. Run each under both and diff:

    ```bash
    uv run playground/test_thing.py        # CPython
    cargo run -- playground/test_thing.py  # Monty
    ```

1. Prioritise **silent divergence** — same code, different result — over a clean
    `AttributeError`. A missing feature that raises is recoverable; a wrong answer isn't.

An undocumented divergence is also a `./limitations/` finding.

## Report

Per divergence: the code, CPython's output, Monty's output, how likely a model is to
write it. Then unsupported-but-common idioms with the error the user sees, and briefly
what worked — it bounds the review. Leave the playground files in place.

Report only, unless the user asks for fixes.
