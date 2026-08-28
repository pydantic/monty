//! Exercises an installed, instrumented `monty` binary for PGO training.

use std::{
    env,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use monty_pool::{Checkout, OnPrint, Pool, PoolConfig, PoolError, ReplConfig, ResumeValue, TurnEvent, on_print_sync};
use monty_types::MontyObject;

/// Runs Monty's test-case corpus through subprocess pool sessions.
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let binary = find_monty_binary()?;
    println!("Training monty runtime at {}", binary.display());

    let pool = Pool::new(PoolConfig::subprocess(binary)).await?;
    let mut test_cases = test_cases()?;
    test_cases.sort();

    let mut completed = 0;
    let mut typing_errors = 0;
    for test_case in &test_cases {
        let code = fs::read_to_string(test_case)?;
        let mut session = pool
            .checkout(&ReplConfig {
                script_name: test_case
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("main.py")
                    .to_owned(),
                type_check: true,
                ..ReplConfig::default()
            })
            .await?;
        let mut on_print = on_print_sync(|_, _| {});

        match run_code(&mut session, &code, false, &mut on_print).await {
            Ok(()) => {
                completed += 1;
                session.finish().await?;
            }
            Err(PoolError::Typing(_)) => {
                typing_errors += 1;
                match run_code(&mut session, &code, true, &mut on_print).await {
                    Ok(()) => {
                        completed += 1;
                        session.finish().await?;
                    }
                    Err(PoolError::Runtime(_)) => session.finish().await?,
                    Err(error) => return Err(error.into()),
                }
            }
            Err(PoolError::Runtime(_)) => session.finish().await?,
            Err(error) => return Err(error.into()),
        }
    }

    // A clean shutdown lets instrumented workers flush their `.profraw` data.
    // Dropping the pool kills idle workers, which can discard buffered profiles.
    pool.close().await;
    println!(
        "Exercised {} test cases, {completed} completed, {typing_errors} retried without type checking",
        test_cases.len()
    );
    Ok(())
}

/// Runs a snippet through completion, answering every suspension with fixture-like values.
async fn run_code(
    session: &mut Checkout,
    code: &str,
    skip_type_check: bool,
    on_print: OnPrint<'_>,
) -> Result<(), PoolError> {
    let mut event = session
        .feed(code, vec![], vec![], skip_type_check, &mut *on_print)
        .await?;
    loop {
        event = match event {
            TurnEvent::Complete(_) => break Ok(()),
            TurnEvent::FunctionCall { .. } => {
                session
                    .resume(ResumeValue::Return(MontyObject::None), &mut *on_print)
                    .await?
            }
            TurnEvent::OsCall { .. } => match session.resume_from_mounts(&mut *on_print).await? {
                Some(event) => event,
                None => session.resume(ResumeValue::NotHandled, &mut *on_print).await?,
            },
            TurnEvent::NameLookup { name } => {
                session
                    .resume_name_lookup(name_lookup_value(name), &mut *on_print)
                    .await?
            }
            TurnEvent::ResolveFutures { pending_call_ids } => {
                let results = pending_call_ids
                    .into_iter()
                    .map(|call_id| (call_id, ResumeValue::Return(MontyObject::None)))
                    .collect();
                session.resume_futures(results, &mut *on_print).await?
            }
        };
    }
}

/// Returns representative host values for names used by the shared test corpus.
fn name_lookup_value(name: String) -> Option<MontyObject> {
    match name.as_str() {
        "add_ints" | "concat_strings" | "return_value" | "get_list" | "raise_error" | "make_point"
        | "make_mutable_point" | "make_user" | "make_empty" | "async_call" | "async_fail" => {
            Some(MontyObject::Function { name, docstring: None })
        }
        "CONST_INT" => Some(MontyObject::Int(42)),
        "CONST_STR" => Some(MontyObject::String("hello".to_owned())),
        #[expect(clippy::approx_constant, reason = "3.14 is the test fixture value")]
        "CONST_FLOAT" => Some(MontyObject::Float(3.14)),
        "CONST_BOOL" => Some(MontyObject::Bool(true)),
        "CONST_LIST" => Some(MontyObject::List(vec![
            MontyObject::Int(1),
            MontyObject::Int(2),
            MontyObject::Int(3),
        ])),
        "CONST_NONE" => Some(MontyObject::None),
        "root" => Some(MontyObject::Path("/mnt".to_owned())),
        _ => None,
    }
}

/// Resolves `monty` from the environment maturin prepares for PGO training.
fn find_monty_binary() -> io::Result<PathBuf> {
    let path = env::var_os("PATH").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "PATH is not set"))?;
    let executable = if cfg!(windows) { "monty.exe" } else { "monty" };
    env::split_paths(&path)
        .map(|directory| directory.join(executable))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.canonicalize().unwrap_or(candidate))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{executable} was not found on PATH")))
}

/// Lists the shared interpreter test cases from the workspace checkout.
fn test_cases() -> io::Result<Vec<PathBuf>> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("monty-pool should be inside the workspace crates directory")
        .join("monty/test_cases");
    fs::read_dir(directory)?
        .filter_map(|entry| match entry {
            Ok(entry) if entry.path().extension().is_some_and(|extension| extension == "py") => Some(Ok(entry.path())),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}
