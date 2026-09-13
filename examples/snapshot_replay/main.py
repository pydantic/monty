"""Command-line capture, replay, response branching and local HTML comparison."""

from __future__ import annotations

import argparse
import html
import json
from pathlib import Path
from typing import Any, cast

from . import pypi_tools, rewind


def differences(before: object, after: object, path: str = '$') -> list[tuple[str, Any, Any]]:
    """Compare JSON values without discarding observable mapping order."""
    if rewind.encode(before) == rewind.encode(after):
        return []
    before_type, after_type = type(before), type(after)
    if before_type is dict and after_type is dict:
        left, right = cast(dict[str, Any], before), cast(dict[str, Any], after)
        if list(left) == list(right):
            return [row for key in left for row in differences(left[key], right[key], f'{path}.{key}')]
    if before_type is list and after_type is list:
        left_items, right_items = cast(list[Any], before), cast(list[Any], after)
        if len(left_items) == len(right_items):
            return [
                row
                for index, pair in enumerate(zip(left_items, right_items))
                for row in differences(*pair, f'{path}[{index}]')
            ]
    return [(path, before, after)]


def report(recording: dict[str, Any], branch: dict[str, Any] | None = None) -> str:
    def pretty(value: Any) -> str:
        return html.escape(json.dumps(value, indent=2, ensure_ascii=False))

    cards: list[str] = []
    for event in recording['events']:
        cards.append(
            f'<details><summary><span>{event["index"]:02d}</span> {html.escape(event["call"]["name"])} '
            f'<small>{len(event["snapshot"]) * 3 // 4:,} snapshot bytes</small></summary>'
            f'<h3>Arguments</h3><pre>{pretty(event["call"])}</pre>'
            f'<h3>Recorded response</h3><pre>{pretty(event["response"])}</pre></details>'
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
        rows = ''.join(
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
    return """<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width">
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
</style><main><header><div class="eyebrow">MONTY / SNAPSHOT REPLAY</div><h1>Compare recorded tool responses.</h1>
<p>Recorded execution, inspectable calls, offline response experiments.</p></header>""" + (
        f'<section><h2>Capture identity</h2><code>{recording["sha256"]}</code>'
        f'<p>Monty {html.escape(recording["header"]["runtime"]["monty"])} / {len(recording["events"])} calls. '
        'Checksum verifies integrity, not authorship.</p></section>'
        f'<section><h2>Program</h2><pre>{html.escape(recording["header"]["code"])}</pre></section>'
        '<section><h2>Call timeline</h2>' + ''.join(cards) + '</section>'
        f'<section><h2>Observed result</h2><pre>{pretty(recording["result"])}</pre></section>'
        + branch_html
        + '<footer>Local files only. No telemetry, remote assets, or live replay fallback. Snapshots may contain sensitive data. '
        'Keep recordings private. Async tasks and host objects are not supported.</footer></main></html>'
    )


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
            with open(args.output, 'x', encoding='utf-8') as output:
                output.write(document)
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
                with open(args.output, 'x', encoding='utf-8') as output:
                    json.dump(result, output, indent=2)
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
