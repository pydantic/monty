#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["rich==15.0.0"]
# ///
"""Show the LOC diff of each commit on the current branch, split into production, test and other code.

Usage:
    uv run scripts/branch_diff.py [--check] [--markdown] [--head HEAD] [base]

Rows are the commits in `base..HEAD`, then uncommitted changes (untracked files included), then the total.
Every path must match a glob in `RULES`; `--check` tests that for all tracked files.
In GitHub Actions the table is also written to the job summary and posted as a PR comment.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from collections.abc import Iterator
from dataclasses import dataclass, field
from enum import StrEnum
from pathlib import Path, PurePosixPath

from rich.console import Console
from rich.markup import escape
from rich.table import Table

MSG_WIDTH = 30
# hidden marker identifying the comment this script owns, so later runs edit it rather than adding another
COMMENT_MARKER = '<!-- branch-diff -->'
# author of the workflow's comments, checked since anyone can write the marker
COMMENT_AUTHOR = 'github-actions[bot]'


class Category(StrEnum):
    """The kind of code a file holds, one table column each."""

    PRODUCTION = 'production'
    TEST = 'test'
    OTHER = 'other'


# lines added and removed in a path, as `git diff --numstat` reports them
Numstat = tuple[int, int, str]

PRODUCTION = Category.PRODUCTION
TEST = Category.TEST
OTHER = Category.OTHER

# `PurePosixPath.full_match` globs, `**` spans any number of directories; the first match wins.
RULES: list[tuple[str, Category]] = [
    # everything in a crate's `src/` is production, whatever the crate is for
    ('crates/*/src/**', PRODUCTION),
    # generated and vendored code
    ('crates/monty-proto/tests/oracle/**', OTHER),
    ('crates/monty-js/ts/worker/component/**', OTHER),
    ('crates/monty-typeshed/vendor/**', OTHER),
    # tests, benchmarks and the crates that only exist to run them
    ('crates/*/tests/**', TEST),
    ('crates/*/benches/**', TEST),
    ('crates/monty/test_cases/**', TEST),
    ('crates/monty-bench/**', TEST),
    ('crates/monty-datatest/**', TEST),
    ('crates/monty-doctest/**', TEST),
    ('crates/fuzz/**', TEST),
    ('crates/monty-js/__test__/**', TEST),
    ('crates/monty-js/smoke-test/**', TEST),
    ('crates/monty-js/test-support/**', TEST),
    ('crates/monty-js/vitest*.config.ts', TEST),
    # docs, config, lock files and tooling
    ('**/*.md', OTHER),
    ('**/.gitignore', OTHER),
    ('**/.prettierignore', OTHER),
    ('**/Cargo.toml', OTHER),
    ('**/pyproject.toml', OTHER),
    ('**/package.json', OTHER),
    ('**/package-lock.json', OTHER),
    ('**/tsconfig.json', OTHER),
    ('.*', OTHER),
    ('.*/**', OTHER),
    ('docs/**', OTHER),
    ('limitations', OTHER),
    ('examples/**', OTHER),
    ('scripts/**', OTHER),
    ('Cargo.lock', OTHER),
    ('uv.lock', OTHER),
    ('clippy.toml', OTHER),
    ('mkdocs.yml', OTHER),
    ('Makefile', OTHER),
    ('LICENSE', OTHER),
    ('crates/monty-js/.cargo/**', OTHER),
    ('crates/monty-js/scripts/**', OTHER),
    ('crates/monty-proto/proto/buf.yaml', OTHER),
    ('crates/monty-python/example.py', OTHER),
    ('crates/monty-python/exercise.py', OTHER),
    ('crates/monty-typeshed/check.py', OTHER),
    ('crates/monty-typeshed/update.py', OTHER),
    # shipped code outside `src/`
    ('crates/*/build.rs', PRODUCTION),
    ('crates/monty-js/ts/**', PRODUCTION),
    ('crates/monty-js/*.d.ts', PRODUCTION),
    ('crates/monty-python/python/**', PRODUCTION),
    ('crates/monty-proto/proto/**/*.proto', PRODUCTION),
    ('crates/monty-wasm-runtime/wit/**', PRODUCTION),
    ('crates/monty-typeshed/custom/**', PRODUCTION),
]


@dataclass
class Counts:
    """Lines added and removed within one category."""

    added: int = 0
    removed: int = 0


@dataclass
class Row:
    """A single line of the table: a commit, the working changes or the branch total."""

    commit: str
    message: str
    counts: dict[Category, Counts] = field(default_factory=lambda: {c: Counts() for c in Category})


@dataclass
class Classifier:
    """Maps paths to categories, remembering those no rule matches so they can be reported together."""

    unclassified: set[str] = field(default_factory=set)

    def classify(self, path: str) -> Category | None:
        """The category of the first matching rule, recording the path if there is none."""
        pure_path = PurePosixPath(path)
        for pattern, category in RULES:
            if pure_path.full_match(pattern):
                return category
        self.unclassified.add(path)
        return None

    def exit_if_unclassified(self) -> None:
        """Exit with an error naming every path that matched no rule."""
        if self.unclassified:
            paths = '\n'.join(f'  {path}' for path in sorted(self.unclassified))
            sys.exit(f'Unable to classify these files, add rules for them to RULES in {__file__}:\n{paths}')


def main() -> None:
    args = parse_args()
    classifier = Classifier()

    if args.check:
        for path in git('ls-files', '-z').split('\0'):
            if path:
                classifier.classify(path)
        classifier.exit_if_unclassified()
        print('all tracked files are classified')
        return

    rows = build_rows(args.base, args.head, classifier)
    classifier.exit_if_unclassified()

    branch = os.environ.get('GITHUB_HEAD_REF') or git('rev-parse', '--abbrev-ref', 'HEAD')
    markdown = render_markdown(branch, rows)
    if args.markdown:
        print(markdown)
    else:
        print_table(branch, rows)
    publish(markdown)


def parse_args() -> argparse.Namespace:
    """Parse the command line, defaulting `base` to the PR's base branch in GitHub Actions."""
    base_ref = os.environ.get('GITHUB_BASE_REF')
    parser = argparse.ArgumentParser(description='Per-commit LOC diff split into production, test and other code.')
    parser.add_argument('base', nargs='?', default=f'origin/{base_ref}' if base_ref else 'main')
    parser.add_argument('--head', help='ref to report on instead of the checkout, omits working changes')
    parser.add_argument('--check', action='store_true', help='check every tracked file can be classified')
    parser.add_argument('--markdown', action='store_true', help='print the markdown table instead of the rich one')
    return parser.parse_args()


def build_rows(base: str, head: str | None, classifier: Classifier) -> list[Row]:
    """The commits of `base..head` oldest first, then working changes if any, then the branch total.

    Without `head` the checkout is used, and its uncommitted changes get a row and count towards the total.
    """
    # `git diff` never reports untracked files, so they are counted separately
    untracked = [] if head else list(untracked_numstat())
    rows: list[Row] = []
    # merges are skipped, in particular the `refs/pull/N/merge` commit CI checks out
    log = git('log', '--reverse', '--no-merges', '--format=%H%x09%s', f'{base}..{head or "HEAD"}')
    for line in log.splitlines():
        sha, message = line.split('\t', 1)
        rows.append(diff_row(sha[:9], message, classifier, [], 'show', '--format=', sha))

    if head is None:
        working = diff_row('', 'working changes', classifier, untracked, 'diff', 'HEAD')
        if any(c.added or c.removed for c in working.counts.values()):
            rows.append(working)

    # with no second ref the working tree is diffed, so the total includes uncommitted changes
    merge_base = git('merge-base', base, head or 'HEAD')
    rows.append(diff_row('', 'TOTAL', classifier, untracked, 'diff', merge_base, *([head] if head else [])))
    return rows


def untracked_numstat() -> Iterator[Numstat]:
    """Yield the line counts of untracked files that are not ignored, binary files counting as no lines."""
    # `--numstat` paths are relative to the top level, so list from there
    top_level = git('rev-parse', '--show-toplevel')
    for path in git('-C', top_level, 'ls-files', '--others', '--exclude-standard', '-z').split('\0'):
        file = Path(top_level, path)
        if path and file.is_symlink():
            # git stores a symlink as its target path, a single line
            yield (1, 0, path)
        elif path and file.is_file():
            yield (count_lines(file), 0, path)


def count_lines(file: Path) -> int:
    """The lines in a file, none if it is binary; read in chunks since untracked files can be large."""
    lines, last = 0, b'\n'
    with file.open('rb') as f:
        while chunk := f.read(1 << 20):
            if b'\0' in chunk:
                return 0
            lines += chunk.count(b'\n')
            last = chunk[-1:]
    # a final line without a newline still counts
    return lines + (last != b'\n')


def diff_row(commit: str, message: str, classifier: Classifier, extra: list[Numstat], *git_args: str) -> Row:
    """Sum the counts of a git diff command and of `extra` by category."""
    row = Row(commit, message)
    for added, removed, path in (*parse_numstat(git(*git_args, '--numstat', '-z')), *extra):
        if category := classifier.classify(path):
            row.counts[category].added += added
            row.counts[category].removed += removed
    return row


def parse_numstat(output: str) -> Iterator[Numstat]:
    """Yield `(added, removed, path)` from `--numstat -z` output, binary files counting as no lines.

    A rename is `added\\tremoved\\t` followed by the old and new paths as separate fields, it is reported
    under the new path.
    """
    fields = iter(output.lstrip('\n').split('\0'))
    for entry in fields:
        if entry:
            added, removed, path = entry.split('\t', 2)
            if not path:
                next(fields)
                path = next(fields)
            yield (0 if added == '-' else int(added), 0 if removed == '-' else int(removed), path)


def git(*args: str) -> str:
    """Run a git command and return its stdout without the trailing newline, raising on failure."""
    # paths and commit messages on a PR can hold bytes that are not UTF-8, replaced rather than failing to decode
    result = subprocess.run(('git', *args), capture_output=True, encoding='utf-8', errors='replace', check=True)
    return result.stdout.removesuffix('\n')


def truncate(message: str) -> str:
    """Truncate a commit message to `MSG_WIDTH` characters with an ellipsis."""
    return message if len(message) <= MSG_WIDTH else message[: MSG_WIDTH - 1] + '…'


def print_table(branch: str, rows: list[Row]) -> None:
    """Print the table with rich, the `+` and `-` counts of each column aligned across rows."""
    # escape untrusted branch names and commit messages, rich would parse them as markup
    table = Table(title=f'Branch: {escape(branch)}', title_style='bold cyan', title_justify='left')
    table.add_column('Commit', style='yellow')
    table.add_column('Message')
    for category in Category:
        table.add_column(category.capitalize(), justify='right')

    # pad the counts ourselves so the `+` and `-` halves each form their own aligned sub-column
    widths = {
        category: (
            max(len(f'+{row.counts[category].added}') for row in rows),
            max(len(f'-{row.counts[category].removed}') for row in rows),
        )
        for category in Category
    }
    for i, row in enumerate(rows):
        cells: list[str] = []
        for category in Category:
            counts = row.counts[category]
            added_width, removed_width = widths[category]
            added, removed = f'+{counts.added}', f'-{counts.removed}'
            dim = '' if counts.added or counts.removed else 'dim '
            cells.append(f'[{dim}green]{added:>{added_width}}[/] [{dim}red]{removed:<{removed_width}}[/]')
        last = i == len(rows) - 1
        message = escape(truncate(row.message))
        table.add_row(row.commit, message, *cells, style='bold' if last else None, end_section=not last)

    Console().print(table)


def render_markdown(branch: str, rows: list[Row]) -> str:
    """The table as GitHub markdown, starting with the marker that identifies the PR comment."""
    lines = [
        COMMENT_MARKER,
        f'### Branch diff: {code_span(branch)}',
        '',
        '| Commit | Message | Production | Test | Other |',
        '| --- | --- | --: | --: | --: |',
    ]
    for i, row in enumerate(rows):
        # commit messages on a PR are untrusted, keep them from adding table cells or HTML
        message = truncate(row.message).replace('|', '\\|').replace('<', '&lt;')
        cells = [f'+{row.counts[c].added} -{row.counts[c].removed}' for c in Category]
        if i == len(rows) - 1:
            message = f'**{message}**'
            cells = [f'**{cell}**' for cell in cells]
        # a bare sha is linked to the commit by GitHub
        lines.append(f'| {row.commit} | {message} | {" | ".join(cells)} |')
    return '\n'.join(lines) + '\n'


def code_span(text: str) -> str:
    """Wrap untrusted text in a markdown code span it cannot close."""
    # a span only closes at a backtick run as long as its delimiter
    longest_run = max((len(run) for run in re.findall('`+', text)), default=0)
    delimiter = '`' * (longest_run + 1)
    return f'{delimiter} {text} {delimiter}'


def publish(markdown: str) -> None:
    """In GitHub Actions write the job summary and, on a pull request with a token, the PR comment."""
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        return

    if summary_path := os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(summary_path, 'a') as f:
            f.write(markdown)

    token = os.environ.get('GH_TOKEN') or os.environ.get('GITHUB_TOKEN')
    if os.environ.get('GITHUB_EVENT_NAME') not in ('pull_request', 'pull_request_target') or not token:
        return

    with open(os.environ['GITHUB_EVENT_PATH']) as f:
        pr_number: int = json.load(f)['pull_request']['number']
    try:
        post_comment(os.environ['GITHUB_REPOSITORY'], pr_number, markdown, token)
    except subprocess.CalledProcessError as e:
        # the token is read-only on PRs from forks, the job summary still has the table
        print(f'::warning::unable to post the branch diff comment: {e.stderr.strip()}')


def post_comment(repo: str, pr_number: int, markdown: str, token: str) -> None:
    """Update the workflow's PR comment starting with `COMMENT_MARKER`, creating it if there is none."""
    env = {**os.environ, 'GH_TOKEN': token}

    def gh_api(*args: str, stdin: str | None = None) -> str:
        result = subprocess.run(('gh', 'api', *args), input=stdin, capture_output=True, text=True, check=True, env=env)
        return result.stdout

    comments_url = f'repos/{repo}/issues/{pr_number}/comments'
    jq = f'.[] | select(.user.login == "{COMMENT_AUTHOR}" and (.body | startswith("{COMMENT_MARKER}"))) | .id'
    comment_ids = gh_api(comments_url, '--paginate', '--jq', jq).split()
    if comment_ids:
        gh_api('-X', 'PATCH', f'repos/{repo}/issues/comments/{comment_ids[0]}', '-F', 'body=@-', stdin=markdown)
    else:
        gh_api('-X', 'POST', comments_url, '-F', 'body=@-', stdin=markdown)


if __name__ == '__main__':
    main()
