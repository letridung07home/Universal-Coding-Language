//! Command-line entry point for the `ucl` binary.
//!
//! This module provides the CLI interface for evaluating UCL source files,
//! inline programs (`-e/--eval`), and piped stdin, as well as interactive
//! sessions. It orchestrates the compiler pipeline
//! (lexer → parser → evaluator) and handles diagnostic formatting and output.

use std::env;
use std::fs;
use std::io::Read;
use std::process::ExitCode;

use ucl::fmt::format_source;
use ucl::{DiagnosticSink, Environment, Evaluator, Lexer, Parser, SourceFile};

mod render;
mod repl;

use render::{format_value, render_diagnostics};

/// Usage text printed on argument errors and `--help`.
const USAGE: &str =
    "usage: ucl [-p <dir>]... [-e <code> | <file>]\n       ucl fmt [--check] [<file> | -]";

/// The environment variable holding module search directories, separated by
/// the platform's path separator (`:` on Unix, `;` on Windows).
const SEARCH_PATH_ENV: &str = "UCL_PATH";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    // The formatter is a subcommand: `ucl fmt ...`.
    if args.get(1).map(String::as_str) == Some("fmt") {
        return run_fmt(&args[2..]);
    }

    let (input, mut search_paths) = match parse_args(&args) {
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

    let (name, contents) = match read_input(&input) {
        Ok(parts) => parts,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };

    run_program(&name, contents, &search_paths)
}

/// Where the program to evaluate comes from.
enum Input {
    /// A source file path.
    File(String),
    /// The program text piped to standard input (`ucl -`).
    Stdin,
    /// Inline program text from `-e/--eval`.
    Eval(String),
}

/// The formatter subcommand's parsed arguments.
struct FmtArgs {
    check: bool,
    input: Option<Input>,
}

/// Runs `ucl fmt`: format a file in place, pipe stdin to stdout, or check
/// formatting without rewriting.
///
/// Exit codes: 0 when the output is formatted (or rewritten), 1 when
/// `--check` finds unformatted input or the source has errors, and 2 for
/// usage and I/O problems. Files with errors are never touched.
fn run_fmt(args: &[String]) -> ExitCode {
    let Some(parsed) = parse_fmt_args(args) else {
        return ExitCode::from(2);
    };
    let Some(input) = parsed.input else {
        eprintln!("error: `ucl fmt` expects a file or `-` for standard input");
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    let (name, contents) = match read_input(&input) {
        Ok(parts) => parts,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };

    match format_source(&name, &contents) {
        Ok(formatted) => {
            if formatted == contents {
                return ExitCode::SUCCESS;
            }
            if parsed.check {
                println!("{name}: not formatted");
                return ExitCode::from(1);
            }
            match input {
                Input::Stdin => print!("{formatted}"),
                Input::File(ref file) => {
                    if let Err(error) = fs::write(file, &formatted) {
                        eprintln!("error: cannot write `{file}`: {error}");
                        return ExitCode::from(2);
                    }
                }
                Input::Eval(_) => {
                    unreachable!("`ucl fmt` never accepts inline programs");
                }
            }
            ExitCode::SUCCESS
        }
        Err(sink) => {
            let source = SourceFile::new(&name, contents);
            render_diagnostics(&sink, &source);
            ExitCode::from(1)
        }
    }
}

/// Interprets `ucl fmt` arguments: an optional `--check` flag plus one
/// optional source (a file path or `-`).
fn parse_fmt_args(args: &[String]) -> Option<FmtArgs> {
    let mut parsed = FmtArgs {
        check: false,
        input: None,
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--check" => {
                if parsed.check {
                    eprintln!("error: repeated `--check` flag");
                    return None;
                }
                parsed.check = true;
            }
            "-" => {
                if parsed.input.is_some() {
                    eprintln!("error: expected a single source file");
                    return None;
                }
                parsed.input = Some(Input::Stdin);
            }
            arg if !arg.starts_with('-') => {
                if parsed.input.is_some() {
                    eprintln!("error: expected a single source file");
                    return None;
                }
                parsed.input = Some(Input::File(arg.to_owned()));
            }
            other => {
                eprintln!("error: unknown option `{other}`");
                return None;
            }
        }
        index += 1;
    }
    Some(parsed)
}

/// Reads the program text for `input`, returning its display name and
/// contents. File names stay as given so relative imports resolve against
/// the file's directory; `-` reads standard input under the name `<stdin>`.
fn read_input(input: &Input) -> Result<(String, String), String> {
    match input {
        Input::File(file) => {
            let contents = fs::read_to_string(file)
                .map_err(|error| format!("cannot read `{file}`: {error}"))?;
            Ok((file.clone(), contents))
        }
        Input::Stdin => {
            let mut contents = String::new();
            std::io::stdin()
                .read_to_string(&mut contents)
                .map_err(|error| format!("cannot read standard input: {error}"))?;
            Ok(("<stdin>".to_owned(), contents))
        }
        Input::Eval(code) => Ok(("<eval>".to_owned(), code.clone())),
    }
}

/// Runs the compiler pipeline over `contents`: source text → tokens → AST →
/// value.
///
/// Each stage runs only if every earlier stage succeeded, so a program with
/// lexical errors is never parsed and one with syntax errors is never
/// evaluated. This keeps diagnostics focused on the root cause instead of
/// cascading through downstream stages fed garbage input.
fn run_program(name: &str, contents: String, search_paths: &[String]) -> ExitCode {
    let source = SourceFile::new(name, contents);
    let mut sink = DiagnosticSink::new();

    let mut environment = Environment::new();
    for dir in search_paths {
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
/// Returns the program input together with the search directories given
/// through `-p/--path` flags, or `None` after handling an informational flag
/// or running the interactive session. Usage errors are returned as messages.
fn parse_args(args: &[String]) -> Result<Option<(Input, Vec<String>)>, String> {
    let mut file = None;
    let mut eval = None;
    let mut search_paths = Vec::new();

    let mut index = 1;
    while index < args.len() {
        let arg = args[index].as_str();
        index += 1;
        match arg {
            "-h" | "--help" => {
                println!("{USAGE}");
                println!();
                println!("Evaluate a Universal Coding Language program.");
                println!("Run without arguments to start an interactive session.");
                println!();
                println!("Options:");
                println!("  -e, --eval <code> evaluate inline program text");
                println!("  -p, --path <dir>  add a module search directory (repeatable)");
                println!("  -h, --help        show this help");
                println!("  -V, --version     show the version");
                println!();
                println!("Formatter:");
                println!("  ucl fmt [--check] [<file> | -]");
                println!("      format a file in place, or pipe stdin to stdout;");
                println!("      `--check` exits 1 when the input is not formatted");
                println!();
                println!(
                    "A file name of `-` reads the program from standard input; \
module imports also consult {SEARCH_PATH_ENV} directories \
(see https://github.com/letridung07home/Universal-Coding-Language)."
                );
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("ucl {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-e" | "--eval" => {
                let Some(code) = args.get(index) else {
                    return Err(format!("`{arg}` requires program text"));
                };
                if eval.is_some() || file.as_deref() == Some("-") {
                    return Err("expected a single `--eval` program".to_owned());
                }
                eval = Some(code.clone());
                index += 1;
            }
            "-p" | "--path" => {
                let Some(directory) = args.get(index) else {
                    return Err(format!("`{arg}` requires a directory argument"));
                };
                search_paths.push(directory.clone());
                index += 1;
            }
            // `-` is the conventional placeholder for standard input and
            // must be matched before the unknown-option guard below.
            "-" => {
                if file.is_some() {
                    return Err("expected a single source file".to_owned());
                }
                if eval.is_some() {
                    return Err("cannot combine `--eval` with standard input".to_owned());
                }
                file = Some("-".to_owned());
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

    match (file, eval) {
        (Some(_), Some(_)) => Err("cannot combine `--eval` with a source file".to_owned()),
        (Some(file), None) => {
            if file == "-" {
                Ok(Some((Input::Stdin, search_paths)))
            } else {
                Ok(Some((Input::File(file), search_paths)))
            }
        }
        (None, Some(code)) => Ok(Some((Input::Eval(code), search_paths))),
        (None, None) => {
            // No input: run the interactive REPL with the same search paths.
            repl::run(&search_paths).map_or_else(
                |error| Err(format!("interactive session failed: {error}")),
                |_| Ok(None),
            )
        }
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
