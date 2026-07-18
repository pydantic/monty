# call-external
# run-async
# === Async external function exceptions ===
# Tests for exceptions raised by awaited external async functions. Mirrors
# `ext_call__ext_exc.py` section-by-section via `async_fail`, then appends
# async-specific call shapes (asyncio.gather, user-defined async wrappers,
# gathered coroutines).
import asyncio

# === Basic exception propagation ===

# External function raising ValueError
caught_value_error = False
try:
    await async_fail('ValueError', 'test error')  # pyright: ignore
    assert False, 'should not reach here'
except ValueError:
    caught_value_error = True
assert caught_value_error

# External function raising TypeError
caught_type_error = False
try:
    await async_fail('TypeError', 'type error message')  # pyright: ignore
    assert False, 'should not reach here'
except TypeError:
    caught_type_error = True
assert caught_type_error

# External function raising KeyError
caught_key_error = False
try:
    await async_fail('KeyError', 'missing key')  # pyright: ignore
    assert False, 'should not reach here'
except KeyError:
    caught_key_error = True
assert caught_key_error

# External function raising RuntimeError
caught_runtime_error = False
try:
    await async_fail('RuntimeError', 'runtime error')  # pyright: ignore
    assert False, 'should not reach here'
except RuntimeError:
    caught_runtime_error = True
assert caught_runtime_error

# === Exception not caught by wrong handler ===

# ValueError not caught by TypeError handler
caught_outer = False
try:
    try:
        await async_fail('ValueError', 'inner error')  # pyright: ignore
    except TypeError:
        assert False, 'TypeError should not catch ValueError'
except ValueError:
    caught_outer = True
assert caught_outer

# === Exception in expression with multiple ext calls ===

# Awaited external call raises mid-expression — surrounding ops don't run
try:
    x = 1 + (await async_fail('ValueError', 'mid-expr')) + 2  # pyright: ignore
    assert False, 'should not reach here'
except ValueError:
    pass  # Expected

# First ext call raises, second should not be called
try:
    x = (await async_fail('ValueError', 'first')) + add_ints(1, 2)  # pyright: ignore
    assert False, 'should not reach here'
except ValueError:
    pass  # Expected

# === External exception in try body with finally ===

finally_ran = False
try:
    await async_fail('ValueError', 'in try')  # pyright: ignore
except ValueError:
    pass  # Caught
finally:
    finally_ran = True
assert finally_ran

# External exception propagating through finally
outer_caught = False
finally_ran2 = False
try:
    try:
        await async_fail('KeyError', 'will propagate')  # pyright: ignore
    except ValueError:
        assert False, 'ValueError should not catch KeyError'
    finally:
        finally_ran2 = True
except KeyError:
    outer_caught = True
assert finally_ran2
assert outer_caught

# === Mix of normal returns and exceptions ===

# Normal return, then exception
value1 = await async_call(30)  # pyright: ignore
assert value1 == 30
try:
    await async_fail('ValueError', 'after success')  # pyright: ignore
    assert False, 'should not reach here'
except ValueError:
    pass  # Expected

# Exception, then normal return (after catching)
caught_exc = False
try:
    await async_fail('TypeError', 'will be caught')  # pyright: ignore
except TypeError:
    caught_exc = True
value2 = await async_call(10)  # pyright: ignore
assert caught_exc
assert value2 == 10

# === Exception in except handler from external function ===

outer_catch = False
try:
    try:
        raise ValueError('inner')
    except ValueError:
        await async_fail('TypeError', 'from handler')  # pyright: ignore
except TypeError:
    outer_catch = True
assert outer_catch

# === Exception in else block from external function ===

else_exc_caught = False
try:
    try:
        pass  # No exception
    except:
        assert False, 'should not reach except'
    else:
        await async_fail('RuntimeError', 'from else')  # pyright: ignore
except RuntimeError:
    else_exc_caught = True
assert else_exc_caught

# === Exception in finally block ===

# Note: exception in finally replaces any pending exception
finally_exc_caught = False
try:
    try:
        pass
    finally:
        await async_fail('ValueError', 'from finally')  # pyright: ignore
except ValueError:
    finally_exc_caught = True
assert finally_exc_caught

# === Nested try blocks with external exceptions ===

inner_handled = False
outer_handled = False
finally_count = 0
try:
    try:
        await async_fail('ValueError', 'inner error')  # pyright: ignore
    except ValueError:
        inner_handled = True
        await async_fail('TypeError', 'from inner handler')  # pyright: ignore
    finally:
        finally_count += 1
except TypeError:
    outer_handled = True
finally:
    finally_count += 1

assert inner_handled
assert outer_handled
assert finally_count == 2

# === Tests with no sync counterpart in `ext_call__ext_exc.py`. ===

# === Exception message is preserved across async boundary ===
try:
    await async_fail('ValueError', 'exact message')  # pyright: ignore
    assert False, 'should not reach here'
except ValueError as e:
    assert str(e) == 'exact message', f'message preserved through await: {e}'


# === Bare raise re-raises across an async frame boundary ===
async def wrapper_rereraise():
    try:
        await async_fail('ValueError', 're-raised')
    except ValueError:
        raise


outer_reraised = False
try:
    await wrapper_rereraise()  # pyright: ignore
except ValueError as e:
    assert str(e) == 're-raised'
    outer_reraised = True
assert outer_reraised


# === Exception unwinds through user-defined async wrapper ===
async def wrapper_catches():
    try:
        await async_fail('ValueError', 'inside wrapper')
        return 'no error'
    except ValueError as e:
        return 'caught: ' + str(e)


wrapper_result = await wrapper_catches()  # pyright: ignore
assert wrapper_result == 'caught: inside wrapper', f'caught inside async wrapper: {wrapper_result}'


# === Exception unwinds through multiple async wrapper frames ===
async def inner_wrapper():
    await async_fail('ValueError', 'deep')


async def outer_wrapper():
    await inner_wrapper()


nested_caught = None
try:
    await outer_wrapper()  # pyright: ignore
except ValueError as e:
    nested_caught = str(e)
assert nested_caught == 'deep'

# === asyncio.gather: failure propagates out ===
caught_gather = None
try:
    await asyncio.gather(async_call(1), async_fail('ValueError', 'gather failure'))  # pyright: ignore
except ValueError as e:
    caught_gather = str(e)
assert caught_gather == 'gather failure'


# === Gathered coroutine: child failure surfaces at the main task's gather ===
async def get_a():
    return await async_fail('ValueError', 'async boom')


async def get_b():
    return await async_call('b')


gathered_coro_caught = None
try:
    await asyncio.gather(get_a(), get_b())  # pyright: ignore
except ValueError as e:
    gathered_coro_caught = str(e)
assert gathered_coro_caught == 'async boom', f'gathered child exception caught at gather: {gathered_coro_caught}'


# === Re-awaiting a failed external future re-raises the cached error ===

reawait_fail = async_fail('ValueError', 'cached failure')
errors = []
for _ in range(3):
    try:
        await reawait_fail  # pyright: ignore
        assert False, 'await of failed future should raise'
    except ValueError as e:
        errors.append(str(e))
assert errors == ['cached failure', 'cached failure', 'cached failure'], (
    f'each re-await of a failed future replays the cached error: {errors}'
)
