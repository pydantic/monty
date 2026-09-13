# ecommerce

The PyData London example's failure mode: the prompt documents `customers.channel`, the table has
`acquisition_channel`, so the first query returns `[{'error': ...}]` and the code has to call `describe_table`
before retrying.

The question is which acquisition channel brings in the highest average total spend per customer (customers with no
orders count as 0).
The answer must also be written with `insert_rows` into `reports` as a `best_channel` row.
Host functions: `query`, `list_tables`, `describe_table`, `insert_rows`, `table_count`, all sync, on an in-memory
SQLite database rebuilt by `setup`.

Scored with `ApproxExpected` on `{'success': True, 'result': {'channel', 'avg_spend'}}`, a `Predicate` that reads
the `reports` row back from the database, and `result_size` (200 bytes).
