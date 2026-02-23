use std::{
    fmt, fs,
    process::ExitCode,
    time::{Duration, Instant},
};

use clap::Parser;
use monty::{
    MontyObject, MontyRepl, MontyRun, NoLimitTracker, PrintWriter, ReplContinuationMode, RunProgress,
    detect_repl_continuation_mode,
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
}

const EXT_FUNCTIONS: bool = false;

fn main() -> ExitCode {
    let cli = Cli::parse();

    let type_check_enabled = cli.type_check;

    if let Some(cmd) = cli.command {
        if cli.file.is_some() {
            eprintln!("{BOLD_RED}error{RESET}: cannot specify both -c and a file");
            return ExitCode::FAILURE;
        }
        return if cli.interactive {
            run_repl("<string>", cmd)
        } else {
            run_script("<string>", cmd, type_check_enabled)
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
            run_repl(file_path, code)
        } else {
            run_script(file_path, code, type_check_enabled)
        };
    }

    run_repl("repl.py", String::new())
}

/// Executes a Python file in one-shot CLI mode.
///
/// This path keeps the existing CLI behavior: run type-checking for visibility,
/// compile the file as a full module, and execute it either through direct
/// execution or through the suspendable progress loop when external functions
/// are enabled.
///
/// Returns `ExitCode::SUCCESS` for successful execution and
/// `ExitCode::FAILURE` for parse/type/runtime failures.
fn run_script(file_path: &str, code: String, type_check_enabled: bool) -> ExitCode {
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
    let ext_functions = vec!["add_ints".to_owned()];

    let runner = match MontyRun::new(code, file_path, input_names, ext_functions) {
        Ok(ex) => ex,
        Err(err) => {
            eprintln!("{BOLD_RED}error{RESET}:\n{err}");
            return ExitCode::FAILURE;
        }
    };

    if EXT_FUNCTIONS {
        let start = Instant::now();
        let progress = match runner.start(inputs, NoLimitTracker, &mut PrintWriter::Stdout) {
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

        match run_until_complete(progress) {
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
        let value = match runner.run_no_limits(inputs) {
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
fn run_repl(file_path: &str, code: String) -> ExitCode {
    let input_names = vec![];
    let inputs = vec![];
    let ext_functions = vec!["add_ints".to_owned()];

    let (mut repl, init_output) = match MontyRepl::new(
        code,
        file_path,
        input_names,
        ext_functions,
        inputs,
        NoLimitTracker,
        &mut PrintWriter::Stdout,
    ) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("{BOLD_RED}error{RESET} initializing repl:\n{err}");
            return ExitCode::FAILURE;
        }
    };

    if init_output != MontyObject::None {
        println!("{init_output}");
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
            execute_repl_snippet(&mut repl, &pending_snippet);
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
                execute_repl_snippet(&mut repl, &pending_snippet);
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
fn execute_repl_snippet(repl: &mut MontyRepl<NoLimitTracker>, snippet: &str) {
    match repl.feed_no_print(snippet) {
        Ok(output) => {
            if output != MontyObject::None {
                println!("{output}");
            }
        }
        Err(err) => {
            eprintln!("{BOLD_RED}error{RESET}: {err}");
        }
    }
}

/// Drives suspendable execution until completion.
///
/// This repeatedly resumes `RunProgress` values by resolving supported
/// external calls and returns the final value when execution reaches
/// `RunProgress::Complete`.
///
/// Returns an error string for unsupported suspend points (OS calls or async
/// futures) or invalid external-function dispatch.
fn run_until_complete(mut progress: RunProgress<NoLimitTracker>) -> Result<MontyObject, String> {
    loop {
        match progress {
            RunProgress::Complete(value) => return Ok(value),
            RunProgress::FunctionCall {
                function_name,
                args,
                state,
                ..
            } => {
                let return_value = resolve_external_call(&function_name, &args)?;
                progress = state
                    .run(return_value, &mut PrintWriter::Stdout)
                    .map_err(|err| format!("{err}"))?;
            }
            RunProgress::ResolveFutures(state) => {
                return Err(format!(
                    "async futures not supported in CLI: {:?}",
                    state.pending_call_ids()
                ));
            }
            RunProgress::OsCall { function, args, .. } => {
                return Err(format!("OS calls not supported in CLI: {function:?}({args:?})"));
            }
        }
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
