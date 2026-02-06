"""
Fetch coverage diff from Codecov for a GitHub pull request.

This script uses Playwright to navigate to a Codecov PR page, extract
line-by-line coverage information, and output a text file with the coverage diff.

Usage:
    uv run scripts/codecov_diff.py pydantic monty 107
    uv run scripts/codecov_diff.py pydantic monty 107 --output coverage.txt
"""

from __future__ import annotations

import argparse
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from playwright.sync_api import Page, sync_playwright


@dataclass
class FileCoverage:
    """Coverage information for a single file."""

    path: str
    missed_lines: int
    head_coverage: str
    patch_coverage: str
    change: str
    uncovered_lines: list[int]
    partial_lines: list[int]


def wait_for_page_load(page: Page, timeout: float = 15.0) -> None:
    """Wait for the Codecov page to fully load."""
    start = time.time()
    while time.time() - start < timeout:
        # Check if the file list is visible
        if page.locator('text=Files changed').is_visible():
            # Wait a bit more for the data to load
            time.sleep(2)
            return
        time.sleep(0.5)
    raise TimeoutError('Page did not load within timeout')


def extract_file_list(page: Page) -> list[dict[str, Any]]:
    """
    Extract the list of files from the Codecov page.

    Returns a list of dicts with file path and coverage info.
    """
    return page.evaluate("""() => {
        const files = [];
        const filePattern = /^[a-zA-Z0-9_\\/-]+\\.(rs|py|ts|js|tsx|jsx|go|java|cpp|c|h)$/;

        // Find all file path elements in the file list
        const walker = document.createTreeWalker(
            document.body,
            NodeFilter.SHOW_TEXT,
            null,
            false
        );

        const seen = new Set();
        while (walker.nextNode()) {
            const text = walker.currentNode.textContent.trim();
            if (text.includes('/') && filePattern.test(text) && !seen.has(text)) {
                seen.add(text);
                files.push({path: text});
            }
        }
        return files;
    }""")


def get_file_row_info(page: Page, file_path: str) -> dict[str, Any]:
    """Get the file row info (missed lines, percentages) from the file list."""
    return page.evaluate(
        """(filePath) => {
        // Find the file path text element
        const walker = document.createTreeWalker(
            document.body,
            NodeFilter.SHOW_TEXT,
            null,
            false
        );

        while (walker.nextNode()) {
            const text = walker.currentNode.textContent.trim();
            if (text === filePath) {
                // Found it - get parent row and extract info
                let row = walker.currentNode.parentElement;
                // Walk up to find the row container (up to 10 levels)
                for (let i = 0; i < 10; i++) {
                    if (!row) break;

                    // Get all text content of this row element
                    const rowText = row.innerText || '';

                    // Look for patterns with percentages
                    // The row typically contains: filename | missed | head% | patch% | change%
                    const lines = rowText.split('\\n').map(s => s.trim()).filter(s => s);

                    let missed = 0;
                    let pcts = [];

                    for (const line of lines) {
                        // Check for standalone numbers (missed lines count)
                        if (/^\\d+$/.test(line)) {
                            missed = parseInt(line);
                        }
                        // Check for percentages
                        if (/^[+-]?[0-9.]+%$/.test(line)) {
                            pcts.push(line);
                        }
                    }

                    // If we found at least 2 percentages, we found the row
                    if (pcts.length >= 2) {
                        return {
                            missed: missed,
                            head: pcts[0] || '',
                            patch: pcts[1] || '',
                            change: pcts[2] || ''
                        };
                    }

                    row = row.parentElement;
                }
            }
        }
        return {missed: 0, head: '', patch: '', change: ''};
    }""",
        file_path,
    )


def parse_expanded_file_coverage(page: Page, file_path: str) -> dict[str, Any]:
    """
    Parse coverage for an expanded file section.

    The DOM structure is:
    - File row with path and stats
    - Expanded section with diff hunks
    - Each hunk has: old line nums | new line nums | code
    - Coverage markers in new line nums column:
      - Uncovered: <img> followed by line number
      - Partial: "!" text followed by line number
    """
    return page.evaluate(
        """(filePath) => {
        const uncovered = [];
        const partial = [];

        // Find all elements that could contain our file's diff
        // The pattern we're looking for in the DOM:
        // - A container element that has the file path in its text
        // - Within that container, look for line number patterns

        const allElements = document.querySelectorAll('*');

        for (const el of allElements) {
            const children = Array.from(el.children);
            if (children.length < 2) continue;

            for (let i = 0; i < children.length - 1; i++) {
                const first = children[i];
                const second = children[i + 1];

                // Check for the pattern: marker followed by line number
                const firstText = first.textContent?.trim();
                const secondText = second.textContent?.trim();

                // Partial coverage: "!" followed by number
                if (firstText === '!' && /^\\d+$/.test(secondText)) {
                    const lineNum = parseInt(secondText);
                    if (lineNum > 0 && lineNum < 100000) {
                        partial.push(lineNum);
                    }
                }

                // Uncovered: img/svg followed by number
                // Check if first element is an image or contains one
                const hasImg = first.tagName === 'IMG' ||
                               first.tagName === 'SVG' ||
                               first.querySelector('img, svg');

                if (hasImg && /^\\d+$/.test(secondText)) {
                    const lineNum = parseInt(secondText);
                    if (lineNum > 0 && lineNum < 100000) {
                        uncovered.push(lineNum);
                    }
                }
            }
        }

        // Deduplicate, sort, and filter out spurious small numbers (likely from UI elements)
        // Real line numbers in diffs are typically > 100
        const filterLineNums = (arr) => [...new Set(arr)]
            .filter(n => n > 50)  // Filter out spurious small numbers from UI elements
            .sort((a, b) => a - b);

        return {
            uncovered: filterLineNums(uncovered),
            partial: filterLineNums(partial)
        };
    }""",
        file_path,
    )


def scrape_codecov_pr(org: str, repo: str, pr_number: int) -> str:
    """
    Scrape coverage diff from Codecov for a GitHub PR.

    Args:
        org: GitHub organization name
        repo: Repository name
        pr_number: Pull request number

    Returns:
        Formatted coverage diff as a string
    """
    url = f'https://app.codecov.io/gh/{org}/{repo}/pull/{pr_number}'

    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        page = browser.new_page()

        print(f'Navigating to {url}...', file=sys.stderr)
        page.goto(url)

        # Wait for page to load
        print('Waiting for page to load...', file=sys.stderr)
        wait_for_page_load(page)

        output_lines: list[str] = []
        output_lines.append(f'# Coverage Report for {org}/{repo} PR #{pr_number}')
        output_lines.append(f'# URL: {url}')
        output_lines.append('')

        # Get summary percentages
        summary = page.evaluate("""() => {
            const text = document.body.innerText;
            const pcts = text.match(/(\\d+\\.\\d+)%/g) || [];
            return {
                head: pcts[0] || '',
                patch: pcts[1] || '',
                change: pcts[2] || ''
            };
        }""")

        if summary.get('head'):
            output_lines.append(f'HEAD Coverage: {summary["head"]}')
        if summary.get('patch'):
            output_lines.append(f'Patch Coverage: {summary["patch"]}')
        if summary.get('change'):
            output_lines.append(f'Change: {summary["change"]}')
        output_lines.append('')

        # Get file list
        files = extract_file_list(page)
        print(f'Found {len(files)} files with changes', file=sys.stderr)

        # Process each file ONE AT A TIME
        # Reload the page between files to ensure clean state
        for i, file_info in enumerate(files):
            file_path = file_info['path']
            print(f'Processing {file_path}...', file=sys.stderr)

            try:
                # Reload page if not first file (to clear any expanded state)
                if i > 0:
                    page.goto(url)
                    wait_for_page_load(page)

                # Get file row info from the main table
                row_info = get_file_row_info(page, file_path)

                # Build output header
                output_lines.append(f'## {file_path}')
                if row_info.get('missed'):
                    output_lines.append(f'   Missed: {row_info["missed"]} lines')
                if row_info.get('patch'):
                    output_lines.append(f'   Patch: {row_info["patch"]}')

                # If file has 100% patch coverage, skip detailed parsing
                patch_pct = row_info.get('patch', '')
                if patch_pct == '100.00%':
                    output_lines.append('   All changed lines covered!')
                    output_lines.append('')
                    continue

                # Click to expand for files with issues
                file_row = page.locator(f'text="{file_path}"').first
                if file_row.is_visible():
                    file_row.click()
                    time.sleep(1.0)

                    # Get coverage for THIS file only
                    coverage = parse_expanded_file_coverage(page, file_path)

                    if coverage['uncovered']:
                        output_lines.append(f'   Uncovered lines: {format_line_ranges(coverage["uncovered"])}')
                    if coverage['partial']:
                        output_lines.append(f'   Partial lines: {format_line_ranges(coverage["partial"])}')

                    if not coverage['uncovered'] and not coverage['partial']:
                        output_lines.append('   (Could not parse individual line coverage)')
                    output_lines.append('')

            except Exception as e:
                print(f'  Warning: Could not process {file_path}: {e}', file=sys.stderr)
                output_lines.append(f'## {file_path}')
                output_lines.append(f'   Error: {e}')
                output_lines.append('')

        browser.close()

    return '\n'.join(output_lines)


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


def main() -> None:
    parser = argparse.ArgumentParser(description='Fetch coverage diff from Codecov for a GitHub PR')
    parser.add_argument('org', help='GitHub organization name')
    parser.add_argument('repo', help='Repository name')
    parser.add_argument('pr_number', type=int, help='Pull request number')
    parser.add_argument(
        '--output',
        '-o',
        help='Output file path (default: stdout)',
        default=None,
    )

    args = parser.parse_args()

    result = scrape_codecov_pr(args.org, args.repo, args.pr_number)

    if args.output:
        Path(args.output).write_text(result)
        print(f'Coverage diff written to {args.output}', file=sys.stderr)
    else:
        print(result)


if __name__ == '__main__':
    main()
