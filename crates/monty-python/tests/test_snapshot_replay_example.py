"""Regression tests for the synchronous snapshot replay example."""

from __future__ import annotations

import copy
import hashlib
import io
import json
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


def test_guest_exception_caught_and_uncaught(case: Any) -> None:
    kinds: list[str] = []
    for code in [
        'try:\n    fetch("a")\nexcept ValueError:\n    result = 7\nresult',
        'fetch("a")',
        '1 / 0',
    ]:
        if case.path.exists():
            case.path.unlink()
        r.capture(code, case.path, lambda call: {'exc_type': 'ValueError', 'message': 'bad'}, binary=case.binary)
        recording = r.load(case.path)
        kinds.append(r.replay(recording, binary=case.binary)['result']['kind'])
    assert kinds == snapshot(['return', 'error', 'error'])


def test_divergence_has_no_dispatcher(case: Any) -> None:
    case.capture()
    recording = r.load(case.path)
    for code in ['fetch("different")', 'other("a")', 'fetch("a")', 'fetch("a")\nfetch("b")\nfetch("c")']:
        with pytest.raises(r.ReplayError, match='DIVERGED'):
            r.replay(recording, code=code, binary=case.binary)
    with pytest.raises(r.ReplayError, match='DIVERGED'):
        r.replay(recording, code='fetch(True)', binary=case.binary)


def test_unknown_and_exclusive_creation(case: Any) -> None:
    def crash(call: dict[str, Any]) -> dict[str, Any]:
        raise OSError('host interrupted')

    with pytest.raises(OSError):
        r.capture('fetch("a")', case.path, crash, binary=case.binary)
    original = case.path.read_bytes()
    assert (b'"type":"call"' in original) == snapshot(True)
    with pytest.raises(r.ReplayError, match='UNKNOWN'):
        r.load(case.path)
    with pytest.raises(FileExistsError):
        case.capture()
    assert original == snapshot(case.path.read_bytes())


def test_truncation_and_checksum(case: Any) -> None:
    case.capture()
    original = case.path.read_bytes()
    for broken in [original[:-1], original.replace(b'"value":12', b'"value":99'), b'']:
        case.path.write_bytes(broken)
        with pytest.raises(r.ReplayError):
            r.load(case.path)


def test_runtime_and_branch_parameters(case: Any) -> None:
    case.capture()
    recording = r.load(case.path)
    invalid_arguments: list[dict[str, Any]] = [
        {'at': -1, 'response': {'return_value': 0}},
        {'at': 0},
        {'response': {'return_value': 0}},
        {'at': True, 'response': {'return_value': 0}},
        {'at': 0, 'response': {'return_value': 0}, 'code': '1'},
        {'code': 'x' * (r.MAX_SOURCE + 1)},
    ]
    for kwargs in invalid_arguments:
        with pytest.raises(r.ReplayError):
            r.replay(recording, **kwargs, binary=case.binary)
    recording['header']['runtime']['monty'] = 'wrong'
    with pytest.raises(r.ReplayError, match='Runtime differs'):
        r.replay(recording, binary=case.binary)


def test_json_boundary(case: Any) -> None:
    for value in [float('nan'), float('inf'), (1,), {1: 'value'}, object()]:
        with pytest.raises(r.ReplayError):
            r.encode(value)
    assert (r.encode(True) != r.encode(1)) == snapshot(True)
    assert (r.encode(1) != r.encode(1.0)) == snapshot(True)
    assert r.json_value(1.5) == snapshot(1.5)
    nested = None
    for _ in range(34):
        nested = [nested]
    with pytest.raises(r.ReplayError):
        r.encode(nested)
    with pytest.raises(r.ReplayError):
        r.bounded_value('x' * r.MAX_VALUE)
    bad_responses: list[Any] = [[], {'future': 1}, {'exc_type': 'Unknown', 'message': 'x'}]
    for response in bad_responses:
        with pytest.raises(r.ReplayError):
            r.check_response(response)


def test_limits_and_unsupported_suspension(case: Any) -> None:
    with pytest.raises(r.ReplayError):
        r.capture('x' * (r.MAX_SOURCE + 1), case.path, lambda call: {}, binary=case.binary)
    with pytest.raises(r.ReplayError, match='Only direct'):
        case.capture('unknown_name')
    with pytest.raises(r.ReplayError, match='Unsupported'):
        r.terminal(None, r.Output())


def test_saved_file_limits(case: Any) -> None:
    case.path.write_bytes(b'x' * (r.MAX_FILE + 1))
    with pytest.raises(r.ReplayError, match='8 MiB'):
        r.load(case.path)
    case.path.unlink()
    journal = r.Journal(case.path)
    try:
        with patch.object(r, 'MAX_FILE', 1), pytest.raises(r.ReplayError):
            journal.append({'a': 1})
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
    with pytest.raises(r.ReplayError):
        app.report(recording, branch)


def test_record_structure_rejects_resealed_invalid_data(case: Any) -> None:
    case.capture()
    original = [json.loads(line) for line in case.path.read_bytes().splitlines()]
    variants: list[list[dict[str, Any]]] = []
    mutations: list[tuple[int, str, Any]] = [
        (0, 'schema', 9),
        (0, 'schema', 1),
        (0, 'schema', True),
        (0, 'code', 42),
        (0, 'limits', {}),
        (0, 'runtime', None),
        (0, 'runtime', {'monty': False, 'worker_sha256': '0' * 64}),
        (0, 'runtime', {'monty': '0.0.23', 'worker_sha256': 'bad'}),
        (1, 'index', 99),
        (1, 'index', False),
        (2, 'index', False),
        (1, 'snapshot_sha256', 'bad'),
        (1, 'snapshot', '!'),
        (1, 'call', []),
        (1, 'call', {'name': False, 'args': [], 'kwargs': {}}),
        (1, 'call', {'name': 'fetch', 'args': {}, 'kwargs': {}}),
        (1, 'stdout_before', []),
        (3, 'stdout_before', 'unrelated'),
        (-1, 'calls', 99),
        (-1, 'result', 'bad'),
        (-1, 'result', {'kind': 'potato'}),
        (-1, 'result', {'kind': 'return', 'value': 1}),
        (-1, 'result', {'kind': 'error', 'message': [], 'stdout': ''}),
        (-1, 'result', {'kind': 'return', 'value': 1, 'stdout': 'unrelated'}),
    ]
    for row, key, value in mutations:
        rows = copy.deepcopy(original)
        rows[row][key] = value
        variants.append(rows)
    variants.append([original[0], original[1], original[-1]])
    rows = copy.deepcopy(original)
    del rows[1]['call']
    variants.append(rows)
    for rows in variants:
        body = b''.join(r.encode(row) + b'\n' for row in rows[:-1])
        end = {key: value for key, value in rows[-1].items() if key != 'seal_sha256'}
        end['seal_sha256'] = hashlib.sha256(body + r.encode(end)).hexdigest()
        case.path.write_bytes(body + r.encode(end) + b'\n')
        with pytest.raises(r.ReplayError):
            r.load(case.path)
    for raw in [
        b'[]\n',
        b'{broken}\n',
        b'{"n":' + b'1' * 5000 + b'}\n',
        b'{"a":1,"a":2}\n',
        b'{"a":"\\ud800"}\n',
        b'{"a":NaN}\n',
    ]:
        case.path.write_bytes(raw)
        with pytest.raises(r.ReplayError):
            r.load(case.path)


def test_mapping_order_survives_storage_and_comparison(case: Any) -> None:
    r.capture('list(fetch().keys())', case.path, lambda call: {'return_value': {'b': 1, 'a': 2}}, binary=case.binary)
    recording = r.load(case.path)
    assert list(recording['events'][0]['response']['return_value']) == snapshot(['b', 'a'])
    assert r.replay(recording, binary=case.binary)['result']['value'] == snapshot(['b', 'a'])
    assert r.replay(recording, binary=case.binary)['comparison'] == snapshot('equal')
    assert app.differences({'b': 1, 'a': 2}, {'a': 2, 'b': 1}) == snapshot([('$', {'b': 1, 'a': 2}, {'a': 2, 'b': 1})])
    for original, changed in [
        ("fetch({'b': 1, 'a': 2})", "fetch({'a': 2, 'b': 1})"),
        ('fetch(b=1, a=2)', 'fetch(a=2, b=1)'),
    ]:
        case.path.unlink()
        r.capture(original, case.path, lambda call: {'return_value': 1}, binary=case.binary)
        recording = r.load(case.path)
        assert r.replay(recording, code=original, binary=case.binary)['same_result'] == snapshot(True)
        with pytest.raises(r.ReplayError, match='tool call 0 differs'):
            r.replay(recording, code=changed, binary=case.binary)


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
    with pytest.raises(MemoryError):
        output('stdout', 'x')
    assert len(output.output.encode()) == snapshot(r.MAX_VALUE)


def test_late_restore_preserves_suspension_budget(case: Any) -> None:
    r.capture('[fetch(i) for i in range(17)]', case.path, lambda call: {'return_value': 1}, binary=case.binary)
    recording = r.load(case.path)
    assert len(recording['events']) == snapshot(16)
    for index in (0, 7, 15):
        branch = r.replay(recording, at=index, response={'return_value': 1}, binary=case.binary)
        assert branch['result']['kind'] == snapshot('error')
        assert ('suspension' in branch['result']['message'].lower()) == snapshot(True)
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
    assert ('sandbox supplied' in result['result']['message']) == snapshot(True)
    assert result['original']['message'] == snapshot('a different observed error')


def test_bounded_inputs_and_branch_validation(case: Any) -> None:
    case.path.write_bytes(b'x' * 10)
    assert r.read_text(case.path, 10, 'Source') == snapshot('x' * 10)
    with pytest.raises(r.ReplayError):
        r.read_text(case.path, 9, 'Source')
    case.path.write_bytes(b'\xff')
    with pytest.raises(r.ReplayError):
        r.read_text(case.path, 9, 'Source')
    for code in (1, '\ud800'):
        with pytest.raises(r.ReplayError):
            r.check_code(code)
    with pytest.raises(r.ReplayError):
        r.encode({'\ud800': None})
    with pytest.raises(r.ReplayError):
        r.encode(10**5000)
    case.path.unlink()
    case.capture()
    recording = r.load(case.path)
    original = r.replay(recording, at=1, response={'return_value': 9}, binary=case.binary)
    for key, value in [
        ('mode', 'bad'),
        ('at', True),
        ('same_result', 'false'),
        ('comparison', 'equal'),
        ('original', None),
        ('result', {'kind': 'error'}),
        ('extra', None),
    ]:
        with pytest.raises(r.ReplayError):
            app.report(recording, {**original, key: value})
    assert ('What changed' in app.report(recording, r.replay(recording, binary=case.binary))) == snapshot(True)


def test_capture_limits_and_final_result_mismatch(case: Any) -> None:
    for field in ['MAX_CALLS', 'MAX_SNAPSHOT']:
        with patch.object(r, field, 0), pytest.raises(r.ReplayError):
            case.capture()
        case.path.unlink()
    case.capture()
    recording = r.load(case.path)
    recording['result']['value'] = 999
    with pytest.raises(r.ReplayError, match='final result differs'):
        r.replay(recording, binary=case.binary)


def test_live_adapter_boundary(case: Any) -> None:
    good: dict[str, Any] = {'name': 'package_metadata', 'args': ['pydantic-ai-slim'], 'kwargs': {}}
    with pytest.raises(ValueError):
        pypi_tools.dispatch({**good, 'args': ['other']})
    with pytest.raises(ValueError):
        pypi_tools.dispatch({**good, 'kwargs': {'url': 'https://example.com'}})
    response = MagicMock()
    response.__enter__.return_value = response
    opener = MagicMock()
    opener.open.return_value = response
    with patch.object(pypi_tools.urllib.request, 'build_opener', return_value=opener):
        info: dict[str, Any] = {'version': '1', 'requires_python': '>=3.10', 'requires_dist': []}
        response.read.return_value = json.dumps({'info': info}).encode()
        assert pypi_tools.dispatch(good) == snapshot({'return_value': info})
        response.read.return_value = b'x' * (1024 * 1024 + 1)
        with pytest.raises(ValueError, match='1 MiB'):
            pypi_tools.dispatch(good)
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
    with pytest.raises(FileExistsError):
        cli('report', case.path, '--output', report)
    broken = json.loads(branch.read_text())
    broken['recording_sha256'] = 'different'
    branch.write_text(json.dumps(broken))
    rejected = Path(case.temp.name) / 'rejected.html'
    with pytest.raises(r.ReplayError):
        cli('report', case.path, '--branch', branch, '--output', rejected)
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
