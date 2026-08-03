# Control flow (break/continue/return) interacting with finally blocks,
# especially on the exception path — regression tests for
# https://github.com/pydantic/monty/issues/647 (finally ran twice when
# entered via an in-flight exception and exited via break/return).

# === break in finally swallows an in-flight exception, finally runs once ===
xs = []
while True:
    try:
        raise ValueError
    finally:
        xs.append(1)
        break
assert xs == [1]

# === return in finally swallows an in-flight exception, finally runs once ===
xs = []


def return_in_finally():
    try:
        raise ValueError
    finally:
        xs.append(1)
        return 2


assert return_in_finally() == 2
assert xs == [1]

# === continue in finally swallows an in-flight exception ===
xs = []
for i in range(3):
    try:
        raise ValueError
    finally:
        xs.append(i)
        continue
assert xs == [0, 1, 2]


# === after a swallow, there is no active exception to re-raise ===
def swallow_then_bare_raise():
    while True:
        try:
            raise ValueError('swallowed')
        finally:
            break
    raise


try:
    swallow_then_bare_raise()
    assert False, 'expected RuntimeError'
except RuntimeError as e:
    assert str(e) == 'No active exception to reraise'

# === finally raising on the return path runs once and propagates ===
xs = []


def raise_in_finally_during_return():
    try:
        return 1
    finally:
        xs.append(1)
        raise ValueError('boom')


try:
    raise_in_finally_during_return()
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'boom'
assert xs == [1]

# === return value is evaluated before the finally body runs ===
order = []


def note(tag, value):
    order.append(tag)
    return value


def return_evaluation_order():
    try:
        return note('value', 42)
    finally:
        order.append('finally')


assert return_evaluation_order() == 42
assert order == ['value', 'finally']


# === break in finally discards a pending return value ===
def break_discards_return():
    for i in range(3):
        try:
            return 1
        finally:
            break
    return 'after loop'


assert break_discards_return() == 'after loop'


# === continue in finally discards a pending return value ===
def continue_discards_return():
    runs = []
    for i in range(2):
        try:
            return 1
        finally:
            runs.append(i)
            continue
    return runs


assert continue_discards_return() == [0, 1]


# === return in finally overrides return in try ===
def finally_overrides_return():
    try:
        return 'try'
    finally:
        return 'finally'


assert finally_overrides_return() == 'finally'

# === nested finallys each run once, inner first, on break ===
xs = []
while True:
    try:
        try:
            raise ValueError
        finally:
            xs.append('inner')
    finally:
        xs.append('outer')
        break
assert xs == ['inner', 'outer']

# === break crossing an inner finally from inside an outer finally ===
xs = []
while True:
    try:
        raise ValueError
    finally:
        try:
            xs.append('inner-try')
        finally:
            xs.append('inner-finally')
        xs.append('outer-finally')
        break
assert xs == ['inner-try', 'inner-finally', 'outer-finally']

# === exception raised inside a finally during break is caught outside ===
xs = []
try:
    while True:
        try:
            break
        finally:
            xs.append('finally')
            raise ValueError('from finally')
except ValueError as e:
    xs.append(str(e))
assert xs == ['finally', 'from finally']

# === try/except nested inside an exception-path finally ===
# The inner handler must not disturb the in-flight exception: the outer
# except still sees the original, and a bare raise in the finally would too.
out = []
try:
    try:
        raise ValueError('original')
    finally:
        try:
            raise TypeError('inner')
        except TypeError:
            pass
        out.append('finally done')
except ValueError as e:
    out.append(str(e))
assert out == ['finally done', 'original']


# === bare raise in a finally inside an except handler sees the handler's exception ===
def bare_raise_in_inner_finally():
    seen = []
    try:
        raise ValueError('original')
    except ValueError:
        try:
            return seen
        finally:
            try:
                raise
            except ValueError as e:
                seen.append(str(e))


assert bare_raise_in_inner_finally() == ['original']

# === handler variable is unbound after break out of the handler ===
for i in range(3):
    try:
        raise ValueError('x')
    except ValueError as caught:
        break
try:
    caught
    assert False, 'expected NameError'
except NameError:
    pass


# === handler target is cleared before an exceptional exit reaches finally ===
handler_cleanup = []
try:
    try:
        try:
            raise ValueError('original')
        except ValueError as abandoned:
            raise TypeError('replacement')
    finally:
        try:
            abandoned
        except NameError:
            handler_cleanup.append('unbound')
        else:
            handler_cleanup.append('bound')
except TypeError as replacement:
    handler_cleanup.append(str(replacement))
assert handler_cleanup == ['unbound', 'replacement']


# === captured handler target is cleared on an exceptional exit ===
# The cleanup path for a replacement exception uses DeleteCell when a closure
# captures the handler target; normal fall-through and return use other paths.
def exceptional_handler_cell_cleanup():
    probe = None
    try:
        try:
            raise ValueError('original')
        except ValueError as captured:
            probe = lambda: captured
            raise TypeError('replacement')
    except TypeError:
        pass

    try:
        probe()
        return 'still bound'
    except NameError as e:
        return str(e)


assert exceptional_handler_cell_cleanup() == (
    "cannot access free variable 'captured' where it is not associated with a value in enclosing scope"
)


# === handler variable is unbound after return out of the handler ===
# the probe closure captures `exc2`, so calling it after the return observes
# the handler cleanup unbinding the cell (a module-level lookup could not —
# it would be NameError regardless)


def return_from_handler():
    try:
        raise ValueError('x')
    except ValueError as exc2:
        return 'returned', lambda: exc2


value, probe = return_from_handler()
assert value == 'returned'
try:
    probe()
    assert False, 'expected NameError'
except NameError:
    pass


# === owning frame reads the handler variable after cleanup: UnboundLocalError ===
# only the nested closure's read (above) is the free-variable NameError; the
# frame that owns the cell gets UnboundLocalError, like any unassigned local
def owner_read_after_cleanup():
    try:
        raise ValueError('x')
    except ValueError as exc3:
        hold = lambda: exc3
    return exc3


try:
    owner_read_after_cleanup()
    assert False, 'expected UnboundLocalError'
except UnboundLocalError as e:
    assert str(e) == "cannot access local variable 'exc3' where it is not associated with a value"


# === handler variable is unbound after continue out of the handler ===
for i in range(2):
    try:
        raise ValueError('x')
    except ValueError as caught2:
        continue
try:
    caught2
    assert False, 'expected NameError'
except NameError:
    pass

# === with + finally: __exit__ and finally each run once on break ===
events = []


class Tracker:
    def __enter__(self):
        events.append('enter')
        return self

    def __exit__(self, typ, val, tb):
        events.append('exit')
        return False


while True:
    try:
        with Tracker():
            raise ValueError
    finally:
        events.append('finally')
        break
assert events == ['enter', 'exit', 'finally']

# === __exit__ failure replaces return and is caught by the enclosing region ===
events = []


class ExplodingExit:
    def __init__(self, tag):
        self.tag = tag

    def __enter__(self):
        events.append(f'enter {self.tag}')
        return self

    def __exit__(self, typ, val, tb):
        events.append(f'exit {self.tag}')
        raise RuntimeError(f'exit failed {self.tag}')


def return_through_exploding_exit():
    try:
        with ExplodingExit('return'):
            return 'discarded'
    except RuntimeError as e:
        return str(e)


assert return_through_exploding_exit() == 'exit failed return'
assert events == ['enter return', 'exit return']

# === __exit__ failure replaces break and is caught outside the loop ===
events = []
try:
    while True:
        with ExplodingExit('break'):
            break
except RuntimeError as e:
    events.append(str(e))
assert events == ['enter break', 'exit break', 'exit failed break']

# === break inside with inside finally calls __exit__ once ===
events = []
while True:
    try:
        raise ValueError
    finally:
        with Tracker():
            events.append('body')
            break
assert events == ['enter', 'body', 'exit']

# === return through with + for + finally unwinds innermost first ===
events = []


def deep_unwind():
    try:
        for i in range(5):
            with Tracker():
                return 'done'
    finally:
        events.append('finally')


assert deep_unwind() == 'done'
assert events == ['enter', 'exit', 'finally']


# === exception in the exception-path finally replaces the original ===
def replace_exception():
    try:
        raise ValueError('original')
    finally:
        raise TypeError('replacement')


try:
    replace_exception()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'replacement'

# === finally runs once per path in try/except/else/finally ===
xs = []
for mode in ['ok', 'caught', 'uncaught', 'ret']:

    def run(mode=mode):
        try:
            if mode in ('caught', 'uncaught'):
                raise KeyError(mode)
        except KeyError as e:
            xs.append(f'except {mode}')
            if mode == 'uncaught':
                raise ValueError(mode)
        else:
            xs.append(f'else {mode}')
            if mode == 'ret':
                return 'ret'
        finally:
            xs.append(f'finally {mode}')

    try:
        run()
    except ValueError:
        xs.append(f'caught {mode}')
assert xs == [
    'else ok',
    'finally ok',
    'except caught',
    'finally caught',
    'except uncaught',
    'finally uncaught',
    'caught uncaught',
    'else ret',
    'finally ret',
]

# === break through try/except/finally from the handler body ===
xs = []
while True:
    try:
        raise ValueError
    except ValueError:
        xs.append('except')
        break
    finally:
        xs.append('finally')
assert xs == ['except', 'finally']


# === continue clears a named handler before the enclosing finally runs ===
def continue_from_handler_through_finally():
    seen = []
    for i in range(2):
        try:
            try:
                raise ValueError(str(i))
            except ValueError as current:
                seen.append(f'handler {current}')
                continue
        finally:
            try:
                current
                seen.append('bound')
            except UnboundLocalError:
                seen.append('unbound')
            seen.append(f'finally {i}')
    return seen


assert continue_from_handler_through_finally() == [
    'handler 0',
    'unbound',
    'finally 0',
    'handler 1',
    'unbound',
    'finally 1',
]

# === continue in exception-path finally keeps looping, then loop ends normally ===
xs = []
for i in range(2):
    try:
        raise ValueError(str(i))
    finally:
        xs.append(i)
        continue
else:
    xs.append('else')
assert xs == [0, 1, 'else']

# === break through finally skips the loop else block ===
xs = []
for i in range(2):
    try:
        raise ValueError
    finally:
        break
else:
    xs.append('else')
assert xs == []

# === break targeting a loop *inside* the finally does not swallow ===
# The loop lives entirely within the finally body, so its break must not
# discard the in-flight exception.
xs = []
try:
    try:
        raise ValueError('kept')
    finally:
        for k in range(5):
            xs.append(k)
            break
        xs.append('after inner loop')
except ValueError as e:
    xs.append(str(e))
assert xs == [0, 'after inner loop', 'kept']


# === break targeting a loop inside an except body keeps handler state ===
def break_in_handler_loop():
    try:
        raise ValueError('original')
    except ValueError:
        while True:
            break
        try:
            raise
        except ValueError as e:
            return str(e)


assert break_in_handler_loop() == 'original'


# === for+break inside an except body, then bare raise propagates ===
def for_break_then_reraise():
    try:
        raise ValueError('kept by handler')
    except ValueError:
        for i in [1, 2]:
            break
        raise


try:
    for_break_then_reraise()
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'kept by handler'


# === return crossing two nested finallys runs both, inner first ===
order = []


def return_two_finallys():
    try:
        try:
            return 'value'
        finally:
            order.append('inner')
    finally:
        order.append('outer')


assert return_two_finallys() == 'value'
assert order == ['inner', 'outer']

# === return in inner finally swallows exception, still crosses outer finally ===
order = []


def swallow_then_outer():
    try:
        try:
            raise ValueError
        finally:
            order.append('inner')
            return 'swallowed'
    finally:
        order.append('outer')


assert swallow_then_outer() == 'swallowed'
assert order == ['inner', 'outer']

# === break from a non-first except handler runs the finally ===
xs = []
while True:
    try:
        raise KeyError('k')
    except ValueError:
        xs.append('wrong handler')
    except KeyError:
        xs.append('key')
        break
    finally:
        xs.append('finally')
assert xs == ['key', 'finally']

# === break inside with inside except handler unwinds both ===
events = []


class Recorder:
    def __enter__(self):
        events.append('enter')
        return self

    def __exit__(self, typ, val, tb):
        events.append(f'exit {typ is None}')
        return False


while True:
    try:
        raise ValueError
    except ValueError:
        with Recorder():
            break
assert events == ['enter', 'exit True']


# === __exit__ swallowing the exception, then loop control flow continues ===
class Swallow:
    def __enter__(self):
        return self

    def __exit__(self, typ, val, tb):
        return True


xs = []
for i in range(3):
    with Swallow():
        raise ValueError(str(i))
    xs.append(f'resumed {i}')
assert xs == ['resumed 0', 'resumed 1', 'resumed 2']

xs = []
for i in range(3):
    with Swallow():
        if i == 1:
            raise ValueError('swallowed')
        xs.append(i)
    xs.append(f'after {i}')
assert xs == [0, 'after 0', 'after 1', 2, 'after 2']

# === try/finally inside with body: break swallows, __exit__ sees no exception ===
events = []
while True:
    with Recorder():
        try:
            raise ValueError
        finally:
            events.append('finally')
            break
assert events == ['enter', 'finally', 'exit True']

# === exception propagating out of with body reaches __exit__ with the type ===
events = []
try:
    with Recorder():
        raise ValueError('through exit')
except ValueError as e:
    events.append(str(e))
assert events == ['enter', 'exit False', 'through exit']

# === for → with → try/finally → return unwinds innermost first ===
events = []


def deep_mixed():
    for i in range(3):
        with Recorder():
            try:
                return 'deep'
            finally:
                events.append('finally')


assert deep_mixed() == 'deep'
assert events == ['enter', 'finally', 'exit True']

# === continue crossing with + finally in one step ===
events = []
for i in range(2):
    with Recorder():
        try:
            events.append(i)
            continue
        finally:
            events.append('finally')
assert events == ['enter', 0, 'finally', 'exit True', 'enter', 1, 'finally', 'exit True']

# === exception raised by the call in `return f()` still reaches handlers ===
# Regression: the return's unwind used to interrupt the protected region at
# exactly the callee's exception-delivery offset (the instruction after the
# call), so except/finally/__exit__ were all skipped when the call raised.


def boom():
    raise ValueError('boom')


def return_call_except():
    try:
        return boom()
    except ValueError as e:
        return f'caught {e}'


assert return_call_except() == 'caught boom'


def return_call_short_circuit():
    try:
        return False or boom()
    except ValueError as e:
        return f'caught {e}'


assert return_call_short_circuit() == 'caught boom'


# Attribute and indirect calls have distinct bytecode call shapes, while `and`
# reaches its right operand through the same fall-through path as `or`.
class ReturnBoom:
    def fail(self):
        raise ValueError('attribute boom')


def return_attr_call_except():
    try:
        return ReturnBoom().fail()
    except ValueError as e:
        return f'caught {e}'


assert return_attr_call_except() == 'caught attribute boom'


def get_boom():
    return boom


def return_indirect_call_except():
    try:
        return get_boom()()
    except ValueError as e:
        return f'caught {e}'


assert return_indirect_call_except() == 'caught boom'


def return_call_and_fallthrough():
    try:
        return True and boom()
    except ValueError as e:
        return f'caught {e}'


assert return_call_and_fallthrough() == 'caught boom'


def return_call_ternary_fallthrough():
    try:
        return 0 if False else boom()
    except ValueError as e:
        return f'caught {e}'


assert return_call_ternary_fallthrough() == 'caught boom'


def return_call_ternary_jump():
    try:
        return boom() if True else 0
    except ValueError as e:
        return f'caught {e}'


assert return_call_ternary_jump() == 'caught boom'

events = []


def return_call_finally():
    try:
        return boom()
    finally:
        events.append('finally')


try:
    return_call_finally()
except ValueError as e:
    events.append(str(e))
assert events == ['finally', 'boom']

events = []


def return_call_with():
    with Recorder():
        return boom()


try:
    return_call_with()
except ValueError:
    events.append('caught')
assert events == ['enter', 'exit False', 'caught']

events = []


def return_call_nested():
    try:
        try:
            return boom()
        except ValueError:
            events.append('inner')
            raise
    finally:
        events.append('finally')


try:
    return_call_nested()
except ValueError:
    events.append('outer')
assert events == ['inner', 'finally', 'outer']


def return_call_in_loop():
    for i in range(3):
        try:
            return boom()
        except ValueError:
            return f'caught in loop {i}'


assert return_call_in_loop() == 'caught in loop 0'
