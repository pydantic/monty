import datetime

# === datetime.now() in standard execution (no # call-external marker) ===
# Regression test for https://github.com/pydantic/monty/issues/330
# Before the fix, calling datetime.now() in standard (non-suspendable) execution
# raised NotImplementedError because it was only wired as an OS call that
# required the iterative host callback path.

# naive datetime
now_naive = datetime.datetime.now()
assert isinstance(now_naive, datetime.datetime), 'datetime.now() should return a datetime'
assert now_naive.tzinfo is None, 'datetime.now() without tz should be naive'
assert 1 <= now_naive.month <= 12, 'datetime.now() month should be in 1..=12'
assert 1 <= now_naive.day <= 31, 'datetime.now() day should be in 1..=31'
assert 0 <= now_naive.hour <= 23, 'datetime.now() hour should be in 0..=23'
assert 0 <= now_naive.minute <= 59, 'datetime.now() minute should be in 0..=59'
assert 0 <= now_naive.second <= 59, 'datetime.now() second should be in 0..=59'
assert now_naive.year >= 2024, 'datetime.now() year should not be in the distant past'

# aware UTC datetime
now_utc = datetime.datetime.now(datetime.timezone.utc)
assert isinstance(now_utc, datetime.datetime), 'datetime.now(utc) should return a datetime'
assert now_utc.tzinfo is datetime.timezone.utc, 'datetime.now(timezone.utc) should preserve the utc singleton'

# aware fixed-offset datetime
plus_two = datetime.timezone(datetime.timedelta(hours=2))
now_plus_two = datetime.datetime.now(plus_two)
assert now_plus_two.tzinfo == plus_two, 'datetime.now(+2h) should preserve the offset'

# keyword tz argument
now_kw = datetime.datetime.now(tz=datetime.timezone.utc)
assert now_kw.tzinfo is not None, 'datetime.now(tz=...) should return aware datetime'

# explicit tz=None is equivalent to naive
now_tz_none = datetime.datetime.now(tz=None)
assert now_tz_none.tzinfo is None, 'datetime.now(tz=None) should be naive'

# date.today() shares the same OS-call plumbing, so the standard-execution
# fallback naturally applies to it as well.
today = datetime.date.today()
assert isinstance(today, datetime.date), 'date.today() should return a date'
assert today.year >= 2024, 'date.today() year should not be in the distant past'
assert 1 <= today.month <= 12, 'date.today() month should be in 1..=12'
assert 1 <= today.day <= 31, 'date.today() day should be in 1..=31'
# date.today() and datetime.now().date() should agree on the calendar date
# when called back-to-back (modulo a vanishingly-rare midnight crossing).
assert today == datetime.datetime.now().date(), 'date.today() should match datetime.now().date()'
