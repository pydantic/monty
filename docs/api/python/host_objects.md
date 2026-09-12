---
# Show individual methods/attributes (h5) in the docs site's on-page TOC.
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 5
---

# Host Objects

The policy wrappers that put a host object, or a host class, in front of the sandbox, and the read-only proxies the
sandbox hands back for instances the host has no original object for.
See [host objects](../../host-objects.md) for the concepts.

::: pydantic_monty
    options:
        members:
            - ClassInstance
            - ClassType
            - MontyClassProxy
            - MontyClassTypeProxy
