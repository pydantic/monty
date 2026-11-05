# Browser control over CDP

A Pydantic AI agent drives Chrome by writing Python, which Monty runs in a sandbox.

- [`cdp.py`](cdp.py) has the agent's one tool, `browser_cdp`, which sends a Chrome DevTools Protocol command to an
    open tab, and optionally waits for an event such as `Page.loadEventFired`. The rest of the file launches Chrome and
    implements a minimal CDP client with `websockets`.
- [`main.py`](main.py) is the agent. The
    [`CodeMode`](https://github.com/pydantic/pydantic-ai-harness/tree/main/pydantic_ai_harness/code_mode) capability
    replaces `browser_cdp` with one `run_code` tool, so the model calls it as a function from Python running in Monty.
    Its signature and docstring are the stub the model sees and its code is type checked against.

The sandbox reaches the browser only through `browser_cdp`, which accepts only the commands in `ALLOWED_METHODS`.
That list leaves out commands that take host file paths (`DOM.setFileInputFiles`, `Input.dispatchDragEvent`),
`Page.navigate` only opens `http(s)` URLs, downloads are denied, and Chrome runs with a throwaway profile.
The browser itself has network access.

## To run

Set `CHROME_PATH` if Chrome is not in its usual location, then:

```bash
uv run --group examples python -m examples.browser_cdp.main
uv run --group examples python -m examples.browser_cdp.main 'Find the latest pydantic release on PyPI'
```

Add `--headed` to open a visible Chrome window and watch the agent browse.
