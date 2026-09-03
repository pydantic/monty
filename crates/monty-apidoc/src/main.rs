//! Generates the Rust API reference under `docs/api/rust/`.
//!
//! Run via `make generate-api-docs`. The output is gitignored: CI's
//! `publish-docs` job regenerates it on every push to `main` and publishes it
//! with the rest of `docs/` to the `docs-source` branch that pydantic.dev
//! reads, so the pages cannot drift from the code. Each crate in [`CRATES`]
//! is documented by the pinned nightly rustdoc's JSON output and rendered to
//! one markdown page, built into the mkdocs site and synced to pydantic.dev —
//! so the output must be MDX-safe (signatures only inside fences or
//! backticks) and may not contain broken links (`mkdocs --strict`).

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use rustdoc_types::{Crate, FORMAT_VERSION};

use crate::{render::render_page, symbols::SymbolMap};

mod docs_md;
mod render;
mod sig;
mod symbols;

/// Nightly toolchain whose rustdoc JSON format matches the exact-pinned
/// `rustdoc-types` in Cargo.toml ([`FORMAT_VERSION`]). Bump all three
/// together: this constant, the Cargo.toml pin, and the toolchain installed
/// by CI's `lint` job.
const NIGHTLY: &str = "nightly-2026-08-31";

/// The crates rendered into `docs/api/rust/`, in the order of the mkdocs nav.
const CRATES: &[CrateConfig] = &[
    CrateConfig {
        name: "monty",
        intro: "The in-process interpreter: compile, run, suspend and resume sandboxed Python \
                inside your own process. Most hosts should use [`monty-pool`](monty-pool.md) \
                instead, which keeps a sandbox crash from taking the host process down.",
        render_crate_docs: false,
        // reading order: one-shot runs and their suspensions, the REPL and
        // its suspensions, then session dump/restore and the internal macros
        order: &[
            "MontyRun",
            "RunProgress",
            "FunctionCall",
            "NameLookup",
            "OsCall",
            "ResolveFutures",
            "MontyRepl",
            "ReplProgress",
            "ReplFunctionCall",
            "ReplNameLookup",
            "ReplOsCall",
            "ReplResolveFutures",
            "ReplStartError",
            "ReplContinuationMode",
            "detect_repl_continuation_mode",
            "Session",
            "SessionRef",
            "dump",
            "Dump",
            "DumpError",
            "DUMP_VERSION",
        ],
    },
    CrateConfig {
        name: "monty-pool",
        intro: "An async pool of `monty` worker subprocesses: crash isolation, hard per-turn \
                timeouts and elastic scaling for running untrusted Python. This is the recommended \
                Rust embedding surface — see the [Rust quickstart](../../quickstart/rust.md).",
        render_crate_docs: false,
        order: &[],
    },
    CrateConfig {
        name: "monty-types",
        intro: "The shared boundary types exchanged between hosts and the sandbox: values, \
                exceptions, resource limits and host-call payloads. Host-side code depends on \
                this crate rather than on the interpreter.",
        render_crate_docs: false,
        order: &[],
    },
    CrateConfig {
        name: "monty-fs",
        intro: "Host-side filesystem mounts: a `MountTable` maps real directories into the \
                sandbox at virtual paths and services the sandbox's OS calls.",
        render_crate_docs: true,
        order: &[],
    },
    CrateConfig {
        name: "monty-proto",
        intro: "The wire protocol between pool parents and `monty` workers: the protobuf-generated \
                messages, 4-byte length-prefixed framing, and validated conversions between wire \
                frames and [`monty-types`](monty-types.md) values.",
        render_crate_docs: false,
        order: &[],
    },
    CrateConfig {
        name: "monty-type-checking",
        intro: "Type checking for Monty sessions, powered by [ty](https://github.com/astral-sh/ty): \
                checks code against Monty's trimmed typeshed before execution.",
        render_crate_docs: false,
        order: &[],
    },
];

/// One crate page. Most of the crates use
/// `#![doc = include_str!("../README.md")]` as crate docs, which would
/// duplicate install instructions and crates.io links into the reference —
/// so every page opens with a short hand-written `intro` instead, and
/// `render_crate_docs` includes the crate docs only where they are genuine
/// `//!` module documentation (`monty-fs`).
struct CrateConfig {
    name: &'static str,
    intro: &'static str,
    render_crate_docs: bool,
    /// Explicit reading order for root items: listed names come first, in
    /// this order; everything else keeps source order. A name not found at
    /// the crate root is a generation error (catches renames).
    order: &'static [&'static str],
}

fn main() {
    let workspace_root = workspace_root();
    let crates: Vec<(&CrateConfig, Crate)> = CRATES
        .iter()
        .map(|cfg| (cfg, load_crate(&workspace_root, cfg.name)))
        .collect();
    let named: Vec<(&str, &Crate)> = crates.iter().map(|(cfg, krate)| (cfg.name, krate)).collect();
    let symbols = SymbolMap::build(&named);
    let out_dir = workspace_root.join("docs/api/rust");
    fs::create_dir_all(&out_dir).expect("failed to create docs/api/rust");
    for (cfg, krate) in &crates {
        let page = render_page(cfg, krate, &symbols);
        let out_path = out_dir.join(format!("{}.md", cfg.name));
        fs::write(&out_path, page).expect("failed to write generated page");
        println!("regenerated {}", out_path.display());
    }
}

/// Documents `name` with the pinned nightly rustdoc and parses the JSON.
fn load_crate(workspace_root: &Path, name: &str) -> Crate {
    let status = Command::new("cargo")
        .arg(format!("+{NIGHTLY}"))
        .args([
            "rustdoc",
            "-p",
            name,
            "--lib",
            "--",
            "--output-format",
            "json",
            "-Z",
            "unstable-options",
        ])
        .current_dir(workspace_root)
        .status()
        .expect("failed to run cargo — is rustup on PATH?");
    assert!(
        status.success(),
        "cargo +{NIGHTLY} rustdoc -p {name} failed; if the toolchain is missing, install it with \
         `rustup toolchain install {NIGHTLY} --profile minimal`"
    );
    let json_path = workspace_root
        .join("target/doc")
        .join(format!("{}.json", name.replace('-', "_")));
    let raw =
        fs::read_to_string(&json_path).unwrap_or_else(|err| panic!("failed to read {}: {err}", json_path.display()));
    let value: serde_json::Value = serde_json::from_str(&raw).expect("rustdoc JSON is not valid JSON");
    let format_version = value.get("format_version").and_then(serde_json::Value::as_u64);
    assert_eq!(
        format_version,
        Some(u64::from(FORMAT_VERSION)),
        "rustdoc JSON format version mismatch for {name}: {NIGHTLY} should produce format \
         {FORMAT_VERSION} (the pinned rustdoc-types); bump the nightly pin and rustdoc-types together"
    );
    serde_json::from_value(value).expect("failed to deserialize rustdoc JSON")
}

/// `crates/monty-apidoc` lives two levels below the workspace root.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root not found")
        .to_path_buf()
}
