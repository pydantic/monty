"""Codec edge cases: error handling, malformed input, and exception fidelity."""

import pytest
import wire_protocol as wp
from inline_snapshot import snapshot


def test_encode_rejects_non_request() -> None:
    with pytest.raises(TypeError) as exc_info:
        wp.encode_parent_request(wp.Complete(value=1))  # a ChildEvent, not a ParentRequest
    assert exc_info.value.args[0] == snapshot(
        'expected a ParentRequest (StartSession, Feed, ResumeCall, ...), got Complete'
    )


def test_encode_rejects_non_event() -> None:
    with pytest.raises(TypeError) as exc_info:
        wp.encode_child_event(wp.Feed('1'))  # a ParentRequest, not a ChildEvent
    assert exc_info.value.args[0] == snapshot('expected a ChildEvent (Print, FunctionCall, Complete, ...), got Feed')


def test_decode_rejects_garbage() -> None:
    with pytest.raises(ValueError) as exc_info:
        wp.decode_parent_request(b'\xff\xff\xff\xff not protobuf')
    assert exc_info.value.args[0] == snapshot(
        'invalid wire message: failed to decode Protobuf message: invalid key value: 8858370047'
    )


def test_decode_parent_does_not_decode_event() -> None:
    """A ChildEvent's bytes must not silently decode as a ParentRequest."""
    event_bytes = wp.encode_child_event(wp.FatalError('boom'))
    # Tag 11 (FatalError) is not a valid ParentRequest arm; it is skipped as an
    # unknown field, leaving an empty request with no kind.
    with pytest.raises(ValueError) as exc_info:
        wp.decode_parent_request(event_bytes)
    assert exc_info.value.args[0] == snapshot('invalid wire message: ParentRequest has no kind')


def test_invalid_mount_mode() -> None:
    with pytest.raises(ValueError) as exc_info:
        wp.Mount('/mnt', '/srv', mode='sideways')
    assert exc_info.value.args[0] == snapshot(
        "invalid mount mode 'sideways'; expected 'read-only', 'read-write', or 'overlay'"
    )


def test_invalid_print_stream() -> None:
    with pytest.raises(ValueError) as exc_info:
        wp.Print('stddout', 'x')
    assert exc_info.value.args[0] == snapshot("invalid print stream 'stddout'; expected 'stdout' or 'stderr'")


def test_unknown_exception_type() -> None:
    with pytest.raises(ValueError) as exc_info:
        wp.RaisedException('NotARealError', 'msg')
    assert exc_info.value.args[0] == snapshot("unknown exception type 'NotARealError'")


def test_raised_exception_from_native() -> None:
    exc = wp.RaisedException.from_exception(KeyError('missing'))
    assert exc.exc_type == snapshot('KeyError')
    # KeyError str() includes the repr of the key
    assert exc.message == snapshot("'missing'")


def test_raised_exception_as_exception_round_trips_type() -> None:
    raised = wp.RaisedException('ValueError', 'bad value')
    native = raised.as_exception()
    assert isinstance(native, ValueError)
    assert str(native) == snapshot('bad value')


def test_raised_exception_preserves_traceback() -> None:
    frame = wp.StackFrame('job.py', 3, 5, 3, 12, function_name='run', preview_line='    raise ValueError()')
    raised = wp.RaisedException('ValueError', 'x', traceback=[frame])
    event = wp.decode_child_event(wp.encode_child_event(wp.Error(raised)))
    assert len(event.exception.traceback) == 1
    decoded = event.exception.traceback[0]
    assert (decoded.filename, decoded.line, decoded.function_name) == snapshot(('job.py', 3, 'run'))


def test_non_string_kwarg_key_rejected() -> None:
    with pytest.raises(TypeError) as exc_info:
        wp.FunctionCall('f', kwargs={1: 'v'})
    assert exc_info.value.args[0] == snapshot('keyword argument names must be strings')
