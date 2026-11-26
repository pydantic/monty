import pytest

from pydantic_monty import (
    AsyncMonty,
    CallbackError,
    FunctionSnapshot,
    Monty,
    MontyError,
    MontyRuntimeError,
    MontySyntaxError,
)

_CODE = """
errors = []
for i in [0, 1]:
    try:
        CALL
    except ValueError as exc:
        assert exc.args == ('same text',)
        assert not hasattr(exc, 'callback_reason')
        try:
            exc.args = ('changed',)
        except AttributeError:
            pass
        else:
            raise AssertionError('exception args must stay read-only')
        errors.append(exc)
if pick == 2:
    raise ValueError('same text') from errors[0]
raise errors[pick]
"""


@pytest.mark.parametrize(('pick', 'reason'), [(0, 11), (1, 12), (2, None)])
def test_callback_reason_sync(pick, reason):
    def reject(n):
        raise CallbackError(ValueError('same text'), 11 + n)

    with Monty() as pool, pool.checkout() as session, pytest.raises(MontyRuntimeError) as raised:
        session.feed_run(_CODE.replace('CALL', 'reject(i)'), inputs={'pick': pick}, external_lookup={'reject': reject})
    assert raised.value.callback_reason == reason
    assert type(raised.value.exception()) is ValueError
    assert raised.value.exception().args == ('same text',)


@pytest.fixture
def anyio_backend():
    return 'asyncio'


@pytest.mark.anyio
@pytest.mark.parametrize(('pick', 'reason'), [(0, 11), (1, 12), (2, None)])
@pytest.mark.parametrize('call', ['await reject(i)', 'pending = reject(i)\n        await pending'])
async def test_callback_reason_async(pick, reason, call, anyio_backend):
    async def reject(n):
        raise CallbackError(ValueError('same text'), 11 + n)

    async with AsyncMonty(stream_async_calls=call.startswith('pending')) as pool, pool.checkout() as session:
        with pytest.raises(MontyRuntimeError) as raised:
            await session.feed_run(
                _CODE.replace('CALL', call), inputs={'pick': pick}, external_lookup={'reject': reject}
            )
    assert raised.value.callback_reason == reason
    assert type(raised.value.exception()) is ValueError
    assert raised.value.exception().args == ('same text',)


def test_callback_reason_survives_pause_and_call_id_reuse():
    with Monty() as pool:
        with pool.checkout() as session:
            first = session.feed_start('try:\n    reject()\nexcept ValueError as exc:\n    old = exc')
            assert isinstance(first, FunctionSnapshot)
            first_id = first.call_id
            first.resume({'exception': CallbackError(ValueError('same text'), 11)})
            second = session.feed_start('try:\n    reject()\nexcept ValueError:\n    raise old')
            assert isinstance(second, FunctionSnapshot)
            assert second.call_id == first_id == 0
            paused = second.dump()
        with pool.checkout() as session:
            restored = session.load_snapshot(paused)
            assert isinstance(restored, FunctionSnapshot)
            with pytest.raises(MontyRuntimeError) as raised:
                restored.resume({'exception': CallbackError(ValueError('same text'), 12)})
    assert raised.value.callback_reason == 11


def test_only_opt_in_errors_carry_a_reason():
    class OrdinaryError(ValueError):
        callback_reason = 12

    def reject():
        raise OrdinaryError('same text')

    def syntax():
        raise CallbackError(SyntaxError('from a callback'), 255)

    with Monty() as pool, pool.checkout() as session:
        with pytest.raises(MontyRuntimeError) as ordinary:
            session.feed_run('reject()', external_lookup={'reject': reject})
        with pytest.raises(MontySyntaxError) as tagged:
            session.feed_run('syntax()', external_lookup={'syntax': syntax})
        with pytest.raises(MontySyntaxError) as parser:
            session.feed_run('invalid (')
    assert ordinary.value.callback_reason is None
    assert tagged.value.callback_reason == 255
    assert isinstance(tagged.value, MontyError)
    assert parser.value.callback_reason is None


@pytest.mark.parametrize(('reason', 'exc_type'), [(0, ValueError), (-1, OverflowError), (256, OverflowError)])
def test_callback_reason_range(reason, exc_type):
    with pytest.raises(exc_type):
        CallbackError(ValueError('same text'), reason)
