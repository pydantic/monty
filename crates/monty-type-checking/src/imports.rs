//! Re-emitting a snippet's imports for the type checks of later snippets.

use std::fmt::Write;

use ruff_python_ast::{Alias, ExceptHandler, Stmt};
use ruff_python_parser::parse_module;

/// The `import` / `from ... import` statements of `source` outside any function
/// or class body, one per line with aliases kept; `__future__` and relative
/// imports are dropped, and source that does not parse yields nothing.
///
/// Committed snippets reach later checks through a `.pyi` star import, and a
/// stub re-exports an import only as `import x as x`, so `import math` in one
/// snippet would be lost to the next; these lines, injected ahead of the star
/// import, restore it. Imports under a module-level `if`/`for`/`while`/`try`/
/// `with`/`match` are kept whether or not that branch ran: the checker cannot
/// know, and a name the runtime did bind must resolve.
#[must_use]
pub fn top_level_imports(source: &str) -> String {
    let Ok(parsed) = parse_module(source) else {
        return String::new();
    };
    let mut imports = String::new();
    collect(&parsed.syntax().body, &mut imports);
    imports
}

/// Appends the imports of `body`, descending into compound statements but not
/// into function or class bodies, whose imports bind locally.
fn collect(body: &[Stmt], imports: &mut String) {
    for statement in body {
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
            Stmt::If(node) => {
                collect(&node.body, imports);
                for clause in &node.elif_else_clauses {
                    collect(&clause.body, imports);
                }
            }
            Stmt::For(node) => {
                collect(&node.body, imports);
                collect(&node.orelse, imports);
            }
            Stmt::While(node) => {
                collect(&node.body, imports);
                collect(&node.orelse, imports);
            }
            Stmt::With(node) => collect(&node.body, imports),
            Stmt::Try(node) => {
                collect(&node.body, imports);
                for ExceptHandler::ExceptHandler(handler) in &node.handlers {
                    collect(&handler.body, imports);
                }
                collect(&node.orelse, imports);
                collect(&node.finalbody, imports);
            }
            Stmt::Match(node) => {
                for case in &node.cases {
                    collect(&case.body, imports);
                }
            }
            _ => {}
        }
    }
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
