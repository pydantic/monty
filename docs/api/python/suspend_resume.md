---
# Show individual methods/attributes (h5) in the docs site's on-page TOC.
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 5
---

# Suspend & Resume

What `feed_start` (and each `resume` / `resume_auto`) yields — a completed run, or a suspension to answer — and the
`ExternalResult` shapes those answers take.
See [snapshots](../../snapshots.md) for the concepts.

::: pydantic_monty
    options:
        members:
            - MontyComplete
            - FunctionSnapshot
            - NameLookupSnapshot
            - FutureSnapshot
            - AsyncFunctionSnapshot
            - AsyncNameLookupSnapshot
            - AsyncFutureSnapshot
            - ExternalReturnValue
            - ExternalException
            - ExternalExceptionData
            - ExternalFuture
            - SyncSnapshot
            - AsyncSnapshot
            - ExternalResult
            - ExternalSettledResult
