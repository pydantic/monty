# /// monty
# call-external = true
# run-async = true
# ///
# asyncio.wait over Tasks and external futures: completion records, conditions,
# timeouts, error messages and re-awaiting.
import asyncio
import sys

# === wait constants ===
assert asyncio.FIRST_COMPLETED == 'FIRST_COMPLETED'
assert asyncio.FIRST_EXCEPTION == 'FIRST_EXCEPTION'
assert asyncio.ALL_COMPLETED == 'ALL_COMPLETED'


async def value(x):
    return await async_call(x)


# === ALL_COMPLETED returns every task in the done set ===
first = asyncio.create_task(value(1))
second = asyncio.create_task(value(2))
done, pending = await asyncio.wait([first, second])  # pyright: ignore
assert type(done) is set
assert type(pending) is set
assert done == {first, second}
assert pending == set()
assert first.result() == 1
assert second.result() == 2


# === many children settle one at a time ===
async def immediate(x):
    return x


many = [asyncio.create_task(immediate(i)) for i in range(500)]
done, pending = await asyncio.wait(many)  # pyright: ignore
assert pending == set()
assert done == set(many)
assert {task.result() for task in done} == set(range(500))


# === FIRST_COMPLETED returns once one task is done ===
async def slow_value(x):
    await asyncio.sleep(0.05)
    return x


fast = asyncio.create_task(value('fast'))
slow = asyncio.create_task(slow_value('slow'))
done, pending = await asyncio.wait([fast, slow], return_when=asyncio.FIRST_COMPLETED)  # pyright: ignore
assert len(done) == 1
winner = next(iter(done))
assert winner is fast or winner is slow
assert winner not in pending
assert (slow if winner is fast else fast) in pending
assert await slow == 'slow'  # pyright: ignore


# === FIRST_COMPLETED includes every input already done at the first await ===
finished_one = asyncio.create_task(value('finished-one'))
finished_two = asyncio.create_task(value('finished-two'))
await finished_one  # pyright: ignore
await finished_two  # pyright: ignore
still_running = asyncio.create_task(slow_value('still-running'))
done, pending = await asyncio.wait([finished_one, finished_two, still_running], return_when=asyncio.FIRST_COMPLETED)  # pyright: ignore
assert done == {finished_one, finished_two}
assert pending == {still_running}
assert await still_running == 'still-running'  # pyright: ignore


# === a host future left pending by an early wait stays usable ===
left_pending = async_call('left-pending')
quick = asyncio.create_task(immediate('quick'))
done, pending = await asyncio.wait([quick, left_pending], return_when=asyncio.FIRST_COMPLETED)  # pyright: ignore
if sys.platform == 'monty':
    # The wait must complete on the finished task, with the host future still
    # pending; CPython's async_call() fixture returns an already-resolved Future.
    assert done == {quick}
    assert pending == {left_pending}
assert await left_pending == 'left-pending'  # pyright: ignore
assert await quick == 'quick'  # pyright: ignore

retried = async_call('retried')
quick_again = asyncio.create_task(immediate('quick-again'))
await asyncio.wait([quick_again, retried], return_when=asyncio.FIRST_COMPLETED)  # pyright: ignore
done, pending = await asyncio.wait([retried])  # pyright: ignore
assert done == {retried}
assert pending == set()

timed_out = async_call('timed-out')
done, pending = await asyncio.wait([timed_out], timeout=0.01)  # pyright: ignore
if sys.platform == 'monty':
    assert done == set()
    assert pending == {timed_out}
assert await timed_out == 'timed-out'  # pyright: ignore


# === FIRST_EXCEPTION accumulates successes before the failure ===
async def fail_after(task):
    await task
    raise ValueError('late-failure')


successful = asyncio.create_task(value('successful'))
late_failure = asyncio.create_task(fail_after(successful))
done, pending = await asyncio.wait([successful, late_failure], return_when=asyncio.FIRST_EXCEPTION)  # pyright: ignore
assert done == {successful, late_failure}
assert pending == set()
assert successful.result() == 'successful'
assert repr(late_failure.exception()) == "ValueError('late-failure')"


# === FIRST_EXCEPTION without a failure completes like ALL_COMPLETED ===
quiet_one = asyncio.create_task(value('quiet-one'))
quiet_two = asyncio.create_task(value('quiet-two'))
done, pending = await asyncio.wait([quiet_one, quiet_two], return_when=asyncio.FIRST_EXCEPTION)  # pyright: ignore
assert done == {quiet_one, quiet_two}
assert pending == set()


# === FIRST_EXCEPTION does not raise for a failed task ===
async def boom():
    raise ValueError('wait-bad')


bad = asyncio.create_task(boom())
good = asyncio.create_task(value('good'))
done, pending = await asyncio.wait([bad, good], return_when=asyncio.FIRST_EXCEPTION)  # pyright: ignore
assert bad in done
assert good in done or good in pending
assert repr(bad.exception()) == "ValueError('wait-bad')"
assert await good == 'good'  # pyright: ignore


# === wait accepts external futures directly ===
future = async_call('future-value')
done, pending = await asyncio.wait([future])  # pyright: ignore
assert done == {future}
assert pending == set()
assert await future == 'future-value'  # pyright: ignore


# === duplicate entries are deduplicated by identity ===
dedup = asyncio.create_task(value('dedup'))
done, pending = await asyncio.wait([dedup, dedup])  # pyright: ignore
assert len(done) == 1
assert next(iter(done)) is dedup
assert pending == set()


# === wait accepts arbitrary iterables of tasks ===
iterator_task = asyncio.create_task(value('iterator'))
done, pending = await asyncio.wait(iter([iterator_task]))  # pyright: ignore
assert done == {iterator_task}
assert pending == set()

generator_task = asyncio.create_task(value('generator'))
done, pending = await asyncio.wait((task for task in [generator_task]))  # pyright: ignore
assert done == {generator_task}
assert pending == set()

set_task = asyncio.create_task(value('set'))
done, pending = await asyncio.wait({set_task})  # pyright: ignore
assert done == {set_task}
assert pending == set()

tuple_task = asyncio.create_task(value('tuple'))
done, pending = await asyncio.wait((tuple_task,))  # pyright: ignore
assert done == {tuple_task}
assert pending == set()


# === a failed external future is reported in done, not raised ===
failed_future = async_fail('ValueError', 'future-bad')
done, pending = await asyncio.wait([failed_future])  # pyright: ignore
assert done == {failed_future}
assert pending == set()
try:
    await failed_future  # pyright: ignore
    raise AssertionError('expected ValueError')
except ValueError as exc:
    assert str(exc) == 'future-bad'


# === timeout zero leaves a pending task untouched ===
timed = asyncio.create_task(slow_value('timed'))
done, pending = await asyncio.wait([timed], timeout=0)  # pyright: ignore
assert done == set()
assert pending == {timed}
assert await timed == 'timed'  # pyright: ignore


# === a short positive timeout also returns the pending task ===
timed_positive = asyncio.create_task(slow_value('timed-positive'))
done, pending = await asyncio.wait([timed_positive], timeout=0.001)  # pyright: ignore
assert done == set()
assert pending == {timed_positive}
assert await timed_positive == 'timed-positive'  # pyright: ignore


# === a timeout lets a created task run before the wait is awaited ===
wait_started = []


async def mark_wait_started():
    wait_started.append('ran')
    return 'ran'


async def wait_for_started():
    waited_task = asyncio.create_task(mark_wait_started())
    done, pending = await asyncio.wait([waited_task], timeout=0.01)
    assert wait_started == ['ran']
    assert done == {waited_task}
    assert pending == set()


await wait_for_started()  # pyright: ignore


# === a timeout can return both completed and pending tasks ===
already_done = asyncio.create_task(value('already-done'))
await already_done  # pyright: ignore
partial_slow = asyncio.create_task(slow_value('partial-slow'))
done, pending = await asyncio.wait([already_done, partial_slow], timeout=0.01)  # pyright: ignore
assert done == {already_done}
assert pending == {partial_slow}
assert await partial_slow == 'partial-slow'  # pyright: ignore


# === pending inputs can be waited again after a timeout ===
waited_again = asyncio.create_task(slow_value('waited-again'))
done, pending = await asyncio.wait([waited_again], timeout=0)  # pyright: ignore
assert done == set()
assert pending == {waited_again}
done, pending = await asyncio.wait(pending)  # pyright: ignore
assert done == {waited_again}
assert pending == set()
assert waited_again.result() == 'waited-again'


# === timeout None is the default ===
timed_none = asyncio.create_task(value('timed-none'))
done, pending = await asyncio.wait([timed_none], timeout=None)  # pyright: ignore
assert done == {timed_none}
assert pending == set()


# === waiting twice on the same Task is allowed ===
reused = asyncio.create_task(value('reused'))
done1, pending1 = await asyncio.wait([reused])  # pyright: ignore
done2, pending2 = await asyncio.wait([reused])  # pyright: ignore
assert done1 == {reused}
assert done2 == {reused}
assert pending1 == set()
assert pending2 == set()


# === create_task accepts a wait future ===
created_wait_input = asyncio.create_task(value('created-wait'))
created_wait_task = asyncio.create_task(asyncio.wait([created_wait_input]))
assert type(created_wait_task).__name__ == 'Task'
done, pending = await created_wait_task  # pyright: ignore
assert done == {created_wait_input}
assert pending == set()

invalid_wait_task = asyncio.create_task(asyncio.wait([]))
try:
    await invalid_wait_task  # pyright: ignore
    raise AssertionError('expected ValueError')
except ValueError as exc:
    assert str(exc) == 'Set of Tasks/Futures is empty.'


# === a WaitFuture cannot be awaited twice ===
reawait_task = asyncio.create_task(value('reawait'))
wait_future = asyncio.wait([reawait_task])
await wait_future  # pyright: ignore
try:
    await wait_future  # pyright: ignore
    raise AssertionError('expected RuntimeError')
except RuntimeError as exc:
    assert str(exc) == 'cannot reuse already awaited coroutine'


# === empty inputs ===
try:
    await asyncio.wait([])  # pyright: ignore
    raise AssertionError('expected ValueError')
except ValueError as exc:
    assert str(exc) == 'Set of Tasks/Futures is empty.'

for empty in (None, 0):
    try:
        await asyncio.wait(empty)  # pyright: ignore
        raise AssertionError('expected ValueError')
    except ValueError as exc:
        assert str(exc) == 'Set of Tasks/Futures is empty.'

# A truthy empty iterator is an AssertionError in CPython and a ValueError in
# Monty, because Monty treats every empty collection the same way.
try:
    await asyncio.wait(iter(()))  # pyright: ignore
    raise AssertionError('expected an empty-set error')
except AssertionError as exc:
    assert str(exc) == 'Set of Futures is empty.'
except ValueError as exc:
    assert str(exc) == 'Set of Tasks/Futures is empty.'


# === single awaitables are rejected with their type name ===
completed = asyncio.create_task(value('single'))
await completed  # pyright: ignore
try:
    await asyncio.wait(completed)  # pyright: ignore
    raise AssertionError('expected TypeError')
except TypeError as exc:
    assert str(exc) == 'expect a list of futures, not Task'

single_coro = value('single-coro')
try:
    await asyncio.wait(single_coro)  # pyright: ignore
    raise AssertionError('expected TypeError')
except TypeError as exc:
    assert str(exc) == 'expect a list of futures, not coroutine'
assert await single_coro == 'single-coro'  # pyright: ignore

single_future = async_call('single-future')
try:
    await asyncio.wait(single_future)  # pyright: ignore
    raise AssertionError('expected TypeError')
except TypeError as exc:
    assert str(exc) in (
        'expect a list of futures, not Future',
        'expect a list of futures, not coroutine',
    )
assert await single_future == 'single-future'  # pyright: ignore


# === a coroutine inside the iterable is rejected ===
list_coro = value('list-coro')
try:
    await asyncio.wait([list_coro])  # pyright: ignore
    raise AssertionError('expected TypeError')
except TypeError as exc:
    assert str(exc) == 'Passing coroutines is forbidden, use tasks explicitly.'
assert await list_coro == 'list-coro'  # pyright: ignore


# === a wait future is rejected as a coroutine ===
nested_task = asyncio.create_task(value('nested-wait'))
nested_wait = asyncio.wait([nested_task])
try:
    await asyncio.wait([nested_wait])  # pyright: ignore
    raise AssertionError('expected TypeError')
except TypeError as exc:
    assert str(exc) == 'Passing coroutines is forbidden, use tasks explicitly.'
done, pending = await nested_wait  # pyright: ignore
assert done == {nested_task}
assert pending == set()

single_nested_task = asyncio.create_task(value('single-nested-wait'))
single_nested_wait = asyncio.wait([single_nested_task])
try:
    await asyncio.wait(single_nested_wait)  # pyright: ignore
    raise AssertionError('expected TypeError')
except TypeError as exc:
    assert str(exc) == 'expect a list of futures, not coroutine'
done, pending = await single_nested_wait  # pyright: ignore
assert done == {single_nested_task}
assert pending == set()


# === non-future elements fail the add_done_callback lookup ===
for element in (42, None):
    try:
        await asyncio.wait([element])  # pyright: ignore
        raise AssertionError('expected AttributeError')
    except AttributeError as exc:
        assert str(exc) == f"'{type(element).__name__}' object has no attribute 'add_done_callback'"


# === non-iterable inputs fail during iteration ===
try:
    await asyncio.wait(42)  # pyright: ignore
    raise AssertionError('expected TypeError')
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"


# === invalid return_when ===
for invalid in ('INVALID', 1):
    try:
        await asyncio.wait([completed], return_when=invalid)  # pyright: ignore
        raise AssertionError('expected ValueError')
    except ValueError as exc:
        assert str(exc) == f'Invalid return_when value: {invalid}'


# CPython tests return_when with equality, not str().
class FirstCompletedAlias:
    def __eq__(self, other):
        return other == 'FIRST_COMPLETED'


class StringifiesFirstCompleted:
    def __str__(self):
        return 'FIRST_COMPLETED'


aliased = asyncio.create_task(slow_value('aliased'))
done, pending = await asyncio.wait([aliased], return_when=FirstCompletedAlias(), timeout=0)  # pyright: ignore
assert done == set()
assert pending == {aliased}
assert await aliased == 'aliased'  # pyright: ignore

try:
    await asyncio.wait([completed], return_when=StringifiesFirstCompleted())  # pyright: ignore
    raise AssertionError('expected ValueError')
except ValueError as exc:
    assert str(exc) == 'Invalid return_when value: FIRST_COMPLETED'


# === wait future repr and identity comparison ===
repr_task = asyncio.create_task(value('repr-wait'))
await repr_task  # pyright: ignore
repr_wait = asyncio.wait([repr_task])
assert repr(repr_wait).startswith('<coroutine object wait')
assert repr_wait != 1
assert not (repr_wait == 1)
done, pending = await repr_wait  # pyright: ignore
assert done == {repr_task}
assert pending == set()


# === argument errors ===
try:
    await asyncio.wait([completed], bad=1)  # pyright: ignore
    raise AssertionError('expected TypeError')
except TypeError as exc:
    assert str(exc) == "wait() got an unexpected keyword argument 'bad'"

try:
    await asyncio.wait()  # pyright: ignore
    raise AssertionError('expected TypeError')
except TypeError as exc:
    assert str(exc) == "wait() missing 1 required positional argument: 'fs'"

try:
    await asyncio.wait([completed], timeout='x')  # pyright: ignore
    raise AssertionError('expected TypeError')
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for +: 'float' and 'str'"
