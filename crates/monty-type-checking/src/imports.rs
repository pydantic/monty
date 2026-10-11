//! The imports of a snippet: re-emitted for the type checks of later
//! snippets, and the modules a worker asks the host for stubs of.

use std::iter::once;

use ruff_python_ast::{Alias, ExceptHandler, Stmt};
use ruff_python_parser::parse_module;
use ruff_text_size::{Ranged, TextRange};

/// What one scan of a snippet's imports yields; see [`scan_imports`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SnippetImports {
    /// The `import` / `from ... import` statements, one per line with aliases
    /// kept, injected ahead of the `.pyi` star import of later checks: a stub
    /// re-exports an import only as `import x as x`, so `import math` in one
    /// snippet would otherwise be lost to the next.
    pub prelude: String,
    /// Each top-level module imported, once, in source order.
    pub modules: Vec<ImportedModule>,
}

/// A top-level module a snippet imports (`a` for `import a.b` and `from a.b import c`).
#[derive(Debug, PartialEq, Eq)]
pub struct ImportedModule {
    pub name: String,
    /// The first import statement naming it.
    pub range: TextRange,
}

/// Scans `source` for its imports outside any function or class body;
/// `__future__` and relative imports are dropped, and source that does not
/// parse yields nothing. Imports under a module-level `if`/`for`/`while`/
/// `try`/`with`/`match` are kept whether or not that branch ran: the checker
/// cannot know, and a name the runtime did bind must resolve.
#[must_use]
pub fn scan_imports(source: &str) -> SnippetImports {
    let Ok(parsed) = parse_module(source) else {
        return SnippetImports::default();
    };
    let mut imports = SnippetImports::default();
    collect(&parsed.syntax().body, &mut imports);
    imports
}

impl SnippetImports {
    /// Records one import statement: its prelude line, and each top-level
    /// module it names that has not been seen.
    fn add(&mut self, line: &str, modules: impl Iterator<Item = (String, TextRange)>) {
        self.prelude.push_str(line);
        self.prelude.push('\n');
        for (name, range) in modules {
            if !self.modules.iter().any(|module| module.name == name) {
                self.modules.push(ImportedModule { name, range });
            }
        }
    }
}

/// Appends the imports of `body`, descending into compound statements but not
/// into function or class bodies, whose imports bind locally.
fn collect(body: &[Stmt], imports: &mut SnippetImports) {
    for statement in body {
        match statement {
            Stmt::Import(import) => {
                let range = import.range();
                imports.add(
                    &format!("import {}", aliases(&import.names)),
                    import.names.iter().map(|alias| (top_level(alias.name.as_str()), range)),
                );
            }
            Stmt::ImportFrom(import) if import.level == 0 => {
                if let Some(module) = &import.module
                    && module.as_str() != "__future__"
                {
                    imports.add(
                        &format!("from {} import {}", module.as_str(), aliases(&import.names)),
                        once((top_level(module.as_str()), import.range())),
                    );
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

/// `a` for `a.b.c`.
fn top_level(dotted: &str) -> String {
    dotted.split('.').next().unwrap_or_default().to_owned()
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
