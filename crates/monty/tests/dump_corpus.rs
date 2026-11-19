//! Checks how this build treats every dump it may be asked to read, old and malformed alike.
//!
//! The fixtures under `golden_dumps/` are real bytes from the builds that wrote
//! them (see `golden_dumps/AGENTS.md`). Each case in `golden_dumps/corpus.json`
//! states the verdict it expects per version; the harness produces one and the two must agree.
//!
//! Whatever is refused, no dump may load into a session that disagrees with the
//! one dumped (`Verdict::Corrupt`): [`no_fixture_is_ever_corrupt`] is that contract.
//! These tests cover dump compatibility only, not interpreter behaviour.

mod common;

use std::collections::BTreeSet;

use common::dump_corpus::{
    Verdict, case_files, corpus, manifest, record_current, verify_restore, verify_restore_bytes,
};
use monty::{DUMP_VERSION, Dump};
use serde::{Deserialize, Serialize};

/// Offset of the little-endian version field, straight after the magic.
const VERSION_OFFSET: usize = 6;

/// Every fixture gets the verdict `corpus.json` predicts.
///
/// A failure is not automatically a bug: a migration that loads a previously
/// refused dump should update the corpus, and that diff records the change.
#[test]
fn every_fixture_gets_its_expected_verdict() {
    let corpus = corpus();
    let mut wrong = Vec::new();

    for version in &corpus.versions {
        let manifest = manifest(version.version);
        for case in &corpus.cases {
            let recorded = manifest
                .cases
                .get(&case.name)
                .unwrap_or_else(|| panic!("v{} has no fixture for {}", version.version, case.name));
            let verdict = verify_restore(version.version, case, recorded);
            let expected = case.verdict_for(version.version);
            if verdict.kind() != expected {
                wrong.push(format!(
                    "v{} {}: expected {expected}, got {verdict}",
                    version.version, case.name
                ));
            }
        }
    }

    assert!(
        wrong.is_empty(),
        "verdicts disagree with corpus.json:\n{}",
        wrong.join("\n")
    );
}

/// Refusing an old dump is acceptable; loading one into a session that has
/// quietly lost or renamed its globals is not.
#[test]
fn no_fixture_is_ever_corrupt() {
    let corpus = corpus();
    let mut corrupt = Vec::new();

    for version in &corpus.versions {
        let manifest = manifest(version.version);
        for case in &corpus.cases {
            let recorded = &manifest.cases[&case.name];
            if let Verdict::Corrupt(problems) = verify_restore(version.version, case, recorded) {
                corrupt.push(format!("v{} {}: {}", version.version, case.name, problems.join("; ")));
            }
        }
    }

    assert!(
        corrupt.is_empty(),
        "these dumps loaded into sessions that disagree with the ones dumped:\n{}",
        corrupt.join("\n")
    );
}

/// Guards the harness: if it could never report `Loads`, a suite that refused
/// everything would pass while measuring nothing.
#[test]
fn a_current_dump_gets_the_loads_verdict() {
    for case in &corpus().cases {
        let (bytes, recorded) = record_current(case);
        let verdict = verify_restore_bytes(&bytes, case, &recorded);
        assert_eq!(
            verdict,
            Verdict::Loads,
            "current-version {} verified as {verdict}",
            case.name
        );
    }
}

/// A dump is untrusted input: each structural mangling here must be refused without panicking.
///
/// Arbitrary byte flips are not tested — a flipped payload can be a valid dump
/// of something else, so there is no outcome to assert beyond not crashing.
#[test]
fn malformed_dumps_are_refused() {
    let corpus = corpus();
    let case = &corpus.cases[0];
    let (valid, _) = record_current(case);

    let mut mangled: Vec<(String, Vec<u8>)> = vec![
        ("empty".to_owned(), Vec::new()),
        ("header only".to_owned(), valid[..8].to_vec()),
        ("truncated header".to_owned(), valid[..3].to_vec()),
        ("half a payload".to_owned(), valid[..valid.len() / 2].to_vec()),
        ("payload with no header".to_owned(), valid[8..].to_vec()),
    ];

    let mut bad_magic = valid.clone();
    bad_magic[0] = b'X';
    mangled.push(("bad magic".to_owned(), bad_magic));

    let mut trailing = valid.clone();
    trailing.push(0);
    mangled.push(("trailing byte".to_owned(), trailing));

    // Version numbers nothing has ever written, in both directions.
    for version in [0u16, 1, DUMP_VERSION + 1, u16::MAX] {
        let mut relabelled = valid.clone();
        relabelled[VERSION_OFFSET..VERSION_OFFSET + 2].copy_from_slice(&version.to_le_bytes());
        mangled.push((format!("version {version}"), relabelled));
    }

    let accepted: Vec<String> = mangled
        .into_iter()
        .filter(|(_, bytes)| Dump::load(bytes).is_ok())
        .map(|(what, _)| what)
        .collect();
    assert!(accepted.is_empty(), "these malformed dumps were accepted: {accepted:?}");
}

/// Shows why a dump's layout must be known from its version, not found by trial decoding.
///
/// Postcard writes no field names, tags or lengths, so bytes read against a
/// layout with an extra field can still decode — into different values.
/// `Ok` is not evidence that the layout matched.
#[test]
fn a_shifted_decode_can_succeed_with_wrong_values() {
    #[derive(Serialize)]
    struct OldInner {
        a: u8,
        b: u8,
    }
    #[derive(Serialize)]
    struct Old {
        inner: OldInner,
        opt: Option<u8>,
        z: u8,
    }
    #[derive(Debug, Deserialize)]
    struct NewInner {
        a: u8,
        b: u8,
        added: u8,
    }
    #[derive(Debug, Deserialize)]
    struct New {
        inner: NewInner,
        opt: Option<u8>,
        z: u8,
    }

    let bytes = postcard::to_allocvec(&Old {
        inner: OldInner { a: 1, b: 2 },
        opt: Some(0),
        z: 9,
    })
    .expect("serializing succeeds");

    let decoded: New = postcard::from_bytes(&bytes).expect("the shifted layout still decodes");
    // `added` ate the `Some` tag, so `opt` lost its value and `z` realigned by
    // chance — a plausible-looking result from bytes that meant something else.
    assert_eq!((decoded.inner.a, decoded.inner.b), (1, 2));
    assert_eq!(decoded.inner.added, 1);
    assert_eq!(decoded.opt, None);
    assert_eq!(decoded.z, 9);
}

/// Every fixture must trace back to a commit and have a verdict in `corpus.json`,
/// or it is an unexplained blob that nothing tests.
#[test]
fn the_corpus_describes_itself() {
    let corpus = corpus();
    assert!(!corpus.versions.is_empty(), "the corpus has no versions");
    assert!(!corpus.cases.is_empty(), "the corpus has no cases");

    for version in &corpus.versions {
        let manifest = manifest(version.version);
        assert_eq!(
            manifest.commit, version.commit,
            "v{} was generated from {} but corpus.json records {} ({})",
            version.version, manifest.commit, version.commit, version.subject
        );
        // `<=` rather than `<`: a layout can change without a bump, as version 9's
        // did, leaving a fixture that shares this build's number.
        assert!(
            version.version <= DUMP_VERSION,
            "v{} is newer than what this build writes ({DUMP_VERSION})",
            version.version
        );
        for case in &corpus.cases {
            assert!(
                manifest.cases.contains_key(&case.name),
                "v{} has no fixture for {}; run `make generate-golden-dumps`",
                version.version,
                case.name
            );
            // Panics if neither this version nor `*` has a verdict, so a new
            // case must be ruled on before it can pass.
            case.verdict_for(version.version);
        }
    }
}

/// `corpus.json` and `cases/` must describe the same set of cases.
///
/// A case with no file cannot be dumped, and a file with no case is never
/// dumped yet looks like coverage; nothing else catches either.
#[test]
fn every_case_has_a_python_file() {
    let listed: BTreeSet<String> = corpus().cases.into_iter().map(|case| case.name).collect();
    let present = case_files();

    let missing: Vec<&String> = listed.difference(&present).collect();
    assert!(
        missing.is_empty(),
        "corpus.json lists cases with no cases/<name>.py: {missing:?}"
    );
    let stray: Vec<&String> = present.difference(&listed).collect();
    assert!(
        stray.is_empty(),
        "cases/ holds Python files no case in corpus.json dumps: {stray:?}"
    );
}
