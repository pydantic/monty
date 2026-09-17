# dashboard

Six named panels from a year of orders, each needing its own SQL and a different transform in Python: monthly
revenue (line), revenue by region per quarter (stacked_bar, a pivot), order counts by weekday and month (heatmap, a
grid built with `date.weekday()`), unit cost against units sold per product (scatter, a join), order amounts binned
by 100 with an open last bin (histogram), and revenue by category (pie).
The code also writes `/output/report.md` linking the image path each `draw_chart` call returned.
Host functions: `query`, `list_tables`, `describe_table` (sync) and `draw_chart` (async, kinds line, bar,
stacked_bar, scatter, histogram, heatmap, pie; `series` carries the rows of a stacked bar or heatmap).

Scored with `ApproxExpected` on `{'total_revenue', 'top_category'}`, a `Predicate` over the recorded chart calls
(names, kinds, point counts and the fixture's exact numbers per panel) and the report file, and `result_size`.
