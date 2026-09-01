//! Maps every rendered item to its page anchor for intra-doc link resolution.
//!
//! rustdoc resolves `` [`Pool::checkout`] `` shorthand into an [`Item::links`]
//! entry pointing at an item id. This module records, across all generated
//! pages, which anchor each id lands on so those links can be rewritten to
//! real markdown links; anything unresolvable is degraded to a plain code
//! span by the caller (a wrong or missing anchor would fail `mkdocs --strict`
//! or silently 404 on the docs site).

use std::collections::{HashMap, HashSet};

use rustdoc_types::{Crate, Id, Item, ItemEnum};

/// Anchor locations for every item rendered on any page.
///
/// Methods, fields and variants map to their *parent* item's anchor: method
/// headings repeat across a page (`new`, `len`, ...) and mkdocs and Starlight
/// deduplicate anchors with different suffix schemes, so only the unique
/// top-level anchors are safe link targets on both platforms.
pub struct SymbolMap {
    /// (defining crate as rustdoc names it e.g. `monty_pool`, item name) → anchor.
    root_items: HashMap<(String, String), String>,
    /// Per rustdoc crate name: any id in a rendered subtree → its page anchor.
    ids: HashMap<String, HashMap<Id, String>>,
    /// rustdoc crate name → page file name, e.g. `monty_pool` → `monty-pool.md`.
    pages: HashMap<String, String>,
}

impl SymbolMap {
    /// Indexes the root items (and their subtrees) of every crate to be
    /// rendered. `crates` pairs each publishable crate name with its JSON.
    pub fn build(crates: &[(&str, &Crate)]) -> Self {
        let mut map = Self {
            root_items: HashMap::new(),
            ids: HashMap::new(),
            pages: HashMap::new(),
        };
        for (name, krate) in crates {
            let rustdoc_name = name.replace('-', "_");
            map.pages.insert(rustdoc_name.clone(), format!("{name}.md"));
            map.index_crate(&rustdoc_name, krate);
        }
        map
    }

    /// Resolves a link target id found in `from_crate`'s JSON to a URL
    /// relative to `from_crate`'s own page.
    pub fn resolve(&self, from_crate: &str, krate: &Crate, id: Id) -> Option<String> {
        if let Some(anchor) = self.ids.get(from_crate).and_then(|ids| ids.get(&id)) {
            return Some(format!("#{anchor}"));
        }
        // not in this crate's index: look up the defining path of the
        // external id and match it against another rendered crate's root
        let summary = krate.paths.get(&id)?;
        let defining_crate = summary.path.first()?;
        let item_name = summary.path.last()?;
        let anchor = self.root_items.get(&(defining_crate.clone(), item_name.clone()))?;
        if defining_crate == from_crate {
            Some(format!("#{anchor}"))
        } else {
            Some(format!("{}#{anchor}", self.pages.get(defining_crate)?))
        }
    }

    /// Walks one crate's root module, assigning each root item an anchor and
    /// mapping its whole subtree (methods, fields, variants) to that anchor.
    /// Only page-unique anchors are recorded — see the struct docs.
    fn index_crate(&mut self, rustdoc_name: &str, krate: &Crate) {
        let mut taken = HashSet::new();
        let ids = self.ids.entry(rustdoc_name.to_owned()).or_default();
        let root = &krate.index[&krate.root];
        let ItemEnum::Module(module) = &root.inner else {
            panic!("crate root of {rustdoc_name} is not a module")
        };
        for entry in &module.items {
            let Some((name, item)) = resolve_root_entry(krate, *entry) else {
                continue;
            };
            let anchor = name.to_lowercase();
            if !taken.insert(anchor.clone()) {
                continue; // a later duplicate would get a platform-specific suffix
            }
            self.root_items.insert((rustdoc_name.to_owned(), name), anchor.clone());
            index_subtree(krate, item, &anchor, ids);
            // nested public modules: their children get their own anchors
            if let ItemEnum::Module(nested) = &item.inner {
                for child_id in &nested.items {
                    let Some((child_name, child)) = resolve_root_entry(krate, *child_id) else {
                        continue;
                    };
                    let child_anchor = child_name.to_lowercase();
                    if !taken.insert(child_anchor.clone()) {
                        continue;
                    }
                    self.root_items
                        .insert((rustdoc_name.to_owned(), child_name), child_anchor.clone());
                    index_subtree(krate, child, &child_anchor, ids);
                }
            }
        }
    }
}

/// Resolves one module entry to the (rendered name, target item) pair,
/// following `pub use` re-exports; `None` for glob or cross-crate re-exports.
pub fn resolve_root_entry(krate: &Crate, id: Id) -> Option<(String, &Item)> {
    let item = &krate.index[&id];
    match &item.inner {
        ItemEnum::Use(use_) => {
            let target = use_.id.filter(|_| !use_.is_glob)?;
            let target = krate.index.get(&target)?;
            Some((use_.name.clone(), target))
        }
        _ => Some((item.name.clone()?, item)),
    }
}

/// Maps `item` and everything rendered under it (impl methods, fields,
/// variants, trait items) to `anchor`.
fn index_subtree(krate: &Crate, item: &Item, anchor: &str, ids: &mut HashMap<Id, String>) {
    ids.insert(item.id, anchor.to_owned());
    let children: &[Id] = match &item.inner {
        ItemEnum::Struct(s) => &s.impls,
        ItemEnum::Enum(e) => &e.variants,
        ItemEnum::Trait(t) => &t.items,
        ItemEnum::Impl(i) => &i.items,
        ItemEnum::Variant(_) | ItemEnum::Union(_) => &[],
        _ => return,
    };
    // enums also carry impls alongside variants
    if let ItemEnum::Enum(e) = &item.inner {
        for impl_id in &e.impls {
            if let Some(impl_item) = krate.index.get(impl_id) {
                index_subtree(krate, impl_item, anchor, ids);
            }
        }
    }
    for child_id in children {
        if let Some(child) = krate.index.get(child_id) {
            index_subtree(krate, child, anchor, ids);
        }
    }
}
