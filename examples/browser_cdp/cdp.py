"""A Chrome tab the agent drives with raw Chrome DevTools Protocol commands.

`browser_cdp` is the agent's only tool, taking the tab's `CDPSession` as its dependency. Its signature
and docstring are what the model sees, and what its code is type checked against.
`ALLOWED_METHODS`, at the bottom, is the security boundary.

Everything below `browser_cdp` is host-side plumbing: launching Chrome and a minimal CDP client.
"""

from __future__ import annotations

import asyncio
import itertools
import json
import os
import shutil
import subprocess
import sys
import tempfile
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager, suppress
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

from pydantic_ai import RunContext
from websockets.asyncio.client import ClientConnection, connect


async def browser_cdp(
    ctx: RunContext[CDPSession],
    method: str,
    params: dict[str, Any] | None = None,
    wait_for_event: str | None = None,
    timeout: float = 30,
) -> Any:
    """Send one Chrome DevTools Protocol command to the open tab and return its result.

    Only `Page`, `Runtime`, `DOM` and `Input` commands for reading and interacting with the
    page are allowed, and `Page.navigate` only opens `http://` / `https://` URLs.

    Examples:
        `browser_cdp(method='Page.navigate', params={'url': url}, wait_for_event='Page.loadEventFired')`
        `browser_cdp(method='Runtime.evaluate', params={'expression': 'document.title', 'returnByValue': True})`
        `browser_cdp(method='Input.dispatchMouseEvent', params={'type': 'mousePressed', 'x': 10, 'y': 20, 'button': 'left', 'clickCount': 1})`
        `browser_cdp(method='Input.dispatchKeyEvent', params={'type': 'keyDown', 'key': 'Enter', 'text': '\\r'})`

    Args:
        ctx: The run context; its `deps` is the tab's `CDPSession`.
        method: A CDP command, e.g. `'Runtime.evaluate'`.
        params: The command's parameters.
        wait_for_event: A CDP event, e.g. `'Page.loadEventFired'`, to wait for after sending the command.
            The return value is then `{'result': <command result>, 'event': <event params>}`.
        timeout: Seconds to wait for `wait_for_event`.
    """
    params = params or {}
    if method not in ALLOWED_METHODS:
        raise ValueError(f'{method} is not allowed, allowed commands: {", ".join(sorted(ALLOWED_METHODS))}')
    if method == 'Page.navigate' and urlparse(url := str(params.get('url'))).scheme not in ('http', 'https'):
        raise ValueError(f'only http and https URLs can be opened, got {url!r}')
    session = ctx.deps
    if wait_for_event is None:
        return await session.send(method, params)
    event = session.wait_for_event(wait_for_event)
    result = await session.send(method, params)
    return {'result': result, 'event': await asyncio.wait_for(event, timeout)}


@asynccontextmanager
async def launch_chrome(*, headless: bool = True) -> AsyncGenerator[CDPSession]:
    """Start Chrome with a throwaway profile and yield a `CDPSession` for its one open tab.

    Set `CHROME_PATH` to choose the binary; otherwise the usual install locations are searched.
    """
    with tempfile.TemporaryDirectory(prefix='monty-chrome-') as profile:
        args = [
            _find_chrome(),
            f'--user-data-dir={profile}',
            '--remote-debugging-port=0',
            '--no-first-run',
            '--no-default-browser-check',
            'about:blank',
        ]
        if headless:
            args.insert(1, '--headless=new')
        process = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            ws_url = await _read_devtools_url(Path(profile))
            async with connect(ws_url, max_size=MAX_MESSAGE_SIZE) as ws:
                client = CDPClient(ws)
                async with client.run():
                    # downloads would write to the host filesystem
                    await client.send('Browser.setDownloadBehavior', {'behavior': 'deny'})
                    yield await client.new_page_session()
        finally:
            process.terminate()
            process.wait()


def _find_chrome() -> str:
    """Locate a Chrome or Chromium binary."""
    if path := os.environ.get('CHROME_PATH'):
        return path
    if sys.platform == 'darwin':
        candidates = ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome']
    elif sys.platform == 'win32':
        roots = [os.environ.get(var) for var in ('PROGRAMFILES', 'PROGRAMFILES(X86)', 'LOCALAPPDATA')]
        candidates = [rf'{root}\Google\Chrome\Application\chrome.exe' for root in roots if root]
    else:
        candidates = ['google-chrome', 'google-chrome-stable', 'chromium', 'chromium-browser']
    for candidate in candidates:
        if found := shutil.which(candidate) or (Path(candidate).is_file() and candidate):
            return found
    raise FileNotFoundError('Chrome not found, set CHROME_PATH to the Chrome or Chromium binary')


async def _read_devtools_url(profile: Path) -> str:
    """Wait for Chrome to write `DevToolsActivePort`, which holds its debugging port and browser path."""
    port_file = profile / 'DevToolsActivePort'
    async with asyncio.timeout(15):
        while not port_file.exists() or len(lines := port_file.read_text().splitlines()) < 2:
            await asyncio.sleep(0.05)
    return f'ws://127.0.0.1:{lines[0]}{lines[1]}'


class CDPClient:
    """A minimal CDP connection: numbered requests matched to responses, events fanned out to waiters.

    One websocket carries the browser target and every attached page, distinguished by `sessionId`.
    """

    def __init__(self, ws: ClientConnection):
        self._ws = ws
        self._ids = itertools.count(1)
        self._pending: dict[int, asyncio.Future[dict[str, Any]]] = {}
        self._event_waiters: list[tuple[str | None, str, asyncio.Future[dict[str, Any]]]] = []

    @asynccontextmanager
    async def run(self) -> AsyncGenerator[None]:
        """Read messages in the background for the lifetime of the block."""
        reader = asyncio.create_task(self._read_loop())
        try:
            yield
        finally:
            reader.cancel()
            with suppress(asyncio.CancelledError):
                await reader

    async def send(self, method: str, params: dict[str, Any] | None = None, session_id: str | None = None) -> Any:
        """Send one CDP command and return its `result`, raising `RuntimeError` on a protocol error."""
        message_id = next(self._ids)
        message: dict[str, Any] = {'id': message_id, 'method': method, 'params': params or {}}
        if session_id:
            message['sessionId'] = session_id
        future = self._pending[message_id] = asyncio.get_running_loop().create_future()
        await self._ws.send(json.dumps(message))
        response = await future
        if error := response.get('error'):
            raise RuntimeError(f'{method} failed: {error["message"]}')
        return response['result']

    def wait_for_event(self, method: str, session_id: str | None = None) -> asyncio.Future[dict[str, Any]]:
        """Return a future for the next `method` event; create it *before* the command that triggers it."""
        future = asyncio.get_running_loop().create_future()
        self._event_waiters.append((session_id, method, future))
        return future

    async def new_page_session(self) -> CDPSession:
        """Attach to the tab Chrome opened at startup and enable the `Page` domain for its events."""
        targets = await self.send('Target.getTargets')
        target_id = next(t['targetId'] for t in targets['targetInfos'] if t['type'] == 'page')
        attached = await self.send('Target.attachToTarget', {'targetId': target_id, 'flatten': True})
        session = CDPSession(self, attached['sessionId'])
        await session.send('Page.enable')
        return session

    async def _read_loop(self) -> None:
        try:
            async for raw in self._ws:
                message = json.loads(raw)
                if 'id' in message:
                    # a cancelled `send()` leaves its future here, already done
                    if (future := self._pending.pop(message['id'], None)) and not future.done():
                        future.set_result(message)
                else:
                    self._dispatch_event(message)
        finally:
            self._fail_waiters()

    def _fail_waiters(self) -> None:
        """Fail every outstanding request and event waiter once the connection is gone, so none hangs."""
        error = ConnectionError('Chrome DevTools connection closed')
        waiters = [*self._pending.values(), *(future for _, _, future in self._event_waiters)]
        self._pending.clear()
        self._event_waiters.clear()
        for future in waiters:
            if not future.done():
                future.set_exception(error)

    def _dispatch_event(self, message: dict[str, Any]) -> None:
        remaining: list[tuple[str | None, str, asyncio.Future[dict[str, Any]]]] = []
        for session_id, method, future in self._event_waiters:
            if method == message['method'] and session_id == message.get('sessionId') and not future.done():
                future.set_result(message.get('params', {}))
            elif not future.done():
                remaining.append((session_id, method, future))
        self._event_waiters = remaining


@dataclass
class CDPSession:
    """A `CDPClient` bound to one attached page target."""

    _client: CDPClient
    _session_id: str

    async def send(self, method: str, params: dict[str, Any] | None = None) -> Any:
        return await self._client.send(method, params, self._session_id)

    def wait_for_event(self, method: str) -> asyncio.Future[dict[str, Any]]:
        return self._client.wait_for_event(method, self._session_id)


# Largest CDP message accepted, in bytes; full-page screenshots arrive base64-encoded in one message.
MAX_MESSAGE_SIZE = 64 * 1024 * 1024

# Commands the sandbox may send. Anything else, e.g. `DOM.setFileInputFiles` or
# `Input.dispatchDragEvent` (both take host file paths) or `Browser.*`, is refused.
ALLOWED_METHODS = frozenset(
    {
        'Page.navigate',
        'Page.reload',
        'Page.getNavigationHistory',
        'Page.navigateToHistoryEntry',
        'Page.getFrameTree',
        'Page.getLayoutMetrics',
        'Page.captureScreenshot',
        'Runtime.evaluate',
        'Runtime.callFunctionOn',
        'Runtime.getProperties',
        'Runtime.releaseObject',
        'DOM.getDocument',
        'DOM.querySelector',
        'DOM.querySelectorAll',
        'DOM.describeNode',
        'DOM.resolveNode',
        'DOM.getOuterHTML',
        'DOM.getBoxModel',
        'DOM.getContentQuads',
        'DOM.scrollIntoViewIfNeeded',
        'DOM.focus',
        'Input.dispatchMouseEvent',
        'Input.dispatchKeyEvent',
        'Input.insertText',
    }
)
