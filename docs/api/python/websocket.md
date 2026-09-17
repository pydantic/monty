---
# Show individual methods/attributes (h5) in the docs site's on-page TOC.
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 5
---

# Websocket Client

A pool of remote `monty` workers reached over a WebSocket instead of local subprocesses — the intended peer is
`monty-server`.
See [running monty-server](../../server.md).

::: pydantic_monty
    options:
        members:
            - AsyncMontyWebsocket
