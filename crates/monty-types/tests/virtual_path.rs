//! Shared virtual-path semantics and preservation of unvalidated OS-call paths.

use std::borrow::Cow;

use monty_types::{GetenvArgs, MontyObject, MontyPath, OsFunctionCall, PathStringDataArgs, normalize_virtual_path};

/// Normalization is POSIX on every host and never climbs above the virtual root.
#[test]
fn virtual_paths_normalize_lexically() {
    for (path, expected) in [
        ("", "/"),
        ("/", "/"),
        ("//", "/"),
        ("/data/main.py", "/data/main.py"),
        ("./data//sub/../main.py/", "/data/main.py"),
        ("/data/./sub/../main.py", "/data/main.py"),
        ("/../..", "/"),
        ("../../file", "/file"),
        ("/a/../../b", "/b"),
        ("/données/日本語/../file", "/données/file"),
        ("/a\\b", "/a\\b"),
    ] {
        let normalized = normalize_virtual_path(path);
        assert_eq!(normalized, expected, "{path:?}");
        assert!(matches!(normalize_virtual_path(&normalized), Cow::Borrowed(_)));
    }
}

/// Constructing OS-call arguments must retain components normalization would erase.
#[test]
fn monty_path_preserves_unvalidated_input() {
    for raw in [
        "/data/bad\0/../file".to_owned(),
        format!("/data/{}/../file", "a".repeat(256)),
    ] {
        let path = MontyPath::new(raw.clone());
        assert_eq!(path.as_str(), raw);
        assert_eq!(normalize_virtual_path(&path), "/data/file");
        assert_eq!(path.into_string(), raw);
    }
}

/// Callback projection rewrites filesystem paths without changing payloads or empty paths.
#[test]
fn callback_normalization_only_changes_filesystem_paths() {
    let raw = "/data/sub/../file";
    let call = OsFunctionCall::WriteText(PathStringDataArgs {
        path: MontyPath::new(raw.to_owned()),
        data: raw.to_owned(),
    });
    let (args, _) = call.clone().to_args();
    assert_eq!(
        args,
        vec![
            MontyObject::Path("/data/file".to_owned()),
            MontyObject::String(raw.to_owned())
        ]
    );
    assert_eq!(call.fs_primary_path(), Some(raw));

    let (args, _) = OsFunctionCall::Stat(MontyPath::new(String::new())).to_args();
    assert_eq!(args, vec![MontyObject::Path(String::new())]);

    let (args, _) = OsFunctionCall::Getenv(GetenvArgs {
        key: raw.to_owned(),
        default: MontyObject::Path(raw.to_owned()),
    })
    .to_args();
    assert_eq!(
        args,
        vec![MontyObject::String(raw.to_owned()), MontyObject::Path(raw.to_owned())]
    );
}
