---
# Show individual methods/attributes (h5) in the docs site's on-page TOC.
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 5
---

# Pools

The execution surface of `pydantic_monty`: a `Monty` or `AsyncMonty` pool of worker subprocesses, checked out one
session at a time, plus the [resource limits](../../resource-limits.md) and `print()` sinks sessions are configured
with.
Install as [`pydantic-monty`](https://pypi.org/project/pydantic-monty/) — see the
[Python quickstart](../../quickstart/python.md).

::: pydantic_monty
    options:
        members:
            - Monty
            - MontySession
            - AsyncMonty
            - AsyncMontySession
            - ResourceLimits
            - CollectStreams
            - CollectString
            - TypeCheckFormat
            - PrintCallback
