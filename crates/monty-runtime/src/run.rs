//! The standard `monty` CLI: run a file, a `-c` program, or a REPL session.
//!
//! Everything here is the *standalone* path. `monty subprocess` shares only the
//! allocator and the argument definitions, and reaches none of it — which is
//! why telemetry lives here rather than in `main`.

// only the exporter reads the environment; see the `telemetry` feature
#[cfg(feature = "telemetry")]
use std::env;
use std::{
    fmt, fs, io,
    process::ExitCode,
    time::{Duration, Instant},
};

// Shadows `std::eprintln!` on purpose: every diagnostic here goes to stderr
// through `anstream`, which strips styling when stderr is not a terminal (or
// `NO_COLOR` is set) and enables virtual-terminal mode on Windows. Sandbox
// output keeps using `std::println!` — it is program data, not our styling.
use anstream::{AutoStream, ColorChoice, eprintln};
use anstyle::{AnsiColor, Color, Style};
use monty::{MontyRepl, MontyRun, ReplContinuationMode, ReplProgress, RunProgress, detect_repl_continuation_mode};
use monty_fs::{MountCallOutcome, MountMode, MountTable, OverlayState};
use monty_type_checking::{SourceFile, TypeChecker};
use monty_types::{
    CompileOptions, DEFAULT_MAX_SUSPENSIONS, ExcType, ExtFunctionResult, MontyException, MontyObject, NameLookupResult,
    OsFunctionCall, PrintWriter, ResourceLimits, ResourceTracker, TypeCheckingConfig,
};
use rustyline::{DefaultEditor, error::ReadlineError};
#[cfg(feature = "telemetry")]
use tracing::field::Empty;

use crate::Cli;

/// Dim/gray text (timings). `{DIM}` opens the style, `{DIM:#}` closes it.
const DIM: Style = Style::new().dimmed();
/// Bold red text (errors).
const BOLD_RED: Style = Style::new().bold().fg_color(Some(Color::Ansi(AnsiColor::Red)));
/// Bold green text (success, headings).
const BOLD_GREEN: Style = Style::new().bold().fg_color(Some(Color::Ansi(AnsiColor::Green)));
/// Bold cyan text (commands, prompts).
const BOLD_CYAN: Style = Style::new().bold().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
const ARROW: &str = "❯";

const EXT_FUNCTIONS: bool = false;

/// Runs the CLI inside a `run monty` span, having configured the exporter.
/// Split from [`run_cli`] so the whole stack compiles away without the feature.
#[cfg(feature = "telemetry")]
pub(crate) fn run(cli: Cli) -> ExitCode {
    let logfire = match configure_logfire() {
        Ok(logfire) => logfire,
        Err(err) => {
            eprintln!("{BOLD_RED}error{BOLD_RED:#}: failed to configure telemetry: {err}");
            return ExitCode::FAILURE;
        }
    };
    let run_span = logfire.as_ref().map(|_| logfire::span!("run monty", success = Empty,));
    let exit_code = run_cli(cli);
    if let Some(run_span) = run_span {
        run_span.record("success", exit_code == ExitCode::SUCCESS);
    }
    exit_code
}

/// Runs the CLI with no telemetry linked in — the default build, where
/// `LOGFIRE_TOKEN` is ignored rather than diagnosed.
#[cfg(not(feature = "telemetry"))]
pub(crate) fn run(cli: Cli) -> ExitCode {
    run_cli(cli)
}

/// Configures process-level telemetry for standalone CLI execution.
///
/// Worker subprocesses bypass this function so inherited credentials never
/// create an exporter in each sandbox process.
#[cfg(feature = "telemetry")]
fn configure_logfire() -> Result<Option<logfire::ShutdownGuard>, logfire::ConfigureError> {
    if env::var_os("LOGFIRE_TOKEN").is_some() {
        logfire::configure()
            .with_service_name("monty")
            .with_service_version(env!("CARGO_PKG_VERSION"))
            .finish()
            .map(logfire::Logfire::shutdown_guard)
            .map(Some)
    } else {
        Ok(None)
    }
}

/// Dispatches a parsed standalone CLI invocation.
fn run_cli(cli: Cli) -> ExitCode {
    // `Some` enables the check and carries how its diagnostics are rendered.
    let type_check = cli.type_check.then(|| TypeCheckingConfig {
        format: cli.type_check_format.unwrap_or_default(),
        color: stderr_styled(),
    });

    let limits = match cli.resource_limits() {
        Ok(limits) => limits,
        Err(err) => {
            eprintln!("{BOLD_RED}error{BOLD_RED:#}: {err}");
            return ExitCode::FAILURE;
        }
    };
    monty_alloc::set_limit(limits.max_memory, type_check.is_some())
        .expect("monty-runtime must install LimitedAllocator globally");

    // Build mount table early to fail fast on bad -m args.
    let mount_table = match build_mount_table(&cli.mounts) {
        Ok(mt) => mt,
        Err(err) => {
            eprintln!("{BOLD_RED}error{BOLD_RED:#}: {err}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(cmd) = cli.command {
        if cli.file.is_some() {
            eprintln!("{BOLD_RED}error{BOLD_RED:#}: cannot specify both -c and a file");
            return ExitCode::FAILURE;
        }
        return if cli.interactive {
            dispatch_repl("<string>", &cmd, limits, mount_table)
        } else {
            dispatch_script("<string>", cmd, type_check, limits, mount_table)
        };
    }

    if let Some(file_path) = cli.file.as_deref() {
        let code = match read_file(file_path) {
            Ok(code) => code,
            Err(err) => {
                eprintln!("{BOLD_RED}error{BOLD_RED:#}: {err}");
                return ExitCode::FAILURE;
            }
        };
        return if cli.interactive {
            dispatch_repl(file_path, &code, limits, mount_table)
        } else {
            dispatch_script(file_path, code, type_check, limits, mount_table)
        };
    }

    dispatch_repl("repl.py", "", limits, mount_table)
}

/// Builds the tracker from the CLI resource limits and runs the script.
fn dispatch_script(
    file_path: &str,
    code: String,
    type_check: Option<TypeCheckingConfig>,
    limits: ResourceLimits,
    mount_table: Option<MountTable>,
) -> ExitCode {
    run_script(file_path, code, type_check, ResourceTracker::new(limits), mount_table)
}

/// REPL analog of [`dispatch_script`].
fn dispatch_repl(file_path: &str, code: &str, limits: ResourceLimits, mount_table: Option<MountTable>) -> ExitCode {
    run_repl(file_path, code, ResourceTracker::new(limits), mount_table)
}

/// Executes a Python file in one-shot CLI mode.
///
/// This path keeps the existing CLI behavior: run type-checking for visibility,
/// compile the file as a full module, and execute it either through direct
/// execution or through the suspendable progress loop when mounts or external
/// functions are enabled.
///
/// Returns `ExitCode::SUCCESS` for successful execution and
/// `ExitCode::FAILURE` for parse/type/runtime failures.
fn run_script(
    file_path: &str,
    code: String,
    type_check: Option<TypeCheckingConfig>,
    tracker: ResourceTracker,
    mut mount_table: Option<MountTable>,
) -> ExitCode {
    if let Some(config) = type_check {
        let start = Instant::now();
        let mut checker = TypeChecker::default();
        if let Some(failure) = checker.run(&SourceFile::new(&code, file_path), None, config).unwrap() {
            let elapsed = start.elapsed();
            eprintln!(
                "{DIM}{}{DIM:#} {BOLD_CYAN}{ARROW}{BOLD_CYAN:#} {BOLD_RED}type check failed{BOLD_RED:#}:\n{failure}",
                FormattedDuration(elapsed)
            );
        } else {
            let elapsed = start.elapsed();
            eprintln!(
                "{DIM}{}{DIM:#} {BOLD_CYAN}{ARROW}{BOLD_CYAN:#} {BOLD_GREEN}type check passed{BOLD_GREEN:#}",
                FormattedDuration(elapsed)
            );
        }
    }

    let input_names = vec![];
    let inputs = vec![];

    let runner = match MontyRun::new(code, file_path, input_names, CompileOptions::default()) {
        Ok(ex) => ex,
        Err(err) => {
            eprintln!("{BOLD_RED}error{BOLD_RED:#}:\n{err}");
            return ExitCode::FAILURE;
        }
    };

    // Use the start() + loop path when mounts are configured or external functions
    // are enabled, since we need to intercept OsCalls.
    if EXT_FUNCTIONS || mount_table.is_some() {
        let start = Instant::now();
        let progress = match runner.start(inputs, tracker, PrintWriter::Stdout) {
            Ok(p) => p,
            Err(err) => {
                let elapsed = start.elapsed();
                eprintln!(
                    "{DIM}{}{DIM:#} {BOLD_CYAN}{ARROW}{BOLD_CYAN:#} {BOLD_RED}error{BOLD_RED:#}: {err}",
                    FormattedDuration(elapsed)
                );
                return ExitCode::FAILURE;
            }
        };

        let mut suspensions = SuspensionBudget::from_progress(&progress);
        match run_until_complete(progress, &mut mount_table, &mut suspensions) {
            Ok(value) => {
                let elapsed = start.elapsed();
                eprintln!(
                    "{DIM}{}{DIM:#} {BOLD_CYAN}{ARROW}{BOLD_CYAN:#} {value}",
                    FormattedDuration(elapsed)
                );
                ExitCode::SUCCESS
            }
            Err(err) => {
                let elapsed = start.elapsed();
                eprintln!(
                    "{DIM}{}{DIM:#} {BOLD_CYAN}{ARROW}{BOLD_CYAN:#} {BOLD_RED}error{BOLD_RED:#}: {err}",
                    FormattedDuration(elapsed)
                );
                ExitCode::FAILURE
            }
        }
    } else {
        let start = Instant::now();
        let value = match runner.run(inputs, tracker, PrintWriter::Stdout) {
            Ok(p) => p,
            Err(err) => {
                let elapsed = start.elapsed();
                eprintln!(
                    "{DIM}{}{DIM:#} {BOLD_CYAN}{ARROW}{BOLD_CYAN:#} {BOLD_RED}error{BOLD_RED:#}: {err}",
                    FormattedDuration(elapsed)
                );
                return ExitCode::FAILURE;
            }
        };
        let elapsed = start.elapsed();
        eprintln!(
            "{DIM}{}{DIM:#} {BOLD_CYAN}{ARROW}{BOLD_CYAN:#} {value}",
            FormattedDuration(elapsed)
        );
        ExitCode::SUCCESS
    }
}

/// Starts an interactive line-by-line REPL session.
///
/// Initializes `MontyRepl` once and incrementally feeds entered snippets without
/// replaying previous snippets, which matches the intended stateful REPL model.
/// Multiline input follows CPython-style prompts:
/// - `❯ ` for a new statement
/// - `… ` for continuation lines
///
/// Returns `ExitCode::SUCCESS` on EOF or `exit`, and `ExitCode::FAILURE` on
/// initialization or I/O errors.
fn run_repl(file_path: &str, code: &str, tracker: ResourceTracker, mut mount_table: Option<MountTable>) -> ExitCode {
    let mut suspensions = SuspensionBudget::new(&tracker);
    let mut repl = Some(MontyRepl::new(file_path, tracker, CompileOptions::default()));

    if !code.is_empty() {
        execute_repl_snippet(&mut repl, code, &mut mount_table, &mut suspensions);
    }

    eprintln!("Monty v{} REPL. Type `exit` to exit.", env!("CARGO_PKG_VERSION"));

    let mut rl = match DefaultEditor::new() {
        Ok(rl) => rl,
        Err(err) => {
            eprintln!("{BOLD_RED}error{BOLD_RED:#} initializing editor: {err}");
            return ExitCode::FAILURE;
        }
    };

    let mut pending_snippet = String::new();
    let mut continuation_mode = ReplContinuationMode::Complete;

    // rustyline writes the prompt to stdout itself, so `anstream` never sees
    // it — style it up front, against stdout, or not at all.
    let statement_prompt = if stdout_styled() {
        format!("{BOLD_CYAN}{ARROW}{BOLD_CYAN:#} ")
    } else {
        format!("{ARROW} ")
    };

    loop {
        let prompt = if continuation_mode == ReplContinuationMode::Complete {
            statement_prompt.as_str()
        } else {
            "… "
        };

        let line = match rl.readline(prompt) {
            Ok(line) => line,
            Err(ReadlineError::Eof) => return ExitCode::SUCCESS,
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C: discard pending input and start fresh
                pending_snippet.clear();
                continuation_mode = ReplContinuationMode::Complete;
                continue;
            }
            Err(err) => {
                eprintln!("{BOLD_RED}error{BOLD_RED:#} reading input: {err}");
                return ExitCode::FAILURE;
            }
        };

        let snippet = line.trim_end();
        if continuation_mode == ReplContinuationMode::Complete && snippet.is_empty() {
            continue;
        }
        if continuation_mode == ReplContinuationMode::Complete && snippet == "exit" {
            return ExitCode::SUCCESS;
        }

        pending_snippet.push_str(snippet);
        pending_snippet.push('\n');

        if continuation_mode == ReplContinuationMode::IncompleteBlock && snippet.is_empty() {
            let _ = rl.add_history_entry(pending_snippet.trim_end());
            execute_repl_snippet(&mut repl, &pending_snippet, &mut mount_table, &mut suspensions);
            pending_snippet.clear();
            continuation_mode = ReplContinuationMode::Complete;
            continue;
        }

        let detected_mode = detect_repl_continuation_mode(&pending_snippet);
        match detected_mode {
            ReplContinuationMode::Complete => {
                if continuation_mode == ReplContinuationMode::IncompleteBlock {
                    continue;
                }
                let _ = rl.add_history_entry(pending_snippet.trim_end());
                execute_repl_snippet(&mut repl, &pending_snippet, &mut mount_table, &mut suspensions);
                pending_snippet.clear();
                continuation_mode = ReplContinuationMode::Complete;
            }
            ReplContinuationMode::IncompleteBlock => continuation_mode = ReplContinuationMode::IncompleteBlock,
            ReplContinuationMode::IncompleteImplicit => {
                if continuation_mode != ReplContinuationMode::IncompleteBlock {
                    continuation_mode = ReplContinuationMode::IncompleteImplicit;
                }
            }
        }
    }
}

/// Executes one collected REPL snippet, printing the result or error.
///
/// When mounts are configured, uses `feed_start()` + a progress loop to intercept
/// `OsCall`s. Otherwise uses the simpler `feed_run()` path.
///
/// Takes `&mut Option<MontyRepl>` because `feed_start` consumes the repl —
/// we `take()` it out, run to completion, then put it back.
fn execute_repl_snippet(
    repl: &mut Option<MontyRepl>,
    snippet: &str,
    mount_table: &mut Option<MountTable>,
    suspensions: &mut SuspensionBudget,
) {
    let r = repl.take().expect("repl must be present");

    if mount_table.is_some() {
        match execute_repl_with_mounts(r, snippet, mount_table, suspensions) {
            Ok((returned_repl, output)) => {
                if output != MontyObject::None {
                    println!("{output}");
                }
                *repl = Some(returned_repl);
            }
            Err((returned_repl, err)) => {
                eprintln!("{BOLD_RED}error{BOLD_RED:#}: {err}");
                *repl = Some(returned_repl);
            }
        }
    } else {
        // No mounts — use the simple feed_run path (takes &mut self).
        let mut r = r;
        match r.feed_run(snippet, vec![], PrintWriter::Stdout) {
            Ok(output) => {
                if output != MontyObject::None {
                    println!("{output}");
                }
            }
            Err(err) => {
                eprintln!("{BOLD_RED}error{BOLD_RED:#}: {err}");
            }
        }
        *repl = Some(r);
    }
}

/// Runs a REPL snippet with mount support via the `feed_start` + progress loop path.
///
/// Returns `Ok((repl, value))` on success, or `Err((repl, message))` on failure.
/// The repl is always returned so the caller can continue the session.
#[expect(clippy::result_large_err)]
fn execute_repl_with_mounts(
    r: MontyRepl,
    snippet: &str,
    mount_table: &mut Option<MountTable>,
    suspensions: &mut SuspensionBudget,
) -> Result<(MontyRepl, MontyObject), (MontyRepl, String)> {
    let mut progress = match r.feed_start(snippet, vec![], PrintWriter::Stdout) {
        Ok(p) => p,
        Err(err) => return Err((err.repl, format!("{}", err.error))),
    };

    loop {
        // The CLI, as host, enforces `--max-suspensions`.
        if !matches!(progress, ReplProgress::Complete { .. })
            && let Some(exc) = suspensions.record_suspension()
        {
            let outcome = match progress {
                ReplProgress::OsCall(call) => call.abort(exc, PrintWriter::Stdout),
                ReplProgress::FunctionCall(call) => call.abort(exc, PrintWriter::Stdout),
                ReplProgress::NameLookup(lookup) => lookup.abort(exc, PrintWriter::Stdout),
                ReplProgress::ResolveFutures(state) => state.abort(exc, PrintWriter::Stdout),
                ReplProgress::Complete { .. } => unreachable!("only suspensions are counted"),
            };
            match outcome {
                Ok(p) => progress = p,
                Err(err) => return Err((err.repl, format!("{}", err.error))),
            }
            continue;
        }
        match progress {
            ReplProgress::Complete { repl, value } => return Ok((repl, value)),
            ReplProgress::OsCall(call) => {
                match call.resume_with(PrintWriter::Stdout, |fc| handle_os_call(fc, mount_table)) {
                    Ok(p) => progress = p,
                    Err(err) => return Err((err.repl, format!("{}", err.error))),
                }
            }
            ReplProgress::FunctionCall(call) => {
                return Err((
                    call.into_repl(),
                    "external function calls not supported in CLI".to_owned(),
                ));
            }
            ReplProgress::NameLookup(lookup) => match lookup.resume(NameLookupResult::Undefined, PrintWriter::Stdout) {
                Ok(p) => progress = p,
                Err(err) => return Err((err.repl, format!("{}", err.error))),
            },
            ReplProgress::ResolveFutures(state) => {
                return Err((state.into_repl(), "async futures not supported in CLI".to_owned()));
            }
        }
    }
}

/// Drives suspendable execution until completion.
///
/// This repeatedly resumes `RunProgress` values by resolving supported
/// external calls and returns the final value when execution reaches
/// `RunProgress::Complete`.
///
/// When a mount table is provided, filesystem `OsCall`s are handled via the
/// mount table. Non-filesystem `OsCall`s and `OsCall`s without a mount table
/// produce an error.
fn run_until_complete(
    mut progress: RunProgress,
    mount_table: &mut Option<MountTable>,
    suspensions: &mut SuspensionBudget,
) -> Result<MontyObject, String> {
    loop {
        // The CLI, as host, enforces `--max-suspensions`.
        if !matches!(progress, RunProgress::Complete(_))
            && let Some(exc) = suspensions.record_suspension()
        {
            progress = match progress {
                RunProgress::OsCall(call) => call.abort(exc, PrintWriter::Stdout),
                RunProgress::FunctionCall(call) => call.abort(exc, PrintWriter::Stdout),
                RunProgress::NameLookup(lookup) => lookup.abort(exc, PrintWriter::Stdout),
                RunProgress::ResolveFutures(state) => state.abort(exc, PrintWriter::Stdout),
                RunProgress::Complete(_) => unreachable!("only suspensions are counted"),
            }
            .map_err(|err| format!("{err}"))?;
            continue;
        }
        match progress {
            RunProgress::Complete(value) => return Ok(value),
            RunProgress::FunctionCall(call) => {
                let return_value = resolve_external_call(&call.function_name, &call.args)?;
                progress = call
                    .resume(return_value, PrintWriter::Stdout)
                    .map_err(|err| format!("{err}"))?;
            }
            RunProgress::ResolveFutures(state) => {
                return Err(format!(
                    "async futures not supported in CLI: {:?}",
                    state.pending_call_ids()
                ));
            }
            RunProgress::NameLookup(lookup) => {
                let result = if lookup.name == "add_ints" {
                    NameLookupResult::Value(MontyObject::Function {
                        name: "add_ints".to_string(),
                        docstring: None,
                    })
                } else {
                    NameLookupResult::Undefined
                };
                progress = lookup
                    .resume(result, PrintWriter::Stdout)
                    .map_err(|err| format!("{err}"))?;
            }
            RunProgress::OsCall(call) => {
                progress = call
                    .resume_with(PrintWriter::Stdout, |fc| handle_os_call(fc, mount_table))
                    .map_err(|err| format!("{err}"))?;
            }
        }
    }
}

/// Tracks the CLI-enforced suspension limit across an entire REPL session.
struct SuspensionBudget {
    limit: usize,
    seen: usize,
}

impl SuspensionBudget {
    /// Initializes a budget from the tracker's limits.
    fn new(tracker: &ResourceTracker) -> Self {
        Self {
            limit: tracker.max_suspensions(),
            seen: 0,
        }
    }

    /// Reads a one-shot run's limit from its initial progress.
    fn from_progress(progress: &RunProgress) -> Self {
        let limit = match progress {
            RunProgress::FunctionCall(call) => call.tracker().max_suspensions(),
            RunProgress::OsCall(call) => call.tracker().max_suspensions(),
            RunProgress::NameLookup(lookup) => lookup.tracker().max_suspensions(),
            RunProgress::ResolveFutures(state) => state.tracker().max_suspensions(),
            // never counted, so the value is immaterial
            RunProgress::Complete(_) => DEFAULT_MAX_SUSPENSIONS,
        };
        Self { limit, seen: 0 }
    }

    /// Counts one suspension and returns the exception to abort it with once
    /// the count is over budget.
    fn record_suspension(&mut self) -> Option<MontyException> {
        self.seen += 1;
        let limit = self.limit;
        (self.seen > limit).then(|| {
            MontyException::new(
                ExcType::RuntimeError,
                Some(format!("suspension limit {limit} exceeded")),
            )
        })
    }
}

/// Handles a filesystem `OsCall` using the mount table if available.
///
/// Consumes the call (moving write payloads into the mount backend) and
/// returns the operation result as an `ExtFunctionResult` — either a
/// successful `MontyObject` or an exception for errors / unsupported
/// operations.
fn handle_os_call(call: OsFunctionCall, mount_table: &mut Option<MountTable>) -> ExtFunctionResult {
    match mount_table.as_mut() {
        Some(mounts) => match mounts.handle_os_call(call) {
            MountCallOutcome::Handled(Ok(obj)) => obj.into(),
            MountCallOutcome::Handled(Err(err)) => err.into_exception().into(),
            MountCallOutcome::NotHandled(call) => call.on_no_handler().into(),
        },
        None => call.on_no_handler().into(),
    }
}

/// Resolves supported CLI external function calls.
///
/// The CLI currently supports only `add_ints(int, int)`, which makes it
/// possible to exercise the suspend/resume path in a deterministic way.
///
/// Returns a runtime-like error string for unknown function names, wrong arity,
/// or incorrect argument types.
fn resolve_external_call(function_name: &str, args: &[MontyObject]) -> Result<MontyObject, String> {
    if function_name != "add_ints" {
        return Err(format!("unknown external function: {function_name}({args:?})"));
    }

    if args.len() != 2 {
        return Err(format!("add_ints requires exactly 2 arguments, got {}", args.len()));
    }

    if let (MontyObject::Int(a), MontyObject::Int(b)) = (&args[0], &args[1]) {
        Ok(MontyObject::Int(a + b))
    } else {
        Err(format!("add_ints requires integer arguments, got {args:?}"))
    }
}

// =============================================================================
// Mount parsing
// =============================================================================

/// Builds a [`MountTable`] from CLI `-m` arguments.
///
/// Returns `None` if no mounts were specified. Fails early with a descriptive
/// error if any mount spec is malformed or the host path doesn't exist.
fn build_mount_table(mount_args: &[String]) -> Result<Option<MountTable>, String> {
    if mount_args.is_empty() {
        return Ok(None);
    }

    let mut table = MountTable::new();
    for arg in mount_args {
        let (host_path, virtual_path, mode, write_bytes_limit) = parse_mount(arg)?;
        table
            .mount(&virtual_path, &host_path, mode, write_bytes_limit)
            .map_err(|e| format!("mount {arg}: {e}"))?;
    }
    Ok(Some(table))
}

/// Parses a single mount specification string.
///
/// Format: `host_path::virtual_path[::mode[::write_limit_bytes]]`
///
/// Uses `::` as the separator to avoid ambiguity with Windows drive letters
/// (e.g., `C:\data::/mnt::rw::1000000`).
///
/// Mode defaults to `ro` (read-only) when omitted. Valid modes:
/// - `ro` — read-only
/// - `rw` — read-write
/// - `overlay` — in-memory copy-on-write overlay
fn parse_mount(spec: &str) -> Result<(String, String, MountMode, Option<u64>), String> {
    let parts: Vec<&str> = spec.split("::").collect();

    let (host_path, virtual_path, mode_str, limit_str) = match parts.len() {
        2 => (parts[0], parts[1], "ro", None),
        3 => (parts[0], parts[1], parts[2], None),
        4 => (parts[0], parts[1], parts[2], Some(parts[3])),
        _ => {
            return Err(format!(
                "invalid mount spec '{spec}': expected host_path::virtual_path[::mode[::write_limit_bytes]]"
            ));
        }
    };

    if host_path.is_empty() || virtual_path.is_empty() {
        return Err(format!(
            "invalid mount spec '{spec}': host and virtual paths must not be empty"
        ));
    }

    let mode = match mode_str {
        "ro" => MountMode::ReadOnly,
        "rw" => MountMode::ReadWrite,
        "overlay" => MountMode::OverlayMemory(OverlayState::new()),
        other => {
            return Err(format!(
                "invalid mount mode '{other}' in '{spec}': expected 'ro', 'rw', or 'overlay'"
            ));
        }
    };

    let write_bytes_limit = match limit_str {
        Some("") => {
            return Err(format!("invalid write limit in '{spec}': value must not be empty"));
        }
        Some(limit) => Some(
            limit
                .parse::<u64>()
                .map_err(|_| format!("invalid write limit '{limit}' in '{spec}': expected a non-negative integer"))?,
        ),
        None => None,
    };

    Ok((host_path.to_owned(), virtual_path.to_owned(), mode, write_bytes_limit))
}

// =============================================================================
// File I/O and formatting utilities
// =============================================================================

/// Reads a Python source file from disk, returning its contents as a string.
///
/// Returns an error message if the path doesn't exist, isn't a file, or can't be read.
fn read_file(file_path: &str) -> Result<String, String> {
    match fs::metadata(file_path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(format!("{file_path} is not a file"));
            }
        }
        Err(err) => {
            return Err(format!("reading {file_path}: {err}"));
        }
    }
    match fs::read_to_string(file_path) {
        Ok(contents) => Ok(contents),
        Err(err) => Err(format!("reading file: {err}")),
    }
}

/// Wrapper around `Duration` that formats with 5 significant digits and an auto-selected unit.
///
/// - `< 1ms` → microseconds, e.g. `123.45μs`
/// - `1ms..1s` → milliseconds, e.g. `12.345ms`
/// - `≥ 1s` → seconds, e.g. `1.2345s`
///
/// The goal is a compact, human-readable duration string that stays consistent in width
/// regardless of whether execution took microseconds or seconds.
struct FormattedDuration(Duration);

impl fmt::Display for FormattedDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let duration = self.0;
        let total_secs = duration.as_secs_f64();

        if total_secs < 1e-3 {
            // Microseconds
            let us = total_secs * 1e6;
            let decimals = sig_digits_after_decimal(us);
            write!(f, "{us:.decimals$}μs")
        } else if total_secs < 1.0 {
            // Milliseconds
            let ms = total_secs * 1e3;
            let decimals = sig_digits_after_decimal(ms);
            write!(f, "{ms:.decimals$}ms")
        } else {
            // Seconds
            let decimals = sig_digits_after_decimal(total_secs);
            write!(f, "{total_secs:.decimals$}s")
        }
    }
}

/// Calculates how many decimal places to show for 5 significant digits.
///
/// Counts the number of digits before the decimal point, then returns `5 - that count`
/// (clamped to 0). For example, `12.345` has 2 digits before the decimal → 3 after = 5 total.
fn sig_digits_after_decimal(value: f64) -> usize {
    let before = if value < 1.0 {
        1
    } else {
        // value is always positive and < 1e6 in practice, so log10 fits in a u32
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let digits = (value.log10().floor() as u32) + 1;
        digits as usize
    };
    5usize.saturating_sub(before)
}

// =============================================================================
// Terminal output
// =============================================================================

/// Whether stderr should carry ANSI styling.
///
/// Resolved by `anstream` from the signals its macros use — is stderr a
/// terminal, `NO_COLOR`, `CLICOLOR{,_FORCE}`, `TERM` — so that what we render
/// ourselves (the type checker's diagnostics) agrees with the styling
/// `eprintln!` puts around it.
fn stderr_styled() -> bool {
    AutoStream::choice(&io::stderr()) != ColorChoice::Never
}

/// The same question for stdout, which is where rustyline writes the REPL
/// prompt. Asking about stderr instead would put escapes in `monty -i > out`.
fn stdout_styled() -> bool {
    AutoStream::choice(&io::stdout()) != ColorChoice::Never
}
