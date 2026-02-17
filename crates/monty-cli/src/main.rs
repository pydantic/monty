use std::{
    fs,
    io::{self, BufRead, Write},
    process::ExitCode,
    time::Instant,
};

use clap::Parser;
use monty::{
    MontyObject, MontyRepl, MontyRun, NoLimitTracker, PrintWriter, ReplContinuationMode, RunProgress,
    detect_repl_continuation_mode,
};
// disabled due to format failing on https://github.com/pydantic/monty/pull/75 where CI and local wanted imports ordered differently
// TODO re-enabled soon!
#[rustfmt::skip]
use monty_type_checking::{SourceFile, type_check};

/// Monty — a sandboxed Python interpreter written in Rust.
///
/// - `monty` runs `example.py`
/// - `monty <file>` runs the file in script mode
/// - `monty -i` starts an empty interactive REPL
/// - `monty -i <file>` seeds the REPL with file contents
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Start interactive REPL mode.
    #[arg(short = 'i', long = "interactive")]
    interactive: bool,

    /// Python file to execute.
    file: Option<String>,
}

const EXT_FUNCTIONS: bool = false;

fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Some(file_path) = cli.file.as_deref() {
        let code = match read_file(file_path) {
            Ok(code) => code,
            Err(err) => {
                eprintln!("error: {err}");
                return ExitCode::FAILURE;
            }
        };
        return if cli.interactive {
            run_repl(file_path, code)
        } else {
            run_script(file_path, code)
        };
    }

    if cli.interactive {
        return run_repl("repl.py", String::new());
    }

    let file_path = "example.py";
    let code = match read_file(file_path) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    run_script(file_path, code)
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
fn run_script(file_path: &str, code: String) -> ExitCode {
    let start = Instant::now();
    if let Some(failure) = type_check(&SourceFile::new(&code, file_path), None).unwrap() {
        eprintln!("type checking failed:\n{failure}");
    } else {
        eprintln!("type checking succeeded");
    }
    let elapsed = start.elapsed();
    println!("time taken to run typing: {elapsed:?}");

    let input_names = vec![];
    let inputs = vec![];
    let ext_functions = vec!["add_ints".to_owned()];

    let runner = match MontyRun::new(code, file_path, input_names, ext_functions) {
        Ok(ex) => ex,
        Err(err) => {
            eprintln!("error:\n{err}");
            return ExitCode::FAILURE;
        }
    };

    if EXT_FUNCTIONS {
        let start = Instant::now();
        let progress = match runner.start(inputs, NoLimitTracker, &mut PrintWriter::Stdout) {
            Ok(p) => p,
            Err(err) => {
                let elapsed = start.elapsed();
                eprintln!("error after: {elapsed:?}\n{err}");
                return ExitCode::FAILURE;
            }
        };

        match run_until_complete(progress) {
            Ok(value) => {
                let elapsed = start.elapsed();
                eprintln!("success after: {elapsed:?}\n{value}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                let elapsed = start.elapsed();
                eprintln!("error after: {elapsed:?}\n{err}");
                ExitCode::FAILURE
            }
        }
    } else {
        let start = Instant::now();
        let value = match runner.run_no_limits(inputs) {
            Ok(p) => p,
            Err(err) => {
                let elapsed = start.elapsed();
                eprintln!("error after: {elapsed:?}\n{err}");
                return ExitCode::FAILURE;
            }
        };
        let elapsed = start.elapsed();
        eprintln!("success after: {elapsed:?}\n{value}");
        ExitCode::SUCCESS
    }
}

/// Starts an interactive line-by-line REPL session.
///
/// Initializes `MontyRepl` once and incrementally feeds entered snippets without
/// replaying previous snippets, which matches the intended stateful REPL model.
/// Multiline input follows CPython-style prompts:
/// - `>>> ` for a new statement
/// - `... ` for continuation
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
            eprintln!("error initializing repl:\n{err}");
            return ExitCode::FAILURE;
        }
    };

    if init_output != MontyObject::None {
        println!("{init_output}");
    }

    eprintln!("Monty REPL mode. Enter Python snippets. Use exit to exit.");
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut pending_snippet = String::new();
    let mut continuation_mode = ReplContinuationMode::Complete;

    loop {
        let prompt = if continuation_mode == ReplContinuationMode::Complete {
            ">>> "
        } else {
            "... "
        };
        print!("{prompt}");
        if io::stdout().flush().is_err() {
            eprintln!("error: failed to flush stdout");
            return ExitCode::FAILURE;
        }

        let mut line = String::new();
        let read = match stdin.read_line(&mut line) {
            Ok(n) => n,
            Err(err) => {
                eprintln!("error reading input: {err}");
                return ExitCode::FAILURE;
            }
        };

        if read == 0 {
            return ExitCode::SUCCESS;
        }

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

/// Executes one collected REPL snippet and prints value/errors for interactive use.
fn execute_repl_snippet(repl: &mut MontyRepl<NoLimitTracker>, snippet: &str) {
    match repl.feed_no_print(snippet) {
        Ok(output) => {
            if output != MontyObject::None {
                println!("{output}");
            }
        }
        Err(err) => eprintln!("error:\n{err}"),
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

fn read_file(file_path: &str) -> Result<String, String> {
    eprintln!("Reading file: {file_path}");
    match fs::metadata(file_path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(format!("Error: {file_path} is not a file"));
            }
        }
        Err(err) => {
            return Err(format!("Error reading {file_path}: {err}"));
        }
    }
    match fs::read_to_string(file_path) {
        Ok(contents) => Ok(contents),
        Err(err) => Err(format!("Error reading file: {err}")),
    }
}
