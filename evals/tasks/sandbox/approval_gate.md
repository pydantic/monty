# approval_gate

A refund run that pauses for a human decision mid-flight and resumes on another worker.
Refunds of `THRESHOLD` (500) or less are approved automatically; larger ones go through `request_approval`, a sync
host function called without `await`, one call per refund.
Return `approved_count`, `approved_total` and the declined ids.

`snapshot_at='request_approval'` makes the executor dump the suspended interpreter at every approval call, discard the
session, restore the dump in a fresh session and answer the call there.
The `snapshots` metric counts how many times that happened; eight of the fifteen refunds cross the threshold.

Scored with `ApproxExpected` on the totals, `result_size` (200 bytes) and nine host calls (one fetch plus eight
approvals).
