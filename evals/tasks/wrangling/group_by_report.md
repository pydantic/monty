# group_by_report

Call `fetch_sales()` for ten rows of `region`, `product`, `amount` and `units`.
Return `{region: {product: {"amount": total to 2dp, "units": total, "orders": row count}}}`.

Scored with `ApproxExpected`; one host call is expected.
