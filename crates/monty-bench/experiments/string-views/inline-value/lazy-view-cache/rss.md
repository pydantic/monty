| Workload                 | Arc MiB | Combined inline-4 MiB | Lazy view-3 MiB |
| ------------------------ | ------: | --------------------: | --------------: |
| unicode_owners_unindexed |   45.38 |                 40.80 |           40.80 |
| unicode_owners_indexed   |   50.16 |                 56.31 |           47.14 |
| ndjson_utf8_tail         |   12.20 |                 12.16 |           12.17 |
| prefix_remove_utf8       |    8.45 |                  8.50 |            8.53 |
| hash_1_views             |    8.53 |                  8.52 |            8.55 |
| hash_4_views             |    8.50 |                  8.53 |            8.55 |
| hash_8_views             |    8.53 |                  8.53 |            8.55 |
| hash_64_views            |    8.53 |                  8.55 |            8.55 |
| splitlines_bulk_utf8     |   52.84 |                 41.33 |           41.38 |
| remainder_2048kib        |   16.20 |                 16.22 |           16.31 |
| offsets_2048kib          |   15.80 |                 15.77 |           15.84 |
