//! Re-emitting a snippet's imports for the type checks of later snippets.

use std::fmt::Write;

use ruff_python_ast::{Alias, Stmt};
use ruff_python_parser::parse_module;

/// The `import` / `from ... import` statements at the top level of `source`,
/// one per line with aliases kept; `__future__` and relative imports are
/// dropped.
///
/// A session's committed snippets are star-imported from a `.pyi` into each
/// later check, and a stub re-exports an import only as `import x as x`, so the
/// `math` bound by `import math` in one snippet would be missing from the
/// next; injecting these lines ahead of that star import restores it. A
/// snippet that does not parse (and so never commits) yields nothing.
#[must_use]
pub fn top_level_imports(source: &str) -> String {
    let Ok(parsed) = parse_module(source) else {
        return String::new();
    };
    let mut imports = String::new();
    for statement in &parsed.syntax().body {
        match statement {
            Stmt::Import(import) => {
                writeln!(imports, "import {}", aliases(&import.names)).unwrap();
            }
            Stmt::ImportFrom(import) if import.level == 0 => {
                if let Some(module) = &import.module
                    && module.as_str() != "__future__"
                {
                    writeln!(imports, "from {} import {}", module.as_str(), aliases(&import.names)).unwrap();
                }
            }
            _ => {}
        }
    }
    imports
}

/// `a, b as c`: the names of one import statement.
fn aliases(names: &[Alias]) -> String {
    names
        .iter()
        .map(|alias| match &alias.asname {
            Some(asname) => format!("{} as {}", alias.name.as_str(), asname.as_str()),
            None => alias.name.as_str().to_owned(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}
