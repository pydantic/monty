//! Private compilation tables with final IDs assigned before publication.

use std::sync::Arc;

use ahash::AHashMap;
use num_bigint::BigInt;

use super::{BytesId, InternedString, Interns, LongIntId, SOURCE_ID_BASE, StaticStrings, StringId, next_string_id};
use crate::{function::Function, hash::WithHash};

/// An unpublished suffix of a session's tables.
/// The session rejects other insertions until this overlay commits or is dropped.
/// All IDs are final; existing entries are borrowed and new entries are owned here.
#[derive(Debug)]
pub(crate) struct CompileInterns<'i> {
    base: &'i Interns,
    strings: Vec<InternedString>,
    string_ids: AHashMap<String, StringId>,
    bytes: Vec<WithHash<Vec<u8>>>,
    long_ints: Vec<WithHash<BigInt>>,
    functions: Vec<Function>,
    sources: Vec<Arc<str>>,
}

impl<'i> CompileInterns<'i> {
    /// Reserves the next IDs without modifying the committed tables.
    pub(crate) fn new(base: &'i Interns) -> Self {
        assert!(!base.compiling.replace(true), "overlapping compilations");
        Self {
            base,
            strings: Vec::new(),
            string_ids: AHashMap::new(),
            bytes: Vec::new(),
            long_ints: Vec::new(),
            functions: Vec::new(),
            sources: Vec::new(),
        }
    }

    /// Publishes entries in provisional-ID order. No borrowed pending entry can survive this move.
    pub(crate) fn commit(mut self) {
        for entry in self.strings.drain(..) {
            let id = next_string_id(self.base.strings.len());
            if let Some(tag) = entry.static_value() {
                self.base.static_string_ids.borrow_mut().insert(tag, id);
            } else {
                self.base
                    .string_id_by_name
                    .borrow_mut()
                    .insert(entry.as_str().to_owned(), id);
            }
            self.base.strings.push(entry);
        }
        for entry in self.bytes.drain(..) {
            self.base.bytes.push(entry);
        }
        for entry in self.long_ints.drain(..) {
            self.base.long_ints.push(entry);
        }
        for entry in self.functions.drain(..) {
            self.base.functions.push(Box::new(entry));
        }
        for source in self.sources.drain(..) {
            self.base.eval_sources.push(source);
        }
    }

    /// Deduplicates against committed text before assigning a provisional ID.
    pub(crate) fn intern(&mut self, text: &str) -> StringId {
        if let Some(id) = self.get_string_id_by_name(text) {
            id
        } else {
            let id = next_string_id(self.base.strings.len() + self.strings.len());
            self.strings.push(match text.parse::<StaticStrings>() {
                Ok(tag) => InternedString::static_string(tag),
                Err(_) => InternedString::owned(text.to_owned()),
            });
            self.string_ids.insert(text.to_owned(), id);
            id
        }
    }

    /// Interns a compiler-generated name through the same private overlay.
    pub(crate) fn intern_static(&mut self, value: StaticStrings) -> StringId {
        self.intern(value.into())
    }

    /// Appends a bytes literal; these are not deduplicated.
    pub(crate) fn intern_bytes(&mut self, bytes: &[u8]) -> BytesId {
        let id = BytesId(
            (self.base.bytes.len() + self.bytes.len())
                .try_into()
                .expect("BytesId overflow"),
        );
        self.bytes.push(WithHash::for_bytes(bytes.to_vec()));
        id
    }

    /// Appends an integer literal too large for an immediate value.
    pub(crate) fn intern_long_int(&mut self, value: BigInt) -> LongIntId {
        let id = LongIntId(
            (self.base.long_ints.len() + self.long_ints.len())
                .try_into()
                .expect("LongIntId overflow"),
        );
        self.long_ints.push(WithHash::for_long_int(value));
        id
    }

    /// Borrows either committed or pending text for preparation and diagnostics.
    pub(crate) fn get_str(&self, id: StringId) -> &str {
        let base = next_string_id(self.base.strings.len()).index();
        if id.index() >= base {
            self.strings[id.index() - base].as_str()
        } else {
            self.base.get_str(id)
        }
    }

    /// Finds a name already present in either table.
    pub(crate) fn get_string_id_by_name(&self, text: &str) -> Option<StringId> {
        self.base
            .get_string_id_by_name(text)
            .or_else(|| self.string_ids.get(text).copied())
    }

    /// Records source separately from canonical strings, preserving equal-string ID equality.
    pub(crate) fn add_eval_source(&mut self, source: Arc<str>) -> StringId {
        let index = SOURCE_ID_BASE + self.base.eval_sources.len() + self.sources.len();
        let id = StringId(index.try_into().expect("source ID overflow"));
        self.sources.push(source);
        id
    }

    /// Appends a compiled function under its final session ID.
    pub(crate) fn push_function(&mut self, function: Function) -> usize {
        let index = self.functions_len();
        self.functions.push(function);
        index
    }

    /// Returns the next function ID, including unpublished functions.
    pub(crate) fn functions_len(&self) -> usize {
        self.base.functions.len() + self.functions.len()
    }
}

impl Drop for CompileInterns<'_> {
    fn drop(&mut self) {
        self.base.compiling.set(false);
    }
}
