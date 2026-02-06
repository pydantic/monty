"""
Fetch coverage diff from Codecov for a GitHub pull request.

This script uses Codecov's GraphQL API to fetch line-by-line coverage
information and outputs a text file with the coverage diff.

See https://x.com/samuelcolvin/status/2019838805210198289 for rationale.

Usage:
    uv run scripts/codecov_diff.py [-h] [--org ORG] [--repo REPO] [pr-number]

By default, the org, repo and PR are auto-detected using the gh CLI.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from typing import Any

import httpx

CODECOV_GRAPHQL_URL = 'https://api.codecov.io/graphql/gh'

# GraphQL query to get PR overview and impacted files
PULL_QUERY = """
query Pull($owner: String!, $repo: String!, $pullId: Int!) {
  owner(username: $owner) {
    repository(name: $repo) {
      __typename
      ... on Repository {
        pull(id: $pullId) {
          pullId
          title
          state
          head {
            commitid
            coverageAnalytics {
              totals {
                percentCovered
              }
            }
          }
          compareWithBase {
            __typename
            ... on Comparison {
              state
              patchTotals {
                percentCovered
              }
              headTotals {
                percentCovered
              }
              changeCoverage
              impactedFiles {
                __typename
                ... on ImpactedFiles {
                  results {
                    fileName
                    headName
                    missesCount
                    patchCoverage {
                      percentCovered
                    }
                    headCoverage {
                      percentCovered
                    }
                    changeCoverage
                  }
                }
              }
            }
            ... on FirstPullRequest {
              message
            }
            ... on MissingBaseCommit {
              message
            }
            ... on MissingHeadCommit {
              message
            }
            ... on MissingComparison {
              message
            }
            ... on MissingBaseReport {
              message
            }
            ... on MissingHeadReport {
              message
            }
          }
        }
      }
      ... on NotFoundError {
        message
      }
      ... on OwnerNotActivatedError {
        message
      }
    }
  }
}
"""

# GraphQL query to get line-level coverage for a specific file
FILE_COVERAGE_QUERY = """
query ImpactedFileComparison($owner: String!, $repo: String!, $pullId: Int!, $path: String!) {
  owner(username: $owner) {
    repository(name: $repo) {
      __typename
      ... on Repository {
        pull(id: $pullId) {
          compareWithBase {
            __typename
            ... on Comparison {
              impactedFile(path: $path) {
                headName
                patchCoverage {
                  percentCovered
                }
                segments {
                  __typename
                  ... on SegmentComparisons {
                    results {
                      header
                      lines {
                        baseNumber
                        headNumber
                        baseCoverage
                        headCoverage
                        content
                      }
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
"""


def get_repo_from_gh() -> tuple[str, str] | None:
    """Get the current repository's owner and name using gh CLI."""
    try:
        result = subprocess.run(
            ['gh', 'repo', 'view', '--json', 'owner,name'],
            capture_output=True,
            text=True,
            check=True,
        )
        data = json.loads(result.stdout)
        return data['owner']['login'], data['name']
    except (subprocess.CalledProcessError, json.JSONDecodeError, KeyError, FileNotFoundError):
        return None


def get_pr_from_gh() -> int | None:
    """Get the current PR number for the current branch using gh CLI."""
    try:
        result = subprocess.run(
            ['gh', 'pr', 'view', '--json', 'number'],
            capture_output=True,
            text=True,
            check=True,
        )
        data = json.loads(result.stdout)
        return data['number']
    except (subprocess.CalledProcessError, json.JSONDecodeError, KeyError, FileNotFoundError):
        return None


def graphql_request(query: str, variables: dict[str, Any]) -> dict[str, Any]:
    """Make a GraphQL request to Codecov API."""
    with httpx.Client(timeout=30.0) as client:
        response = client.post(
            CODECOV_GRAPHQL_URL,
            json={'query': query, 'variables': variables},
            headers={'Content-Type': 'application/json'},
        )
        response.raise_for_status()
        return response.json()


def get_pull_coverage(org: str, repo: str, pr_number: int) -> dict[str, Any] | None:
    """Fetch PR coverage overview from Codecov."""
    result = graphql_request(PULL_QUERY, {'owner': org, 'repo': repo, 'pullId': pr_number})

    # Navigate to the pull data
    data = result.get('data', {})
    owner = data.get('owner', {})
    repository = owner.get('repository', {})

    if repository.get('__typename') != 'Repository':
        print(f'Error: {repository.get("message", "Repository not found")}', file=sys.stderr)
        return None

    pull = repository.get('pull')
    if not pull:
        print('Error: Pull request not found', file=sys.stderr)
        return None

    return pull


def get_file_coverage(org: str, repo: str, pr_number: int, file_path: str) -> dict[str, Any] | None:
    """Fetch line-level coverage for a specific file."""
    result = graphql_request(
        FILE_COVERAGE_QUERY,
        {'owner': org, 'repo': repo, 'pullId': pr_number, 'path': file_path},
    )

    # Navigate to the file data
    data = result.get('data', {})
    owner = data.get('owner', {})
    repository = owner.get('repository', {})

    if repository.get('__typename') != 'Repository':
        return None

    pull = repository.get('pull', {})
    compare = pull.get('compareWithBase', {})

    if compare.get('__typename') != 'Comparison':
        return None

    return compare.get('impactedFile')


def parse_line_coverage(segments: list[dict[str, Any]]) -> tuple[list[int], list[int]]:
    """
    Parse segments to extract uncovered and partial line numbers.

    Coverage values from Codecov:
    - "H" = hit (covered)
    - "M" = miss (uncovered)
    - "P" = partial
    - null = not applicable (e.g., blank line, comment)
    """
    uncovered: list[int] = []
    partial: list[int] = []

    for segment in segments:
        lines = segment.get('lines', [])
        for line in lines:
            head_num = line.get('headNumber')
            head_cov = line.get('headCoverage')

            if head_num is None:
                continue

            # Convert to int since API returns strings
            head_num = int(head_num)

            if head_cov == 'M':
                uncovered.append(head_num)
            elif head_cov == 'P':
                partial.append(head_num)

    return sorted(set(uncovered)), sorted(set(partial))


def format_line_ranges(lines: list[int]) -> str:
    """Format a list of line numbers as ranges where consecutive."""
    if not lines:
        return ''

    ranges: list[str] = []
    start = lines[0]
    end = lines[0]

    for line in lines[1:]:
        if line == end + 1:
            end = line
        else:
            if start == end:
                ranges.append(str(start))
            else:
                ranges.append(f'{start}-{end}')
            start = end = line

    # Don't forget the last range
    if start == end:
        ranges.append(str(start))
    else:
        ranges.append(f'{start}-{end}')

    return ', '.join(ranges)


def format_percentage(value: float | None) -> str:
    """Format a percentage value."""
    if value is None:
        return 'N/A'
    return f'{value:.2f}%'


def fetch_codecov_coverage(org: str, repo: str, pr_number: int) -> None:
    """Fetch and print coverage data from Codecov for a GitHub PR."""
    url = f'https://app.codecov.io/gh/{org}/{repo}/pull/{pr_number}'

    pull = get_pull_coverage(org, repo, pr_number)
    if not pull:
        print(f'Error: Could not fetch coverage for {org}/{repo} PR #{pr_number}')
        return

    print(f'Coverage Report for {org}/{repo} PR #{pr_number}')
    print(f'URL: {url}')
    print(f'Title: {pull.get("title", "N/A")}')
    print(f'State: {pull.get("state", "N/A")}')
    print()

    compare = pull.get('compareWithBase', {})
    if compare.get('__typename') != 'Comparison':
        print(f'# Note: {compare.get("message", "No comparison available")}')
        return

    head_totals = compare.get('headTotals', {})
    patch_totals = compare.get('patchTotals', {})
    change = compare.get('changeCoverage')

    print(f'HEAD Coverage: {format_percentage(head_totals.get("percentCovered"))}')
    print(f'Patch Coverage: {format_percentage(patch_totals.get("percentCovered"))}')
    if change is not None:
        print(f'Change: {change:+.2f}%')
    print()

    impacted = compare.get('impactedFiles', {})
    if impacted.get('__typename') != 'ImpactedFiles':
        print('# No impacted files found')
        return

    for file_info in impacted.get('results', []):
        file_path = file_info.get('headName') or file_info.get('fileName')
        if not file_path:
            continue

        missed = file_info.get('missesCount', 0)
        patch_cov_data = file_info.get('patchCoverage')
        patch_cov = patch_cov_data.get('percentCovered') if patch_cov_data else None

        print(f'## {file_path}')
        if missed:
            print(f'   Missed: {missed} lines')
        if patch_cov is not None:
            print(f'   Patch: {patch_cov:.2f}%')

        if patch_cov is not None and patch_cov >= 100.0:
            print('   All changed lines covered!')
            print()
            continue

        file_data = get_file_coverage(org, repo, pr_number, file_path)
        if file_data:
            segments_data = file_data.get('segments', {})
            if segments_data.get('__typename') == 'SegmentComparisons':
                segments = segments_data.get('results', [])
                uncovered, partial = parse_line_coverage(segments)

                if uncovered:
                    print(f'   Uncovered lines: {format_line_ranges(uncovered)}')
                if partial:
                    print(f'   Partial lines: {format_line_ranges(partial)}')

                if not uncovered and not partial:
                    if missed:
                        print('   (Coverage details not available)')
                    else:
                        print('   All changed lines covered!')
            else:
                print('   (Line coverage not available)')
        else:
            print('   (Could not fetch line coverage)')

        print()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('pr', nargs='?', type=int, help='Pull request number (auto-detected if not provided)')
    parser.add_argument('--org', help='GitHub organization name (auto-detected if not provided)')
    parser.add_argument('--repo', help='Repository name (auto-detected if not provided)')
    args = parser.parse_args()

    org = args.org
    repo = args.repo
    if not org or not repo:
        repo_info = get_repo_from_gh()
        if repo_info:
            org, repo = repo_info
        else:
            print('Error: Could not detect repository. Use --org and --repo.', file=sys.stderr)
            sys.exit(1)

    pr_number = args.pr
    if not pr_number:
        pr_number = get_pr_from_gh()
        if not pr_number:
            print('Error: Could not detect PR number. Provide PR number as argument.', file=sys.stderr)
            sys.exit(1)

    fetch_codecov_coverage(org, repo, pr_number)


if __name__ == '__main__':
    main()
