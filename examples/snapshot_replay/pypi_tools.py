"""Read a fixed set of public PyPI package metadata, without redirects or proxies."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import urllib.request
from pathlib import Path
from typing import Any

PACKAGES = {'pydantic-ai-slim', 'pydantic-ai-harness', 'langchain-monty'}
REQUEST_TIMEOUT = 10


def dispatch(call: dict[str, Any]) -> dict[str, Any]:
    """Fetch allowlisted metadata with a deadline covering the complete HTTP request."""
    if (
        call['name'] != 'package_metadata'
        or len(call['args']) != 1
        or call['kwargs']
        or type(call['args'][0]) is not str
        or call['args'][0] not in PACKAGES
    ):
        raise ValueError('Tool or package is not allowlisted')
    # A child can be killed during DNS or a trickling response; a Python thread cannot.
    try:
        fetched = subprocess.run(
            [sys.executable, '-I', str(Path(__file__).resolve()), call['args'][0]],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            check=True,
            timeout=REQUEST_TIMEOUT,
            env={key: os.environ[key] for key in ('SystemRoot',) if key in os.environ},
            creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0),
        )
    except subprocess.TimeoutExpired as error:
        raise TimeoutError('PyPI request exceeded its deadline') from error
    raw = fetched.stdout
    if len(raw) > 1024 * 1024:
        raise ValueError('PyPI response exceeds 1 MiB')
    data = json.loads(raw)['info']
    return {'return_value': {key: data[key] for key in ('version', 'requires_python', 'requires_dist')}}


def read_package(package: str) -> bytes:
    """Read at most 1 MiB plus the overflow byte in the deadline-controlled child."""
    if package not in PACKAGES:
        raise ValueError('Package is not allowlisted')
    request = urllib.request.Request(
        'https://pypi.org/pypi/' + package + '/json',
        headers={'User-Agent': 'monty-snapshot-replay-example/0.1'},
    )
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    with opener.open(request, timeout=REQUEST_TIMEOUT) as response:
        return response.read(1024 * 1024 + 1)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req: Any, fp: Any, code: int, msg: str, headers: Any, newurl: str) -> None:
        return None


if __name__ == '__main__':
    sys.stdout.buffer.write(read_package(sys.argv[1]))
