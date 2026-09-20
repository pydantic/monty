use monty_types::{ExcType, MontyType};
use strum::IntoEnumIterator;

/// `MontyType::from_type_name` must be the exact inverse of `Display`/`name()`
/// for every nameable variant — boundaries that serialize a type by name (e.g.
/// the subprocess wire protocol) rely on this round-trip. Rendering and
/// parsing share the internal `Type`'s strum attributes (`IntoStaticStr` /
/// `EnumString`), so this mainly guards the `MontyType` ↔ `Type` conversions
/// and the hand-written `Exception` fallback. Classes are not `MontyType`s at
/// all: they cross as `ClassType` arena nodes (see
/// `class_names_do_not_parse_as_types`).
#[test]
fn type_name_round_trips_through_from_type_name() {
    for t in MontyType::iter() {
        let name = t.to_string();
        assert_eq!(
            MontyType::from_type_name(&name),
            Some(t),
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

/// A class name never parses to a `MontyType`: a class binding cannot be
/// reconstructed from a name, so a class crosses the boundary as a `ClassType`
/// node instead (its display as the class name is covered in `repl.rs`).
/// `"object"` does parse, to the builtin of that name — never to a class,
/// which is the confusion this guards against.
#[test]
fn class_names_do_not_parse_as_types() {
    assert_eq!(MontyType::from_type_name("Foo"), None);
    assert_eq!(MontyType::from_type_name("object"), Some(MontyType::Object));
}
