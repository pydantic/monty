# paginate_and_count

Page through `list_orders(cursor)` until `next_cursor` is `None` and return a dict of order status to count.

The fixture has 94 orders in pages of 20, so five calls are needed and each cursor is only known after the previous page
returns.
`expected_call_batches` is therefore 5 as well: sequential calls are the floor here, not a failure to parallelise.

Scored with `EqualsExpected`.
