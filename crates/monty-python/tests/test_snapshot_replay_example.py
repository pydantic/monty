"""Regression tests for the synchronous snapshot replay example."""

from __future__ import annotations

import hashlib
import io
import json
import os
import re
import runpy
import socket
import subprocess
import sys
import threading
from contextlib import redirect_stdout
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from typing import Any
from unittest.mock import MagicMock, patch

import pytest
from inline_snapshot import snapshot

from pydantic_monty._binary import find_monty_binary

# Examples are not installed with the Python client wheel.
with patch.object(sys, 'path', [str(Path(__file__).resolve().parents[3]), *sys.path]):
    from examples.snapshot_replay import main as app, pypi_tools, rewind as r


@pytest.fixture
def case(tmp_path: Path) -> Any:
    path = tmp_path / 'run.jsonl'
    binary = Path(find_monty_binary())

    def capture(
        code: str = 'print("before")\nx = fetch("a")\nprint("between")\ny = fetch("b")\nx + y',
    ) -> dict[str, Any]:
        return r.capture(code, path, lambda call: {'return_value': 10 if call['args'] == ['a'] else 2}, binary=binary)

    return SimpleNamespace(path=path, temp=SimpleNamespace(name=str(tmp_path)), binary=binary, capture=capture)


def test_capture_restore_branch_and_changed_source(case: Any) -> None:
    case.capture()
    recording = r.load(case.path)
    assert r.replay(recording, binary=case.binary)['result'] == snapshot(
        {'kind': 'return', 'value': 12, 'output': [['stdout', 'before\nbetween\n']]}
    )
    branch = r.replay(recording, at=1, response={'return_value': 9}, binary=case.binary)
    assert branch['result']['value'] == snapshot(19)
    assert branch['result']['output'] == snapshot([['stdout', 'before\nbetween\n']])
    assert r.replay(recording, code='x=fetch("a")\ny=fetch("b")\nx-y', binary=case.binary)['result'][
        'value'
    ] == snapshot(8)
    assert r.load(case.path)['sha256'] == snapshot(recording['sha256'])


@pytest.mark.parametrize('before, after', [('stdout', 'stderr'), ('stderr', 'stdout')])
def test_changed_source_preserves_output_stream(case: Any, before: str, after: str) -> None:
    code = 'import sys\nprint("</pre><script>same</script>", file=sys.{stream})\n7'
    case.capture(code.format(stream=before))
    recording = r.load(case.path)
    branch = r.replay(recording, code=code.format(stream=after), binary=case.binary)
    assert branch['same_result'] == snapshot(False)
    assert branch['comparison'] == snapshot('result-differs')
    assert app.differences(branch['original'], branch['result']) == [('$.output[0][0]', before, after)]
    report = app.report(recording, branch)
    assert ('$.output[0][0]' in report) == snapshot(True)
    assert ('<script>' not in report) == snapshot(True)
    assert ('&lt;script&gt;' in report) == snapshot(True)


@pytest.mark.parametrize(
    'at, expected',
    [
        (0, snapshot([['stderr', 'prefix\nmiddle'], ['stdout', 'tail\n']])),
        (1, snapshot([['stderr', 'prefix\n'], ['stdout', 'middle'], ['stderr', 'tail\n']])),
    ],
)
def test_response_branch_restores_stream_prefix(case: Any, at: int, expected: list[list[str]]) -> None:
    case.capture(
        'import sys\nprint("prefix", file=sys.stderr)\nfirst = fetch("a")\n'
        'print("middle", file=sys.stdout if first else sys.stderr, end="")\nsecond = fetch("b")\n'
        'print("tail", file=sys.stdout if second else sys.stderr)\n7'
    )
    recording = r.load(case.path)
    saved = r.encode(recording)
    assert recording['result']['output'] == snapshot([['stderr', 'prefix\n'], ['stdout', 'middletail\n']])
    assert r.replay(recording, binary=case.binary)['same_result'] == snapshot(True)
    branch = r.replay(recording, at=at, response={'return_value': 0}, binary=case.binary)
    assert branch['result']['output'] == expected
    assert branch['result']['value'] == snapshot(7)
    assert branch['same_result'] == snapshot(False)
    assert branch['comparison'] == snapshot('result-differs')
    assert (r.encode(recording) == saved) == snapshot(True)
    assert r.replay(recording, binary=case.binary)['same_result'] == snapshot(True)


def test_changed_source_preserves_cross_stream_order(case: Any) -> None:
    code = 'import sys\nprint("same", file=sys.{first})\nprint("same", file=sys.{second})\n7'
    case.capture(code.format(first='stdout', second='stderr'))
    branch = r.replay(r.load(case.path), code=code.format(first='stderr', second='stdout'), binary=case.binary)
    assert branch['original']['output'] == snapshot([['stdout', 'same\n'], ['stderr', 'same\n']])
    assert branch['result']['output'] == snapshot([['stderr', 'same\n'], ['stdout', 'same\n']])
    assert branch['same_result'] == snapshot(False)


@pytest.mark.parametrize('stream', ['stdout', 'stderr'])
def test_output_ignores_chunk_boundaries(stream: Any) -> None:
    whole, split = r.Output(), r.Output()
    whole(stream, '\u00e9x\n')
    split(stream, '\u00e9')
    split('stderr' if stream == 'stdout' else 'stdout', '')
    split(stream, 'x\n')
    assert split.output == whole.output == [[stream, '\u00e9x\n']]
    assert split.size == whole.size == snapshot(68)


def test_sample_response_changes_only_selected_package(case: Any) -> None:
    example = Path(app.__file__).parent
    info = {
        'version': 'recorded',
        'requires_python': '>=3.10',
        'requires_dist': ['pydantic-monty>=0.0.22', 'httpx>=0.28'],
    }
    dispatch = MagicMock(return_value={'return_value': info})
    original = r.capture((example / 'program.txt').read_text(), case.path, dispatch, binary=case.binary)
    recording = r.load(case.path)
    assert [call.args[0]['args'] for call in dispatch.call_args_list] == snapshot(
        [['pydantic-ai-slim'], ['pydantic-ai-harness'], ['langchain-monty']]
    )
    assert r.replay(recording, binary=case.binary)['result'] == snapshot(original)
    branch = r.replay(
        recording, at=1, response=r.parse_json((example / 'response.json').read_bytes()), binary=case.binary
    )
    assert app.differences(original['value'], branch['result']['value']) == snapshot(
        [
            ('$[1].version', 'recorded', 'what-if'),
            ('$[1].python', '>=3.10', '>=3.14'),
            ('$[1].monty_dependencies[0]', 'pydantic-monty>=0.0.22', 'pydantic-monty==0.0.23'),
        ]
    )
    assert dispatch.call_count == snapshot(3)


@pytest.mark.parametrize(
    'code, message',
    [
        (
            'x = fetch("a")\nfetch("b") if x else other("b")',
            snapshot('DIVERGED: tool call 1 differs; no live fallback'),
        ),
        ('x = fetch("a")\nfetch(x)', snapshot('DIVERGED: tool call 1 differs; no live fallback')),
        (
            'def run():\n    x = fetch("a")\n    if not x:\n        return 0\n    return fetch("b")\nrun()',
            snapshot('DIVERGED: recorded responses remain unused'),
        ),
        (
            'x = fetch("a")\ny = fetch("b")\nif not x:\n    fetch("c")\ny',
            snapshot('DIVERGED: tool call 2 differs; no live fallback'),
        ),
    ],
    ids=['different-function', 'different-argument', 'unused-call', 'extra-call'],
)
def test_response_branch_requires_remaining_recorded_calls(case: Any, code: str, message: str) -> None:
    case.capture(code)
    recording = r.load(case.path)
    with pytest.raises(r.ReplayError) as error:
        r.replay(recording, at=0, response={'return_value': 0}, binary=case.binary)
    assert str(error.value) == message


def test_response_branch_can_return_early_after_last_call(case: Any) -> None:
    case.capture('def run():\n    if not fetch("a"):\n        return 0\n    return 1\nrun()')
    recording = r.load(case.path)
    branch = r.replay(recording, at=0, response={'return_value': 0}, binary=case.binary)
    assert branch['original'] == snapshot({'kind': 'return', 'value': 1, 'output': []})
    assert branch['result'] == snapshot({'kind': 'return', 'value': 0, 'output': []})
    assert branch['comparison'] == snapshot('result-differs')


def test_null_response_and_no_calls(case: Any) -> None:
    case.capture('None')
    assert r.replay(r.load(case.path), binary=case.binary)['result']['value'] == snapshot(None)
    case.path.unlink()
    r.capture('fetch("a")', case.path, lambda call: {'return_value': None}, binary=case.binary)
    assert r.replay(r.load(case.path), binary=case.binary)['result']['value'] == snapshot(None)


@pytest.mark.parametrize(
    'code, expected',
    [
        (
            'try:\n    fetch("a")\nexcept ValueError:\n    result = 7\nresult',
            snapshot({'kind': 'return', 'value': 7, 'output': []}),
        ),
        ('fetch("a")', snapshot({'kind': 'error', 'message': 'ValueError: bad', 'output': []})),
        ('1 / 0', snapshot({'kind': 'error', 'message': 'ZeroDivisionError: division by zero', 'output': []})),
    ],
)
def test_guest_exception_caught_and_uncaught(case: Any, code: str, expected: dict[str, Any]) -> None:
    r.capture(code, case.path, lambda call: {'exc_type': 'ValueError', 'message': 'bad'}, binary=case.binary)
    recording = r.load(case.path)
    assert r.replay(recording, binary=case.binary)['result'] == expected


@pytest.mark.parametrize(
    'code, message',
    [
        ('fetch("different")', snapshot('DIVERGED: tool call 0 differs; no live fallback')),
        ('other("a")', snapshot('DIVERGED: tool call 0 differs; no live fallback')),
        ('fetch("a")', snapshot('DIVERGED: recorded responses remain unused')),
        ('fetch("a")\nfetch("b")\nfetch("c")', snapshot('DIVERGED: tool call 2 differs; no live fallback')),
        ('fetch(True)', snapshot('DIVERGED: tool call 0 differs; no live fallback')),
    ],
)
def test_divergence_has_no_dispatcher(case: Any, code: str, message: str) -> None:
    case.capture()
    recording = r.load(case.path)
    with pytest.raises(r.ReplayError) as error:
        r.replay(recording, code=code, binary=case.binary)
    assert str(error.value) == message


def test_interrupted_callback_and_exclusive_creation(case: Any) -> None:
    calls: list[dict[str, Any]] = []

    def crash(call: dict[str, Any]) -> dict[str, Any]:
        calls.append(call)
        raise OSError('host interrupted')

    with pytest.raises(OSError) as error:
        r.capture('fetch("a")', case.path, crash, binary=case.binary)
    assert str(error.value) == snapshot('host interrupted')
    original = case.path.read_bytes()
    assert [json.loads(line)['type'] for line in original.splitlines()] == snapshot(['header', 'call'])
    with pytest.raises(r.ReplayError) as error:
        r.load(case.path)
    assert str(error.value) == snapshot('Capture incomplete: last call has no recorded response')
    assert calls == snapshot([{'name': 'fetch', 'args': ['a'], 'kwargs': {}}])
    with pytest.raises(FileExistsError) as error:
        case.capture()
    assert error.value.strerror == snapshot('File exists')
    assert error.value.filename == snapshot(str(case.path))
    assert original == snapshot(case.path.read_bytes())


@pytest.mark.parametrize(
    'ending, message',
    [
        ('empty', snapshot('Recording is empty')),
        ('newline', snapshot('Recording is truncated: final newline is missing')),
        ('header', snapshot('Capture incomplete: completion record is missing')),
        ('call', snapshot('Capture incomplete: last call has no recorded response')),
        ('response', snapshot('Capture incomplete: completion record is missing')),
        ('checksum', snapshot('Recording checksum mismatch')),
    ],
)
def test_incomplete_recordings_and_checksum(case: Any, ending: str, message: str) -> None:
    case.capture()
    original = case.path.read_bytes()
    lines = original.splitlines(keepends=True)
    broken = {
        'empty': b'',
        'newline': original[:-1],
        'header': lines[0],
        'call': b''.join(lines[:2]),
        'response': b''.join(lines[:-1]),
        'checksum': original.replace(b'"value":12', b'"value":99'),
    }[ending]
    case.path.write_bytes(broken)
    with pytest.raises(r.ReplayError) as error:
        r.load(case.path)
    assert str(error.value) == message


@pytest.mark.parametrize(
    'kwargs, message',
    [
        (
            {'at': -1, 'response': {'return_value': 0}},
            snapshot('Branch requires a valid call index and response, without changed source'),
        ),
        ({'at': 0}, snapshot('Branch requires a valid call index and response, without changed source')),
        ({'response': {'return_value': 0}}, snapshot('Response override requires a call index')),
        (
            {'at': True, 'response': {'return_value': 0}},
            snapshot('Branch requires a valid call index and response, without changed source'),
        ),
        (
            {'at': 0, 'response': {'return_value': 0}, 'code': '1'},
            snapshot('Branch requires a valid call index and response, without changed source'),
        ),
        ({'code': 'x' * (r.MAX_SOURCE + 1)}, snapshot('Source exceeds 32 KiB')),
    ],
    ids=['negative-index', 'missing-response', 'missing-index', 'bool-index', 'source-and-response', 'large-source'],
)
def test_branch_parameters(case: Any, kwargs: dict[str, Any], message: str) -> None:
    case.capture()
    recording = r.load(case.path)
    with pytest.raises(r.ReplayError) as error:
        r.replay(recording, **kwargs, binary=case.binary)
    assert str(error.value) == message


def test_runtime_mismatch(case: Any) -> None:
    case.capture()
    recording = r.load(case.path)
    recording['header']['runtime']['monty'] = 'wrong'
    with pytest.raises(r.ReplayError) as error:
        r.replay(recording, binary=case.binary)
    assert str(error.value) == snapshot('Runtime differs from capture; rebuild with the recorded version')


@pytest.mark.parametrize('value', [float('nan'), float('inf'), (1,), {1: 'value'}, object()])
def test_non_json_values(value: Any) -> None:
    with pytest.raises(r.ReplayError) as error:
        r.encode(value)
    assert str(error.value) == snapshot('Only finite JSON values are supported')


def test_json_boundary() -> None:
    assert (r.encode(True) != r.encode(1)) == snapshot(True)
    assert (r.encode(1) != r.encode(1.0)) == snapshot(True)
    assert r.json_value(1.5) == snapshot(1.5)
    nested = None
    for _ in range(34):
        nested = [nested]
    with pytest.raises(r.ReplayError) as error:
        r.encode(nested)
    assert str(error.value) == snapshot('Value nesting exceeds 32 levels')
    with pytest.raises(r.ReplayError) as error:
        r.bounded_value('x' * r.MAX_VALUE)
    assert str(error.value) == snapshot('Value exceeds 256 KiB')


@pytest.mark.parametrize(
    'response, message',
    [
        ([], snapshot('Response must be an object')),
        ({'future': 1}, snapshot('Expected return_value or a supported exception response')),
        ({'exc_type': 'Unknown', 'message': 'x'}, snapshot('Expected return_value or a supported exception response')),
    ],
)
def test_invalid_responses(response: Any, message: str) -> None:
    with pytest.raises(r.ReplayError) as error:
        r.check_response(response)
    assert str(error.value) == message


def test_limits_and_unsupported_suspension(case: Any) -> None:
    with pytest.raises(r.ReplayError) as error:
        r.capture('x' * (r.MAX_SOURCE + 1), case.path, lambda call: {}, binary=case.binary)
    assert str(error.value) == snapshot('Source exceeds 32 KiB')
    with pytest.raises(r.ReplayError) as error:
        case.capture('unknown_name')
    assert str(error.value) == snapshot('Only direct synchronous function calls are supported')
    with pytest.raises(r.ReplayError) as error:
        r.terminal(None, r.Output())
    assert str(error.value) == snapshot('Unsupported suspension')


def test_saved_file_limits(case: Any) -> None:
    case.path.write_bytes(b'x' * (r.MAX_FILE + 1))
    with pytest.raises(r.ReplayError) as error:
        r.load(case.path)
    assert str(error.value) == snapshot('Recording exceeds 8 MiB')
    case.path.unlink()
    journal = r.Journal(case.path)
    try:
        with patch.object(r, 'MAX_FILE', 1), pytest.raises(r.ReplayError) as error:
            journal.append({'a': 1})
        assert str(error.value) == snapshot('Recording exceeds 8 MiB')
    finally:
        journal.close()


def test_report_escapes_content_and_checks_parent(case: Any) -> None:
    assert app.differences({'a': [1]}, {'a': [2]}) == snapshot([('$.a[0]', 1, 2)])
    assert app.differences([1], [1, 2]) == snapshot([('$', [1], [1, 2])])
    case.capture()
    recording = r.load(case.path)
    recording['header']['code'] = '</pre><script>alert(1)</script>'
    output = app.report(recording)
    assert ('<script>' not in output) == snapshot(True)
    assert ('&lt;script&gt;' in output) == snapshot(True)
    assert ("default-src 'none'" in output) == snapshot(True)
    branch = r.replay(recording, at=1, response={'return_value': 4}, binary=case.binary)
    assert ('SIMULATED' in app.report(recording, branch)) == snapshot(True)
    branch['recording_sha256'] = 'wrong'
    with pytest.raises(r.ReplayError) as error:
        app.report(recording, branch)
    assert str(error.value) == snapshot('Branch belongs to a different recording')


@pytest.mark.parametrize('encoded, size', [('YQ==', snapshot(1)), ('YWI=', snapshot(2)), ('YWJj', snapshot(3))])
def test_report_snapshot_size_excludes_padding(case: Any, encoded: str, size: int) -> None:
    case.capture('fetch("a")')
    recording = r.load(case.path)
    recording['events'][0]['snapshot'] = encoded
    sizes = re.findall(r'<small>([0-9,]+) snapshot bytes</small>', app.report(recording))
    assert [int(value.replace(',', '')) for value in sizes] == [size]


@pytest.mark.parametrize(
    'before, after, message',
    [
        ({'k' * 8192: [0] * 1024}, {'k' * 8192: [1] * 1024}, snapshot('Comparison path exceeds 1024 characters')),
        ({'k' * 1021: [0]}, {'k' * 1021: [1]}, snapshot('Comparison path exceeds 1024 characters')),
        ([0] * 201, [1] * 201, snapshot('Comparison exceeds 200 differences')),
        ([0] * 10000, [0] * 9999 + [1], snapshot('Comparison exceeds 10000 nodes')),
    ],
)
def test_comparison_budgets(before: Any, after: Any, message: str) -> None:
    with pytest.raises(r.ReplayError) as error:
        app.differences(before, after)
    assert str(error.value) == message


def test_comparison_budget_edges() -> None:
    key = 'k' * (app.MAX_DIFF_PATH - 2)
    assert app.differences({key: 0}, {key: 1}) == [('$.' + key, 0, 1)]
    assert len(app.differences([0] * 200, [1] * 200)) == snapshot(200)
    assert app.differences([0] * 9999, [0] * 9998 + [1]) == snapshot([('$[9998]', 0, 1)])


def test_report_stops_before_consuming_more_fragments() -> None:
    assert app.bounded_html(['\u00e9' * (r.MAX_FILE // 2)]) == '\u00e9' * (r.MAX_FILE // 2)
    parts = iter(['x' * r.MAX_FILE, 'x', 'not consumed'])
    with pytest.raises(r.ReplayError) as error:
        app.bounded_html(parts)
    assert str(error.value) == snapshot('Report exceeds 8 MiB')
    assert next(parts) == snapshot('not consumed')


def test_report_caps_escaped_call_cards(case: Any) -> None:
    r.capture(
        'for i in range(8):\n    fetch(i)\nNone',
        case.path,
        lambda call: {'return_value': "'" * 240000},
        binary=case.binary,
    )
    recording = r.load(case.path)
    with pytest.raises(r.ReplayError) as error:
        app.report(recording)
    assert str(error.value) == snapshot('Report exceeds 8 MiB')


def test_artifact_size_checked_before_creation(case: Any) -> None:
    with pytest.raises(r.ReplayError) as error:
        r.write_artifact(case.path, b'x' * (r.MAX_FILE + 1))
    assert str(error.value) == snapshot('Artifact exceeds 8 MiB')
    assert case.path.exists() == snapshot(False)
    r.write_artifact(case.path, b'x' * r.MAX_FILE)
    assert len(r.read_text(case.path, r.MAX_FILE, 'Branch')) == snapshot(r.MAX_FILE)


@pytest.mark.skipif(os.name != 'posix', reason='Windows artifacts inherit the parent directory ACL')
def test_artifacts_private_under_permissive_umask() -> None:
    with TemporaryDirectory() as directory:
        path = Path(directory)
        previous = os.umask(0o022)
        try:
            journal = r.Journal(path / 'run.jsonl')
            journal.close()
            r.write_artifact(path / 'branch.json', b'{}')
            r.write_artifact(path / 'report.html', b'<html>')
        finally:
            os.umask(previous)
        assert [
            (path / name).stat().st_mode & 0o777 for name in ('run.jsonl', 'branch.json', 'report.html')
        ] == snapshot([0o600, 0o600, 0o600])


def test_deep_comparison_file_roundtrips_through_cli(case: Any) -> None:
    code = 'x = [0] * 119970\nfor _ in range(28):\n    x = [x]\nx'
    case.capture(code)
    comparison, document = Path(case.temp.name) / 'branch.json', Path(case.temp.name) / 'report.html'
    with (
        patch.object(
            sys,
            'argv',
            ['snapshot-replay', '--binary', str(case.binary), 'replay', str(case.path), '--output', str(comparison)],
        ),
        redirect_stdout(io.StringIO()),
    ):
        app.main()
    data = r.parse_json(r.read_text(comparison, r.MAX_FILE, 'Branch'))
    assert (comparison.stat().st_size < 500000) == snapshot(True)
    assert (len(json.dumps(data, indent=2)) > r.MAX_FILE) == snapshot(True)
    with (
        patch.object(
            sys,
            'argv',
            [
                'snapshot-replay',
                '--binary',
                str(case.binary),
                'report',
                str(case.path),
                '--branch',
                str(comparison),
                '--output',
                str(document),
            ],
        ),
        redirect_stdout(io.StringIO()),
    ):
        app.main()
    assert (document.stat().st_size <= r.MAX_FILE) == snapshot(True)


@pytest.mark.parametrize(
    'row, key, value, message',
    [
        (0, 'schema', 9, snapshot('Unsupported recording schema; capture again with schema 3')),
        (0, 'schema', 1, snapshot('Unsupported recording schema; capture again with schema 3')),
        (0, 'schema', 2, snapshot('Unsupported recording schema; capture again with schema 3')),
        (0, 'schema', True, snapshot('Unsupported recording schema; capture again with schema 3')),
        (0, 'code', 42, snapshot('Invalid source')),
        (0, 'limits', {}, snapshot('Source or limits mismatch')),
        (0, 'runtime', None, snapshot('Invalid object fields')),
        (0, 'runtime', {'monty': False, 'worker_sha256': '0' * 64}, snapshot('Invalid runtime version')),
        (0, 'runtime', {'monty': '0.0.23', 'worker_sha256': 'bad'}, snapshot('Invalid worker hash')),
        (1, 'index', 99, snapshot('Invalid call/response sequence')),
        (1, 'index', False, snapshot('Invalid call/response sequence')),
        (2, 'index', False, snapshot('Invalid call/response sequence')),
        (1, 'snapshot_sha256', 'bad', snapshot('Snapshot checksum or size mismatch')),
        (1, 'snapshot', '!', snapshot('Malformed recording')),
        (1, 'call', [], snapshot('Invalid object fields')),
        (1, 'call', {'name': False, 'args': [], 'kwargs': {}}, snapshot('Invalid call identity')),
        (1, 'call', {'name': 'fetch', 'args': {}, 'kwargs': {}}, snapshot('Invalid call identity')),
        (1, 'output_before', '', snapshot('Invalid captured output')),
        (3, 'output_before', [['stdout', 'unrelated']], snapshot('Invalid output prefix')),
        (-1, 'calls', 99, snapshot('Call count mismatch')),
        (-1, 'result', 'bad', snapshot('Invalid terminal result')),
        (-1, 'result', {'kind': 'potato'}, snapshot('Invalid terminal result')),
        (-1, 'result', {'kind': 'return', 'value': 1}, snapshot('Invalid object fields')),
        (-1, 'result', {'kind': 'error', 'message': [], 'output': []}, snapshot('Invalid terminal text')),
        (
            -1,
            'result',
            {'kind': 'return', 'value': 1, 'output': [['stdout', 'unrelated']]},
            snapshot('Invalid final output prefix'),
        ),
    ],
)
def test_record_structure_rejects_resealed_invalid_data(
    case: Any, row: int, key: str, value: Any, message: str
) -> None:
    case.capture()
    rows = [json.loads(line) for line in case.path.read_bytes().splitlines()]
    rows[row][key] = value
    write_sealed_recording(case.path, rows)
    with pytest.raises(r.ReplayError) as error:
        r.load(case.path)
    assert str(error.value) == message


@pytest.mark.parametrize('terminal', [False, True])
@pytest.mark.parametrize(
    'output',
    [
        None,
        '',
        ['stdout'],
        [[]],
        [['stdout']],
        [['stdout', 'x', 'y']],
        [['other', 'x']],
        [[False, 'x']],
        [['stdout', 1]],
        [['stdout', '']],
        [['stdout', 'x'], ['stdout', 'y']],
    ],
)
def test_recording_rejects_invalid_output(case: Any, terminal: bool, output: Any) -> None:
    case.capture('fetch("a")')
    rows = [json.loads(line) for line in case.path.read_bytes().splitlines()]
    if terminal:
        rows[-1]['result']['output'] = output
    else:
        rows[1]['output_before'] = output
    write_sealed_recording(case.path, rows)
    with pytest.raises(r.ReplayError) as error:
        r.load(case.path)
    assert str(error.value) == snapshot('Invalid captured output')


@pytest.mark.parametrize(
    'output, expected',
    [
        ([], snapshot(False)),
        ([['stdout', 'ab']], snapshot(False)),
        ([['stdout', 'ab'], ['stderr', 'c']], snapshot(False)),
        ([['stdout', 'ac'], ['stderr', 'cd']], snapshot(False)),
        ([['stderr', 'ab'], ['stdout', 'cd']], snapshot(False)),
        ([['stdout', 'ab'], ['stderr', 'cd']], snapshot(True)),
        ([['stdout', 'ab'], ['stderr', 'cde'], ['stdout', 'f']], snapshot(True)),
    ],
)
def test_output_prefix_requires_streams_and_text(output: list[list[str]], expected: bool) -> None:
    assert r.has_output_prefix(output, [['stdout', 'ab'], ['stderr', 'cd']]) == expected


@pytest.mark.parametrize(
    'missing, message',
    [('response', snapshot('Invalid call/response sequence')), ('field', snapshot('Invalid object fields'))],
)
def test_missing_recording_entries(case: Any, missing: str, message: str) -> None:
    case.capture()
    rows = [json.loads(line) for line in case.path.read_bytes().splitlines()]
    if missing == 'response':
        rows.pop(2)
    else:
        del rows[1]['call']
    write_sealed_recording(case.path, rows)
    with pytest.raises(r.ReplayError) as error:
        r.load(case.path)
    assert str(error.value) == message


@pytest.mark.parametrize(
    'raw, message',
    [
        (b'[]\n', snapshot('Invalid recording row')),
        (b'{broken}\n', snapshot('Malformed JSON')),
        (b'{"n":' + b'1' * 5000 + b'}\n', snapshot('Malformed JSON')),
        (b'{"a":1,"a":2}\n', snapshot('Malformed JSON')),
        (b'{"a":"\\ud800"}\n', snapshot('Malformed JSON')),
        (b'{"a":NaN}\n', snapshot('Malformed JSON')),
    ],
    ids=['non-object', 'syntax', 'large-integer', 'duplicate-key', 'surrogate', 'nan'],
)
def test_malformed_recording(case: Any, raw: bytes, message: str) -> None:
    case.path.write_bytes(raw)
    with pytest.raises(r.ReplayError) as error:
        r.load(case.path)
    assert str(error.value) == message


def test_mapping_order_survives_storage_and_comparison(case: Any) -> None:
    r.capture('list(fetch().keys())', case.path, lambda call: {'return_value': {'b': 1, 'a': 2}}, binary=case.binary)
    recording = r.load(case.path)
    assert list(recording['events'][0]['response']['return_value']) == snapshot(['b', 'a'])
    assert r.replay(recording, binary=case.binary)['result']['value'] == snapshot(['b', 'a'])
    assert r.replay(recording, binary=case.binary)['comparison'] == snapshot('equal')
    assert app.differences({'b': 1, 'a': 2}, {'a': 2, 'b': 1}) == snapshot([('$', {'b': 1, 'a': 2}, {'a': 2, 'b': 1})])


@pytest.mark.parametrize(
    'original, changed',
    [
        ("fetch({'b': 1, 'a': 2})", "fetch({'a': 2, 'b': 1})"),
        ('fetch(b=1, a=2)', 'fetch(a=2, b=1)'),
    ],
)
def test_call_mapping_order(case: Any, original: str, changed: str) -> None:
    r.capture(original, case.path, lambda call: {'return_value': 1}, binary=case.binary)
    recording = r.load(case.path)
    assert r.replay(recording, code=original, binary=case.binary)['same_result'] == snapshot(True)
    with pytest.raises(r.ReplayError) as error:
        r.replay(recording, code=changed, binary=case.binary)
    assert str(error.value) == snapshot('DIVERGED: tool call 0 differs; no live fallback')


def test_replay_compares_host_visible_arguments(case: Any) -> None:
    case.capture('def f():\n    return 1\nfetch(f)')
    recording = r.load(case.path)
    argument = recording['events'][0]['call']['args'][0]
    assert type(argument).__name__ == snapshot('str')
    changed = f'fetch({argument!r})'
    assert r.replay(recording, code=changed, binary=case.binary)['same_result'] == snapshot(True)


def test_restored_output_budget_matches_fresh_capture(case: Any) -> None:
    code = "print('x' * 200000, end='')\nn = fetch()\nprint('y' * n, end='')\n1"
    r.capture(code, case.path, lambda call: {'return_value': 20000}, binary=case.binary)
    recording = r.load(case.path)
    assert r.replay(recording, binary=case.binary)['same_result'] == snapshot(True)
    branch = r.replay(recording, at=0, response={'return_value': 100000}, binary=case.binary)
    fresh = r.capture(
        code, Path(case.temp.name) / 'fresh.jsonl', lambda call: {'return_value': 100000}, binary=case.binary
    )
    assert branch['result'] == snapshot(fresh)
    assert fresh['kind'] == snapshot('error')
    assert (len(r.encode(branch['result'])) <= r.MAX_VALUE) == snapshot(True)
    text = '\u00e9' + 'x' * (r.MAX_VALUE - r.OUTPUT_ENTRY_OVERHEAD - 2)
    output = r.Output([['stdout', text]])
    with pytest.raises(MemoryError) as error:
        output('stdout', 'x')
    assert str(error.value) == snapshot('Captured output exceeds 256 KiB')
    assert output.size == snapshot(r.MAX_VALUE)
    assert output.output == [['stdout', text]]


def test_output_entry_overhead_and_restored_limit() -> None:
    count = r.MAX_VALUE // (r.OUTPUT_ENTRY_OVERHEAD + 1)
    prefix = [['stdout' if i % 2 == 0 else 'stderr', 'x'] for i in range(count)]
    output = r.Output(prefix)
    assert output.size == snapshot(262080)
    with pytest.raises(MemoryError) as error:
        output('stdout', 'x')
    assert str(error.value) == snapshot('Captured output exceeds 256 KiB')
    assert output.output == prefix
    assert output.size == snapshot(262080)
    with pytest.raises(r.ReplayError) as error:
        r.Output([*prefix, ['stdout', 'x']])
    assert str(error.value) == snapshot('Captured output exceeds 256 KiB')


@pytest.mark.parametrize(
    'index, message',
    [
        (0, snapshot('RuntimeError: suspension limit 16 exceeded')),
        (7, snapshot('RuntimeError: suspension limit 9 exceeded')),
        (15, snapshot('RuntimeError: suspension limit 1 exceeded')),
    ],
)
def test_late_restore_preserves_suspension_budget(case: Any, index: int, message: str) -> None:
    r.capture('[fetch(i) for i in range(17)]', case.path, lambda call: {'return_value': 1}, binary=case.binary)
    recording = r.load(case.path)
    assert len(recording['events']) == snapshot(16)
    branch = r.replay(recording, at=index, response={'return_value': 1}, binary=case.binary)
    assert branch['result'] == {'kind': 'error', 'message': message, 'output': []}
    case.path.unlink()
    r.capture('[fetch(i) for i in range(16)]', case.path, lambda call: {'return_value': 1}, binary=case.binary)
    assert r.replay(r.load(case.path), at=15, response={'return_value': 1}, binary=case.binary)[
        'same_result'
    ] == snapshot(True)


def test_error_observations_are_not_normalized(case: Any) -> None:
    r.capture('while True:\n    pass', case.path, lambda call: {}, binary=case.binary)
    recording = r.load(case.path)
    result = r.replay(recording, binary=case.binary)
    assert result['result']['kind'] == snapshot('error')
    assert result['same_result'] == snapshot(r.encode(result['original']) == r.encode(result['result']))
    assert (result['comparison'] in ('equal', 'error-observation-differs')) == snapshot(True)
    case.path.unlink()
    case.capture('raise TimeoutError("sandbox supplied")')
    recording = r.load(case.path)
    recording['result']['message'] = 'a different observed error'
    result = r.replay(recording, binary=case.binary)
    assert result['same_result'] == snapshot(False)
    assert result['comparison'] == snapshot('error-observation-differs')
    assert result['result']['message'] == snapshot('TimeoutError: sandbox supplied')
    assert result['original']['message'] == snapshot('a different observed error')


def test_bounded_inputs(case: Any) -> None:
    case.path.write_bytes(b'x' * 10)
    assert r.read_text(case.path, 10, 'Source') == snapshot('x' * 10)
    with pytest.raises(r.ReplayError) as error:
        r.read_text(case.path, 9, 'Source')
    assert str(error.value) == snapshot('Source exceeds 9 bytes')
    case.path.write_bytes(b'\xff')
    with pytest.raises(r.ReplayError) as error:
        r.read_text(case.path, 9, 'Source')
    assert str(error.value) == snapshot('Source is not UTF-8')
    with pytest.raises(r.ReplayError) as error:
        r.encode({'\ud800': None})
    assert str(error.value) == snapshot('Invalid UTF-8 string')
    with pytest.raises(ValueError) as native_error:
        json.dumps(10**5000)
    with pytest.raises(r.ReplayError) as error:
        r.encode(10**5000)
    assert (str(error.value) == str(native_error.value)) == snapshot(True)


@pytest.mark.parametrize(
    'code, message', [(1, snapshot('Invalid source')), ('\ud800', snapshot('Invalid UTF-8 string'))]
)
def test_invalid_source(code: Any, message: str) -> None:
    with pytest.raises(r.ReplayError) as error:
        r.check_code(code)
    assert str(error.value) == message


@pytest.mark.parametrize('mode', ['capture', 'changed-source'])
def test_syntax_errors_are_rejected(case: Any, mode: str) -> None:
    recording: dict[str, Any] = {}
    if mode == 'changed-source':
        case.capture()
        recording = r.load(case.path)
    dispatch = MagicMock()
    with pytest.raises(r.ReplayError) as error:
        if mode == 'capture':
            r.capture('if :', case.path, dispatch, binary=case.binary)
        else:
            r.replay(recording, code='if :', binary=case.binary)
    assert str(error.value) == snapshot('Invalid source: Expected an expression')
    assert type(error.value.__cause__).__name__ == snapshot('MontySyntaxError')
    assert dispatch.call_count == snapshot(0)
    if mode == 'capture':
        with pytest.raises(r.ReplayError) as incomplete:
            r.load(case.path)
        assert str(incomplete.value) == snapshot('Capture incomplete: completion record is missing')
    else:
        assert r.load(case.path)['sha256'] == snapshot(recording['sha256'])


def test_replay_does_not_enable_type_checking(case: Any) -> None:
    case.capture('value: int = "text"\nvalue')
    result = r.replay(r.load(case.path), code='value: int = "changed"\nvalue', binary=case.binary)
    assert result['result'] == snapshot({'kind': 'return', 'value': 'changed', 'output': []})


@pytest.mark.parametrize(
    'key, value, message',
    [
        ('mode', 'bad', snapshot('Unexpected response override')),
        ('at', True, snapshot('Invalid branch comparison')),
        ('same_result', 'false', snapshot('Invalid branch comparison')),
        ('comparison', 'equal', snapshot('Invalid branch comparison')),
        ('original', None, snapshot('Branch original differs from recording')),
        ('result', {'kind': 'error'}, snapshot('Invalid object fields')),
        ('extra', None, snapshot('Invalid object fields')),
    ],
)
def test_branch_validation(case: Any, key: str, value: Any, message: str) -> None:
    case.capture()
    recording = r.load(case.path)
    original = r.replay(recording, at=1, response={'return_value': 9}, binary=case.binary)
    with pytest.raises(r.ReplayError) as error:
        app.report(recording, {**original, key: value})
    assert str(error.value) == message


@pytest.mark.parametrize(
    'field, message',
    [('MAX_CALLS', snapshot('Too many calls')), ('MAX_SNAPSHOT', snapshot('Snapshot exceeds 512 KiB'))],
)
def test_capture_limits(case: Any, field: str, message: str) -> None:
    with patch.object(r, field, 0), pytest.raises(r.ReplayError) as error:
        case.capture()
    assert str(error.value) == message


def test_final_result_mismatch(case: Any) -> None:
    case.capture()
    recording = r.load(case.path)
    recording['result']['value'] = 999
    with pytest.raises(r.ReplayError) as error:
        r.replay(recording, binary=case.binary)
    assert str(error.value) == snapshot('DIVERGED: final result differs')


def test_live_adapter_boundary() -> None:
    good: dict[str, Any] = {'name': 'package_metadata', 'args': ['pydantic-ai-slim'], 'kwargs': {}}
    response = MagicMock()
    response.__enter__.return_value = response
    opener = MagicMock()
    opener.open.return_value = response
    with patch.object(pypi_tools.urllib.request, 'build_opener', return_value=opener) as build:
        info: dict[str, Any] = {'version': '1', 'requires_python': '>=3.10', 'requires_dist': []}
        response.read.return_value = json.dumps({'info': info}).encode()
        raw = pypi_tools.read_package('pydantic-ai-slim')
        assert raw == snapshot(json.dumps({'info': info}).encode())
        request = opener.open.call_args.args[0]
        assert (request.full_url, request.get_header('User-agent')) == snapshot(
            ('https://pypi.org/pypi/pydantic-ai-slim/json', 'monty-snapshot-replay-example/0.1')
        )
        assert build.call_args.args[0].proxies == snapshot({})
        assert response.read.call_args.args == snapshot((1024 * 1024 + 1,))
        with io.TextIOWrapper(io.BytesIO()) as output:
            with patch.object(sys, 'argv', [pypi_tools.__file__, 'pydantic-ai-slim']), redirect_stdout(output):
                runpy.run_path(pypi_tools.__file__, run_name='__main__')
            assert output.buffer.getvalue() == snapshot(raw)
    with (
        patch.object(pypi_tools.subprocess, 'run') as run,
        patch.dict(os.environ, {'SystemRoot': 'system-directory', 'PRIVATE_TOKEN': 'test-only'}, clear=True),
    ):
        run.return_value.stdout = raw
        assert pypi_tools.dispatch(good) == snapshot({'return_value': info})
        assert run.call_args.args[0] == snapshot(
            [sys.executable, '-I', str(Path(pypi_tools.__file__).resolve()), 'pydantic-ai-slim']
        )
        creationflags = getattr(subprocess, 'CREATE_NO_WINDOW', 0)
        assert run.call_args.kwargs == snapshot(
            {
                'stdin': subprocess.DEVNULL,
                'capture_output': True,
                'check': True,
                'timeout': 10,
                'env': {'SystemRoot': 'system-directory'},
                'creationflags': creationflags,
            }
        )
        run.return_value.stdout = b'x' * (1024 * 1024 + 1)
        with pytest.raises(ValueError) as error:
            pypi_tools.dispatch(good)
        assert str(error.value) == snapshot('PyPI response exceeds 1 MiB')
    assert pypi_tools.NoRedirect().redirect_request(None, None, 302, 'Found', None, 'http://127.0.0.1') == snapshot(
        None
    )


@pytest.mark.parametrize('phase', ['complete', 'headers', 'body', 'http-error'])
def test_pypi_deadline_stops_the_fetch(tmp_path: Path, phase: str) -> None:
    info: dict[str, Any] = {'version': '1', 'requires_python': '>=3.10', 'requires_dist': []}
    body = json.dumps({'info': info}).encode()
    status = '404 Not Found' if phase == 'http-error' else '200 OK'
    headers = f'HTTP/1.1 {status}\r\nX-Padding: {"x" * 200}\r\nContent-Length: {len(body)}\r\n\r\n'.encode()
    stopped = threading.Event()
    sent: list[int] = []
    children: list[subprocess.Popen[Any]] = []
    popen = subprocess.Popen

    def spawn(*args: Any, **kwargs: Any) -> subprocess.Popen[Any]:
        child = popen(*args, **kwargs)
        children.append(child)
        return child

    with socket.socket() as server:
        server.bind(('127.0.0.1', 0))
        server.listen(1)
        server.settimeout(5)
        endpoint = f'http://127.0.0.1:{server.getsockname()[1]}/'

        def serve() -> None:
            try:
                with server.accept()[0] as connection:
                    connection.settimeout(5)
                    connection.recv(4096)
                    if phase == 'headers':
                        data = headers + body
                    else:
                        connection.sendall(headers)
                        data = body
                    if phase in ('complete', 'http-error'):
                        connection.sendall(data)
                    else:
                        for byte in data:
                            connection.sendall(bytes([byte]))
                            sent.append(byte)
                            if stopped.wait(0.05):
                                break
            except (TimeoutError, BrokenPipeError, ConnectionResetError):
                pass

        helper = tmp_path / 'fetch.py'
        # Redirect only the test child's request to loopback; run the actual adapter entry point.
        helper.write_text(
            'import runpy, urllib.request\n'
            'request = urllib.request.Request\n'
            f'urllib.request.Request = lambda url, **kw: request({endpoint!r}, **kw)\n'
            f'runpy.run_path({pypi_tools.__file__!r}, run_name="__main__")\n'
        )
        serving = threading.Thread(target=serve)
        serving.start()
        try:
            with (
                patch.object(pypi_tools, '__file__', str(helper)),
                patch.object(pypi_tools, 'REQUEST_TIMEOUT', 2),
                patch.object(subprocess, 'Popen', side_effect=spawn),
            ):
                call: dict[str, Any] = {'name': 'package_metadata', 'args': ['pydantic-ai-slim'], 'kwargs': {}}
                if phase == 'complete':
                    assert pypi_tools.dispatch(call) == snapshot({'return_value': info})
                elif phase == 'http-error':
                    with pytest.raises(subprocess.CalledProcessError) as failure:
                        pypi_tools.dispatch(call)
                    assert failure.value.returncode == snapshot(1)
                else:
                    with pytest.raises(TimeoutError) as error:
                        pypi_tools.dispatch(call)
                    assert str(error.value) == snapshot('PyPI request exceeded its deadline')
                    assert type(error.value.__cause__).__name__ == snapshot('TimeoutExpired')
                    assert (0 < len(sent) < len(headers + body if phase == 'headers' else body)) == snapshot(True)
            assert len(children) == snapshot(1)
            assert (children[0].returncode is not None) == snapshot(True)
        finally:
            stopped.set()
            serving.join(timeout=6)
        assert serving.is_alive() == snapshot(False)


def test_capture_stops_after_http_deadline(case: Any) -> None:
    with patch.object(subprocess, 'run', side_effect=subprocess.TimeoutExpired('fetch', 10)) as run:
        with pytest.raises(TimeoutError) as error:
            r.capture('package_metadata("pydantic-ai-slim")', case.path, pypi_tools.dispatch, binary=case.binary)
    assert str(error.value) == snapshot('PyPI request exceeded its deadline')
    assert run.call_count == snapshot(1)
    rows = [json.loads(line) for line in case.path.read_text().splitlines()]
    assert [row['type'] for row in rows] == snapshot(['header', 'call'])
    with pytest.raises(r.ReplayError) as incomplete:
        r.load(case.path)
    assert str(incomplete.value) == snapshot('Capture incomplete: last call has no recorded response')


def test_cli_capture_replay_branch_report(case: Any) -> None:
    source = Path(case.temp.name) / 'source.py'
    source.write_text('fetch("a") + 1')

    def cli(*arguments: Any) -> None:
        with (
            patch.object(sys, 'argv', ['snapshot-replay', '--binary', str(case.binary), *map(str, arguments)]),
            redirect_stdout(io.StringIO()),
        ):
            app.main()

    with patch.object(pypi_tools, 'dispatch', return_value={'return_value': 4}):
        cli('capture', case.path, '--code', source)
    cli('replay', case.path)
    cli('replay', case.path, '--code', source)
    response = Path(case.temp.name) / 'response.json'
    response.write_text('{"return_value":9}')
    branch = Path(case.temp.name) / 'branch.json'
    cli('branch', case.path, '--at', 0, '--response', response, '--output', branch)
    assert json.loads(branch.read_text())['result']['value'] == snapshot(10)
    report = Path(case.temp.name) / 'report.html'
    cli('report', case.path, '--branch', branch, '--output', report)
    assert ('SIMULATED' in report.read_text()) == snapshot(True)
    with pytest.raises(FileExistsError) as error:
        cli('report', case.path, '--output', report)
    assert error.value.strerror == snapshot('File exists')
    assert error.value.filename == snapshot(str(report))
    broken = json.loads(branch.read_text())
    broken['recording_sha256'] = 'different'
    branch.write_text(json.dumps(broken))
    rejected = Path(case.temp.name) / 'rejected.html'
    with pytest.raises(r.ReplayError) as error:
        cli('report', case.path, '--branch', branch, '--output', rejected)
    assert str(error.value) == snapshot('Branch belongs to a different recording')
    assert rejected.exists() == snapshot(False)


def test_comparison_preserves_what_if_inputs(case: Any) -> None:
    case.capture()
    recording = r.load(case.path)
    override = {'return_value': 9}
    branch = r.replay(recording, at=1, response=override, binary=case.binary)
    assert branch['response'] == snapshot({'return_value': 9})
    assert branch['code'] == snapshot(None)
    source = 'fetch("a") - fetch("b")'
    changed = r.replay(recording, code=source, binary=case.binary)
    assert changed['code'] == snapshot('fetch("a") - fetch("b")')
    assert changed['response'] == snapshot(None)
    assert ('changed-source' in app.report(recording, changed)) == snapshot(True)
    replayed = r.replay(recording, binary=case.binary)
    with pytest.raises(r.ReplayError) as error:
        app.report(recording, {**replayed, 'response': override})
    assert str(error.value) == snapshot('Unexpected response override')
    with pytest.raises(r.ReplayError) as error:
        app.report(recording, {**branch, 'code': source})
    assert str(error.value) == snapshot('Unexpected source override')
    with pytest.raises(r.ReplayError) as error:
        app.report(recording, {**changed, 'code': None})
    assert str(error.value) == snapshot('Invalid source')


@pytest.mark.parametrize('argument', [None, True, [], {}, 'other', 'https://example.com', '../pydantic-ai-slim'])
def test_pypi_arguments_rejected_before_network(argument: Any) -> None:
    with patch.object(pypi_tools.subprocess, 'run') as open_network:
        with pytest.raises(ValueError) as error:
            pypi_tools.dispatch({'name': 'package_metadata', 'args': [argument], 'kwargs': {}})
    assert str(error.value) == snapshot('Tool or package is not allowlisted')
    assert open_network.call_count == snapshot(0)


def test_pypi_child_rejects_unlisted_package() -> None:
    with patch.object(pypi_tools.urllib.request, 'build_opener') as open_network:
        with pytest.raises(ValueError) as error:
            pypi_tools.read_package('../pydantic-ai-slim')
    assert str(error.value) == snapshot('Package is not allowlisted')
    assert open_network.call_count == snapshot(0)


@pytest.mark.parametrize(
    'override',
    [
        {'name': 'other'},
        {'args': []},
        {'args': ['pydantic-ai-slim', 'extra']},
        {'kwargs': {'url': 'https://example.com'}},
    ],
    ids=['function', 'missing-argument', 'extra-argument', 'keyword'],
)
def test_pypi_call_shape_rejected_before_network(override: dict[str, Any]) -> None:
    call: dict[str, Any] = {'name': 'package_metadata', 'args': ['pydantic-ai-slim'], 'kwargs': {}, **override}
    with patch.object(pypi_tools.subprocess, 'run') as open_network:
        with pytest.raises(ValueError) as error:
            pypi_tools.dispatch(call)
    assert str(error.value) == snapshot('Tool or package is not allowlisted')
    assert open_network.call_count == snapshot(0)


def write_sealed_recording(path: Path, rows: list[dict[str, Any]]) -> None:
    # Keep the checksum valid so structural tests reach the validation they target.
    body = b''.join(r.encode(row) + b'\n' for row in rows[:-1])
    end = {key: value for key, value in rows[-1].items() if key != 'seal_sha256'}
    end['seal_sha256'] = hashlib.sha256(body + r.encode(end)).hexdigest()
    path.write_bytes(body + r.encode(end) + b'\n')
