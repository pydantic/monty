# pydantic-monty download bump, Sep 13–18 2026

Analysis of the jump in `pydantic-monty` PyPI downloads, based on the raw
`bigquery-public-data.pypi.file_downloads` rows for `pydantic-monty`,
`pydantic-monty-runtime` and `pydantic-monty-client` from 2026-09-13 00:00 to
2026-09-18 19:33 UTC (4.6M rows, `playground/pypi_downloads_5d.csv`), queried
with DuckDB, plus the ClickPy public ClickHouse instance for longer trends.

## Summary

One new fleet, first seen at 23:26 UTC on Sep 12, accounts for essentially all
of the growth. It installs `pydantic-monty` 0.0.23 in fresh containers on AWS
alongside `strands-agents` (the AWS agent SDK), with no package cache, around
the clock. Everything else is flat at roughly 80–120k downloads a day.

## What the jump is

Daily downloads of `pydantic-monty` 0.0.23 from the fleet versus all other
traffic:

| Day            | Fleet installs | Everything else |
| -------------- | -------------- | --------------- |
| Sep 13 (Sun)   | 36k            | 48k             |
| Sep 14         | 73k            | 115k            |
| Sep 15         | 134k           | 114k            |
| Sep 16         | 44k            | 111k            |
| Sep 17         | 278k           | 118k            |
| Sep 18 (19:30) | 563k           | 80k             |

- Every install is a cold resolve: 99.99% fetch the runtime and client wheels
    within 3 seconds of the metapackage, so there is no cache or proxy in front
    of PyPI.
- The median gap between consecutive installs is 0 seconds. The fleet runs 24/7
    with a mild US-daytime bulge, and per-minute volume peaks at ~2,800.
- Traffic for versions other than 0.0.23 (0.0.18, 0.0.19, 0.0.21) is
    unchanged at ~80k a day.
- Before Sep 12 the same fingerprint appears at most 15 times a day, so the
    fleet is new, not a scale-up of an existing user.

## Infrastructure fingerprint

- **Host**: Ubuntu 26.04 EC2 instances, kernel `7.0.0-1012-aws` (a few on
    `7.0.0-1011-aws`), x86_64, US region. TLS 1.3 with the rustls default
    cipher, i.e. uv's HTTP stack.
- **Containers**: a fixed weighted mix of base images whose shares are the
    same every day: Debian 12 (45%), Debian 13 (30%), Debian 11 (14%), Ubuntu
    20.04 (4%), Alpine 3.18.3 (4%), then Alpine 3.17 / 3.20 / 3.21 / 3.22 and
    Ubuntu 22.04 / 24.04 in low single digits.
- **Toolchain injected at runtime**: every container gets uv and CPython
    3.12.14 regardless of distro, so Python is uv-managed rather than from the
    image. The whole fleet moved to uv 0.12.16 within the hour of its release
    (01:01 UTC Sep 18) and 15k installs were already on 0.12.17 an hour after
    it shipped (18:59 UTC Sep 18). uv is installed fresh per run, not baked in.
- **Not CI**: 96% of the fleet's installs have no CI environment variable set.
    The `ci=true` traffic is a separate GitHub Actions stream on Azure kernels
    that has been flat all month.

## Who is running it

The strongest clue is what else the same fleet downloads. A single BigQuery
query over all PyPI traffic on Sep 17 matching the kernel, Python and uv
fingerprint (`playground/fleet_codownloads_sep17.csv`, ~270 GB scanned) gives,
out of ~350k installs that day:

| Package                      | Installs |
| ---------------------------- | -------- |
| packaging / pydantic / boto3 | ~340k    |
| strands-agents               | 259,071  |
| pydantic-monty               | 258,721  |
| mcp                          | 265,927  |
| openai                       | 164,096  |
| aws-bedrock-token-generator  | 101,942  |
| anthropic                    | 59,158   |
| litellm / langchain-core     | ~53k     |

- `strands-agents` and `pydantic-monty` are installed in lockstep with
    identical per-distro splits, so they are in the same requirements set.
- About 60k of the installs also pull a kitchen-sink set (pytest,
    pytest-xdist, coverage, ruff, scipy, numpy, redis, lxml, openpyxl,
    beautifulsoup4), which looks like task environments being prepared for an
    agent to run tests in.
- No version of `strands-agents`, `strands-agents-tools`, `bedrock-agentcore`
    or the Strands monorepo (`strands-agents/harness-sdk`) depends on or
    mentions `pydantic-monty`, and no public GitHub code references both, so
    the harness is private.

Reading: a single organisation, very likely AWS or an AWS-hosted lab, running a
Strands-based agent that uses Monty as its sandboxed Python tool across a
corpus of containerised tasks, at RL-training or large-evaluation scale. The
fixed distro mix and constant kitchen-sink subset fit a task dataset with
varied Dockerfiles better than a multi-tenant hosting product. Starting on a
Sunday and scaling 15x in five days also points to a scheduled job rather than
organic adoption.

## What the data cannot tell us

PyPI logs carry no IP, ASN or account, so the operator cannot be named. The
co-download fingerprint identifies the application, not who runs it.

## Method and files

- ClickPy public ClickHouse (`sql-clickhouse.clickhouse.com`, user `demo`) for
    per-day / per-version / per-installer trends.
- BigQuery `bigquery-public-data.pypi.file_downloads` via the `pydantic-ai`
    GCP project; the 5-day export was streamed with the BigQuery Storage API
    (`playground/fetch_downloads.py`) because `bq query` stalls paging millions
    of rows. Total BigQuery spend under $2.
- DuckDB scripts: `playground/analyse_downloads.sql`, `playground/analyse2.sql`.
