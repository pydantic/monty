# anomaly_scan

Page through `fetch_metrics(page)` for a year of hourly metrics (8,760 rows, nine pages of 1,000) and return the dates
whose daily total is anomalous.
Anomalous means a two-sided p-value below `P_THRESHOLD`, from the z-score of the day's total against the mean and sample
standard deviation of the previous seven days, with `math.erfc` for the tail.

Four days are scaled in the fixture, spaced so one does not hide the next in the window.
The prompt asks for daily totals only to be kept, not the hourly rows.

Scored with `EqualsExpected`; nine sequential host calls are expected, since each cursor comes from the previous page.
