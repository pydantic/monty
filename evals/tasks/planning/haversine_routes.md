# haversine_routes

Call `fetch_depots()` for three depots and `fetch_deliveries()` for fifteen deliveries, all as lat/lng.
Assign each delivery to its nearest depot by haversine distance, then order each depot's stops as a nearest-neighbour
route starting at the depot.
Return `routes` (depot id to ordered delivery ids) and `total_km` rounded to one decimal place.

The distance needs `math.radians`, `sin`, `cos`, `sqrt` and `atan2`; the route order is deterministic so the per-depot
lists compare exactly.

Scored with `ApproxExpected`; two host calls are expected, in one batch.
