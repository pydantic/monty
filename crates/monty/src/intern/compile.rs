//! Compiler access to session tables, with private overlays for recoverable compilation.

use std::sync::Arc;

use ahash::AHashMap;
use num_bigint::BigInt;

use super::{BytesId, InternedString, Interns, LongIntId, SOURCE_ID_BASE, StaticStrings, StringId, next_string_id};
use crate::{function::Function, hash::WithHash};

/// Compilation tables with final IDs assigned before execution.
/// Existing sessions use a private overlay so rejection retains no products.
/// Fresh programs may insert directly: the caller discards their tables on failure.
#[derive(Debug)]
pub(crate) struct CompileInterns<'i> {
    base: &'i Interns,
    pending: Option<PendingInterns>,
}

/// An unpublished suffix of a session's tables, discarded unless compilation commits.
#[derive(Debug, Default)]
struct PendingInterns {
    strings: Vec<InternedString>,
    string_ids: AHashMap<String, StringId>,
    static_string_ids: AHashMap<StaticStrings, StringId>,
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
            pending: Some(PendingInterns::default()),
        }
    }

    /// Compiles directly into exclusively borrowed tables that the caller discards on failure.
    /// Unlike `new`, this does not roll back; it is only for a fresh program, not a REPL feed.
    pub(crate) fn direct(base: &'i mut Interns) -> Self {
        assert!(!base.compiling.replace(true), "overlapping compilations");
        Self { base, pending: None }
    }

    /// Publishes an overlay in provisional-ID order; direct compilation is already published.
    pub(crate) fn commit(mut self) {
        if let Some(pending) = &mut self.pending {
            for entry in pending.strings.drain(..) {
                self.base.strings.push(entry);
            }
            self.base
                .string_id_by_name
                .borrow_mut()
                .extend(pending.string_ids.drain());
            self.base
                .static_string_ids
                .borrow_mut()
                .extend(pending.static_string_ids.drain());
            for entry in pending.bytes.drain(..) {
                self.base.bytes.push(entry);
            }
            for entry in pending.long_ints.drain(..) {
                self.base.long_ints.push(entry);
            }
            for entry in pending.functions.drain(..) {
                self.base.functions.push(Box::new(entry));
            }
            for source in pending.sources.drain(..) {
                self.base.eval_sources.push(source);
            }
        }
    }

    /// Deduplicates against pending and committed text before assigning an ID.
    pub(crate) fn intern(&mut self, text: &str) -> StringId {
        if text.is_empty() {
            StringId::EMPTY
        } else if text.len() == 1 {
            StringId::from_ascii(text.as_bytes()[0])
        } else if let Some(id) = self.pending.as_ref().and_then(|pending| pending.string_ids.get(text)) {
            *id
        } else if let Ok(tag) = text.parse::<StaticStrings>() {
            self.intern_static(tag)
        } else {
            let existing = self.base.string_id_by_name.borrow().get(text).copied();
            if let Some(id) = existing {
                id
            } else {
                self.push_string(InternedString::owned(text.to_owned()))
            }
        }
    }

    /// Interns a compiler-generated name without parsing its known static tag again.
    pub(crate) fn intern_static(&mut self, value: StaticStrings) -> StringId {
        let text: &'static str = value.into();
        if text.is_empty() {
            StringId::EMPTY
        } else if text.len() == 1 {
            StringId::from_ascii(text.as_bytes()[0])
        } else {
            let existing = self.get_static_id(value);
            if let Some(id) = existing {
                id
            } else {
                self.push_string(InternedString::static_string(value))
            }
        }
    }

    /// Appends a bytes literal; these are not deduplicated.
    pub(crate) fn intern_bytes(&mut self, bytes: &[u8]) -> BytesId {
        let pending_len = self.pending.as_ref().map_or(0, |pending| pending.bytes.len());
        let id = BytesId(
            (self.base.bytes.len() + pending_len)
                .try_into()
                .expect("BytesId overflow"),
        );
        let entry = WithHash::for_bytes(bytes.to_vec());
        if let Some(pending) = &mut self.pending {
            pending.bytes.push(entry);
        } else {
            self.base.bytes.push(entry);
        }
        id
    }

    /// Appends an integer literal too large for an immediate value.
    pub(crate) fn intern_long_int(&mut self, value: BigInt) -> LongIntId {
        let pending_len = self.pending.as_ref().map_or(0, |pending| pending.long_ints.len());
        let id = LongIntId(
            (self.base.long_ints.len() + pending_len)
                .try_into()
                .expect("LongIntId overflow"),
        );
        let entry = WithHash::for_long_int(value);
        if let Some(pending) = &mut self.pending {
            pending.long_ints.push(entry);
        } else {
            self.base.long_ints.push(entry);
        }
        id
    }

    /// Borrows either committed or pending text for preparation and diagnostics.
    pub(crate) fn get_str(&self, id: StringId) -> &str {
        let base = next_string_id(self.base.strings.len()).index();
        if let Some(pending) = &self.pending
            && id.index() >= base
        {
            pending.strings[id.index() - base].as_str()
        } else {
            self.base.get_str(id)
        }
    }

    /// Finds a name already present in either table.
    pub(crate) fn get_string_id_by_name(&self, text: &str) -> Option<StringId> {
        if text.is_empty() {
            Some(StringId::EMPTY)
        } else if text.len() == 1 {
            Some(StringId::from_ascii(text.as_bytes()[0]))
        } else if let Some(id) = self.pending.as_ref().and_then(|pending| pending.string_ids.get(text)) {
            Some(*id)
        } else if let Ok(tag) = text.parse::<StaticStrings>() {
            self.get_static_id(tag)
        } else {
            self.base.string_id_by_name.borrow().get(text).copied()
        }
    }

    /// Records source separately from canonical strings, preserving equal-string ID equality.
    pub(crate) fn add_eval_source(&mut self, source: Arc<str>) -> StringId {
        let pending_len = self.pending.as_ref().map_or(0, |pending| pending.sources.len());
        let index = SOURCE_ID_BASE + self.base.eval_sources.len() + pending_len;
        let id = StringId(index.try_into().expect("source ID overflow"));
        if let Some(pending) = &mut self.pending {
            pending.sources.push(source);
        } else {
            self.base.eval_sources.push(source);
        }
        id
    }

    /// Appends a compiled function under its final session ID.
    pub(crate) fn push_function(&mut self, function: Function) -> usize {
        let index = self.functions_len();
        if let Some(pending) = &mut self.pending {
            pending.functions.push(function);
        } else {
            self.base.functions.push(Box::new(function));
        }
        index
    }

    /// Returns the next function ID, including unpublished functions.
    pub(crate) fn functions_len(&self) -> usize {
        self.base.functions.len() + self.pending.as_ref().map_or(0, |pending| pending.functions.len())
    }

    /// Finds a known static tag without converting it back to text.
    fn get_static_id(&self, value: StaticStrings) -> Option<StringId> {
        self.pending
            .as_ref()
            .and_then(|pending| pending.static_string_ids.get(&value).copied())
            .or_else(|| self.base.static_string_ids.borrow().get(&value).copied())
    }

    /// Assigns an ID to new text and records its reverse lookup in the same compilation mode.
    fn push_string(&mut self, entry: InternedString) -> StringId {
        if let Some(pending) = &mut self.pending {
            let id = next_string_id(self.base.strings.len() + pending.strings.len());
            if let Some(tag) = entry.static_value() {
                pending.static_string_ids.insert(tag, id);
            } else {
                pending.string_ids.insert(entry.as_str().to_owned(), id);
            }
            pending.strings.push(entry);
            id
        } else {
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
            id
        }
    }
}

impl Drop for CompileInterns<'_> {
    fn drop(&mut self) {
        self.base.compiling.set(false);
    }
}
