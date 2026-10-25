# Timestamp cohort analysis: `fromisoformat`, `strftime`, `weekday` and timedelta
# arithmetic.

import datetime

stamps = []
for i in range(500):
    stamps.append(f'2024-{i % 12 + 1:02d}-{i % 28 + 1:02d}T{i % 24:02d}:{i % 60:02d}:00')

start = datetime.datetime(2024, 1, 1)
weekday_counts = [0, 0, 0, 0, 0, 0, 0]
cohorts = {}
total_days = 0
for _ in range(4):
    for s in stamps:
        dt = datetime.datetime.fromisoformat(s)
        weekday_counts[dt.weekday()] += 1
        cohorts[dt.strftime('%Y-%m')] = cohorts.get(dt.strftime('%Y-%m'), 0) + 1
        total_days += (dt - start).days
weekend = weekday_counts[5] + weekday_counts[6]
assert total_days + weekend + len(cohorts) == 360_100
