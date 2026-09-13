"""Regression tests for the synchronous snapshot replay example."""

from __future__ import annotations

import hashlib
import io
import json
import re
import sys
from contextlib import redirect_stdout
from pathlib import Path
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
        {'kind': 'return', 'value': 12, 'stdout': 'before\nbetween\n'}
    )
    branch = r.replay(recording, at=1, response={'return_value': 9}, binary=case.binary)
    assert branch['result']['value'] == snapshot(19)
    assert branch['result']['stdout'] == snapshot('before\nbetween\n')
    assert r.replay(recording, code='x=fetch("a")\ny=fetch("b")\nx-y', binary=case.binary)['result'][
        'value'
    ] == snapshot(8)
    assert r.load(case.path)['sha256'] == snapshot(recording['sha256'])


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
            snapshot({'kind': 'return', 'value': 7, 'stdout': ''}),
        ),
        ('fetch("a")', snapshot({'kind': 'error', 'message': 'ValueError: bad', 'stdout': ''})),
        ('1 / 0', snapshot({'kind': 'error', 'message': 'ZeroDivisionError: division by zero', 'stdout': ''})),
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
    'row, key, value, message',
    [
        (0, 'schema', 9, snapshot('Unsupported recording schema; capture again with schema 2')),
        (0, 'schema', 1, snapshot('Unsupported recording schema; capture again with schema 2')),
        (0, 'schema', True, snapshot('Unsupported recording schema; capture again with schema 2')),
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
        (1, 'stdout_before', [], snapshot('Invalid output prefix')),
        (3, 'stdout_before', 'unrelated', snapshot('Invalid output prefix')),
        (-1, 'calls', 99, snapshot('Call count mismatch')),
        (-1, 'result', 'bad', snapshot('Invalid terminal result')),
        (-1, 'result', {'kind': 'potato'}, snapshot('Invalid terminal result')),
        (-1, 'result', {'kind': 'return', 'value': 1}, snapshot('Invalid object fields')),
        (-1, 'result', {'kind': 'error', 'message': [], 'stdout': ''}, snapshot('Invalid terminal text')),
        (-1, 'result', {'kind': 'return', 'value': 1, 'stdout': 'unrelated'}, snapshot('Invalid final output prefix')),
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
    output = r.Output('\u00e9' * (r.MAX_VALUE // 2))
    with pytest.raises(MemoryError) as error:
        output('stdout', 'x')
    assert str(error.value) == snapshot('Captured output exceeds 256 KiB')
    assert len(output.output.encode()) == snapshot(r.MAX_VALUE)


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
    assert branch['result'] == {'kind': 'error', 'message': message, 'stdout': ''}
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


def test_live_adapter_boundary(case: Any) -> None:
    good: dict[str, Any] = {'name': 'package_metadata', 'args': ['pydantic-ai-slim'], 'kwargs': {}}
    response = MagicMock()
    response.__enter__.return_value = response
    opener = MagicMock()
    opener.open.return_value = response
    with patch.object(pypi_tools.urllib.request, 'build_opener', return_value=opener):
        info: dict[str, Any] = {'version': '1', 'requires_python': '>=3.10', 'requires_dist': []}
        response.read.return_value = json.dumps({'info': info}).encode()
        assert pypi_tools.dispatch(good) == snapshot({'return_value': info})
        response.read.return_value = b'x' * (1024 * 1024 + 1)
        with pytest.raises(ValueError) as error:
            pypi_tools.dispatch(good)
        assert str(error.value) == snapshot('PyPI response exceeds 1 MiB')
    assert pypi_tools.NoRedirect().redirect_request(None, None, 302, 'Found', None, 'http://127.0.0.1') == snapshot(
        None
    )


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
    with patch.object(pypi_tools.urllib.request, 'build_opener') as open_network:
        with pytest.raises(ValueError) as error:
            pypi_tools.dispatch({'name': 'package_metadata', 'args': [argument], 'kwargs': {}})
    assert str(error.value) == snapshot('Tool or package is not allowlisted')
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
    with patch.object(pypi_tools.urllib.request, 'build_opener') as open_network:
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
