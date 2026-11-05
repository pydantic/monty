| Workload               | Arc ms | Inline before ms | Inline fixed ms | Fixed vs Arc |
| ---------------------- | -----: | ---------------: | --------------: | -----------: |
| prefix_remove_ascii    |  3.999 |            3.844 |           3.738 |        -6.5% |
| prefix_remove_utf8     |  4.715 |           19.514 |           4.169 |       -11.6% |
| suffix_remove_ascii    |  4.104 |            3.835 |           3.824 |        -6.8% |
| suffix_remove_utf8     | 17.420 |            4.540 |           4.390 |       -74.8% |
| ndjson_ascii_tail      |  5.133 |            4.986 |           4.994 |        -2.7% |
| ndjson_utf8_tail       |  4.414 |            6.202 |           5.855 |       +32.7% |
| ndjson_utf8_splitlines |  2.685 |            2.648 |           2.658 |        -1.0% |
| splitlines_bulk_ascii  |  9.129 |            8.387 |           8.303 |        -9.1% |
| splitlines_bulk_utf8   | 10.091 |            9.912 |           9.826 |        -2.6% |
| splitlines_inline      |  1.352 |            1.607 |           1.613 |       +19.3% |
| json_request           |  2.026 |            1.946 |           1.943 |        -4.1% |
| template_fragments     |  1.433 |            1.458 |           1.431 |        -0.2% |
| docx_text              |  1.416 |            1.398 |           1.396 |        -1.4% |
