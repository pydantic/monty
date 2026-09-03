//! Tests for stateful REPL execution with no replay.
//!
//! The REPL session keeps heap/global namespace state between snippets and executes
//! only the newly fed snippet each time.

use std::fmt::Write;

use insta::assert_snapshot;
#[cfg(feature = "test-hooks")]
use monty::FunctionMetadataFault;
use monty::{
    DUMP_VERSION, Dump, DumpError, MontyRepl, ReplContinuationMode, ReplProgress, ReplStartError, Session, SessionRef,
    detect_repl_continuation_mode, dump,
};
use monty_types::{
    CompileOptions, DictPairs, ExcType, ExtFunctionResult, MontyClassInstance, MontyClassType, MontyException,
    MontyObject, MontyType, MontyUuid, NameLookupResult, PrintWriter, ResourceLimits, ResourceTracker,
};

#[test]
fn repl_executes_only_new_code() {
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let init_output = feed_run_print(&mut repl, "counter = 0").unwrap();
    assert_eq!(init_output, MontyObject::None);

    // Execute a snippet that mutates state.
    let output = feed_run_print(&mut repl, "counter = counter + 1").unwrap();
    assert_eq!(output, MontyObject::None);

    // Feed only the read expression. If replay happened, we'd get 2 instead of 1.
    let output = feed_run_print(&mut repl, "counter").unwrap();
    assert_eq!(output, MontyObject::Int(1));
}

fn feed_run_print(repl: &mut MontyRepl, code: &str) -> Result<MontyObject, MontyException> {
    repl.feed_run(code, vec![], PrintWriter::Stdout)
}

fn init_repl(code: &str) -> (MontyRepl, MontyObject) {
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let output = feed_run_print(&mut repl, code).unwrap();
    (repl, output)
}

/// Round-trips an idle session through the dump format, asserting it comes back
/// on the same [`Session`] arm it went out on.
fn round_trip_repl(repl: &MontyRepl) -> MontyRepl {
    let bytes = dump("repl.py", None, SessionRef::Idle(repl)).unwrap();
    match Dump::load(&bytes).unwrap().state {
        Session::Idle(repl) => *repl,
        _ => panic!("dumped an idle session, loaded something else"),
    }
}

/// Round-trips a suspended session through the dump format.
fn round_trip_progress(progress: &ReplProgress) -> ReplProgress {
    let bytes = dump("repl.py", None, SessionRef::Suspended(progress)).unwrap();
    match Dump::load(&bytes).unwrap().state {
        Session::Suspended(progress) => *progress,
        _ => panic!("dumped a suspended session, loaded something else"),
    }
}

/// The header must reject anything this build cannot read, and each rejection
/// must say which of the three it was — a stale snapshot needs rebuilding, a
/// corrupt one needs investigating.
#[test]
fn dump_header_rejects_incompatible_data() {
    let repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let bytes = dump("repl.py", None, SessionRef::Idle(&repl)).unwrap();
    // pins the header layout (magic then little-endian version), not the version itself
    let mut expected_header = b"MONTY\0".to_vec();
    expected_header.extend_from_slice(&DUMP_VERSION.to_le_bytes());
    assert_eq!(&bytes[..8], expected_header.as_slice());

    // too short to even hold a header
    assert_eq!(Dump::load(&bytes[..3]).unwrap_err(), DumpError::NotADump);

    let mut wrong_magic = bytes.clone();
    wrong_magic[0] = b'X';
    assert_eq!(Dump::load(&wrong_magic).unwrap_err(), DumpError::NotADump);

    let mut wrong_version = bytes.clone();
    wrong_version[6] = 1;
    assert_eq!(
        Dump::load(&wrong_version).unwrap_err(),
        DumpError::VersionMismatch {
            found: 1,
            expected: DUMP_VERSION
        }
    );

    // trailing bytes are rejected rather than ignored, so a padded dump cannot
    // decode as the shorter valid one it starts with
    let mut trailing_data = bytes;
    trailing_data.push(0);
    assert_eq!(
        Dump::load(&trailing_data).unwrap_err(),
        DumpError::Payload(postcard::Error::DeserializeBadEncoding)
    );
}

/// A dump is untrusted input, and the heap it carries is installed verbatim. A
/// `time` entry that no constructor could have produced must be rejected at load
/// rather than panicking, or contradicting itself, later — when the ranges and the
/// `tzinfo` reference are read back as established facts.
///
/// A `time` stores only a reference to its zone, so a disagreement between an
/// attached offset and the object it points at is not representable. What is left
/// is a component out of range, a reference that is not a timezone, and an offset
/// out of range on the timezone itself.
#[test]
fn dump_rejects_forged_time_entries() {
    // Distinctive components so the encoded `time` can be found in the payload:
    // three single-byte fields, then 444555 as a postcard varint.
    const COMPONENTS: [u8; 6] = [11, 22, 33, 0x8B, 0x91, 0x1B];

    let naive = dump_repl("import datetime\nt = datetime.time(11, 22, 33, 444555)");
    // ... followed by fold and a `None` tzinfo.
    let hour = offset_of(&naive, &[COMPONENTS.as_slice(), &[0, 0]].concat());
    assert!(Dump::load(&naive).is_ok());

    let mut forged = naive;
    forged[hour] = 255;
    assert_eq!(
        Dump::load(&forged).unwrap_err(),
        DumpError::Payload(postcard::Error::SerdeDeCustom)
    );

    // A *named* offset keeps a timezone entry of its own instead of canonicalizing
    // onto the `timezone.utc` singleton, and 23 hours encodes as a three-byte
    // varint, leaving room to forge a value outside the range `timezone()` accepts.
    let aware = dump_repl(
        "import datetime\ntz = datetime.timezone(datetime.timedelta(hours=23), 'AB')\nt = datetime.time(11, 22, 33, 444555, tzinfo=tz)",
    );
    assert!(Dump::load(&aware).is_ok());

    // ... followed by fold and `Some(_)`, so the heap id ends the marker.
    let tzinfo_ref = offset_of(&aware, &[COMPONENTS.as_slice(), &[0, 1]].concat()) + 8;
    // The timezone entry: 82800 seconds zigzag-encoded, then `Some("AB")`.
    let tz_offset = offset_of(&aware, &[0xE0, 0x8D, 0x0A, 1, 2, b'A', b'B']);

    for (index, byte, what) in [
        // The empty-tuple singleton: a live entry, but not a timezone.
        (
            tzinfo_ref,
            0,
            "a `tzinfo` reference to something that is not a timezone",
        ),
        (tzinfo_ref, 100, "a `tzinfo` reference to no entry at all"),
        // `format_offset_hms` negates the offset, which panics on `i32::MIN`.
        (tz_offset + 2, 0x7f, "a `tzinfo` object whose offset is out of range"),
    ] {
        let mut forged = aware.clone();
        forged[index] = byte;
        assert_eq!(
            Dump::load(&forged).unwrap_err(),
            DumpError::Payload(postcard::Error::SerdeDeCustom),
            "a time with {what} must be rejected"
        );
    }
}

/// The `timezone_utc` cache is a raw heap id restored verbatim, and
/// `get_timezone_utc` hands its target back as `datetime.timezone.utc` after an
/// `inc_ref` that panics on a freed or out-of-range id. A forged cache must be
/// rejected at load, whether it points at nothing, at a live non-timezone, or at
/// a timezone that is not UTC.
#[test]
fn dump_rejects_forged_timezone_utc_cache() {
    let bytes = dump_repl(
        "import datetime\nutc = datetime.timezone.utc\nplus2 = datetime.timezone(datetime.timedelta(hours=2))",
    );
    assert!(Dump::load(&bytes).is_ok());

    // `timezone_utc` is the heap's last serialized field and `globals` is the
    // session's, so the cached id sits a fixed distance from the end: `Some(2)`
    // followed by the three globals, one of which is the `+02:00` timezone at 4.
    let cached_id = bytes.len() - 8;
    assert_eq!(
        &bytes[cached_id - 1..=cached_id],
        &[1, 2],
        "timezone_utc is Some(HeapId(2))"
    );

    for (forged_id, what) in [
        (100, "no entry at all"),
        (0, "the empty-tuple singleton"),
        (4, "the +02:00 timezone"),
    ] {
        let mut forged = bytes.clone();
        forged[cached_id] = forged_id;
        assert_eq!(
            Dump::load(&forged).unwrap_err(),
            DumpError::Payload(postcard::Error::SerdeDeCustom),
            "a timezone.utc cache pointing at {what} must be rejected"
        );
    }
}

/// Dumps an idle session after running `code`.
fn dump_repl(code: &str) -> Vec<u8> {
    let (repl, _) = init_repl(code);
    dump("repl.py", None, SessionRef::Idle(&repl)).unwrap()
}

/// The offset of the one occurrence of `marker` in `bytes`, so a forged dump can
/// be built by patching a known field rather than by rebuilding the payload.
fn offset_of(bytes: &[u8], marker: &[u8]) -> usize {
    let mut found = bytes
        .windows(marker.len())
        .enumerate()
        .filter_map(|(index, window)| (window == marker).then_some(index));
    let offset = found.next().expect("marker not found in dump");
    assert_eq!(found.next(), None, "marker is not unique in dump");
    offset
}

#[test]
fn repl_persists_state_and_definitions() {
    let (mut repl, _) = init_repl("x = 10");

    feed_run_print(&mut repl, "def add(v):\n    return x + v").unwrap();
    feed_run_print(&mut repl, "x = 20").unwrap();
    let output = feed_run_print(&mut repl, "add(22)").unwrap();
    assert_eq!(output, MontyObject::Int(42));
}

#[test]
fn repl_function_redefinition_uses_latest_definition() {
    let (mut repl, init_output) = init_repl("");
    assert_eq!(init_output, MontyObject::None);

    feed_run_print(&mut repl, "def f():\n    return 1").unwrap();
    assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(1));

    feed_run_print(&mut repl, "def f():\n    return 2").unwrap();
    assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(2));
}

#[test]
fn repl_nested_function_redefinition_updates_callers() {
    let (mut repl, init_output) = init_repl("");
    assert_eq!(init_output, MontyObject::None);

    feed_run_print(&mut repl, "def g():\n    return 10").unwrap();
    feed_run_print(&mut repl, "def f():\n    return g() + 1").unwrap();
    assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(11));

    feed_run_print(&mut repl, "def g():\n    return 41").unwrap();
    assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(42));
}

/// A later snippet's `def` for a builtin name must shadow that builtin for
/// future calls of an earlier-defined function that references the name.
#[test]
fn repl_function_late_binds_user_def_over_builtin() {
    let (mut repl, _) = init_repl("");
    feed_run_print(&mut repl, "def call_sum():\n    return sum([1, 2, 3])").unwrap();
    assert_eq!(
        feed_run_print(&mut repl, "call_sum()").unwrap(),
        MontyObject::Int(6),
        "first call resolves via the builtin sum() fallback",
    );

    feed_run_print(&mut repl, "def sum(*args):\n    return 42").unwrap();
    assert_eq!(
        feed_run_print(&mut repl, "call_sum()").unwrap(),
        MontyObject::Int(42),
        "after `def sum`, the previously-compiled call_sum picks up the new module binding",
    );
}

/// Similar to `repl_function_late_binds_user_def_over_builtin`, but for
/// global variables directly.
#[test]
fn repl_module_scope_binds_user_def_over_builtin() {
    let (mut repl, _) = init_repl("");
    assert_eq!(
        feed_run_print(&mut repl, "max(1, 2)").unwrap(),
        MontyObject::Int(2),
        "snippet 1: builtin max wins because nothing else is bound",
    );

    feed_run_print(&mut repl, "def max(*args):\n    return 'shadowed'").unwrap();
    assert_eq!(
        feed_run_print(&mut repl, "max(1, 2)").unwrap(),
        MontyObject::String("shadowed".to_owned()),
        "snippet 3: module-level call sees the user-defined max bound in snippet 2",
    );
}

#[test]
fn repl_runtime_error_keeps_partial_state_consistent() {
    let (mut repl, init_output) = init_repl("");
    assert_eq!(init_output, MontyObject::None);

    let result = feed_run_print(&mut repl, "def f():\n    return 41\nx = 1\nraise RuntimeError('boom')");
    assert!(result.is_err(), "snippet should raise RuntimeError");

    // Definitions and assignments that happened before the exception should remain valid.
    assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(41));
    assert_eq!(feed_run_print(&mut repl, "x").unwrap(), MontyObject::Int(1));
}

#[test]
fn repl_heap_mutations_are_not_replayed() {
    let (mut repl, _) = init_repl("items = []");

    feed_run_print(&mut repl, "items.append(1)").unwrap();
    assert_eq!(
        feed_run_print(&mut repl, "items").unwrap(),
        MontyObject::List(vec![MontyObject::Int(1)])
    );

    feed_run_print(&mut repl, "items.append(2)").unwrap();
    assert_eq!(
        feed_run_print(&mut repl, "items").unwrap(),
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])
    );
}

#[test]
fn repl_detects_continuation_mode_for_common_cases() {
    assert_eq!(
        detect_repl_continuation_mode("value = 1\n"),
        ReplContinuationMode::Complete
    );
    assert_eq!(
        detect_repl_continuation_mode("if True:\n"),
        ReplContinuationMode::IncompleteBlock
    );
    assert_eq!(
        detect_repl_continuation_mode("[1,\n"),
        ReplContinuationMode::IncompleteImplicit
    );
    for source in [
        "value = '''first line\n",
        "value = \"\"\"first line\n",
        "value = r\"\"\"first line\n",
        "value = b\"\"\"first line\n",
        "value = f\"\"\"first line\n",
        "value = t\"\"\"first line\n",
    ] {
        assert_eq!(
            detect_repl_continuation_mode(source),
            ReplContinuationMode::IncompleteImplicit,
            "source: {source:?}",
        );
    }
    for source in ["value = 'first line\n", "value = \"first line\n"] {
        assert_eq!(
            detect_repl_continuation_mode(source),
            ReplContinuationMode::Complete,
            "source: {source:?}",
        );
    }
    assert_eq!(
        detect_repl_continuation_mode("value = \"\"\"first line\nsecond line\"\"\"\n"),
        ReplContinuationMode::Complete
    );
    assert_eq!(
        detect_repl_continuation_mode("@decorator\n"),
        ReplContinuationMode::IncompleteImplicit
    );
    assert_eq!(
        detect_repl_continuation_mode("@first\n@second\n"),
        ReplContinuationMode::IncompleteImplicit
    );
    assert_eq!(
        detect_repl_continuation_mode("@decorator\nvalue = 1"),
        ReplContinuationMode::Complete
    );
    assert_eq!(
        detect_repl_continuation_mode("@decorator\nvalue = 1\n"),
        ReplContinuationMode::Complete
    );
    assert_eq!(
        detect_repl_continuation_mode("@decorator\nclass SearchResult:\n"),
        ReplContinuationMode::IncompleteBlock
    );
    assert_eq!(
        detect_repl_continuation_mode("@decorator\ndef search():\n"),
        ReplContinuationMode::IncompleteBlock
    );
    assert_eq!(
        detect_repl_continuation_mode("@decorator\nasync def search():\n"),
        ReplContinuationMode::IncompleteBlock
    );
    assert_eq!(detect_repl_continuation_mode("@\n"), ReplContinuationMode::Complete);
}

#[test]
fn repl_tracebacks_use_incrementing_python_input_filenames() {
    let (mut repl, init_output) = init_repl("");
    assert_eq!(init_output, MontyObject::None);

    let first = feed_run_print(&mut repl, "missing_name").unwrap_err();
    let second = feed_run_print(&mut repl, "missing_name").unwrap_err();

    assert_eq!(first.traceback().len(), 1);
    assert_eq!(second.traceback().len(), 1);
    assert_eq!(first.traceback()[0].filename, "<python-input-0>");
    assert_eq!(second.traceback()[0].filename, "<python-input-1>");
}

#[test]
fn repl_cross_snippet_traceback_resolves_against_defining_source() {
    // Tracebacks for a function defined in snippet 0 and called in snippet 1
    // must resolve frame positions against the source of the snippet that
    // actually produced the `CodeRange`, not the source of the snippet that
    // triggered the exception. `CodeRange` stores raw byte offsets, so
    // indexing snippet 0's offsets into snippet 1's source would give wrong
    // line/column/preview-line data (or worse).
    let (mut repl, _) = init_repl("");

    feed_run_print(&mut repl, "def f():\n    raise ValueError('boom')").unwrap();
    let err = feed_run_print(&mut repl, "f()").unwrap_err();

    let tb = err.traceback();
    assert_eq!(tb.len(), 2, "expected call-site + raise-site frames");

    // Frame 0: the call site, snippet 1.
    assert_eq!(tb[0].filename, "<python-input-1>");
    assert_eq!(tb[0].start.line, 1);
    assert_eq!(tb[0].preview_line.as_deref(), Some("f()"));

    // Frame 1: the raise inside f(), defined in snippet 0.
    assert_eq!(tb[1].filename, "<python-input-0>");
    assert_eq!(tb[1].start.line, 2);
    assert_eq!(
        tb[1].preview_line.as_deref(),
        Some("    raise ValueError('boom')"),
        "preview line must come from the snippet that defined f, not the current snippet"
    );
}

#[test]
fn repl_dump_load_survives_between_snippets() {
    let (mut repl, _) = init_repl("total = 1");
    feed_run_print(&mut repl, "total = total + 1").unwrap();

    let mut loaded = round_trip_repl(&repl);

    feed_run_print(&mut loaded, "total = total * 21").unwrap();
    let output = feed_run_print(&mut loaded, "total").unwrap();
    assert_eq!(output, MontyObject::Int(42));
}

#[test]
fn repl_dump_load_derives_exact_positional_call_plans() {
    let (repl, _) = init_repl("def add(a, b):\n    return a + b\n\nasync def async_add(a, b):\n    return a + b");
    let mut loaded = round_trip_repl(&repl);

    assert_eq!(
        feed_run_print(&mut loaded, "add(20, 22)").unwrap(),
        MontyObject::Int(42)
    );
    assert_eq!(
        feed_run_print(&mut loaded, "await async_add(20, 22)").unwrap(),
        MontyObject::Int(42)
    );

    // The fast path's arg-count guard must also survive the round trip: a
    // mismatched call has to fall back to the general binder (and its error),
    // not silently misfire the cached plan.
    let err = feed_run_print(&mut loaded, "add(1)").unwrap_err();
    assert_eq!(err.message(), Some("add() missing 1 required positional argument: 'b'"));
}

#[cfg(feature = "test-hooks")]
#[test]
fn repl_dump_load_rejects_invalid_function_metadata() {
    /// Checks forged function metadata is rejected at dump load.
    fn assert_rejected(function: &str, fault: FunctionMetadataFault) {
        let code = r"
def variadic(*args, **kwargs):
    return args, kwargs

def pos_defaults(value=1, /):
    return value

def defaults(value=1):
    return value

def kw_defaults(*, first=1, second=2):
    return first, second

def outer(first, second):
    def middle():
        local = 1
        def inner():
            return first + second + local
        return inner
    return middle
";
        let (mut repl, _) = init_repl(code);
        repl.__corrupt_function_metadata_for_tests(function, fault);
        let bytes = dump("repl.py", None, SessionRef::Idle(&repl)).unwrap();
        assert_eq!(
            Dump::load(&bytes).unwrap_err(),
            DumpError::Payload(postcard::Error::SerdeDeCustom)
        );
    }

    assert_rejected("variadic", FunctionMetadataFault::SignatureSlotsBeyondNamespace);
    assert_rejected("variadic", FunctionMetadataFault::NamespaceTooLarge);
    assert_rejected("inner", FunctionMetadataFault::FreeVarLengthMismatch);
    assert_rejected("outer", FunctionMetadataFault::CellVarLengthMismatch);
    assert_rejected("inner", FunctionMetadataFault::FreeVarSlotOutOfRange);
    assert_rejected("outer", FunctionMetadataFault::CellVarSlotOutOfRange);
    assert_rejected("outer", FunctionMetadataFault::CellParamIndexOutOfRange);
    assert_rejected("pos_defaults", FunctionMetadataFault::PosDefaultsCountOutOfRange);
    assert_rejected("defaults", FunctionMetadataFault::ArgDefaultsCountOutOfRange);
    assert_rejected("kw_defaults", FunctionMetadataFault::KwargDefaultMapLengthMismatch);
    assert_rejected("kw_defaults", FunctionMetadataFault::KwargDefaultIndexGap);
    assert_rejected("defaults", FunctionMetadataFault::DefaultsCountMismatch);
    assert_rejected("inner", FunctionMetadataFault::DuplicateFreeVarSlot);
    assert_rejected("middle", FunctionMetadataFault::CellFreeVarSlotOverlap);
}

#[test]
fn repl_dump_load_preserves_heap_aliasing() {
    let (mut repl, _) = init_repl("a = []\nb = a");

    feed_run_print(&mut repl, "a.append(1)").unwrap();

    let mut loaded = round_trip_repl(&repl);

    feed_run_print(&mut loaded, "b.append(2)").unwrap();
    assert_eq!(
        feed_run_print(&mut loaded, "a").unwrap(),
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])
    );
    assert_eq!(
        feed_run_print(&mut loaded, "b").unwrap(),
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])
    );
}

#[test]
fn repl_start_external_call_resumes_to_updated_repl() {
    let (repl, init_output) = init_repl("");
    assert_eq!(init_output, MontyObject::None);

    // With LoadGlobalCallable, function calls go directly to FunctionCall
    let progress = repl.feed_start("ext_fn(41) + 1", vec![], PrintWriter::Stdout).unwrap();
    let call = progress.into_function_call().expect("expected function call");
    assert_eq!(call.function_name, "ext_fn");
    assert_eq!(call.args, vec![MontyObject::Int(41)]);

    let progress = call.resume(MontyObject::Int(41), PrintWriter::Stdout).unwrap();
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::Int(42));
    assert_eq!(feed_run_print(&mut repl, "x = 5").unwrap(), MontyObject::None);
    assert_eq!(feed_run_print(&mut repl, "x").unwrap(), MontyObject::Int(5));
}

#[test]
fn repl_feed_start_restores_comprehension_slots_before_next_turn() {
    let (repl, _) = init_repl("");

    let progress = repl
        .feed_start(
            "items = [i for i in [1]]\nitems = [i for i in [2]]\n",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
    let (repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::None);

    let progress = repl.feed_start("foo()", vec![], PrintWriter::Stdout).unwrap();
    let call = progress.into_function_call().expect("expected function call");
    assert_eq!(call.function_name, "foo");
    assert!(call.args.is_empty());
    let _repl = call.into_repl();
}

#[test]
fn repl_feed_start_restores_comprehension_slots_after_runtime_error() {
    let (repl, _) = init_repl("");

    let err = repl
        .feed_start("items = [1 / i for i in [0]]", vec![], PrintWriter::Stdout)
        .expect_err("expected runtime error");

    let progress = err.repl.feed_start("foo()", vec![], PrintWriter::Stdout).unwrap();
    let call = progress.into_function_call().expect("expected function call");
    assert_eq!(call.function_name, "foo");
    assert!(call.args.is_empty());
    let _repl = call.into_repl();
}

/// A snippet that rebinds existing globals to a fresh literal and function
/// before suspending, then gets abandoned, must leave those globals usable:
/// the ids they now hold were appended by the abandoned snippet.
#[test]
fn repl_abandoned_snippet_keeps_rebound_globals_usable() {
    const REBIND: &str = "x = 'rebound literal'\ndef f():\n    return 2\next_fn()";
    let check = |mut repl: MontyRepl| {
        assert_eq!(
            feed_run_print(&mut repl, "x").unwrap(),
            MontyObject::String("rebound literal".to_owned())
        );
        assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(2));
        // A new definition must not collide with the abandoned snippet's ids.
        feed_run_print(&mut repl, "def g():\n    return f() + 1").unwrap();
        assert_eq!(feed_run_print(&mut repl, "g()").unwrap(), MontyObject::Int(3));
    };

    let (repl, _) = init_repl("x = 'old'\ndef f():\n    return 1");
    let progress = repl.feed_start(REBIND, vec![], PrintWriter::Stdout).unwrap();
    check(
        progress
            .into_function_call()
            .expect("expected function call")
            .into_repl(),
    );

    let (repl, _) = init_repl("x = 'old'\ndef f():\n    return 1");
    let progress = repl.feed_start(REBIND, vec![], PrintWriter::Stdout).unwrap();
    check(round_trip_progress(&progress).into_repl());

    let (repl, _) = init_repl("x = 'old'\ndef f():\n    return 1\nasync def main():\n    await ext_fn()");
    let progress = repl
        .feed_start(
            "x = 'rebound literal'\ndef f():\n    return 2\nawait main()",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
    let call = progress.into_function_call().expect("expected function call");
    let progress = call.resume_pending(PrintWriter::Stdout).unwrap();
    check(
        progress
            .into_resolve_futures()
            .expect("expected resolve futures")
            .into_repl(),
    );
}

/// Snippets that fail before running (syntax error, compile error, invalid
/// input) leave the session's earlier definitions callable and later
/// definitions working — the tables are handed back, not lost.
#[test]
fn repl_failed_snippets_keep_session_tables() {
    let (mut repl, _) = init_repl("def f():\n    return 1");

    let err = feed_run_print(&mut repl, "def g(:").unwrap_err();
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(1));

    let err = feed_run_print(&mut repl, "__name__ = 'x'").unwrap_err();
    assert_eq!(err.exc_type(), ExcType::NotImplementedError);
    assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(1));

    let err = repl
        .feed_run(
            "bad",
            vec![("bad".to_owned(), MontyObject::Repr("bad".to_owned()))],
            PrintWriter::Stdout,
        )
        .unwrap_err();
    assert_eq!(
        err.message(),
        Some("invalid input type: 'Repr' is not a valid input value")
    );
    assert_eq!(feed_run_print(&mut repl, "f()").unwrap(), MontyObject::Int(1));

    feed_run_print(&mut repl, "def h():\n    return f() + 1").unwrap();
    assert_eq!(feed_run_print(&mut repl, "h()").unwrap(), MontyObject::Int(2));

    let err = repl
        .feed_start("def g(:", vec![], PrintWriter::Stdout)
        .expect_err("expected syntax error");
    assert_eq!(err.error.exc_type(), ExcType::SyntaxError);
    let mut repl = err.repl;
    assert_eq!(feed_run_print(&mut repl, "h()").unwrap(), MontyObject::Int(2));
}

/// A snippet rejected at compile time, after prepare has allocated its
/// global slots and the compiler has emitted its functions, must not consume
/// those `u16` ids. One successful snippet takes the session to within a few
/// ids of both caps, so a handful of rejected snippets would overflow them
/// if their ids leaked — cheaper than 65k feeds, and just as conclusive.
#[test]
fn repl_rejected_snippets_do_not_consume_slots_or_function_ids() {
    const HEADROOM: usize = 8;
    let mut prefill = String::new();
    for i in 0..usize::from(u16::MAX) + 1 - HEADROOM {
        write!(prefill, "def g_{i}():\n    pass\n").unwrap();
    }
    let (mut repl, _) = init_repl(&prefill);

    // Each would take four slots (input, function, global, `__name__`) and a
    // function id; a second rejection would overflow if the first one leaked.
    for i in 0..4 * HEADROOM {
        let code = format!("def bad_{i}():\n    pass\nname_{i} = 1\n__name__ = 'x'");
        let err = repl
            .feed_run(
                &code,
                vec![(format!("input_{i}"), MontyObject::Int(1))],
                PrintWriter::Stdout,
            )
            .unwrap_err();
        assert_eq!(err.exc_type(), ExcType::NotImplementedError);
    }
    feed_run_print(&mut repl, "def h():\n    return g_0() is None\nok = h()").unwrap();
    assert_eq!(feed_run_print(&mut repl, "ok").unwrap(), MontyObject::Bool(true));
}

#[test]
fn repl_progress_dump_load_roundtrip() {
    let (repl, _) = init_repl("");

    // With LoadGlobalCallable, ext_fn goes directly to FunctionCall
    let progress = repl.feed_start("ext_fn(20) + 22", vec![], PrintWriter::Stdout).unwrap();

    let loaded = round_trip_progress(&progress);

    let call = loaded.into_function_call().expect("expected function call");
    assert_eq!(call.args, vec![MontyObject::Int(20)]);

    let progress = call.resume(MontyObject::Int(20), PrintWriter::Stdout).unwrap();
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::Int(42));
    assert_eq!(feed_run_print(&mut repl, "z = 1").unwrap(), MontyObject::None);
    assert_eq!(feed_run_print(&mut repl, "z").unwrap(), MontyObject::Int(1));
}

#[test]
fn repl_start_run_pending_resolve_futures_roundtrip() {
    let (mut repl, _) = init_repl("");
    feed_run_print(
        &mut repl,
        r"
async def main():
    value = await foo()
    return value + 1
",
    )
    .unwrap();

    let progress = repl.feed_start("await main()", vec![], PrintWriter::Stdout).unwrap();
    // With LoadGlobalCallable, foo() goes directly to FunctionCall
    let call = progress.into_function_call().expect("expected function call");
    let call_id = call.call_id;

    let progress = call.resume_pending(PrintWriter::Stdout).unwrap();
    let loaded = round_trip_progress(&progress);
    let state = loaded.into_resolve_futures().expect("expected resolve futures");
    assert_eq!(state.pending_call_ids(), &[call_id]);

    let progress = state
        .resume(
            vec![(call_id, ExtFunctionResult::Return(MontyObject::Int(41)))],
            PrintWriter::Stdout,
        )
        .unwrap();
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::Int(42));
    assert_eq!(
        feed_run_print(&mut repl, "final_value = 42").unwrap(),
        MontyObject::None
    );
    assert_eq!(feed_run_print(&mut repl, "final_value").unwrap(), MontyObject::Int(42));
}

#[test]
fn repl_start_runtime_error_preserves_repl_state() {
    // Simulate an agent loop: create variables, then a later snippet raises.
    // The REPL must survive so subsequent snippets can access prior variables.
    let (repl, _) = init_repl("x = 10");

    // Snippet that sets a new variable then raises — returned via ReplStartError.
    let err = repl
        .feed_start("y = 20\nraise ValueError('boom')", vec![], PrintWriter::Stdout)
        .expect_err("expected ReplStartError");
    let ReplStartError { mut repl, error } = *err;
    assert_eq!(error.exc_type(), ExcType::ValueError);
    assert_eq!(error.message(), Some("boom"));

    // Variables from BEFORE the error snippet survive.
    assert_eq!(feed_run_print(&mut repl, "x").unwrap(), MontyObject::Int(10));
    // Variable assigned BEFORE the raise within the erroring snippet also survives.
    assert_eq!(feed_run_print(&mut repl, "y").unwrap(), MontyObject::Int(20));
    // New snippets continue to work normally.
    assert_eq!(feed_run_print(&mut repl, "x + y + 12").unwrap(), MontyObject::Int(42));
}

#[test]
fn repl_start_runtime_error_during_external_call_preserves_repl_state() {
    // An external function returns an error, which should come back as ReplStartError
    // with the REPL session preserved.
    let (repl, _) = init_repl("z = 99");

    let progress = repl.feed_start("ext_fn(1)", vec![], PrintWriter::Stdout).unwrap();
    let call = progress.into_function_call().expect("expected function call");

    // Resume with an exception from the external function.
    let exc = MontyException::new(ExcType::RuntimeError, Some("ext failed".to_string()));
    let err = call
        .resume(ExtFunctionResult::Error(exc), PrintWriter::Stdout)
        .expect_err("expected ReplStartError");
    let ReplStartError { mut repl, error } = *err;
    assert_eq!(error.exc_type(), ExcType::RuntimeError);

    // Variable from before the error is still accessible.
    assert_eq!(feed_run_print(&mut repl, "z").unwrap(), MontyObject::Int(99));
}

#[test]
fn repl_class_instance_method_call_yields_function_call_with_instance_id() {
    // Create a REPL with a host class instance input and call a method on it.
    // This exercises the MethodCall path in repl.rs handle_repl_vm_result.
    let point = MontyObject::ClassInstance(Box::new(MontyClassInstance {
        class_type: MontyClassType {
            name: "Point".to_string(),
            id: MontyUuid::from_u128(7),
            host_defined: true,
            is_dataclass: true,
            attrs: DictPairs::default(),
        },
        instance_id: MontyUuid::from_u128(42),
        attrs: vec![
            (MontyObject::String("x".to_string()), MontyObject::Int(1)),
            (MontyObject::String("y".to_string()), MontyObject::Int(2)),
        ]
        .into(),
    }));

    let repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());

    // Calling point.sum() should yield a FunctionCall routed by instance_id.
    // Pass the instance as an input to feed_start() so it gets a namespace slot.
    let progress = repl
        .feed_start("point.sum()", vec![("point".to_string(), point)], PrintWriter::Stdout)
        .unwrap();
    let call = progress.into_function_call().expect("expected method call");

    assert_eq!(call.function_name, "sum");
    assert_eq!(
        call.object_id,
        Some(MontyUuid::from_u128(42)),
        "should be a method call on instance 42"
    );
    assert!(call.args.is_empty(), "receiver must not be included in args");

    // Resume with a return value (sum of x + y = 3)
    let progress = call.resume(MontyObject::Int(3), PrintWriter::Stdout).unwrap();
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::Int(3));

    // Verify REPL state is preserved after method call
    assert_eq!(feed_run_print(&mut repl, "1 + 1").unwrap(), MontyObject::Int(2));
}

/// `hasattr()` / `getattr(obj, name, default)` suspend a lazy lookup carrying
/// a pending effect that shapes the answer on resume, and the effect must
/// survive a dump/restore of the suspended session (including a heap-owning
/// `getattr()` default).
#[test]
fn repl_hasattr_getattr_lookup_effects_survive_dump() {
    let point = host_point();
    let repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let code = "(hasattr(point, 'dims'), hasattr(point, 'nope'), getattr(point, 'nope', 7), \
                getattr(point, 'nope', [1]), getattr(point, 'dims', 0))";
    let mut progress = repl
        .feed_start(code, vec![("point".to_string(), point)], PrintWriter::Stdout)
        .unwrap();

    // (name, answer, round-trip through the dump format first)
    let steps = [
        ("dims", NameLookupResult::Value(MontyObject::Int(2)), true),
        ("nope", NameLookupResult::Undefined, true),
        ("nope", NameLookupResult::Undefined, true),
        ("nope", NameLookupResult::Undefined, true),
        ("dims", NameLookupResult::Value(MontyObject::Int(2)), false),
    ];
    for (name, answer, round_trip) in steps {
        let progress_in = if round_trip {
            let restored = round_trip_progress(&progress);
            drop(progress.into_name_lookup().unwrap().into_repl());
            restored
        } else {
            progress
        };
        let lookup = progress_in
            .into_name_lookup()
            .expect("expected a lazy attribute lookup");
        assert_eq!(lookup.name, name);
        assert_eq!(lookup.object_id(), Some(MontyUuid::from_u128(42)));
        progress = lookup.resume(answer, PrintWriter::Stdout).unwrap();
    }
    let (_repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(
        value,
        MontyObject::Tuple(vec![
            MontyObject::Bool(true),
            MontyObject::Bool(false),
            MontyObject::Int(7),
            MontyObject::List(vec![MontyObject::Int(1)]),
            MontyObject::Int(2),
        ])
    );
}

/// A lazy lookup answered with a host exception raises it where the
/// attribute was read: `hasattr()` / `getattr()` defaults only cover
/// `Undefined` (CPython swallows only `AttributeError` there), so the error
/// reaches the sandbox `try/except` — and the session stays usable.
#[test]
fn repl_lookup_error_raises_in_sandbox() {
    let point = host_point();
    let repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let code = [
        "caught = []",
        "for read in (lambda: point.boom, lambda: hasattr(point, 'boom'), lambda: getattr(point, 'boom', 7)):",
        "    try:",
        "        read()",
        "    except KeyError as e:",
        "        caught.append(str(e))",
        "caught",
    ]
    .join("\n");
    let mut progress = repl
        .feed_start(&code, vec![("point".to_string(), point)], PrintWriter::Stdout)
        .unwrap();
    for _ in 0..3 {
        let lookup = progress.into_name_lookup().expect("expected a lazy attribute lookup");
        assert_eq!(lookup.name, "boom");
        assert_eq!(lookup.object_id(), Some(MontyUuid::from_u128(42)));
        let error = MontyException::new(ExcType::KeyError, Some("boom".to_owned()));
        progress = lookup
            .resume(NameLookupResult::Error(error), PrintWriter::Stdout)
            .unwrap();
    }
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(
        value,
        MontyObject::List(vec![MontyObject::String("'boom'".to_owned()); 3])
    );
    assert_eq!(feed_run_print(&mut repl, "1 + 1").unwrap(), MontyObject::Int(2));
}

/// An uncaught lookup error ends the snippet with a traceback pointing at
/// the read, and a namespace lookup answered with an error raises the same
/// way (no `NameError`).
#[test]
fn repl_lookup_error_uncaught_has_traceback() {
    let point = host_point();
    let repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let progress = repl
        .feed_start(
            "x = 1
point.boom",
            vec![("point".to_string(), point)],
            PrintWriter::Stdout,
        )
        .unwrap();
    let lookup = progress.into_name_lookup().unwrap();
    let error = MontyException::new(ExcType::KeyError, Some("boom".to_owned()));
    let err = lookup
        .resume(NameLookupResult::Error(error), PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err.error.to_string(), @r#"
    Traceback (most recent call last):
      File "<python-input-0>", line 2, in <module>
        point.boom
        ~~~~~~~~~~
    KeyError: boom
    "#);
    // the session keeps the globals the snippet set before raising
    let mut repl = err.repl;
    assert_eq!(feed_run_print(&mut repl, "x").unwrap(), MontyObject::Int(1));

    let progress = repl.feed_start("missing", vec![], PrintWriter::Stdout).unwrap();
    let lookup = progress.into_name_lookup().unwrap();
    assert_eq!(lookup.name, "missing");
    let error = MontyException::new(ExcType::PermissionError, Some("no lookups".to_owned()));
    let err = lookup
        .resume(NameLookupResult::Error(error), PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err.error.to_string(), @r#"
    Traceback (most recent call last):
      File "<python-input-2>", line 1, in <module>
        missing
        ~~~~~~~
    PermissionError: no lookups
    "#);
}

/// A host-defined `Point` instance with lazy attributes, for lookup tests.
fn host_point() -> MontyObject {
    host_point_instance(42)
}

/// A host `Point` instance (class id 7) with the given instance id and an
/// attr-less class branch, as the bindings send instances.
fn host_point_instance(instance_id: u128) -> MontyObject {
    MontyObject::ClassInstance(Box::new(MontyClassInstance {
        class_type: host_point_class_type("Point", DictPairs::default()),
        instance_id: MontyUuid::from_u128(instance_id),
        attrs: DictPairs::default(),
    }))
}

/// The host `Point` class (id 7) as a type input, with eager class attrs.
fn host_point_type(attrs: DictPairs) -> MontyObject {
    MontyObject::Type(MontyType::Instance(Box::new(host_point_class_type("Point", attrs))))
}

/// The wire class type for host class id 7 under `name`.
fn host_point_class_type(name: &str, attrs: DictPairs) -> MontyClassType {
    MontyClassType {
        name: name.to_owned(),
        id: MontyUuid::from_u128(7),
        host_defined: true,
        is_dataclass: true,
        attrs,
    }
}

/// `{name: int}` eager attrs.
fn int_attrs(pairs: &[(&str, i64)]) -> DictPairs {
    pairs
        .iter()
        .map(|(k, v)| (MontyObject::String((*k).to_owned()), MontyObject::Int(*v)))
        .collect::<Vec<_>>()
        .into()
}

/// The sandbox keeps one type object per host class uuid: instances share it,
/// `type(x)` / `__class__` return it, a `ClassType` input with the same id
/// resolves to it (its eager attrs land on the shared entry), and
/// `isinstance` / `dataclasses.is_dataclass` see through it.
#[test]
fn repl_host_class_type_is_one_object_per_class() {
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    feed_run_print(&mut repl, "from dataclasses import is_dataclass").unwrap();
    let inputs = vec![
        ("a".to_owned(), host_point_instance(42)),
        ("b".to_owned(), host_point_instance(43)),
        ("Point".to_owned(), host_point_type(int_attrs(&[("SIDES", 4)]))),
    ];
    let code = "(type(a) is type(b), type(a) is Point, type(a) == Point, type(a).SIDES, a.__class__ is Point, \
                isinstance(a, Point), isinstance(a, (int, Point)), isinstance(1, Point), isinstance(a, int), \
                is_dataclass(Point), is_dataclass(a), repr(type(a)))";
    assert_eq!(
        repl.feed_run(code, inputs, PrintWriter::Stdout).unwrap(),
        MontyObject::Tuple(vec![
            MontyObject::Bool(true),
            MontyObject::Bool(true),
            MontyObject::Bool(true),
            MontyObject::Int(4),
            MontyObject::Bool(true),
            MontyObject::Bool(true),
            MontyObject::Bool(true),
            MontyObject::Bool(false),
            MontyObject::Bool(false),
            MontyObject::Bool(true),
            MontyObject::Bool(true),
            MontyObject::String("<class 'Point'>".to_owned()),
        ])
    );
}

/// Eager class attrs carried on an instance's class branch reach `type(x)`.
#[test]
fn repl_host_class_attrs_visible_via_type_from_instance_branch() {
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let attrs: DictPairs = vec![(
        MontyObject::String("KIND".to_owned()),
        MontyObject::String("pt".to_owned()),
    )]
    .into();
    let instance = MontyObject::ClassInstance(Box::new(MontyClassInstance {
        class_type: host_point_class_type("Point", attrs),
        instance_id: MontyUuid::from_u128(42),
        attrs: DictPairs::default(),
    }));
    let value = repl
        .feed_run("type(x).KIND", vec![("x".to_owned(), instance)], PrintWriter::Stdout)
        .unwrap();
    assert_eq!(value, MontyObject::String("pt".to_owned()));
}

/// A re-sent class type refreshes the shared entry: non-empty eager attrs
/// replace the old set, an empty set (an instance's class branch) leaves it
/// alone, and the name follows the host.
#[test]
fn repl_host_class_type_attrs_refresh_on_resend() {
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let point = |attrs| vec![("Point".to_owned(), host_point_type(attrs))];
    assert_eq!(
        repl.feed_run("Point.SIDES", point(int_attrs(&[("SIDES", 4)])), PrintWriter::Stdout)
            .unwrap(),
        MontyObject::Int(4)
    );
    let a = vec![("a".to_owned(), host_point_instance(42))];
    assert_eq!(
        repl.feed_run("(type(a) is Point, Point.SIDES)", a, PrintWriter::Stdout)
            .unwrap(),
        MontyObject::Tuple(vec![MontyObject::Bool(true), MontyObject::Int(4)])
    );
    let again = vec![("Point2".to_owned(), host_point_type(int_attrs(&[("SIDES", 5)])))];
    assert_eq!(
        repl.feed_run(
            "(Point2 is Point, Point.SIDES, type(a).SIDES)",
            again,
            PrintWriter::Stdout
        )
        .unwrap(),
        MontyObject::Tuple(vec![MontyObject::Bool(true), MontyObject::Int(5), MontyObject::Int(5)])
    );
    let renamed = MontyObject::Type(MontyType::Instance(Box::new(host_point_class_type(
        "Renamed",
        DictPairs::default(),
    ))));
    assert_eq!(
        repl.feed_run(
            "(r is Point, type(a).__name__, Point.SIDES)",
            vec![("r".to_owned(), renamed)],
            PrintWriter::Stdout
        )
        .unwrap(),
        MontyObject::Tuple(vec![
            MontyObject::Bool(true),
            MontyObject::String("Renamed".to_owned()),
            MontyObject::Int(5),
        ])
    );
}

/// Host type objects hash and compare by class id alone, so they work as
/// dict keys and set members across `type(x)` and `ClassType` inputs.
#[test]
fn repl_host_class_types_hash_and_eq_by_id() {
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let other = MontyObject::Type(MontyType::Instance(Box::new(MontyClassType {
        name: "Other".to_owned(),
        id: MontyUuid::from_u128(8),
        host_defined: true,
        is_dataclass: false,
        attrs: DictPairs::default(),
    })));
    let inputs = vec![
        ("a".to_owned(), host_point_instance(42)),
        ("Point".to_owned(), host_point_type(DictPairs::default())),
        ("Other".to_owned(), other),
    ];
    let code = "d = {type(a): 1}\nd[Point] = 2\n\
                (len(d), d[type(a)], hash(type(a)) == hash(Point), type(a) in {Point}, type(a) == Other, Point != Other)";
    assert_eq!(
        repl.feed_run(code, inputs, PrintWriter::Stdout).unwrap(),
        MontyObject::Tuple(vec![
            MontyObject::Int(1),
            MontyObject::Int(2),
            MontyObject::Bool(true),
            MontyObject::Bool(true),
            MontyObject::Bool(false),
            MontyObject::Bool(true),
        ])
    );
}

/// The host type index is derived state: after a dump/restore the shared
/// entry still resolves, and re-sending the class reuses it.
#[test]
fn repl_host_class_type_survives_dump_restore() {
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let inputs = vec![
        ("a".to_owned(), host_point_instance(42)),
        ("b".to_owned(), host_point_instance(43)),
        ("Point".to_owned(), host_point_type(int_attrs(&[("SIDES", 4)]))),
    ];
    feed_run_print(&mut repl, "x = 1").unwrap();
    repl.feed_run("x = 2", inputs, PrintWriter::Stdout).unwrap();
    let mut restored = round_trip_repl(&repl);
    assert_eq!(
        feed_run_print(&mut restored, "(type(a) is type(b), type(a) is Point, type(a).SIDES)").unwrap(),
        MontyObject::Tuple(vec![
            MontyObject::Bool(true),
            MontyObject::Bool(true),
            MontyObject::Int(4)
        ])
    );
    let again = vec![("P".to_owned(), host_point_type(DictPairs::default()))];
    assert_eq!(
        restored
            .feed_run("(P is Point, P.SIDES)", again, PrintWriter::Stdout)
            .unwrap(),
        MontyObject::Tuple(vec![MontyObject::Bool(true), MontyObject::Int(4)])
    );
}

/// A `__class__` entry in the instance's attrs — sent by the host or assigned
/// by sandbox code — never shadows the shared type object.
#[test]
fn repl_host_class_dunder_class_ignores_attrs() {
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let inputs = vec![(
        "a".to_owned(),
        MontyObject::ClassInstance(Box::new(MontyClassInstance {
            class_type: host_point_class_type("Point", DictPairs::default()),
            instance_id: MontyUuid::from_u128(42),
            attrs: int_attrs(&[("__class__", 5)]),
        })),
    )];
    let code = "before = a.__class__ is type(a)\na.__class__ = 6\n(before, a.__class__ is type(a))";
    assert_eq!(
        repl.feed_run(code, inputs, PrintWriter::Stdout).unwrap(),
        MontyObject::Tuple(vec![MontyObject::Bool(true), MontyObject::Bool(true)])
    );
}

/// The shared type entry is owned by its instances and by whoever holds
/// `type(x)`: it is freed with the last holder, and no sooner.
#[cfg(feature = "ref-count-return")]
#[test]
fn repl_host_class_type_freed_with_last_holder() {
    let control = {
        let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
        feed_run_print(&mut repl, "x = 1").unwrap();
        repl.heap_entry_count()
    };
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let inputs = vec![
        ("a".to_owned(), host_point_instance(42)),
        ("b".to_owned(), host_point_instance(43)),
        ("Point".to_owned(), host_point_type(DictPairs::default())),
    ];
    repl.feed_run("x = 1\nt = type(a)", inputs, PrintWriter::Stdout)
        .unwrap();
    feed_run_print(&mut repl, "a = b = Point = None").unwrap();
    // Only the type entry remains, kept alive by `t` and still usable.
    assert_eq!(repl.heap_entry_count(), control + 1);
    assert_eq!(
        feed_run_print(&mut repl, "t.__name__").unwrap(),
        MontyObject::String("Point".to_owned())
    );
    feed_run_print(&mut repl, "t = None").unwrap();
    assert_eq!(repl.heap_entry_count(), control);
}

/// Abandoning a suspended snippet via `into_repl` keeps its globals but
/// releases everything else in flight — the operand stack and the heap-owning
/// `getattr()` default here — so the session heap ends up exactly as if the
/// snippet had stopped before suspending.
#[cfg(feature = "ref-count-return")]
#[test]
fn repl_abandoned_lookup_releases_in_flight_state() {
    let inputs = || vec![("point".to_string(), host_point())];
    let control = {
        let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
        repl.feed_run("x = [0]", inputs(), PrintWriter::Stdout).unwrap();
        repl.heap_entry_count()
    };

    let repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    let progress = repl
        .feed_start(
            "x = [0]\n[[1], getattr(point, 'nope', [2, [3]])]",
            inputs(),
            PrintWriter::Stdout,
        )
        .unwrap();
    let repl = progress
        .into_name_lookup()
        .expect("expected a lazy attribute lookup")
        .into_repl();
    assert_eq!(repl.heap_entry_count(), control);
}

/// A sandbox class or instance the host hands back (by the uuid it crossed
/// out with) resolves to the original object rather than a host-backed copy,
/// including after a dump/restore rebuilds the uuid index; one the sandbox has
/// since freed is rejected instead.
#[test]
fn repl_sandbox_objects_round_trip_by_identity() {
    let (mut repl, _) = init_repl("class Foo:\n    def __init__(self):\n        self.x = 1\nfoo = Foo()");
    let instance = feed_run_print(&mut repl, "foo").unwrap();
    let MontyObject::ClassInstance(boxed) = instance.clone() else {
        panic!("expected a ClassInstance, got {instance:?}");
    };
    let MontyClassInstance {
        class_type,
        instance_id,
        ..
    } = *boxed;
    assert!(!class_type.host_defined);
    // The class itself crosses out as repr text; its wire type (as carried by
    // the instance) is what a host can hand back.
    let class_object = MontyObject::Type(MontyType::Instance(Box::new(class_type)));

    let checks = "(back is foo, cls is Foo, isinstance(back, cls), back.x)";
    let inputs = vec![("back".to_owned(), instance.clone()), ("cls".to_owned(), class_object)];
    let expected = MontyObject::Tuple(vec![
        MontyObject::Bool(true),
        MontyObject::Bool(true),
        MontyObject::Bool(true),
        MontyObject::Int(1),
    ]);
    let progress = repl.feed_start(checks, inputs.clone(), PrintWriter::Stdout).unwrap();
    let (repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, expected);

    // The index is derived state: a restored heap must resolve the same ids.
    let restored = round_trip_repl(&repl);
    let progress = restored.feed_start(checks, inputs, PrintWriter::Stdout).unwrap();
    let (mut restored, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, expected);

    // Once the instance is freed its id is unknown; a host copy would be wrong.
    feed_run_print(&mut restored, "foo = back = None").unwrap();
    let err = restored
        .feed_start("back", vec![("back".to_owned(), instance)], PrintWriter::Stdout)
        .expect_err("expected ReplStartError");
    assert_eq!(
        err.error.to_string(),
        format!("RuntimeError: invalid input type: sandbox instance of 'Foo' (id {instance_id}) no longer exists")
    );
}

/// Edge cases of resolving host-supplied ids against live sandbox objects:
/// values nested in containers, the (ignored) attrs payload, ids of the wrong
/// kind, an instance kept alive by a cycle until the collector runs, and a
/// class freed after its instances.
#[test]
fn repl_sandbox_object_resolution_edge_cases() {
    // Collect cycles at every checkpoint so the collector path is exercised.
    let tracker = ResourceTracker::new(ResourceLimits::default().gc_interval(1));
    let mut repl = MontyRepl::new("repl.py", tracker, CompileOptions::default());
    feed_run_print(
        &mut repl,
        "class Foo:\n    def __init__(self):\n        self.x = 1\nfoo = Foo()",
    )
    .unwrap();
    let instance = feed_run_print(&mut repl, "foo").unwrap();
    let MontyObject::ClassInstance(boxed) = instance.clone() else {
        panic!("expected a ClassInstance, got {instance:?}");
    };
    let MontyClassInstance {
        class_type,
        instance_id,
        ..
    } = *boxed;
    let complete = |repl: MontyRepl, code: &str, inputs: Vec<(String, MontyObject)>| {
        let progress = repl.feed_start(code, inputs, PrintWriter::Stdout).unwrap();
        progress.into_complete().expect("expected completion")
    };
    let start_error = |repl: MontyRepl, input: MontyObject| {
        let err = repl
            .feed_start("value", vec![("value".to_owned(), input)], PrintWriter::Stdout)
            .expect_err("expected ReplStartError");
        let ReplStartError { repl, error } = *err;
        (repl, error.to_string())
    };

    // Nested in containers, carrying a stale attrs payload that is ignored.
    let edited = MontyObject::ClassInstance(Box::new(MontyClassInstance {
        class_type: class_type.clone(),
        instance_id,
        attrs: vec![(MontyObject::String("x".to_owned()), MontyObject::Int(99))].into(),
    }));
    let inputs = vec![
        ("items".to_owned(), MontyObject::List(vec![edited.clone()])),
        (
            "mapping".to_owned(),
            MontyObject::dict(vec![(MontyObject::String("k".to_owned()), edited)]),
        ),
    ];
    let (repl, value) = complete(repl, "(items[0] is foo, mapping['k'] is foo, foo.x)", inputs);
    assert_eq!(
        value,
        MontyObject::Tuple(vec![
            MontyObject::Bool(true),
            MontyObject::Bool(true),
            MontyObject::Int(1)
        ])
    );

    // An id of the wrong kind never resolves: with a host origin it becomes a
    // host-backed copy, with a sandbox origin it is rejected.
    let host_class_type = MontyClassType {
        host_defined: true,
        ..class_type.clone()
    };
    let class_as_instance = |host_defined: bool| {
        MontyObject::ClassInstance(Box::new(MontyClassInstance {
            class_type: MontyClassType {
                host_defined,
                ..class_type.clone()
            },
            instance_id: class_type.id,
            attrs: DictPairs::default(),
        }))
    };
    let instance_as_type = |host_defined: bool| {
        MontyObject::Type(MontyType::Instance(Box::new(MontyClassType {
            id: instance_id,
            host_defined,
            ..class_type.clone()
        })))
    };
    let inputs = vec![
        ("a".to_owned(), class_as_instance(true)),
        ("b".to_owned(), instance_as_type(true)),
    ];
    let code = "(a is foo, isinstance(a, Foo), type(a).__name__, b is Foo, b == Foo, b.__name__)";
    let (repl, value) = complete(repl, code, inputs);
    assert_eq!(
        value,
        MontyObject::Tuple(vec![
            MontyObject::Bool(false),
            MontyObject::Bool(false),
            MontyObject::String("Foo".to_owned()),
            MontyObject::Bool(false),
            MontyObject::Bool(false),
            MontyObject::String("Foo".to_owned()),
        ])
    );
    let (repl, error) = start_error(repl, class_as_instance(false));
    assert_eq!(
        error,
        format!(
            "RuntimeError: invalid input type: sandbox instance of 'Foo' (id {}) no longer exists",
            class_type.id
        )
    );
    let (mut repl, error) = start_error(repl, instance_as_type(false));
    assert_eq!(
        error,
        format!("RuntimeError: invalid input type: sandbox class 'Foo' (id {instance_id}) no longer exists")
    );

    // A cycle keeps the instance alive past its last binding, so its id still
    // resolves; the collector frees it and the id is forgotten.
    feed_run_print(&mut repl, "foo.me = foo\nfoo = None").unwrap();
    let (mut repl, value) = complete(repl, "back.x", vec![("back".to_owned(), instance.clone())]);
    assert_eq!(value, MontyObject::Int(1));
    feed_run_print(&mut repl, "back = items = mapping = None\n[i for i in range(300)]").unwrap();
    let (repl, error) = start_error(repl, instance);
    assert_eq!(
        error,
        format!("RuntimeError: invalid input type: sandbox instance of 'Foo' (id {instance_id}) no longer exists")
    );

    // The class resolves while alive and is forgotten once freed.
    let class_object = MontyObject::Type(MontyType::Instance(Box::new(class_type.clone())));
    let (mut repl, value) = complete(repl, "cls is Foo", vec![("cls".to_owned(), class_object.clone())]);
    assert_eq!(value, MontyObject::Bool(true));
    feed_run_print(&mut repl, "cls = Foo = None").unwrap();
    let (_repl, error) = start_error(repl, class_object);
    assert_eq!(
        error,
        format!(
            "RuntimeError: invalid input type: sandbox class 'Foo' (id {}) no longer exists",
            host_class_type.id
        )
    );
}

#[test]
fn repl_start_new_external_function_in_later_block() {
    // Verify that an external function never referenced in prior blocks can be
    // called for the first time in a later REPL snippet.
    let (mut repl, _) = init_repl("x = 10");

    feed_run_print(&mut repl, "y = x + 5").unwrap();

    // Now call a brand-new external function that was never mentioned before.
    let progress = repl.feed_start("new_ext(y)", vec![], PrintWriter::Stdout).unwrap();
    let call = progress.into_function_call().expect("expected function call");
    assert_eq!(call.function_name, "new_ext");
    assert_eq!(call.args, vec![MontyObject::Int(15)]);

    let progress = call.resume(MontyObject::Int(100), PrintWriter::Stdout).unwrap();
    let (mut repl, value) = progress.into_complete().expect("expected completion");
    assert_eq!(value, MontyObject::Int(100));

    // REPL state from before the external call is still intact.
    assert_eq!(feed_run_print(&mut repl, "x").unwrap(), MontyObject::Int(10));
    assert_eq!(feed_run_print(&mut repl, "y").unwrap(), MontyObject::Int(15));
}

// ===========================================================================
// Function-call mode — calling Python functions from Rust
// ===========================================================================

/// Helper to create a REPL session pre-seeded with code for function calling.
fn repl_with_code(code: &str) -> MontyRepl {
    let mut repl = MontyRepl::new("session_test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(code, vec![], PrintWriter::Stdout).unwrap();
    repl
}

#[test]
fn call_simple_function() {
    let mut s = repl_with_code("def add(a, b): return a + b");
    let result = s
        .call_function(
            "add",
            vec![MontyObject::Int(2), MontyObject::Int(3)],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::Int(5));
}

#[test]
fn call_function_no_args() {
    let mut s = repl_with_code("def greet(): return 'hello'");
    let result = s.call_function("greet", vec![], PrintWriter::Stdout).unwrap();
    assert_eq!(result, MontyObject::String("hello".to_owned()));
}

#[test]
fn call_function_runs_asyncio_gather() {
    let mut repl = repl_with_code(
        "\
import asyncio
async def double(value):
    return value * 2
async def gather_values():
    return await asyncio.gather(double(1), double(2), double(3))
def run():
    return asyncio.run(gather_values())
",
    );

    let result = repl.call_function("run", vec![], PrintWriter::Stdout).unwrap();
    assert_eq!(
        result,
        MontyObject::List(vec![MontyObject::Int(2), MontyObject::Int(4), MontyObject::Int(6)])
    );
}

#[test]
fn call_function_returns_none() {
    let mut s = repl_with_code("def noop(): pass");
    let result = s.call_function("noop", vec![], PrintWriter::Stdout).unwrap();
    assert_eq!(result, MontyObject::None);
}

#[test]
fn call_function_one_arg() {
    let mut s = repl_with_code("def double(x): return x * 2");
    let result = s
        .call_function("double", vec![MontyObject::Int(21)], PrintWriter::Stdout)
        .unwrap();
    assert_eq!(result, MontyObject::Int(42));
}

#[test]
fn call_function_string_args() {
    let mut s = repl_with_code("def concat(a, b): return a + b");
    let result = s
        .call_function(
            "concat",
            vec![
                MontyObject::String("hello ".to_owned()),
                MontyObject::String("world".to_owned()),
            ],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::String("hello world".to_owned()));
}

#[test]
fn call_function_multiple_times() {
    let mut s = repl_with_code("def inc(x): return x + 1");
    for i in 0..5 {
        let result = s
            .call_function("inc", vec![MontyObject::Int(i)], PrintWriter::Stdout)
            .unwrap();
        assert_eq!(result, MontyObject::Int(i + 1));
    }
}

#[test]
fn call_function_survives_repl_round_trip() {
    let mut repl = repl_with_code("def double(value): return value * 2");
    assert_eq!(
        repl.call_function("double", vec![MontyObject::Int(2)], PrintWriter::Stdout)
            .unwrap(),
        MontyObject::Int(4)
    );

    let mut repl = round_trip_repl(&repl);
    assert_eq!(
        repl.call_function("double", vec![MontyObject::Int(3)], PrintWriter::Stdout)
            .unwrap(),
        MontyObject::Int(6)
    );
}

#[test]
fn call_function_with_list() {
    let mut s = repl_with_code("def length(lst): return len(lst)");
    let result = s
        .call_function(
            "length",
            vec![MontyObject::List(vec![
                MontyObject::Int(1),
                MontyObject::Int(2),
                MontyObject::Int(3),
            ])],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::Int(3));
}

#[test]
fn call_function_retains_global_state() {
    let mut s = repl_with_code(
        "\
counter = 0
def increment():
    global counter
    counter = counter + 1
    return counter
",
    );
    assert_eq!(
        s.call_function("increment", vec![], PrintWriter::Stdout).unwrap(),
        MontyObject::Int(1)
    );
    assert_eq!(
        s.call_function("increment", vec![], PrintWriter::Stdout).unwrap(),
        MontyObject::Int(2)
    );
    assert_eq!(
        s.call_function("increment", vec![], PrintWriter::Stdout).unwrap(),
        MontyObject::Int(3)
    );
}

#[test]
fn call_function_multiple_functions() {
    let mut s = repl_with_code(
        "\
def add(a, b): return a + b
def mul(a, b): return a * b
",
    );
    assert_eq!(
        s.call_function(
            "add",
            vec![MontyObject::Int(3), MontyObject::Int(4)],
            PrintWriter::Stdout
        )
        .unwrap(),
        MontyObject::Int(7)
    );
    assert_eq!(
        s.call_function(
            "mul",
            vec![MontyObject::Int(3), MontyObject::Int(4)],
            PrintWriter::Stdout
        )
        .unwrap(),
        MontyObject::Int(12)
    );
}

#[test]
fn call_function_calls_other_function() {
    let mut s = repl_with_code(
        "\
def double(x): return x * 2
def quadruple(x): return double(double(x))
",
    );
    let result = s
        .call_function("quadruple", vec![MontyObject::Int(5)], PrintWriter::Stdout)
        .unwrap();
    assert_eq!(result, MontyObject::Int(20));
}

#[test]
fn call_function_with_defaults() {
    let mut s = repl_with_code("def greet(name, greeting='Hello'): return greeting + ' ' + name");
    let result = s
        .call_function(
            "greet",
            vec![MontyObject::String("world".to_owned())],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::String("Hello world".to_owned()));
}

#[test]
fn call_closure() {
    let mut s = repl_with_code(
        "\
def make_adder(n):
    def adder(x):
        return x + n
    return adder

add5 = make_adder(5)
",
    );
    let result = s
        .call_function("add5", vec![MontyObject::Int(10)], PrintWriter::Stdout)
        .unwrap();
    assert_eq!(result, MontyObject::Int(15));
}

#[test]
fn call_nonexistent_function() {
    let mut s = repl_with_code("def foo(): return 1");
    let err = s.call_function("bar", vec![], PrintWriter::Stdout).unwrap_err();
    assert_snapshot!(err, @"NameError: name 'bar' is not defined");
}

#[test]
fn call_conditionally_undefined_functions() {
    let mut s = repl_with_code("if False:\n    def foo(): return 1\n    def len(): return 1");

    let err = s.call_function("foo", vec![], PrintWriter::Stdout).unwrap_err();
    assert_snapshot!(err, @"NameError: name 'foo' is not defined");

    let err = s.call_function("len", vec![], PrintWriter::Stdout).unwrap_err();
    assert_snapshot!(err, @"NameError: name 'len' is not defined");
}

#[test]
fn call_non_callable() {
    let mut s = repl_with_code("x = 42");
    let err = s.call_function("x", vec![], PrintWriter::Stdout).unwrap_err();
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "<python-input-1>", line 1, in <module>
        x()
        ~~~
    TypeError: 'int' object is not callable
    "#);
}

#[test]
fn call_function_raises_exception() {
    let mut s = repl_with_code("def boom(): raise ValueError('kaboom')");
    let err = s.call_function("boom", vec![], PrintWriter::Stdout).unwrap_err();
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "<python-input-1>", line 1, in <module>
        boom()
        ~~~~~~
      File "<python-input-0>", line 1, in boom
        def boom(): raise ValueError('kaboom')
    ValueError: kaboom
    "#);
}

#[test]
fn call_function_wrong_arg_count() {
    let mut s = repl_with_code("def add(a, b): return a + b");
    let err = s
        .call_function("add", vec![MontyObject::Int(1)], PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "<python-input-0>", line 1, in <module>
        def add(a, b): return a + b
            ~~~
    TypeError: add() missing 1 required positional argument: 'b'
    "#);
}

#[test]
fn function_names() {
    let s = repl_with_code(
        "\
x = 42
def foo(): pass
def bar(): pass
",
    );
    let mut names = s.function_names();
    names.sort_unstable();
    assert_eq!(names, vec!["bar", "foo"]);
}

#[test]
fn function_names_excludes_classes_and_methods() {
    // The helper is deliberately narrower than `is_callable`: plain functions
    // and lambdas count, but classes, namedtuple classes, and bound methods —
    // all callable — must not be surfaced as "functions".
    // Import via the module so the only function-valued global is `foo`/`lam`
    // (a bare `from collections import namedtuple` would surface `namedtuple`
    // itself, which is correctly a function).
    let s = repl_with_code(
        "\
import collections
def foo(): pass
lam = lambda: 1
class Cls:
    def method(self): pass
Point = collections.namedtuple('Point', ['a'])
inst = Cls()
bound = inst.method
x = 42
",
    );
    let mut names = s.function_names();
    names.sort_unstable();
    assert_eq!(names, vec!["foo", "lam"]);
    assert!(s.has_function("foo"));
    assert!(s.has_function("lam"));
    assert!(!s.has_function("Cls")); // a class is callable but not a function
    assert!(!s.has_function("Point")); // a namedtuple class likewise
    assert!(!s.has_function("bound")); // a bound method likewise
    assert!(!s.has_function("inst"));
    assert!(!s.has_function("x"));
}

#[test]
fn has_function() {
    let s = repl_with_code("def my_func(): pass\nx = 10");
    assert!(s.has_function("my_func"));
    assert!(!s.has_function("x")); // not callable
    assert!(!s.has_function("nonexistent"));
}

#[test]
fn call_function_captures_print() {
    let mut s = repl_with_code("def say_hello(name): print('Hello ' + name)");
    let mut output = String::new();
    let result = s
        .call_function(
            "say_hello",
            vec![MontyObject::String("world".to_owned())],
            PrintWriter::collect_string(&mut output),
        )
        .unwrap();
    assert_eq!(result, MontyObject::None);
    assert_eq!(output, "Hello world\n");
}

#[test]
fn call_function_returns_list() {
    let mut s = repl_with_code("def make_list(n): return list(range(n))");
    let result = s
        .call_function("make_list", vec![MontyObject::Int(3)], PrintWriter::Stdout)
        .unwrap();
    assert_eq!(
        result,
        MontyObject::List(vec![MontyObject::Int(0), MontyObject::Int(1), MontyObject::Int(2)])
    );
}

#[test]
fn call_function_returns_dict() {
    let mut s = repl_with_code(
        "\
def make_point(x, y):
    return {'x': x, 'y': y}
",
    );
    let result = s
        .call_function(
            "make_point",
            vec![MontyObject::Int(1), MontyObject::Int(2)],
            PrintWriter::Stdout,
        )
        .unwrap();
    if let MontyObject::Dict(pairs) = result {
        assert_eq!(pairs.into_iter().count(), 2);
    } else {
        panic!("expected dict, got: {result:?}");
    }
}

#[test]
fn call_function_many_args() {
    let mut s = repl_with_code("def sum_all(a, b, c, d, e): return a + b + c + d + e");
    let result = s
        .call_function(
            "sum_all",
            vec![
                MontyObject::Int(1),
                MontyObject::Int(2),
                MontyObject::Int(3),
                MontyObject::Int(4),
                MontyObject::Int(5),
            ],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::Int(15));
}

#[test]
fn call_function_that_calls_undefined_name_fails() {
    let mut s = repl_with_code("def call_missing(): return unknown_func()");
    let err = s
        .call_function("call_missing", vec![], PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "<python-input-1>", line 1, in <module>
        call_missing()
        ~~~~~~~~~~~~~~
      File "<python-input-0>", line 1, in call_missing
        def call_missing(): return unknown_func()
                                   ~~~~~~~~~~~~~~
    NotImplementedError: MontyRepl::call_function: external function 'unknown_func' is not yet supported in this context
    "#);
}

#[test]
fn call_function_catches_unsupported_os_call() {
    let mut s =
        repl_with_code("def try_open():\n    try:\n        open('/x.txt')\n    except:\n        return 'caught'");
    let result = s.call_function("try_open", vec![], PrintWriter::Stdout).unwrap();
    assert_eq!(result, MontyObject::String("caught".to_owned()));
}

#[test]
fn call_function_with_heap_defaults() {
    let mut s = repl_with_code("def greet(name, greeting='Hi'): return greeting + ' ' + name");
    let result = s
        .call_function(
            "greet",
            vec![MontyObject::String("Alice".to_owned())],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::String("Hi Alice".to_owned()));
}

#[test]
fn convert_args_single_repr_fails() {
    let mut s = repl_with_code("def identity(x): return x");
    let err = s
        .call_function(
            "identity",
            vec![MontyObject::Repr("bad".to_owned())],
            PrintWriter::Stdout,
        )
        .unwrap_err();
    assert_snapshot!(err, @"RuntimeError: invalid argument type: 'Repr' is not a valid input value");
}

#[test]
fn convert_args_two_second_repr_fails() {
    let mut s = repl_with_code("def add(a, b): return a + b");
    let err = s
        .call_function(
            "add",
            vec![MontyObject::Int(1), MontyObject::Repr("bad".to_owned())],
            PrintWriter::Stdout,
        )
        .unwrap_err();
    assert_snapshot!(err, @"RuntimeError: invalid argument type: 'Repr' is not a valid input value");
}

#[test]
fn convert_args_two_first_repr_fails() {
    let mut s = repl_with_code("def add(a, b): return a + b");
    let err = s
        .call_function(
            "add",
            vec![MontyObject::Repr("bad".to_owned()), MontyObject::Int(1)],
            PrintWriter::Stdout,
        )
        .unwrap_err();
    assert_snapshot!(err, @"RuntimeError: invalid argument type: 'Repr' is not a valid input value");
}

#[test]
fn convert_args_many_middle_repr_fails() {
    let mut s = repl_with_code("def f(a, b, c, d): return a");
    let err = s
        .call_function(
            "f",
            vec![
                MontyObject::Int(1),
                MontyObject::Int(2),
                MontyObject::Repr("bad".to_owned()),
                MontyObject::Int(4),
            ],
            PrintWriter::Stdout,
        )
        .unwrap_err();
    assert_snapshot!(err, @"RuntimeError: invalid argument type: 'Repr' is not a valid input value");
}

#[test]
fn call_builtin_via_session() {
    let mut s = repl_with_code("my_len = len");
    let result = s
        .call_function(
            "my_len",
            vec![MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::Int(2));
}
