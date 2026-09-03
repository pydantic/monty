# Calling an unknown method still owns the arguments the call evaluated, so the
# AttributeError path has to release them. Regression test: sandboxed code could
# otherwise leak a reference per call in a loop.
import re
from datetime import date, datetime, time, timedelta

lst = [1, 2, 3]


def call_bogus():
    # One object per `py_call_attr` implementation that can reach an unknown
    # attribute; each must hand `lst` back before raising.
    mapping = {'k': 'v'}
    pattern = re.compile('a')
    for obj in [
        time(1),
        datetime(2020, 1, 1),
        date(2020, 1, 1),
        timedelta(hours=1),
        pattern,
        pattern.match('a'),
        mapping.keys(),
        mapping.items(),
    ]:
        try:
            obj.bogus(lst)
            assert False, 'expected AttributeError'
        except AttributeError:
            pass
    return len(lst)


for _ in range(3):
    assert call_bogus() == 3
# ref-counts={'lst': 1, 're': 1}
