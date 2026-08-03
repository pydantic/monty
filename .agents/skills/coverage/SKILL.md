---
name: coverage
description: Fetch coverage diff from Codecov for the current branch or a specific PR. Shows uncovered lines, patch coverage, and overall coverage change.
---

# Coverage

Fetch line-by-line coverage information from Codecov for a GitHub pull request.

## Instructions

Use this skill to check code coverage for your changes before merging.

### Current branch (auto-detect PR)

```bash
uv run scripts/codecov_diff.py
```

This auto-detects the org, repo, and PR number using the `gh` CLI based on the current branch.

### Specific PR number

```bash
uv run scripts/codecov_diff.py 123
```

## Output

The script outputs:
- PR title and state
- HEAD coverage (overall coverage on the branch)
- Patch coverage (coverage of changed lines only)
- Coverage change (+/- percentage)
- Per-file breakdown with:
  - Missed line count
  - Patch coverage percentage
  - Specific uncovered line numbers (as ranges like `45-48, 52, 60-65`)
  - Partial coverage line numbers

## Requirements

- The `gh` CLI must be installed and authenticated for auto-detection
- The PR must have Codecov coverage data uploaded
