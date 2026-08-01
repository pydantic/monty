# Exceptions swallowed by break/continue/return escaping a finally block must
# be released: the unwind emits ClearException for the in-flight
# exception_stack entry, so the exception value doesn't leak (issue #647).
while True:
    try:
        raise ValueError('swallowed by break')
    finally:
        break


def swallow_return():
    try:
        raise ValueError('swallowed by return')
    finally:
        return ['done']


result = swallow_return()

for i in range(2):
    try:
        raise ValueError('swallowed by continue')
    finally:
        continue
# ref-counts={'result': 1}
