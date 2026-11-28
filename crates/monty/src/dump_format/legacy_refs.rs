//! Versions 13–18 stored only the arena index in `Value::Ref`. Read the heap's
//! variant names first so references can acquire their tags during deserialization.

use std::{cell::RefCell, fmt};

use minicbor_serde::{Deserializer, error::DecodeError};
use serde::{
    Deserialize,
    de::{Error as _, IgnoredAny, MapAccess, Visitor},
};

use crate::heap::{HeapId, HeapKind};

#[derive(Deserialize)]
struct Slot {
    #[serde(rename = "D", deserialize_with = "deserialize_kind")]
    kind: HeapKind,
}

thread_local! {
    static LEGACY_SLOTS: RefCell<Option<Vec<Option<Slot>>>> = const { RefCell::new(None) };
}

/// Installs the arena index for the duration of one legacy CBOR decode.
pub(super) fn load<T>(bytes: &[u8], decode: impl FnOnce() -> Result<T, DecodeError>) -> Result<T, DecodeError> {
    struct Restore(Option<Vec<Option<Slot>>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            LEGACY_SLOTS.replace(self.0.take());
        }
    }

    let slots = read_slots(bytes)?;
    let _restore = Restore(LEGACY_SLOTS.replace(Some(slots)));
    decode()
}

pub(crate) fn deserialize_ref<'de, D: serde::Deserializer<'de>>(de: D) -> Result<(HeapKind, HeapId), D::Error> {
    LEGACY_SLOTS.with_borrow(|slots| match slots {
        Some(slots) => {
            let id = HeapId::deserialize(de)?;
            let slot = slots
                .get(id.index())
                .and_then(Option::as_ref)
                .ok_or_else(|| D::Error::custom("missing heap slot for legacy reference"))?;
            Ok((slot.kind, id))
        }
        None => <(HeapKind, HeapId)>::deserialize(de),
    })
}

/// Follows the session and suspension variants to their single heap.
fn read_slots(bytes: &[u8]) -> Result<Vec<Option<Slot>>, DecodeError> {
    let (_, state) = member(bytes, &["state"])?;
    let (session, inner) = member(state, &["Idle", "Suspended", "Running"])?;
    let owner = if session == "Idle" {
        inner
    } else {
        let (progress, inner) = member(
            inner,
            &["FunctionCall", "OsCall", "NameLookup", "ResolveFutures", "Complete"],
        )?;
        if session == "Running" && progress == "Complete" {
            return Ok(Vec::new());
        }
        let snapshot = match progress {
            "Complete" | "ResolveFutures" => inner,
            _ => member(inner, &["snapshot"])?.1,
        };
        if session == "Suspended" {
            member(snapshot, &["repl"])?.1
        } else {
            snapshot
        }
    };
    let (_, heap) = member(owner, &["heap"])?;
    let (_, entries) = member(heap, &["entries"])?;
    minicbor_serde::from_slice(entries)
}

/// Finds a named map member, skipping unrelated values without allocating.
fn member<'a>(input: &'a [u8], names: &[&str]) -> Result<(&'a str, &'a [u8]), DecodeError> {
    let mut decoder = Deserializer::new(input).into_decoder();
    let count = decoder
        .map()?
        .ok_or_else(|| DecodeError::custom("expected a definite-length dump map"))?;
    for _ in 0..count {
        let key = decoder.str()?;
        if names.contains(&key) {
            return Ok((key, &input[decoder.position()..]));
        }
        decoder.skip()?;
    }
    Err(DecodeError::custom("missing field in legacy dump"))
}

/// A heap payload is an externally tagged enum. Its contents can include more refs.
fn deserialize_kind<'de, D: serde::Deserializer<'de>>(de: D) -> Result<HeapKind, D::Error> {
    struct KindVisitor;
    impl<'de> Visitor<'de> for KindVisitor {
        type Value = HeapKind;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a heap payload")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<HeapKind, A::Error> {
            let (kind, IgnoredAny) = map
                .next_entry()?
                .ok_or_else(|| A::Error::custom("empty heap payload"))?;
            Ok(kind)
        }
    }
    de.deserialize_map(KindVisitor)
}
