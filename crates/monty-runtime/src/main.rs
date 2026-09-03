#![doc = include_str!("../README.md")]

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use monty_types::TypeCheckingFormat;

#[cfg(feature = "standalone")]
mod run;
mod subprocess;

/// Bounds worker memory and classifies allocation failure as an exit code
/// rather than an abort, so the pool can report `MemoryError`. Declared here
/// because only a binary may; see the `monty-alloc` crate. Applies to every
/// mode, `subprocess` and CLI alike.
#[global_allocator]
static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

/// Monty — a sandboxed Python interpreter written in Rust.
///
/// Run `monty` to start an empty interactive REPL. Run a python file with `monty <file>`.
/// Execute a command with `monty -c <cmd>`.
#[derive(Parser)]
#[command(version)]
pub(crate) struct Cli {
    /// Start interactive REPL mode, this is the default if no file or command is provided.
    #[arg(short = 'i', long = "interactive")]
    interactive: bool,

    /// Run the type checker before executing.
    #[arg(short = 't', long = "type-check")]
    type_check: bool,

    /// Rendering of the type checker's diagnostics: full (default), concise,
    /// azure, json, jsonlines, rdjson, pylint, gitlab or github.
    #[arg(long = "type-check-format", value_parser = TypeCheckingFormat::from_name, requires = "type_check")]
    type_check_format: Option<TypeCheckingFormat>,

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
    ///
    /// WARNING: with `rw`, files written by sandboxed code persist on the
    /// host, where your own tools may later execute them. Prefer `overlay`.
    #[arg(short = 'm', long = "mount")]
    mounts: Vec<String>,

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

    /// Maximum suspensions serviced in one CLI session (defaults to 1000).
    #[arg(long)]
    max_suspensions: Option<usize>,

    #[command(subcommand)]
    subcommand: Option<Command>,
}

/// Subcommands that switch `monty` out of its normal file/REPL behaviour.
#[derive(Subcommand)]
enum Command {
    /// Run as a protocol child: read framed protobuf requests on stdin and
    /// write framed events on stdout (see the monty-proto crate). Intended to
    /// be driven by a parent process such as monty-pool, not by hand.
    Subprocess,
}

impl Cli {
    /// The name of the first execution flag that conflicts with the `subprocess`
    /// subcommand, if any. `subprocess` reads all its configuration from the
    /// protocol, so any normal-execution flag passed alongside it is a
    /// misconfiguration we reject rather than silently ignore.
    fn subprocess_conflict(&self) -> Option<&'static str> {
        if self.interactive {
            Some("--interactive")
        } else if self.command.is_some() {
            Some("-c")
        } else if self.file.is_some() {
            Some("a file argument")
        } else if self.type_check {
            Some("--type-check")
        } else if !self.mounts.is_empty() {
            Some("--mount")
        } else if self.any_resource_limit_flag() {
            Some("a resource-limit flag")
        } else {
            None
        }
    }

    /// Whether any resource-limit flag was *supplied* (regardless of whether its
    /// value is valid). Used for the `subprocess` conflict check: we must not go
    /// through `resource_limits()` there, because its parse errors (e.g. a
    /// `--max-duration` that fails `std::time::Duration::try_from_secs_f64`) would be
    /// swallowed and let an invalid flag slip past the conflict guard.
    fn any_resource_limit_flag(&self) -> bool {
        self.max_duration.is_some()
            || self.max_memory.is_some()
            || self.gc_interval.is_some()
            || self.max_recursion_depth.is_some()
            || self.max_suspensions.is_some()
    }

    /// Builds `ResourceLimits` from the parsed CLI arguments.
    ///
    /// When no resource flags were provided, returns the default
    /// recursion-only limits (`ResourceLimits::default()`).
    /// Returns `Err` if a supplied flag cannot be converted into a valid limit.
    #[cfg(feature = "standalone")]
    fn resource_limits(&self) -> Result<monty_types::ResourceLimits, String> {
        let mut limits = monty_types::ResourceLimits::default();
        if let Some(secs) = self.max_duration {
            limits = limits.max_duration(
                #[expect(clippy::absolute_paths)]
                std::time::Duration::try_from_secs_f64(secs).map_err(|err| format!("invalid --max-duration: {err}"))?,
            );
        }
        if let Some(bytes) = self.max_memory {
            limits = limits.max_memory(bytes);
        }
        if let Some(interval) = self.gc_interval {
            limits = limits.gc_interval(interval);
        }
        if let Some(depth) = self.max_recursion_depth {
            limits = limits.max_recursion_depth(depth);
        }
        if let Some(max) = self.max_suspensions {
            limits = limits.max_suspensions(max);
        }
        Ok(limits)
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Some(Command::Subprocess) = cli.subcommand {
        if let Some(flag) = cli.subprocess_conflict() {
            eprintln!("error: `subprocess` cannot be combined with {flag}");
            ExitCode::FAILURE
        } else {
            subprocess::run()
        }
    } else {
        run_standalone(cli)
    }
}

/// Hands a non-`subprocess` invocation to the standalone CLI.
#[cfg(feature = "standalone")]
fn run_standalone(cli: Cli) -> ExitCode {
    run::run(cli)
}

/// Without `standalone` this binary is a worker and nothing else. The flags still
/// parse — the `subprocess` conflict check reads them — so the refusal has to
/// happen here rather than in clap.
#[cfg(not(feature = "standalone"))]
fn run_standalone(_cli: Cli) -> ExitCode {
    eprintln!("error: this build runs `monty subprocess` only — rebuild with the `standalone` feature for the CLI");
    ExitCode::FAILURE
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
