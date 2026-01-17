use std::{env, fs, process::ExitCode, time::Instant};

use monty::{MontyObject, MontyRun, NoLimitTracker, RunProgress, StdPrint};
use monty_type_checking::type_check;

const EXT_FUNCTIONS: bool = false;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let file_path = if args.len() > 1 { &args[1] } else { "example.py" };
    let code = match read_file(file_path) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let start = Instant::now();
    if let Some(failure) = type_check(&code, file_path).unwrap() {
        eprintln!("type checking failed: {failure}");
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
        let mut progress = match runner.start(inputs, NoLimitTracker, &mut StdPrint) {
            Ok(p) => p,
            Err(err) => {
                let elapsed = start.elapsed();
                eprintln!("error after: {elapsed:?}\n{err}");
                return ExitCode::FAILURE;
            }
        };

        // Handle external function calls in a loop
        loop {
            match progress {
                RunProgress::Complete(value) => {
                    let elapsed = start.elapsed();
                    eprintln!("success after: {elapsed:?}\n{value}");
                    return ExitCode::SUCCESS;
                }
                RunProgress::FunctionCall {
                    function_name,
                    args,
                    state,
                    ..
                } => {
                    let return_value = if function_name == "add_ints" {
                        // Extract two integer arguments and add them
                        if args.len() != 2 {
                            eprintln!("add_ints requires exactly 2 arguments, got {}", args.len());
                            return ExitCode::FAILURE;
                        }
                        if let (MontyObject::Int(a), MontyObject::Int(b)) = (&args[0], &args[1]) {
                            let ret = MontyObject::Int(a + b);
                            eprintln!("Function call: {function_name}({args:?}) -> {ret:?}");
                            ret
                        } else {
                            eprintln!("add_ints requires integer arguments, got {args:?}");
                            return ExitCode::FAILURE;
                        }
                    } else {
                        let elapsed = start.elapsed();
                        eprintln!("{elapsed:?}, unknown external function: {function_name}({args:?})");
                        return ExitCode::FAILURE;
                    };

                    // Resume execution with the return value
                    match state.run(return_value, &mut StdPrint) {
                        Ok(p) => progress = p,
                        Err(err) => {
                            let elapsed = start.elapsed();
                            eprintln!("error after: {elapsed:?}\n{err}");
                            return ExitCode::FAILURE;
                        }
                    }
                }
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
