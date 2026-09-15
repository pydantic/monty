---
# Show individual methods/attributes (h5) in the docs site's on-page TOC.
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 5
---

# Filesystem & OS

Mounting host directories into the sandbox, and handling the OS calls sandboxed code makes.
See [filesystem access](../../filesystem.md) for how these fit together.

::: pydantic_monty
    options:
        members:
            - MountDir
            - OSAccess
            - AbstractOS
            - AbstractFile
            - MemoryFile
            - CallbackFile
            - StatResult
            - MontyFileHandle
            - OsHandler
            - OsFunction
            - NOT_HANDLED
