"""Read a fixed set of public PyPI package metadata, without redirects or proxies."""

from __future__ import annotations

import json
import urllib.request
from typing import Any

PACKAGES = {'pydantic-ai-slim', 'pydantic-ai-harness', 'langchain-monty'}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req: Any, fp: Any, code: int, msg: str, headers: Any, newurl: str) -> None:
        return None


def dispatch(call: dict[str, Any]) -> dict[str, Any]:
    if (
        call['name'] != 'package_metadata'
        or len(call['args']) != 1
        or call['kwargs']
        or type(call['args'][0]) is not str
        or call['args'][0] not in PACKAGES
    ):
        raise ValueError('Tool or package is not allowlisted')
    request = urllib.request.Request(
        'https://pypi.org/pypi/' + call['args'][0] + '/json',
        headers={'User-Agent': 'monty-snapshot-replay-example/0.1'},
    )
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    with opener.open(request, timeout=10) as response:
        raw = response.read(1024 * 1024 + 1)
    if len(raw) > 1024 * 1024:
        raise ValueError('PyPI response exceeds 1 MiB')
    data = json.loads(raw)['info']
    return {'return_value': {key: data[key] for key in ('version', 'requires_python', 'requires_dist')}}
