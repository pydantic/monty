| Workload                 | Arc ms | Combined inline-4 ms | Lazy view-3 ms |
| ------------------------ | -----: | -------------------: | -------------: |
| unicode_owners_unindexed | 24.972 |               23.440 |         23.434 |
| unicode_owners_indexed   | 51.060 |               49.950 |         47.860 |
| ndjson_utf8_tail         |  4.360 |                4.424 |          4.448 |
| prefix_remove_utf8       |  4.734 |                3.995 |          3.990 |
| hash_1_views             |  1.440 |                1.456 |          1.558 |
| hash_4_views             |  2.342 |                2.513 |          2.348 |
| hash_8_views             |  3.410 |                3.499 |          3.466 |
| hash_64_views            | 17.794 |               18.532 |         18.627 |
| splitlines_bulk_utf8     | 10.110 |                9.893 |          9.927 |
| remainder_2048kib        |  1.652 |                1.699 |          1.723 |
| offsets_2048kib          |  1.376 |                1.475 |          1.471 |
