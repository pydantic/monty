//! Unit tests for the pure-Rust PEP 723 extractor (`monty_cpython::pep_723`).
//!
//! These exercise the regex + TOML parsing directly, with no embedded
//! interpreter, so they are fast and need no serialization (unlike the
//! session-level tests in `stdio_session.rs`).

use monty_cpython::pep_723::{Pep723Error, dependencies};

#[test]
fn no_metadata_block_yields_no_dependencies() {
    assert_eq!(
        dependencies("import os\nprint(os.getcwd())").unwrap(),
        Vec::<String>::new()
    );
    // A `# /// script` opener with no closing `# ///` is not a block.
    assert_eq!(
        dependencies("# /// script\n# dependencies = []\nimport os").unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn extracts_dependencies_from_a_script_block() {
    // Inline array.
    let inline = "# /// script\n# dependencies = [\"httpx\", \"rich>=13\"]\n# ///\nimport httpx";
    assert_eq!(
        dependencies(inline).unwrap(),
        vec!["httpx".to_owned(), "rich>=13".to_owned()]
    );

    // Multi-line array with a bare `#` blank metadata line and other keys.
    let multiline = "\
# /// script
# requires-python = \">=3.11\"
#
# dependencies = [
#   \"numpy\",
#   \"pandas==2.2.0\",
# ]
# ///
import numpy";
    assert_eq!(
        dependencies(multiline).unwrap(),
        vec!["numpy".to_owned(), "pandas==2.2.0".to_owned()]
    );
}

#[test]
fn script_block_without_dependencies_is_empty() {
    let code = "# /// script\n# requires-python = \">=3.12\"\n# ///\nprint('hi')";
    assert_eq!(dependencies(code).unwrap(), Vec::<String>::new());
}

#[test]
fn a_non_script_block_is_ignored() {
    // Only `script` blocks carry dependencies; an unknown type contributes none.
    let code = "# /// pyproject\n# dependencies = [\"ignored\"]\n# ///\nprint('hi')";
    assert_eq!(dependencies(code).unwrap(), Vec::<String>::new());
}

#[test]
fn two_script_blocks_are_an_error() {
    // Separated by a blank line so the regex sees two distinct blocks.
    let code = "# /// script\n# dependencies = [\"a\"]\n# ///\n\n# /// script\n# dependencies = [\"b\"]\n# ///\n";
    assert!(matches!(dependencies(code), Err(Pep723Error::MultipleBlocks)));
    assert_eq!(
        dependencies(code).unwrap_err().to_string(),
        "multiple PEP 723 script blocks found"
    );
}

#[test]
fn invalid_toml_is_an_error() {
    let code = "# /// script\n# dependencies = [unquoted\n# ///\n";
    assert!(matches!(dependencies(code), Err(Pep723Error::Toml(_))));
}

#[test]
fn non_string_dependencies_are_an_error() {
    let code = "# /// script\n# dependencies = [1, 2]\n# ///\n";
    assert!(matches!(dependencies(code), Err(Pep723Error::InvalidDependencies)));

    let not_an_array = "# /// script\n# dependencies = \"httpx\"\n# ///\n";
    assert!(matches!(
        dependencies(not_an_array),
        Err(Pep723Error::InvalidDependencies)
    ));
}
