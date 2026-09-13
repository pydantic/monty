# polars_proxy

Analyse `df`, a host object with a polars-shaped API, and return the two regions with the highest total amount among
line items of at least 5 units, plus the number of such items.

`df` is a `ClassInstance` over a host `Frame`; `filter`, `select`, `sort`, `head`, `group_by(...).agg(...)` and
`to_dicts` are sync methods and `height` is a lazy attribute.
Every method returns a new host object, and a `ClassInstance` subclass's `convert_value` wraps each one in another
proxy so chaining works.
The stubs describe `Frame` and `GroupBy` as classes so the type checker follows the chain.

Scored with `EqualsExpected` against the same chain evaluated host-side, and a 300-byte result limit.
Method calls on host objects are not counted as external calls.
