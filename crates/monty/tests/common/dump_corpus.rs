//! The shared dump corpus harness.
//!
//! Approaches to reading old dumps salvage different dumps at different costs,
//! so they are only comparable when measured the same way. This harness knows
//! nothing about any approach: it calls [`Dump::load`] on bytes an old build
//! wrote and checks the restored session against what that build recorded.
//! An approach branch changes no code here, only the `verdicts` rows of
//! `golden_dumps/corpus.json`.
//!
//! Refusing an old dump is an acceptable outcome; restoring a session that
//! disagrees with the dumped one ([`Verdict::Corrupt`]) or panicking
//! ([`Verdict::Panicked`]) is not.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    panic::{self, AssertUnwindSafe},
    path::PathBuf,
};

use monty::{Dump, MontyRepl, Session, SessionRef, dump};
use monty_types::{CompileOptions, PrintWriter, ResourceTracker};
use serde::{Deserialize, de::DeserializeOwned};

/// Script name every fixture was dumped under, which a restored session must still report.
const SCRIPT_NAME: &str = "golden.py";

/// What this build's [`Dump::load`] did with one fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Loaded, and the restored session agrees with the one that was dumped.
    Loads,
    /// Declined to load, with the error message. Conservative, but safe.
    Refused(String),
    /// Loaded into a session that disagrees with the dumped one. Never allowed.
    Corrupt(Vec<String>),
    /// The load panicked. A dump is untrusted input, so this is a failure to
    /// reject, not a kind of rejection.
    Panicked(String),
}

impl Verdict {
    /// The verdict without its detail, as spelled in `corpus.json`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Loads => "loads",
            Self::Refused(_) => "refused",
            Self::Corrupt(_) => "corrupt",
            Self::Panicked(_) => "panicked",
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Loads => f.write_str("loads"),
            Self::Refused(why) => write!(f, "refused ({why})"),
            Self::Corrupt(problems) => write!(f, "corrupt: {}", problems.join("; ")),
            Self::Panicked(why) => write!(f, "panicked ({why})"),
        }
    }
}

/// Loads one checked-in fixture and checks the restored session against what
/// the writing build recorded.
///
/// A payload can decode cleanly into the wrong values, so the checks feed code
/// (`repr(name)`) to the session rather than inspect the decoded structure.
pub fn verify_restore(version: u16, case: &Case, recorded: &RecordedCase) -> Verdict {
    let bytes = fixture_bytes(version, &recorded.file);
    verify_restore_bytes(&bytes, case, recorded)
}

/// [`verify_restore`] against caller-supplied bytes, e.g. a dump from [`record_current`].
///
/// Runs under [`catch_unwind`](panic::catch_unwind) so a panicking load becomes
/// [`Verdict::Panicked`] rather than taking the suite down.
pub fn verify_restore_bytes(bytes: &[u8], case: &Case, recorded: &RecordedCase) -> Verdict {
    catching(|| verify_restore_inner(bytes, case, recorded))
}

/// Runs `body` with panics caught and the panic hook silenced.
///
/// A panicking load is a measured outcome, so its backtrace is noise; the
/// message is recovered from the payload instead. The hook is process-global,
/// so a concurrent test's panic output is swallowed too.
fn catching(body: impl FnOnce() -> Verdict) -> Verdict {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let result = panic::catch_unwind(AssertUnwindSafe(body));
    panic::set_hook(previous);

    result.unwrap_or_else(|payload| {
        let why = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_owned());
        Verdict::Panicked(why)
    })
}

fn verify_restore_inner(bytes: &[u8], case: &Case, recorded: &RecordedCase) -> Verdict {
    let loaded = match Dump::load(bytes) {
        Ok(loaded) => loaded,
        Err(err) => return Verdict::Refused(err.to_string()),
    };

    let mut problems = Vec::new();
    if loaded.script_name != SCRIPT_NAME {
        problems.push(format!(
            "script name is {:?}, was dumped as {SCRIPT_NAME:?}",
            loaded.script_name
        ));
    }
    let Session::Idle(mut repl) = loaded.state else {
        problems.push("restored a session that is not idle".to_owned());
        return Verdict::Corrupt(problems);
    };

    // In corpus order, so a failure names the first global that moved.
    for name in &case.globals {
        let Some(want) = recorded.expected.get(name) else {
            problems.push(format!("{name} has no recorded value"));
            continue;
        };
        match repr_of(&mut repl, name) {
            Ok(got) if &got == want => {}
            Ok(got) => problems.push(format!("{name} is {got}, was dumped as {want}")),
            Err(err) => problems.push(format!("{name} is gone: {err}")),
        }
    }

    // The session must be usable, not merely readable.
    match repr_of(&mut repl, &format!("({})", case.feed)) {
        Ok(got) if got == recorded.feed_result => {}
        Ok(got) => problems.push(format!(
            "feeding {:?} gave {got}, was {} when dumped",
            case.feed, recorded.feed_result
        )),
        Err(err) => problems.push(format!("feeding {:?} failed: {err}", case.feed)),
    }

    // Otherwise the session can be read once and never stored again.
    match dump(SCRIPT_NAME, None, SessionRef::Idle(&repl)) {
        Ok(redumped) => match Dump::load(&redumped) {
            Ok(_) => {}
            Err(err) => problems.push(format!("re-dumped session does not reload: {err}")),
        },
        Err(err) => problems.push(format!("restored session cannot be dumped again: {err}")),
    }

    if problems.is_empty() {
        Verdict::Loads
    } else {
        Verdict::Corrupt(problems)
    }
}

/// Evaluates `repr(expr)` in `repl`, as the generator records values.
///
/// The sandbox's `repr` is stable across builds where `MontyObject`'s shape is
/// not. Any exception is flattened to a string for reporting.
fn repr_of(repl: &mut MontyRepl, expr: &str) -> Result<String, String> {
    repl.feed_run(&format!("repr({expr})"), vec![], PrintWriter::Stdout)
        .map(|value| value.to_string())
        .map_err(|err| format!("{:?}: {}", err.exc_type(), err.message().unwrap_or("no message")))
}

/// Runs one case in a fresh session and dumps it at the current version,
/// recording it as the generator records a fixture.
///
/// Gives the harness a dump that must load, so a harness that refuses
/// everything cannot pass by matching `refused` verdicts.
pub fn record_current(case: &Case) -> (Vec<u8>, RecordedCase) {
    let mut repl = MontyRepl::new(SCRIPT_NAME, ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(&case_source(&case.name), vec![], PrintWriter::Stdout)
        .unwrap_or_else(|err| panic!("case {} does not run on this build: {err:?}", case.name));

    let expected = case
        .globals
        .iter()
        .map(|name| {
            let value =
                repr_of(&mut repl, name).unwrap_or_else(|err| panic!("case {}: repr({name}) failed: {err}", case.name));
            (name.clone(), value)
        })
        .collect();
    let feed_result = repr_of(&mut repl, &format!("({})", case.feed))
        .unwrap_or_else(|err| panic!("case {} feed failed: {err}", case.name));

    let bytes = dump(SCRIPT_NAME, None, SessionRef::Idle(&repl)).expect("dumping a fresh session succeeds");
    (
        bytes,
        RecordedCase {
            file: format!("{}.dump", case.name),
            expected,
            feed_result,
        },
    )
}

/// Reads a checked-in fixture from `golden_dumps/v<version>/`, panicking if it is missing.
pub fn fixture_bytes(version: u16, file: &str) -> Vec<u8> {
    let path = golden_dumps().join(format!("v{version}")).join(file);
    fs::read(&path).unwrap_or_else(|_| panic!("missing fixture {}", path.display()))
}

// --- the checked-in data ----------------------------------------------------

/// The hand-maintained `corpus.json`.
#[derive(Debug, Deserialize)]
pub struct Corpus {
    /// The builds whose dumps are kept, oldest first.
    pub versions: Vec<CorpusVersion>,
    /// The sessions dumped at every one of those versions.
    pub cases: Vec<Case>,
}

/// One dump version and the commit that wrote its fixtures.
#[derive(Debug, Deserialize)]
pub struct CorpusVersion {
    pub version: u16,
    pub commit: String,
    pub subject: String,
}

/// One session to dump, and what this build should make of it.
///
/// Its source is `cases/<name>.py`, read with [`case_source`].
#[derive(Debug, Deserialize)]
pub struct Case {
    pub name: String,
    /// Globals to read back, in reporting order.
    pub globals: Vec<String>,
    /// Extra code run against the restored session, to prove it is usable.
    pub feed: String,
    /// Expected verdict (`loads` or `refused`) per dump version; `*` covers the rest.
    pub verdicts: BTreeMap<String, String>,
}

impl Case {
    /// The expected verdict for this case's fixture at `version`.
    ///
    /// Panics rather than defaulting: an unruled version is one nobody has considered.
    pub fn verdict_for(&self, version: u16) -> &str {
        self.verdicts
            .get(&version.to_string())
            .or_else(|| self.verdicts.get("*"))
            .unwrap_or_else(|| panic!("corpus.json {} rules on neither v{version} nor *", self.name))
            .as_str()
    }
}

/// What a writing build recorded, generated into `v<N>/manifest.json`.
///
/// The ground truth: taken by the build that dumped the session, so it does not
/// depend on any later build agreeing.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub dump_version: u16,
    pub commit: String,
    pub cases: BTreeMap<String, RecordedCase>,
}

/// One case as the writing build saw it.
#[derive(Debug, Deserialize)]
pub struct RecordedCase {
    pub file: String,
    pub expected: BTreeMap<String, String>,
    pub feed_result: String,
}

/// Reads `corpus.json`, panicking on any verdict other than `loads` or `refused`.
///
/// `corrupt` and `panicked` are what the suite exists to rule out, so no
/// approach may declare them acceptable.
pub fn corpus() -> Corpus {
    let corpus: Corpus = read_json("corpus.json");
    for case in &corpus.cases {
        for (version, verdict) in &case.verdicts {
            assert!(
                matches!(verdict.as_str(), "loads" | "refused"),
                "corpus.json {} v{version} is {verdict:?}; a fixture may only be expected to load or be refused",
                case.name
            );
        }
    }
    corpus
}

/// Reads one case's Python source.
pub fn case_source(name: &str) -> String {
    let path = golden_dumps().join("cases").join(format!("{name}.py"));
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing case source {}", path.display()))
}

/// Every Python file under `cases/`, by case name.
///
/// Lets the corpus check fail on a stray file too, since nothing would dump it.
pub fn case_files() -> BTreeSet<String> {
    let dir = golden_dumps().join("cases");
    fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("missing {}", dir.display()))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            Some(name.strip_suffix(".py")?.to_owned())
        })
        .collect()
}

/// Reads one version's generated manifest, checking it is the version claimed.
pub fn manifest(version: u16) -> Manifest {
    let manifest: Manifest = read_json(&format!("v{version}/manifest.json"));
    assert_eq!(
        manifest.dump_version, version,
        "v{version}/manifest.json was written by a build at version {}",
        manifest.dump_version
    );
    manifest
}

fn read_json<T: DeserializeOwned>(relative: &str) -> T {
    let path = golden_dumps().join(relative);
    let text = fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("{} does not parse: {err}", path.display()))
}

fn golden_dumps() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden_dumps")
}
