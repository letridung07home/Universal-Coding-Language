//! Command-line entry point for the `ucl` binary.
//!
//! This module provides the CLI interface for evaluating UCL source files and
//! for interactive sessions. It orchestrates the compiler pipeline
//! (lexer → parser → evaluator) and handles diagnostic formatting and output.

use std::env;
use std::fs;
use std::process::ExitCode;

use ucl::{DiagnosticSink, Environment, Evaluator, Lexer, Parser, SourceFile};

mod render;
mod repl;

use render::{format_value, render_diagnostics};

/// Usage text printed on argument errors and `--help`.
const USAGE: &str = "usage: ucl [-p <dir>]... [<file>]";

/// The environment variable holding module search directories, separated by
/// the platform's path separator (`:` on Unix, `;` on Windows).
const SEARCH_PATH_ENV: &str = "UCL_PATH";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    let (file, mut search_paths) = match parse_args(&args) {
        Ok(Some(parsed)) => parsed,
        // The REPL, `--help`, and `--version` handle their own output.
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    // `-p/--path` flags are consulted before any `UCL_PATH` directories:
    // explicit flags are the more specific configuration.
    for dir in env_search_paths() {
        search_paths.push(dir);
    }

    let contents = match fs::read_to_string(&file) {
        Ok(contents) => contents,
        Err(error) => {
            eprintln!("error: cannot read `{file}`: {error}");
            return ExitCode::from(2);
        }
    };

    let source = SourceFile::new(file, contents);
    let mut sink = DiagnosticSink::new();

    // Run the pipeline: source text → tokens → AST → value.
    //
    // Each stage runs only if every earlier stage succeeded, so a program
    // with lexical errors is never parsed and one with syntax errors is
    // never evaluated. This keeps diagnostics focused on the root cause
    // instead of cascading through downstream stages fed garbage input.
    let mut environment = Environment::new();
    for dir in &search_paths {
        environment.add_search_path(dir);
    }

    let mut value = None;
    let tokens = Lexer::new(&source).tokenize(&mut sink);
    if !sink.has_errors()
        && let Some(ast) = Parser::new(tokens).parse(&mut sink)
        && !sink.has_errors()
    {
        value = Evaluator::new().evaluate_in(&mut environment, &ast, &source, &mut sink);
    }

    render_diagnostics(&sink, &source);

    match value {
        Some(value) => {
            if let Some(text) = format_value(&value) {
                println!("{text}");
            }
            ExitCode::SUCCESS
        }
        None => ExitCode::FAILURE,
    }
}

/// Interprets command-line arguments.
///
/// Returns the path of the source file to evaluate together with the search
/// directories given through `-p/--path` flags, or `None` after handling a
/// flag or deciding to run the interactive REPL. Usage errors are returned
/// as messages.
fn parse_args(args: &[String]) -> Result<Option<(String, Vec<String>)>, String> {
    let mut file = None;
    let mut search_paths = Vec::new();

    let mut index = 1;
    while index < args.len() {
        let arg = args[index].as_str();
        index += 1;
        match arg {
            "-h" | "--help" => {
                println!("{USAGE}");
                println!();
                println!("Evaluate a Universal Coding Language source file.");
                println!("Run without arguments to start an interactive session.");
                println!();
                println!("Options:");
                println!("  -p, --path <dir>  add a module search directory (repeatable)");
                println!("  -h, --help        show this help");
                println!("  -V, --version     show the version");
                println!();
                println!(
                    "Module imports also consult {SEARCH_PATH_ENV} directories \
(see https://github.com/letridung07home/Universal-Coding-Language)."
                );
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("ucl {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-p" | "--path" => {
                let Some(directory) = args.get(index) else {
                    return Err(format!("`{arg}` requires a directory argument"));
                };
                search_paths.push(directory.clone());
                index += 1;
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown option `{flag}`"));
            }
            path => {
                if file.is_some() {
                    return Err("expected a single source file".to_owned());
                }
                file = Some(path.to_owned());
            }
        }
    }

    match file {
        Some(file) => Ok(Some((file, search_paths))),
        // No input file: run the interactive REPL with the same search paths.
        None => repl::run(&search_paths).map_or_else(
            |error| Err(format!("interactive session failed: {error}")),
            |_| Ok(None),
        ),
    }
}

/// Reads `UCL_PATH`, splitting it on the platform's path separator.
///
/// Missing variables and empty entries are skipped; relative entries stay
/// relative and resolve against the process working directory.
fn env_search_paths() -> Vec<String> {
    env::var_os(SEARCH_PATH_ENV)
        .map(|value| {
            env::split_paths(&value)
                .filter(|path| !path.as_os_str().is_empty())
                .map(|path| path.display().to_string())
                .collect()
        })
        .unwrap_or_default()
}
