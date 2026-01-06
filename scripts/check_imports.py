#!/usr/bin/env python3
"""Check that all `use` statements in Rust files are at the top of the file.

Exits with code 1 if any misplaced imports are found.
"""

import re
import sys
from pathlib import Path


def check_file(path: Path) -> list[str]:
    """Check a single Rust file for misplaced imports.

    Returns a list of error messages for any `use` statements found after
    the import block (i.e., after non-import code has started).

    Handles these valid patterns:
    - mod declarations followed by pub use re-exports
    - use statements inside mod/fn/impl blocks (tracked via brace depth)
    """
    errors: list[str] = []
    past_imports = False
    brace_depth = 0

    # Patterns that are allowed before/during the import block at top level
    allowed_pattern = re.compile(
        r'^(\s*$'  # empty lines
        r'|//[!/]'  # doc comments (//! or ///)
        r'|//'  # regular comments
        r'|/\*'  # block comment start
        r'|\*'  # block comment continuation
        r'|\*/'  # block comment end
        r'|#\['  # attributes
        r'|#!\['  # inner attributes
        r'|pub use '  # pub use statements
        r'|use '  # use statements
        r'|(pub )?mod (r#)?\w+;'  # mod declarations (mod foo; or pub mod r#type;)
        r'|\}'  # closing braces
        r')'
    )

    use_pattern = re.compile(r'^use |^pub use ')

    with open(path) as f:
        for line_num, line in enumerate(f, 1):
            stripped = line.strip()

            # Track brace depth to detect when we're inside a block
            brace_depth += line.count('{') - line.count('}')

            # Only check top-level imports (brace_depth == 0)
            if brace_depth > 0:
                continue

            # Skip allowed lines if we haven't passed the import block
            if not past_imports and allowed_pattern.match(stripped):
                continue

            # Once we see a non-allowed line at top level, we're past imports
            if not past_imports and stripped:
                past_imports = True

            # Check for use statements after the import block at top level
            if past_imports and use_pattern.match(stripped):
                errors.append(f'{path}:{line_num}: {stripped}')

    return errors


def main() -> int:
    """Find all Rust files and check for misplaced imports."""
    all_errors: list[str] = []

    for rs_file in Path('./crates').rglob('*.rs'):
        all_errors += check_file(rs_file)

    if all_errors:
        print('Error: Found `use` statements outside the import block:', file=sys.stderr)
        print('Move these imports to the top of the file.', file=sys.stderr)
        print(file=sys.stderr)
        for error in all_errors:
            print(error, file=sys.stderr)
        return 1

    return 0


if __name__ == '__main__':
    sys.exit(main())
