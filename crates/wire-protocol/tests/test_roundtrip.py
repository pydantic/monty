"""Round-trip every `ParentRequest` and `ChildEvent` arm through encode → decode."""

import datetime
from pathlib import PurePosixPath

import pytest
import wire_protocol as wp


def parent_round_trip(request: wp.ParentRequest) -> wp.ParentRequest:
    return wp.decode_parent_request(wp.encode_parent_request(request))


def child_round_trip(event: wp.ChildEvent) -> wp.ChildEvent:
    return wp.decode_child_event(wp.encode_child_event(event))


PARENT_REQUESTS = [
    wp.StartSession(),
    wp.StartSession(script_name='job.py', type_check=True, type_check_stubs='x: int', monty_version='9.9.9'),
    wp.StartSession(limits={'max_allocations': 1000, 'max_duration_micros': 5_000_000, 'max_recursion_depth': 500}),
    wp.Feed('1 + 1'),
    wp.Feed('a + b', inputs={'a': 1, 'b': 2}, skip_type_check=True),
    wp.Feed('open("/mnt/x")', mounts=[wp.Mount('/mnt', '/srv/data', mode='read-only')]),
    wp.ResumeCall(call_id=4, result=wp.ExtFunctionResult.returns({'k': [1, 2]})),
    wp.ResumeCall(call_id=5, result=wp.ExtFunctionResult.error(wp.RaisedException('ValueError', 'bad'))),
    wp.ResumeCall(call_id=6, result=wp.ExtFunctionResult.future(6)),
    wp.ResumeCall(call_id=7, result=wp.ExtFunctionResult.not_found('missing')),
    wp.ResumeNameLookup.resolved(42),
    wp.ResumeNameLookup.undefined(),
    wp.ResumeFutures([wp.FutureResult(1, wp.ExtFunctionResult.returns('done'))]),
    wp.Dump(),
    wp.Load(b'\x00\x01opaque-state'),
    wp.Reset(),
    wp.Shutdown(),
]


@pytest.mark.parametrize('request_', PARENT_REQUESTS, ids=lambda r: type(r).__name__ + repr(r)[:20])
def test_parent_request_round_trip(request_: wp.ParentRequest) -> None:
    assert parent_round_trip(request_) == request_


CHILD_EVENTS = [
    wp.Print('stdout', 'hello\n'),
    wp.Print('stderr', 'warn\n', total_execution_micros=123),
    wp.FunctionCall('fetch', args=['http://x', 5], kwargs={'timeout': 3.5}, call_id=1),
    wp.FunctionCall('greet', call_id=2, method_call=True),
    wp.OsCall('Path.read_text', args=['/etc/x'], call_id=3),
    wp.OsCall('open', call_id=4, not_handled_error=wp.RaisedException('PermissionError', 'denied')),
    wp.NameLookup('undefined_name'),
    wp.ResolveFutures([1, 2, 3]),
    wp.Complete(value={'result': [1, 2, 3]}),
    wp.Complete(value=None, total_execution_micros=999, max_duration_micros=1_000_000),
    wp.Error(wp.RaisedException('TypeError', 'oops')),
    wp.TypingError('error: incompatible types'),
    wp.DumpResult(b'snapshot-bytes'),
    wp.Ok(),
    wp.FatalError('version skew'),
]


@pytest.mark.parametrize('event', CHILD_EVENTS, ids=lambda e: type(e).__name__ + repr(e)[:20])
def test_child_event_round_trip(event: wp.ChildEvent) -> None:
    assert child_round_trip(event) == event


VALUES = [
    None,
    True,
    42,
    -7,
    2**100,
    3.14,
    'hello',
    b'bytes',
    [1, 'two', 3.0],
    (1, 2, 3),
    {'a': 1, 'b': [2, 3]},
    {1, 2, 3},
    frozenset({4, 5}),
    datetime.date(2026, 6, 22),
    datetime.datetime(2026, 6, 22, 12, 30, 0),
    datetime.timedelta(days=1, seconds=30),
    PurePosixPath('/mnt/data/file.txt'),
]


@pytest.mark.parametrize('value', VALUES, ids=lambda v: type(v).__name__)
def test_value_fidelity(value: object) -> None:
    """Every supported native value survives a Feed input round trip."""
    feed = wp.Feed('x', inputs={'v': value})
    decoded = wp.decode_parent_request(wp.encode_parent_request(feed))
    assert decoded.inputs['v'] == value


def test_start_session_defaults_monty_version() -> None:
    assert wp.StartSession().monty_version == wp.__version__


def test_file_handle_round_trips_as_wire_protocol_class() -> None:
    """A sandbox FileHandle crosses the wire as an exported `wire_protocol` class."""
    handle = wp.MontyFileHandle('/data/x.txt', 'rt')
    assert handle.mode == 'r', 'mode is canonicalized at construction'
    event = wp.OsCall('Path.read_text', args=[handle], call_id=3)
    decoded = wp.decode_child_event(wp.encode_child_event(event))
    got = decoded.args[0]
    assert isinstance(got, wp.MontyFileHandle)
    assert type(got).__module__ == 'wire_protocol'
    assert decoded == event


def test_name_lookup_disambiguates_none() -> None:
    resolved_none = wp.ResumeNameLookup.resolved(None)
    assert resolved_none.is_defined is True
    assert resolved_none.value is None
    undefined = wp.ResumeNameLookup.undefined()
    assert undefined.is_defined is False
    assert resolved_none != undefined
    assert parent_round_trip(resolved_none) == resolved_none
    assert parent_round_trip(undefined) == undefined
