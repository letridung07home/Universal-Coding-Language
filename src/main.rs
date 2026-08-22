//! Command-line entry point for the `ucl` binary.
//!
//! This module provides the CLI interface for evaluating UCL source files and
//! for interactive sessions. It orchestrates the compiler pipeline
//! (lexer → parser → evaluator) and handles diagnostic formatting and output.

use std::env;
use std::fs;
use std::process::ExitCode;

use ucl::{DiagnosticSink, Evaluator, Lexer, Parser, SourceFile};

mod render;
mod repl;

use render::{format_value, render_diagnostics};

/// Usage text printed on argument errors and `--help`.
const USAGE: &str = "usage: ucl [<file>]";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    let file = match parse_args(&args) {
        Ok(Some(file)) => file,
        // The REPL, `--help`, and `--version` handle their own output.
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

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
    let mut value = None;
    let tokens = Lexer::new(&source).tokenize(&mut sink);
    if !sink.has_errors()
        && let Some(ast) = Parser::new(tokens).parse(&mut sink)
        && !sink.has_errors()
    {
        value = Evaluator::new().evaluate(&ast, &source, &mut sink);
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
/// Returns the path of the source file to evaluate, or `None` after handling
/// a flag or deciding to run the interactive REPL. Usage errors are returned
/// as messages.
fn parse_args(args: &[String]) -> Result<Option<String>, String> {
    let mut file = None;

    for arg in &args[1..] {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                println!();
                println!("Evaluate a Universal Coding Language source file.");
                println!("Run without arguments to start an interactive session.");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("ucl {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
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
        Some(file) => Ok(Some(file)),
        // No input file: run the interactive REPL.
        None => repl::run().map_or_else(
            |error| Err(format!("interactive session failed: {error}")),
            |_| Ok(None),
        ),
    }
}
