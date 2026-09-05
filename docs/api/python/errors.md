---
# Show individual methods/attributes (h5) in the docs site's on-page TOC.
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 5
---

# Errors

Every exception `pydantic_monty` raises, plus the traceback `Frame` they carry and the `ExcType` names accepted when
answering a snapshot with an exception.

::: pydantic_monty
    options:
        members:
            - MontyError
            - MontySyntaxError
            - MontyTypingError
            - MontyRuntimeError
            - MontyConversionError
            - MontyCrashedError
            - MontyDisconnectError
            - MontyShutdown
            - MontyCallbackCleanupError
            - Frame
            - ExcType
