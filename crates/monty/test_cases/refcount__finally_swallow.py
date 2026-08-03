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
assert result == ['done']

for i in range(2):
    try:
        raise ValueError('swallowed by continue')
    finally:
        continue

try:
    try:
        raise ValueError('abandoned handler')
    except ValueError as abandoned:
        raise TypeError('replacement')
except TypeError:
    pass
# ref-counts={'result': 1}
