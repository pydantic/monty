---
# Show individual methods/attributes (h5) in the docs site's on-page TOC.
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 5
---

# Errors

Exceptions raised by `pydantic_monty`, their traceback frames and failure classifications, and the `ExcType` names
accepted when answering a snapshot with an exception.

::: pydantic_monty
    options:
        members:
            - MontyError
            - MontySyntaxError
            - MontyTypingError
            - MontyRuntimeError
            - VmLimit
            - SuspensionLimit
            - OrdinaryException
            - MontyConversionError
            - MontyCrashedError
            - MontyDisconnectError
            - MontyRefusedError
            - MontyConnectError
            - MontyConfigurationError
            - MontyShutdown
            - Frame
            - ExcType
