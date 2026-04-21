use std::{
    fmt, fs,
    process::ExitCode,
    time::{Duration, Instant},
};

use clap::Parser;
use monty::{
    LimitedTracker, MontyObject, MontyRepl, MontyRun, NameLookupResult, NoLimitTracker, PrintWriter,
    ReplContinuationMode, ReplProgress, ResourceLimits, ResourceTracker, RunProgress, detect_repl_continuation_mode,
    fs::{MountMode, MountTable, OverlayState},
};
use rustyline::{DefaultEditor, error::ReadlineError};
// disabled due to format failing on https://github.com/pydantic/monty/pull/75 where CI and local wanted imports ordered differently
// TODO re-enabled soon!
#[rustfmt::skip]
use monty_type_checking::{SourceFile, type_check};

/// ANSI escape code for dim/gray text.
const DIM: &str = "\x1b[2m";
/// ANSI escape code for bold red text (errors).
const BOLD_RED: &str = "\x1b[1m\x1b[31m";
/// ANSI escape code for bold green text (success, headings).
const BOLD_GREEN: &str = "\x1b[1m\x1b[32m";
/// ANSI escape code for bold cyan text (commands, prompts).
const BOLD_CYAN: &str = "\x1b[1m\x1b[36m";
/// ANSI escape code to reset all text styling.
const RESET: &str = "\x1b[0m";
const ARROW: &str = "❯";

/// Monty — a sandboxed Python interpreter written in Rust.
///
/// - `monty` starts an empty interactive REPL
/// - `monty <file>` runs the file in script mode
/// - `monty -c <cmd>` executes `<cmd>` as a Python program
/// - `monty -i` starts an empty interactive REPL
/// - `monty -i <file>` seeds the REPL with file contents
/// - `monty -m host::virtual[::mode[::write_limit_bytes]]` mounts a directory into the sandbox
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Start interactive REPL mode.
    #[arg(short = 'i', long = "interactive")]
    interactive: bool,

    /// Run the type checker before executing.
    #[arg(short = 't', long = "type-check")]
    type_check: bool,

    /// Execute a Python program passed as a string (like `python -c`).
    #[arg(short = 'c')]
    command: Option<String>,

    /// Python file to execute.
    file: Option<String>,

    /// Mount a host directory into the sandbox.
    ///
    /// Format: `/host/path::/virtual/path[::mode[::write_limit_bytes]]`
    ///
    /// Uses `::` as separator to avoid ambiguity with Windows drive letters.
    /// Modes: `ro` (read-only, default), `rw` (read-write), `overlay` (in-memory overlay).
    /// `write_limit_bytes` is optional and applies to all write modes.
    #[arg(short = 'm', long = "mount")]
    mounts: Vec<String>,

    /// Maximum number of heap allocations before execution is terminated.
    #[arg(long)]
    max_allocations: Option<usize>,

    /// Maximum execution time in seconds (e.g. `0.5` for 500ms).
    #[arg(long)]
    max_duration: Option<f64>,

    /// Maximum heap memory (e.g. `1024`, `512KB`, `10MB`, `1GB`).
    #[arg(long, value_parser = parse_memory_size)]
    max_memory: Option<usize>,

    /// Run garbage collection every N allocations.
    #[arg(long)]
    gc_interval: Option<usize>,

    /// Maximum call-stack depth (defaults to 1000 when any limit is set).
    #[arg(long)]
    max_recursion_depth: Option<usize>,
}

impl Cli {
    /// Builds `ResourceLimits` from the parsed CLI arguments.
    ///
    /// Returns `Ok(None)` when no resource flags were provided, which lets the
    /// caller fall back to `NoLimitTracker` for zero-overhead execution.
    /// Returns `Err` if a supplied flag cannot be converted into a valid limit.
    fn resource_limits(&self) -> Result<Option<ResourceLimits>, String> {
        if self.max_allocations.is_none()
            && self.max_duration.is_none()
            && self.max_memory.is_none()
            && self.gc_interval.is_none()
            && self.max_recursion_depth.is_none()
        {
            return Ok(None);
        }

        let mut limits = ResourceLimits::new();
        if let Some(n) = self.max_allocations {
            limits = limits.max_allocations(n);
        }
        if let Some(secs) = self.max_duration {
            limits = limits.max_duration(
                Duration::try_from_secs_f64(secs).map_err(|err| format!("invalid --max-duration: {err}"))?,
            );
        }
        if let Some(bytes) = self.max_memory {
            limits = limits.max_memory(bytes);
        }
        if let Some(interval) = self.gc_interval {
            limits = limits.gc_interval(interval);
        }
        if let Some(depth) = self.max_recursion_depth {
            limits = limits.max_recursion_depth(Some(depth));
        }
        Ok(Some(limits))
    }
}

const EXT_FUNCTIONS: bool = false;

fn main() -> ExitCode {
    let cli = Cli::parse();

    let type_check_enabled = cli.type_check;

    let limits = match cli.resource_limits() {
        Ok(limits) => limits,
        Err(err) => {
            eprintln!("{BOLD_RED}error{RESET}: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Build mount table early to fail fast on bad -m args.
    let mount_table = match build_mount_table(&cli.mounts) {
        Ok(mt) => mt,
        Err(err) => {
            eprintln!("{BOLD_RED}error{RESET}: {err}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(cmd) = cli.command {
        if cli.file.is_some() {
            eprintln!("{BOLD_RED}error{RESET}: cannot specify both -c and a file");
            return ExitCode::FAILURE;
        }
        return if cli.interactive {
            dispatch_repl("<string>", &cmd, limits, mount_table)
        } else {
            dispatch_script("<string>", cmd, type_check_enabled, limits, mount_table)
        };
    }

    if let Some(file_path) = cli.file.as_deref() {
        let code = match read_file(file_path) {
            Ok(code) => code,
            Err(err) => {
                eprintln!("{BOLD_RED}error{RESET}: {err}");
                return ExitCode::FAILURE;
            }
        };
        return if cli.interactive {
            dispatch_repl(file_path, &code, limits, mount_table)
        } else {
            dispatch_script(file_path, code, type_check_enabled, limits, mount_table)
        };
    }

    dispatch_repl("repl.py", "", limits, mount_table)
}

/// Dispatches script execution with either `LimitedTracker` or `NoLimitTracker`.
///
/// This top-level branch avoids threading generics through the entire call chain
/// while still keeping the zero-overhead `NoLimitTracker` path when no limits are set.
fn dispatch_script(
    file_path: &str,
    code: String,
    type_check_enabled: bool,
    limits: Option<ResourceLimits>,
    mount_table: Option<MountTable>,
) -> ExitCode {
    if let Some(limits) = limits {
        run_script(
            file_path,
            code,
            type_check_enabled,
            LimitedTracker::new(limits),
            mount_table,
        )
    } else {
        run_script(file_path, code, type_check_enabled, NoLimitTracker, mount_table)
    }
}

/// Dispatches REPL startup with either `LimitedTracker` or `NoLimitTracker`.
fn dispatch_repl(
    file_path: &str,
    code: &str,
    limits: Option<ResourceLimits>,
    mount_table: Option<MountTable>,
) -> ExitCode {
    if let Some(limits) = limits {
        run_repl(file_path, code, LimitedTracker::new(limits), mount_table)
    } else {
        run_repl(file_path, code, NoLimitTracker, mount_table)
    }
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
    type_check_enabled: bool,
    tracker: impl ResourceTracker,
    mut mount_table: Option<MountTable>,
) -> ExitCode {
    if type_check_enabled {
        let start = Instant::now();
        if let Some(failure) = type_check(&SourceFile::new(&code, file_path), None).unwrap() {
            let elapsed = start.elapsed();
            eprintln!(
                "{DIM}{}{RESET} {BOLD_CYAN}{ARROW}{RESET} {BOLD_RED}type check failed{RESET}:\n{failure}",
                FormattedDuration(elapsed)
            );
        } else {
            let elapsed = start.elapsed();
            eprintln!(
                "{DIM}{}{RESET} {BOLD_CYAN}{ARROW}{RESET} {BOLD_GREEN}type check passed{RESET}",
                FormattedDuration(elapsed)
            );
        }
    }

    let input_names = vec![];
    let inputs = vec![];

    let runner = match MontyRun::new(code, file_path, input_names) {
        Ok(ex) => ex,
        Err(err) => {
            eprintln!("{BOLD_RED}error{RESET}:\n{err}");
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
                    "{DIM}{}{RESET} {BOLD_CYAN}{ARROW}{RESET} {BOLD_RED}error{RESET}: {err}",
                    FormattedDuration(elapsed)
                );
                return ExitCode::FAILURE;
            }
        };

        match run_until_complete(progress, &mut mount_table) {
            Ok(value) => {
                let elapsed = start.elapsed();
                eprintln!(
                    "{DIM}{}{RESET} {BOLD_CYAN}{ARROW}{RESET} {value}",
                    FormattedDuration(elapsed)
                );
                ExitCode::SUCCESS
            }
            Err(err) => {
                let elapsed = start.elapsed();
                eprintln!(
                    "{DIM}{}{RESET} {BOLD_CYAN}{ARROW}{RESET} {BOLD_RED}error{RESET}: {err}",
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
                    "{DIM}{}{RESET} {BOLD_CYAN}{ARROW}{RESET} {BOLD_RED}error{RESET}: {err}",
                    FormattedDuration(elapsed)
                );
                return ExitCode::FAILURE;
            }
        };
        let elapsed = start.elapsed();
        eprintln!(
            "{DIM}{}{RESET} {BOLD_CYAN}{ARROW}{RESET} {value}",
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
fn run_repl(
    file_path: &str,
    code: &str,
    tracker: impl ResourceTracker,
    mut mount_table: Option<MountTable>,
) -> ExitCode {
    let mut repl = Some(MontyRepl::new(file_path, tracker));

    if !code.is_empty() {
        execute_repl_snippet(&mut repl, code, &mut mount_table);
    }

    eprintln!("Monty v{} REPL. Type `exit` to exit.", env!("CARGO_PKG_VERSION"));

    let mut rl = match DefaultEditor::new() {
        Ok(rl) => rl,
        Err(err) => {
            eprintln!("{BOLD_RED}error{RESET} initializing editor: {err}");
            return ExitCode::FAILURE;
        }
    };

    let mut pending_snippet = String::new();
    let mut continuation_mode = ReplContinuationMode::Complete;

    loop {
        let prompt = if continuation_mode == ReplContinuationMode::Complete {
            format!("{BOLD_CYAN}{ARROW}{RESET} ")
        } else {
            "… ".to_owned()
        };

        let line = match rl.readline(&prompt) {
            Ok(line) => line,
            Err(ReadlineError::Eof) => return ExitCode::SUCCESS,
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C: discard pending input and start fresh
                pending_snippet.clear();
                continuation_mode = ReplContinuationMode::Complete;
                continue;
            }
            Err(err) => {
                eprintln!("{BOLD_RED}error{RESET} reading input: {err}");
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
            execute_repl_snippet(&mut repl, &pending_snippet, &mut mount_table);
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
                execute_repl_snippet(&mut repl, &pending_snippet, &mut mount_table);
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
/// Takes `&mut Option<MontyRepl<T>>` because `feed_start` consumes the repl —
/// we `take()` it out, run to completion, then put it back.
fn execute_repl_snippet(
    repl: &mut Option<MontyRepl<impl ResourceTracker>>,
    snippet: &str,
    mount_table: &mut Option<MountTable>,
) {
    let r = repl.take().expect("repl must be present");

    if mount_table.is_some() {
        match execute_repl_with_mounts(r, snippet, mount_table) {
            Ok((returned_repl, output)) => {
                if output != MontyObject::None {
                    println!("{output}");
                }
                *repl = Some(returned_repl);
            }
            Err((returned_repl, err)) => {
                eprintln!("{BOLD_RED}error{RESET}: {err}");
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
                eprintln!("{BOLD_RED}error{RESET}: {err}");
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
fn execute_repl_with_mounts<T: ResourceTracker>(
    r: MontyRepl<T>,
    snippet: &str,
    mount_table: &mut Option<MountTable>,
) -> Result<(MontyRepl<T>, MontyObject), (MontyRepl<T>, String)> {
    let mut progress = match r.feed_start(snippet, vec![], PrintWriter::Stdout) {
        Ok(p) => p,
        Err(err) => return Err((err.repl, format!("{}", err.error))),
    };

    loop {
        match progress {
            ReplProgress::Complete { repl, value } => return Ok((repl, value)),
            ReplProgress::OsCall(call) => {
                let result = handle_os_call(call.function, &call.args, &call.kwargs, mount_table);
                match call.resume(result, PrintWriter::Stdout) {
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
    mut progress: RunProgress<impl ResourceTracker>,
    mount_table: &mut Option<MountTable>,
) -> Result<MontyObject, String> {
    loop {
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
                let result = handle_os_call(call.function, &call.args, &call.kwargs, mount_table);
                progress = call
                    .resume(result, PrintWriter::Stdout)
                    .map_err(|err| format!("{err}"))?;
            }
        }
    }
}

/// Handles a filesystem `OsCall` using the mount table if available.
///
/// Returns the operation result as an `ExtFunctionResult` — either a successful
/// `MontyObject` or an exception for errors / unsupported operations.
fn handle_os_call(
    function: monty::OsFunction,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    mount_table: &mut Option<MountTable>,
) -> monty::ExtFunctionResult {
    if let Some(mounts) = mount_table.as_mut() {
        match mounts.handle_os_call(function, args, kwargs) {
            Some(Ok(obj)) => obj.into(),
            Some(Err(err)) => err.into_exception().into(),
            None => function.on_no_handler(args).into(),
        }
    } else {
        function.on_no_handler(args).into()
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

/// Parses a memory size string with optional unit suffix.
///
/// Accepts plain byte counts (`1024`) or values with a case-insensitive suffix:
/// `KB` (kilobytes), `MB` (megabytes), `GB` (gigabytes). The numeric part must
/// be a valid `usize`.
///
/// # Examples
///
/// - `"512"` → 512
/// - `"512KB"` → 524_288
/// - `"10MB"` → 10_485_760
/// - `"1GB"` → 1_073_741_824
fn parse_memory_size(s: &str) -> Result<usize, String> {
    let s = s.trim();
    let (num_str, multiplier) = if let Some(n) = s.strip_suffix("GB").or_else(|| s.strip_suffix("gb")) {
        (n.trim(), 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("MB").or_else(|| s.strip_suffix("mb")) {
        (n.trim(), 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("KB").or_else(|| s.strip_suffix("kb")) {
        (n.trim(), 1024)
    } else {
        (s, 1)
    };

    let value: usize = num_str.parse().map_err(|e| format!("invalid memory size '{s}': {e}"))?;

    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("memory size '{s}' overflows"))
}
