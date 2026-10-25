//! Scan snippet imports for later type checks and host stub requests.

use std::iter::once;

use ruff_python_ast::{Alias, ExceptHandler, Stmt};
use ruff_python_parser::parse_module;
use ruff_text_size::{Ranged, TextRange};

/// Imports collected by [`scan_imports`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SnippetImports {
    /// Import statements with aliases, one per line, injected before later checks' `.pyi` star import.
    /// This preserves bindings such as `import math`, which `.pyi` files do not re-export.
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

/// Collects imports outside function and class bodies, excluding `__future__` and relative imports.
/// Invalid syntax yields no imports. All module-level branches are included because the checker
/// cannot tell which ran, and any names bound at runtime must resolve.
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
    /// Appends a prelude line and records each module's first import.
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

/// Descends into compound statements, excluding function and class bodies whose imports bind locally.
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

fn top_level(dotted: &str) -> String {
    dotted.split('.').next().unwrap_or_default().to_owned()
}

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
