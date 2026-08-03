---
name: review-usability
description: Check whether common Python an LLM would plausibly write still works on this branch, testing real cases in ./playground against CPython. Use to find behaviour that diverges from CPython or trips up ordinary idiomatic code.
---

# Usability review

Monty exists so LLMs can write Python that calls tools and performs actions. Almost all
real usage will therefore be the most well-known, most common patterns — not exotic
corners of the language. A divergence from CPython in a common pattern is the most
destructive kind of bug, because the model has no way to know it needs to write
something else.

Think hard on this one.

## Instructions

Compare the branch against its merge base:

```bash
git fetch origin main
git diff origin/main...HEAD
```

Then work out where an LLM would plausibly write code that does not work here:

1. For each feature the branch touches, list the idioms a model would reach for
   first — the obvious method, the obvious argument form, the obvious combination
   with another builtin. Include the ones the branch does *not* explicitly handle;
   those are where the gaps are.
2. Write real test files in `playground/` exercising them (see the
   `python-playground` skill). Give them recognisable names.
3. Run each under both interpreters and diff the output:

   ```bash
   uv run playground/test_thing.py      # CPython
   cargo run -- playground/test_thing.py  # Monty
   ```

4. Prioritise **silent behavioural divergence** — same code, different result — over
   a clean `AttributeError` or `NotImplementedError`. A missing feature that raises is
   recoverable; a wrong answer is not.

Also check `./limitations/` covers each divergence you find. An undocumented one is a
finding in its own right.

## Report

Write a concise report in your response:

- Each divergence: the code, CPython's output, Monty's output, and how likely a model
  is to write it.
- Each unsupported-but-common idiom, with the error the user actually sees.
- What you tested that worked, briefly — it bounds the review.

Leave the playground files in place so the cases can be re-run.

Report only, unless the user asks for fixes.
