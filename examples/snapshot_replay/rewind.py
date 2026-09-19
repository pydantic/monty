"""Record synchronous host calls and replay their saved responses in fresh workers."""

from __future__ import annotations

import base64
import binascii
import hashlib
import importlib.metadata
import json
import math
import os
from collections.abc import Callable
from pathlib import Path
from typing import Any, BinaryIO, Literal, cast

from pydantic_monty import (
    ExternalResult,
    FunctionSnapshot,
    Monty,
    MontyComplete,
    MontyRuntimeError,
    MontySyntaxError,
    ResourceLimits,
)

MAX_FILE = 8 * 1024 * 1024
MAX_VALUE = 256 * 1024
MAX_SNAPSHOT = 512 * 1024
MAX_CALLS = 16
MAX_SOURCE = 32 * 1024
OUTPUT_ENTRY_OVERHEAD = 64
LIMITS: ResourceLimits = {'max_memory': 64 * 1024 * 1024, 'max_feed_duration_secs': 2, 'max_suspensions': MAX_CALLS}


class ReplayError(ValueError):
    """The recording cannot support the requested replay or comparison."""


def capture(
    code: str, path: str | Path, dispatch: Callable[[dict[str, Any]], dict[str, Any]], *, binary: Path
) -> dict[str, Any]:
    """Flush each pending call before dispatch; never retry an interrupted callback."""
    check_code(code)
    identity = runtime(binary)
    journal = Journal(path)
    try:
        journal.append(
            {
                'type': 'header',
                'schema': 3,
                'runtime': identity,
                'code': code,
                'code_sha256': hashlib.sha256(code.encode()).hexdigest(),
                'limits': LIMITS,
            }
        )
        output = Output()
        with pool(binary) as workers, workers.checkout(limits=LIMITS) as session:
            count = 0
            try:
                progress = session.feed_start(code, print_callback=output)
                while not isinstance(progress, MontyComplete):
                    call = call_identity(progress)
                    progress = cast(FunctionSnapshot, progress)
                    if count >= MAX_CALLS:
                        raise ReplayError('Too many calls')
                    snapshot = progress.dump()
                    if len(snapshot) > MAX_SNAPSHOT:
                        raise ReplayError('Snapshot exceeds 512 KiB')
                    journal.append(
                        {
                            'type': 'call',
                            'index': count,
                            'call': call,
                            'snapshot': base64.b64encode(snapshot).decode(),
                            'snapshot_sha256': hashlib.sha256(snapshot).hexdigest(),
                            'output_before': output.output,
                        }
                    )
                    # A missing response does not prove whether the callback ran.
                    response = bounded_value(dispatch(call))
                    check_response(response)
                    journal.append({'type': 'response', 'index': count, 'response': response})
                    count += 1
                    progress = progress.resume(cast(ExternalResult, response))
                result = terminal(progress, output)
            except MontySyntaxError as error:
                raise ReplayError(f'Invalid source: {error}') from error
            except MontyRuntimeError as error:
                result = failure(error, output)
        end = {'type': 'complete', 'calls': count, 'result': result}
        seal = journal.digest.copy()
        seal.update(encode(end))
        journal.append({**end, 'seal_sha256': seal.hexdigest()})
        return result
    finally:
        journal.close()


def load(path: str | Path) -> dict[str, Any]:
    """Validate a locally owned journal without restoring any snapshot."""
    with open(path, 'rb') as source:
        data = source.read(MAX_FILE + 1)
    if len(data) > MAX_FILE:
        raise ReplayError('Recording exceeds 8 MiB')
    lines = data.splitlines(keepends=True)
    if not lines:
        raise ReplayError('Recording is empty')
    if not lines[-1].endswith(b'\n'):
        raise ReplayError('Recording is truncated: final newline is missing')
    try:
        records = [parse_json(line) for line in lines]
        if any(type(row) is not dict for row in records):
            raise ReplayError('Invalid recording row')
        header, end = records[0], records[-1]
        if header.get('type') != 'header' or type(header.get('schema')) is not int or header['schema'] != 3:
            raise ReplayError('Unsupported recording schema; capture again with schema 3')
        if end.get('type') != 'complete':
            if end.get('type') == 'call':
                raise ReplayError('Capture incomplete: last call has no recorded response')
            raise ReplayError('Capture incomplete: completion record is missing')
        fields(header, 'type schema runtime code code_sha256 limits')
        fields(header['runtime'], 'monty worker_sha256')
        if type(header['runtime']['monty']) is not str or not header['runtime']['monty']:
            raise ReplayError('Invalid runtime version')
        worker_hash = header['runtime']['worker_sha256']
        if (
            type(worker_hash) is not str
            or len(worker_hash) != 64
            or any(c not in '0123456789abcdef' for c in worker_hash)
        ):
            raise ReplayError('Invalid worker hash')
        fields(end, 'type calls result seal_sha256')
        seal = hashlib.sha256(b''.join(lines[:-1]))
        seal.update(encode({key: value for key, value in end.items() if key != 'seal_sha256'}))
        if end['seal_sha256'] != seal.hexdigest():
            raise ReplayError('Recording checksum mismatch')
        check_code(header['code'])
        if header['code_sha256'] != hashlib.sha256(header['code'].encode()).hexdigest() or encode(
            header['limits']
        ) != encode(LIMITS):
            raise ReplayError('Source or limits mismatch')
        events: list[dict[str, Any]] = []
        middle = records[1:-1]
        if len(middle) % 2 or len(middle) > MAX_CALLS * 2:
            raise ReplayError('Invalid call/response sequence')
        for index in range(0, len(middle), 2):
            call, response = middle[index : index + 2]
            fields(call, 'type index call snapshot snapshot_sha256 output_before')
            fields(response, 'type index response')
            if (
                call['type'] != 'call'
                or response['type'] != 'response'
                or type(call['index']) is not int
                or type(response['index']) is not int
                or call['index'] != index // 2
                or response['index'] != index // 2
            ):
                raise ReplayError('Invalid call/response sequence')
            bounded_value(call['call'])
            fields(call['call'], 'name args kwargs')
            if (
                type(call['call']['name']) is not str
                or not call['call']['name']
                or type(call['call']['args']) is not list
                or type(call['call']['kwargs']) is not dict
            ):
                raise ReplayError('Invalid call identity')
            check_output(call['output_before'])
            if not has_output_prefix(call['output_before'], events[-1]['output_before'] if events else []):
                raise ReplayError('Invalid output prefix')
            snapshot = base64.b64decode(call['snapshot'], validate=True)
            if len(snapshot) > MAX_SNAPSHOT or hashlib.sha256(snapshot).hexdigest() != call['snapshot_sha256']:
                raise ReplayError('Snapshot checksum or size mismatch')
            check_response(response['response'])
            events.append({**call, 'response': response['response']})
        if type(end['calls']) is not int or end['calls'] != len(events):
            raise ReplayError('Call count mismatch')
        check_result(end['result'])
        if events and not has_output_prefix(end['result']['output'], events[-1]['output_before']):
            raise ReplayError('Invalid final output prefix')
        return {'header': header, 'events': events, 'result': end['result'], 'sha256': hashlib.sha256(data).hexdigest()}
    except (KeyError, TypeError, UnicodeError, RecursionError, binascii.Error) as error:
        raise ReplayError('Malformed recording') from error


def replay(
    recording: dict[str, Any],
    *,
    binary: Path,
    at: int | None = None,
    response: dict[str, Any] | None = None,
    code: str | None = None,
) -> dict[str, Any]:
    """Restore a trusted snapshot, or run changed source against the recorded calls."""
    if recording['header']['runtime'] != runtime(binary):
        raise ReplayError('Runtime differs from capture; rebuild with the recorded version')
    events = recording['events']
    if at is not None and (type(at) is not int or not 0 <= at < len(events) or response is None or code is not None):
        raise ReplayError('Branch requires a valid call index and response, without changed source')
    if response is not None:
        check_response(response)
        if at is None:
            raise ReplayError('Response override requires a call index')
    if code is not None:
        check_code(code)
    index = at if at is not None else 0
    restoring = bool(events) and code is None
    output = Output(events[index]['output_before'] if restoring else None)
    # A loaded call counts as the first suspension of the new checkout.
    limits: ResourceLimits = {**LIMITS, 'max_suspensions': MAX_CALLS - index if restoring else MAX_CALLS}
    with pool(binary) as workers, workers.checkout(limits=limits) as session:
        try:
            if restoring:
                progress = session.load_snapshot(base64.b64decode(events[index]['snapshot']), print_callback=output)
            else:
                progress = session.feed_start(
                    code if code is not None else recording['header']['code'], print_callback=output
                )
            while not isinstance(progress, MontyComplete):
                call = call_identity(progress)
                if index >= len(events) or encode(call) != encode(events[index]['call']):
                    raise ReplayError(f'DIVERGED: tool call {index} differs; no live fallback')
                value = response if index == at else events[index]['response']
                index += 1
                progress = cast(FunctionSnapshot, progress).resume(cast(ExternalResult, value))
            result = terminal(progress, output)
        except MontySyntaxError as error:
            raise ReplayError(f'Invalid source: {error}') from error
        except MontyRuntimeError as error:
            result = failure(error, output)
    if index != len(events):
        raise ReplayError('DIVERGED: recorded responses remain unused')
    comparison = compare_results(recording['result'], result)
    if at is None and code is None and comparison == 'result-differs':
        raise ReplayError('DIVERGED: final result differs')
    return {
        'mode': 'response-branch' if at is not None else 'changed-source' if code is not None else 'replay',
        'recording_sha256': recording['sha256'],
        'at': at,
        'response': response,
        'code': code,
        'original': recording['result'],
        'result': result,
        'same_result': comparison == 'equal',
        'comparison': comparison,
    }


def json_value(value: Any, depth: int = 0) -> Any:
    if depth > 32:
        raise ReplayError('Value nesting exceeds 32 levels')
    if type(value) is str:
        try:
            value.encode('utf-8')
        except UnicodeError as error:
            raise ReplayError('Invalid UTF-8 string') from error
        return value
    if value is None or type(value) in (bool, int):
        return value
    if type(value) is float and math.isfinite(value):
        return value
    if type(value) is list:
        return [json_value(item, depth + 1) for item in cast(list[Any], value)]
    if type(value) is dict:
        mapping = cast(dict[Any, Any], value)
        if all(type(key) is str for key in mapping):
            return {json_value(key, depth + 1): json_value(item, depth + 1) for key, item in mapping.items()}
    raise ReplayError('Only finite JSON values are supported')


def encode(value: Any) -> bytes:
    try:
        # Python can observe mapping order, including the order of keyword arguments.
        return json.dumps(json_value(value), separators=(',', ':'), allow_nan=False).encode()
    except (ValueError, RecursionError) as error:
        raise ReplayError(str(error)) from error


def parse_json(data: str | bytes) -> Any:
    def object_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise ReplayError('Duplicate JSON key')
            result[key] = value
        return result

    try:
        return json_value(json.loads(data, object_pairs_hook=object_pairs))
    except (ValueError, UnicodeError, RecursionError) as error:
        raise ReplayError('Malformed JSON') from error


def read_text(path: str | Path, limit: int, label: str) -> str:
    with open(path, 'rb') as source:
        data = source.read(limit + 1)
    if len(data) > limit:
        raise ReplayError(f'{label} exceeds {limit} bytes')
    try:
        return data.decode('utf-8')
    except UnicodeError as error:
        raise ReplayError(f'{label} is not UTF-8') from error


def check_code(code: Any) -> None:
    if type(code) is not str:
        raise ReplayError('Invalid source')
    json_value(code)
    if len(code.encode()) > MAX_SOURCE:
        raise ReplayError('Source exceeds 32 KiB')


def fields(value: Any, names: str) -> None:
    if type(value) is not dict or set(cast(dict[str, Any], value)) != set(names.split()):
        raise ReplayError('Invalid object fields')


def check_result(result: Any) -> None:
    bounded_value(result)
    if type(result) is not dict or cast(dict[str, Any], result).get('kind') not in ('return', 'error'):
        raise ReplayError('Invalid terminal result')
    result = cast(dict[str, Any], result)
    fields(result, 'kind output value' if result['kind'] == 'return' else 'kind output message')
    check_output(result['output'])
    if result['kind'] == 'error' and type(result['message']) is not str:
        raise ReplayError('Invalid terminal text')


def check_output(value: Any) -> int:
    bounded_value(value)
    if type(value) is not list:
        raise ReplayError('Invalid captured output')
    size = 0
    previous = None
    for entry in cast(list[Any], value):
        if type(entry) is not list:
            raise ReplayError('Invalid captured output')
        pair = cast(list[Any], entry)
        if (
            len(pair) != 2
            or pair[0] not in ('stdout', 'stderr')
            or pair[0] == previous
            or type(pair[1]) is not str
            or not pair[1]
        ):
            raise ReplayError('Invalid captured output')
        stream, text = cast(list[str], pair)
        size += len(text.encode()) + OUTPUT_ENTRY_OVERHEAD
        previous = stream
    if size > MAX_VALUE:
        raise ReplayError('Captured output exceeds 256 KiB')
    return size


def has_output_prefix(output: list[list[str]], prefix: list[list[str]]) -> bool:
    if not prefix:
        return True
    index = len(prefix) - 1
    return (
        len(output) >= len(prefix)
        and output[:index] == prefix[:index]
        and output[index][0] == prefix[index][0]
        and output[index][1].startswith(prefix[index][1])
    )


def compare_results(before: dict[str, Any], after: dict[str, Any]) -> str:
    if encode(before) == encode(after):
        return 'equal'
    if before['kind'] == after['kind'] == 'error':
        return 'error-observation-differs'
    return 'result-differs'


class Output:
    def __init__(self, prefix: list[list[str]] | None = None) -> None:
        self.size = check_output(prefix) if prefix is not None else 0
        self.output = [entry.copy() for entry in prefix] if prefix is not None else []

    def __call__(self, stream: Literal['stdout', 'stderr'], text: str) -> None:
        if not text:
            return
        same_stream = bool(self.output and self.output[-1][0] == stream)
        size = self.size + len(text.encode()) + (0 if same_stream else OUTPUT_ENTRY_OVERHEAD)
        if size > MAX_VALUE:
            raise MemoryError('Captured output exceeds 256 KiB')
        # Transport chunk boundaries are not part of the observed output.
        if same_stream:
            self.output[-1][1] += text
        else:
            self.output.append([stream, text])
        self.size = size


def bounded_value(value: Any) -> Any:
    if len(encode(value)) > MAX_VALUE:
        raise ReplayError('Value exceeds 256 KiB')
    return value


def runtime(binary: Path) -> dict[str, str]:
    with binary.open('rb') as worker:
        digest = hashlib.sha256()
        for chunk in iter(lambda: worker.read(1024 * 1024), b''):
            digest.update(chunk)
    return {'monty': importlib.metadata.version('pydantic-monty-client'), 'worker_sha256': digest.hexdigest()}


def pool(binary: Path) -> Monty:
    return Monty(binary_path=binary, max_processes=1, request_timeout=5)


def call_identity(progress: Any) -> dict[str, Any]:
    if not isinstance(progress, FunctionSnapshot) or progress.is_os_function or progress.object_id is not None:
        raise ReplayError('Only direct synchronous function calls are supported')
    return bounded_value({'name': progress.function_name, 'args': list(progress.args), 'kwargs': progress.kwargs})


def terminal(progress: Any, output: Output) -> dict[str, Any]:
    if not isinstance(progress, MontyComplete):
        raise ReplayError('Unsupported suspension')
    return bounded_value({'kind': 'return', 'value': progress.output, 'output': output.output})


def failure(error: MontyRuntimeError, output: Output) -> dict[str, Any]:
    return bounded_value({'kind': 'error', 'message': str(error), 'output': output.output})


def private_file(path: str | Path) -> BinaryIO:
    return open(path, 'xb', opener=lambda name, flags: os.open(name, flags, 0o600))


def write_artifact(path: str | Path, data: bytes) -> None:
    if len(data) > MAX_FILE:
        raise ReplayError('Artifact exceeds 8 MiB')
    with private_file(path) as output:
        output.write(data)


class Journal:
    def __init__(self, path: str | Path) -> None:
        self.file = private_file(path)
        self.digest = hashlib.sha256()
        self.size = 0

    def append(self, record: dict[str, Any]) -> None:
        line = encode(record) + b'\n'
        if self.size + len(line) > MAX_FILE:
            raise ReplayError('Recording exceeds 8 MiB')
        self.file.write(line)
        self.file.flush()
        os.fsync(self.file.fileno())
        self.digest.update(line)
        self.size += len(line)

    def close(self) -> None:
        self.file.close()


def check_response(response: Any) -> None:
    bounded_value(response)
    if type(response) is not dict:
        raise ReplayError('Response must be an object')
    response = cast(dict[str, Any], response)
    if set(response) == {'return_value'}:
        return
    if (
        set(response) == {'exc_type', 'message'}
        and response['exc_type'] in ('ValueError', 'TypeError', 'RuntimeError', 'KeyError')
        and type(response['message']) is str
    ):
        return
    raise ReplayError('Expected return_value or a supported exception response')
