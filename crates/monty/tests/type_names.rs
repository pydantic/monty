use monty::{ExcType, MontyType};
use strum::IntoEnumIterator;

/// `MontyType::from_type_name` must be the exact inverse of `Display`/`name()`
/// for every nameable variant — boundaries that serialize a type by name (e.g.
/// the subprocess wire protocol) rely on this round-trip. Rendering and
/// parsing share the internal `Type`'s strum attributes (`IntoStaticStr` /
/// `EnumString`), so this mainly guards the `MontyType` ↔ `Type` conversions
/// and the hand-written `Exception` fallback. `Instance` is
/// `#[strum(disabled)]`: it round-trips through a dedicated wire field, not
/// by name (see `instance_type_is_not_nameable`).
#[test]
fn type_name_round_trips_through_from_type_name() {
    for t in MontyType::iter() {
        let name = t.to_string();
        assert_eq!(
            MontyType::from_type_name(&name),
            Some(t.clone()),
            "MontyType::from_type_name({name:?}) does not round-trip {t:?}"
        );
    }
}

/// Exception types render as their exception name and resolve back through
/// the `ExcType` fallback inside `from_type_name`. The lowercase
/// `"exception"` must NOT parse: the internal `Exception` variant is
/// `#[strum(disabled)]` precisely so `EnumString` never accepts it.
#[test]
fn exception_type_names_round_trip() {
    for exc in [ExcType::ValueError, ExcType::JsonDecodeError, ExcType::Exception] {
        let t = MontyType::Exception(exc);
        assert_eq!(MontyType::from_type_name(&t.to_string()), Some(t));
    }
    assert_eq!(MontyType::from_type_name("exception"), None);
}

/// A sandbox-class type displays as its class name, but names never parse back
/// to `Instance`: a class binding cannot be reconstructed from a name, and
/// `"object"` (the historical generic rendering) is not a Monty type at all.
#[test]
fn instance_type_is_not_nameable() {
    let t = MontyType::Instance("Foo".to_owned());
    assert_eq!(t.to_string(), "Foo");
    assert_eq!(t.name(), "Foo");
    assert_eq!(MontyType::from_type_name("Foo"), None);
    assert_eq!(MontyType::from_type_name("object"), None);
}
