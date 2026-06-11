use monty::{ExcType, Type};
use strum::IntoEnumIterator;

/// `Type::from_type_name` must be the exact inverse of `Display` for every
/// variant — boundaries that serialize a `Type` by name (e.g. the subprocess
/// wire protocol) rely on this round-trip. A new variant whose `Display`
/// string is not added to `from_type_name` fails here.
#[test]
fn type_display_round_trips_through_from_type_name() {
    for t in Type::iter() {
        let name = t.to_string();
        assert_eq!(
            Type::from_type_name(&name),
            Some(t),
            "Type::from_type_name({name:?}) does not round-trip {t:?}"
        );
    }
}

/// Exception types render as their exception name and resolve back through
/// the `ExcType` fallback inside `from_type_name`.
#[test]
fn exception_type_names_round_trip() {
    for exc in [ExcType::ValueError, ExcType::JsonDecodeError, ExcType::Exception] {
        let t = Type::Exception(exc);
        assert_eq!(Type::from_type_name(&t.to_string()), Some(t));
    }
}
