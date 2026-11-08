//! Symbol identity, including private names, without copying a class prefix per use.

use std::{fmt, mem};

use ahash::{AHashMap, AHashSet};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SymbolId(usize);

pub(super) struct SymbolName<'a> {
    prefix: Option<&'a str>,
    name: &'a str,
}

impl fmt::Display for SymbolName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(prefix) = self.prefix {
            write!(f, "_{prefix}")?;
        }
        f.write_str(self.name)
    }
}

#[derive(Clone)]
pub(super) struct Context<'a> {
    prefix: Option<Prefix<'a>>,
    // Generic class headers mangle only type parameters declared so far.
    type_parameters: Option<AHashSet<&'a str>>,
}

#[derive(Clone, Copy)]
struct Prefix<'a> {
    name: &'a str,
    node: Option<usize>,
}

pub(super) struct Names<'a> {
    edges: AHashMap<(usize, u8), usize>,
    symbols: AHashMap<usize, SymbolId>,
    cache: AHashMap<(usize, &'a str), SymbolId>,
    active: bool,
    pub context: Context<'a>,
}

impl<'a> Names<'a> {
    pub fn new() -> Self {
        Self {
            edges: AHashMap::new(),
            symbols: AHashMap::new(),
            cache: AHashMap::new(),
            active: false,
            context: Context {
                prefix: None,
                type_parameters: None,
            },
        }
    }

    fn extend(&mut self, mut parent: usize, bytes: &[u8]) -> usize {
        for &byte in bytes {
            let next = self.edges.len() + 1;
            parent = *self.edges.entry((parent, byte)).or_insert(next);
        }
        parent
    }

    pub fn class_context(name: &'a str, generic: bool) -> Context<'a> {
        let name = name.trim_start_matches('_');
        Context {
            prefix: (!name.is_empty()).then_some(Prefix { name, node: None }),
            type_parameters: generic.then(AHashSet::new),
        }
    }

    pub fn type_parameter(&mut self, name: &'a str) {
        if let Some(parameters) = &mut self.context.type_parameters {
            parameters.insert(name);
        }
    }

    fn activate(&mut self) {
        // Ordinary code only needs a string cache. Build exact byte identities when private aliases first occur.
        if !self.active {
            let cache = mem::take(&mut self.cache);
            for (&(_, name), &symbol) in &cache {
                let node = self.extend(0, name.as_bytes());
                self.symbols.insert(node, symbol);
            }
            self.cache = cache;
            self.active = true;
        }
    }

    fn private_prefix(&self, name: &str) -> Option<Prefix<'a>> {
        let private = name.starts_with("__") && !name.ends_with("__") && !name.contains('.');
        let mangle = private
            && self
                .context
                .type_parameters
                .as_ref()
                .is_none_or(|names| names.contains(name));
        mangle.then_some(self.context.prefix).flatten()
    }

    pub fn display(&self, name: &'a str) -> SymbolName<'a> {
        SymbolName {
            prefix: self.private_prefix(name).map(|prefix| prefix.name),
            name,
        }
    }

    pub fn key(&mut self, name: &'a str) -> SymbolId {
        let parent = if let Some(prefix) = self.private_prefix(name) {
            if let Some(node) = prefix.node {
                node
            } else {
                self.activate();
                let root = self.extend(0, b"_");
                let node = self.extend(root, prefix.name.as_bytes());
                self.context.prefix = Some(Prefix {
                    node: Some(node),
                    ..prefix
                });
                node
            }
        } else {
            0
        };
        if let Some(symbol) = self.cache.get(&(parent, name)) {
            return *symbol;
        }
        let symbol = SymbolId(self.cache.len() + 1);
        let symbol = if self.active {
            let node = self.extend(parent, name.as_bytes());
            *self.symbols.entry(node).or_insert(symbol)
        } else {
            symbol
        };
        self.cache.insert((parent, name), symbol);
        symbol
    }
}
