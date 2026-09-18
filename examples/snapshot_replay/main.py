"""Command-line capture, replay, response branching and local HTML comparison."""

from __future__ import annotations

import argparse
import base64
import html
import json
from collections.abc import Iterable
from pathlib import Path
from typing import Any, cast

from . import pypi_tools, rewind

MAX_DIFF_ROWS = 200
MAX_DIFF_NODES = 10_000
MAX_DIFF_PATH = 1024


def main() -> None:
    parser = argparse.ArgumentParser(description='Capture and replay Monty tool boundaries')
    parser.add_argument('--binary', type=Path, required=True, help='Path to the trusted Monty worker binary')
    commands = parser.add_subparsers(dest='command', required=True)
    cap = commands.add_parser('capture')
    cap.add_argument('recording')
    cap.add_argument('--code', type=Path, default=Path(__file__).with_name('program.txt'))
    for name in ('replay', 'branch', 'report'):
        command = commands.add_parser(name)
        command.add_argument('recording')
        if name == 'branch':
            command.add_argument('--at', type=int, required=True)
            command.add_argument('--response', required=True)
        if name == 'replay':
            command.add_argument('--code')
        if name == 'report':
            command.add_argument('--branch')
            command.add_argument('--output', required=True)
        else:
            command.add_argument('--output')
    args = parser.parse_args()
    if args.command == 'capture':
        result = rewind.capture(
            rewind.read_text(args.code, rewind.MAX_SOURCE, 'Source'),
            args.recording,
            pypi_tools.dispatch,
            binary=args.binary,
        )
    else:
        recording = rewind.load(args.recording)
        if args.command == 'report':
            branch = (
                rewind.parse_json(rewind.read_text(args.branch, rewind.MAX_FILE, 'Branch')) if args.branch else None
            )
            document = report(recording, branch)
            rewind.write_artifact(args.output, document.encode())
            result = {'report': args.output}
        else:
            response = (
                rewind.parse_json(rewind.read_text(args.response, rewind.MAX_VALUE, 'Response'))
                if args.command == 'branch'
                else None
            )
            source = rewind.read_text(args.code, rewind.MAX_SOURCE, 'Source') if getattr(args, 'code', None) else None
            result = rewind.replay(
                recording, binary=args.binary, at=getattr(args, 'at', None), response=response, code=source
            )
            if args.output:
                rewind.write_artifact(args.output, rewind.encode(result))
    print(rewind.encode(result).decode())


def report(recording: dict[str, Any], branch: dict[str, Any] | None = None) -> str:
    def pretty(value: Any) -> str:
        return html.escape(json.dumps(value, separators=(',', ':'), ensure_ascii=False))

    cards = bounded_html(
        f'<details><summary><span>{event["index"]:02d}</span> {html.escape(event["call"]["name"])} '
        f'<small>{len(base64.b64decode(event["snapshot"])):,} snapshot bytes</small></summary>'
        f'<h3>Arguments</h3><pre>{pretty(event["call"])}</pre>'
        f'<h3>Recorded response</h3><pre>{pretty(event["response"])}</pre></details>'
        for event in recording['events']
    )
    branch_html = ''
    if branch is not None:
        rewind.fields(branch, 'mode recording_sha256 at response code original result same_result comparison')
        if branch['recording_sha256'] != recording['sha256']:
            raise rewind.ReplayError('Branch belongs to a different recording')
        rewind.check_result(branch['result'])
        if rewind.encode(branch['original']) != rewind.encode(recording['result']):
            raise rewind.ReplayError('Branch original differs from recording')
        comparison = rewind.compare_results(recording['result'], branch['result'])
        valid_at = type(branch['at']) is int and 0 <= branch['at'] < len(recording['events'])
        if branch['mode'] == 'response-branch':
            rewind.check_response(branch['response'])
        elif branch['response'] is not None:
            raise rewind.ReplayError('Unexpected response override')
        if branch['mode'] == 'changed-source':
            rewind.check_code(branch['code'])
        elif branch['code'] is not None:
            raise rewind.ReplayError('Unexpected source override')
        if (
            branch['mode'] not in ('response-branch', 'changed-source', 'replay')
            or (not valid_at if branch['mode'] == 'response-branch' else branch['at'] is not None)
            or type(branch['same_result']) is not bool
            or branch['same_result'] != (comparison == 'equal')
            or branch['comparison'] != comparison
        ):
            raise rewind.ReplayError('Invalid branch comparison')
        rows = bounded_html(
            f'<tr><td><code>{html.escape(path)}</code></td><td><pre>{pretty(before)}</pre></td>'
            f'<td><pre>{pretty(after)}</pre></td></tr>'
            for path, before, after in differences(recording['result'], branch['result'])
        )
        branch_html = (
            '<section><h2>What changed <em>SIMULATED</em></h2>'
            '<table><thead><tr><th>Path</th><th>Observed</th><th>What-if</th></tr></thead>'
            f'<tbody>{rows}</tbody></table><details><summary>Full branch result</summary>'
            f'<pre>{pretty(branch)}</pre></details></section>'
        )
    return bounded_html(
        (
            """<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'">
<title>Snapshot replay</title><style>
*{box-sizing:border-box}body{margin:0;background:#0b1018;color:#dce6f0;font:16px/1.6 system-ui,sans-serif}
main{max-width:1120px;margin:48px auto;padding:0 28px}header{border-bottom:1px solid #293749;padding-bottom:26px}
h1{font-size:42px;letter-spacing:-2px;margin:8px 0}h2{font-size:20px}h3{font-size:14px;color:#91a6bf}
.eyebrow{color:#72dab3;letter-spacing:3px;font-size:12px}p,small{color:#91a6bf}code{overflow-wrap:anywhere}
section{margin:30px 0}details{border:1px solid #293749;border-radius:10px;margin:12px 0;background:#111b29}
summary{padding:16px;cursor:pointer}summary span{color:#72dab3;margin-right:16px}small{float:right}
pre{background:#0b1421;padding:18px;overflow:auto;font:13px/1.7 ui-monospace,monospace;border-radius:8px}
table{width:100%;border-collapse:collapse;table-layout:fixed}th,td{text-align:left;vertical-align:top;padding:12px;border:1px solid #293749}td code{font-size:12px}td pre{white-space:pre-wrap;overflow-wrap:anywhere;font-size:12px;margin:0;padding:8px}
details pre,details h3{margin:16px}em{font-style:normal;color:#edbd6b;font-size:11px;margin-left:10px}
footer{border-top:1px solid #293749;padding:20px 0;color:#91a6bf;font-size:13px}
</style><main><header><div class="eyebrow">MONTY / SNAPSHOT REPLAY</div><h1>Compare recorded tool responses.</h1></header>""",
            f'<section><h2>Capture identity</h2><code>{recording["sha256"]}</code>'
            f'<p>Monty {html.escape(recording["header"]["runtime"]["monty"])} / {len(recording["events"])} calls. '
            'Checksum verifies integrity, not authorship.</p></section>'
            f'<section><h2>Program</h2><pre>{html.escape(recording["header"]["code"])}</pre></section>',
            '<section><h2>Call timeline</h2>',
            cards,
            '</section>',
            f'<section><h2>Observed result</h2><pre>{pretty(recording["result"])}</pre></section>',
            branch_html,
            '<footer>Local files only. No telemetry, remote assets, or live replay fallback. Snapshots may contain sensitive data. '
            'Keep recordings private. Async tasks and host objects are not supported.</footer></main></html>',
        )
    )


def bounded_html(parts: Iterable[str]) -> str:
    chunks: list[str] = []
    size = 0
    for part in parts:
        size += len(part.encode())
        if size > rewind.MAX_FILE:
            raise rewind.ReplayError('Report exceeds 8 MiB')
        chunks.append(part)
    return ''.join(chunks)


def differences(before: object, after: object) -> list[tuple[str, Any, Any]]:
    """Compare JSON values without discarding observable mapping order."""
    rewind.bounded_value(before)
    rewind.bounded_value(after)
    rows: list[tuple[str, Any, Any]] = []
    nodes = 0

    def visit(left: object, right: object, path: str) -> None:
        nonlocal nodes
        nodes += 1
        if nodes > MAX_DIFF_NODES:
            raise rewind.ReplayError('Comparison exceeds 10000 nodes')
        if rewind.encode(left) == rewind.encode(right):
            return
        left_type, right_type = type(left), type(right)
        if left_type is dict and right_type is dict:
            left_map, right_map = cast(dict[str, Any], left), cast(dict[str, Any], right)
            if list(left_map) == list(right_map):
                for key in left_map:
                    if len(path) + 1 + len(key) > MAX_DIFF_PATH:
                        raise rewind.ReplayError('Comparison path exceeds 1024 characters')
                    visit(left_map[key], right_map[key], f'{path}.{key}')
                return
        if left_type is list and right_type is list:
            left_items, right_items = cast(list[Any], left), cast(list[Any], right)
            if len(left_items) == len(right_items):
                for index, pair in enumerate(zip(left_items, right_items)):
                    suffix = f'[{index}]'
                    if len(path) + len(suffix) > MAX_DIFF_PATH:
                        raise rewind.ReplayError('Comparison path exceeds 1024 characters')
                    visit(*pair, path + suffix)
                return
        if len(rows) == MAX_DIFF_ROWS:
            raise rewind.ReplayError('Comparison exceeds 200 differences')
        rows.append((path, left, right))

    visit(before, after, '$')
    return rows


if __name__ == '__main__':
    main()
